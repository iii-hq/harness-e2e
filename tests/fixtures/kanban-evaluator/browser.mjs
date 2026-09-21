// A small behavioral control for the evaluator, deliberately using presentation
// variants from the audited deliveries. Runtime/iii boundaries are test doubles;
// HTTP, DOM, focus, navigation, mutations and request interception are real.
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { chromium } from '../../../dashboard/node_modules/playwright/index.mjs'
import { PROBES, createCheckRecorder, scoreCriteria } from '../../../scripts/kanban_eval/probe.mjs'

const rubric = JSON.parse(await readFile(new URL('../../../scripts/kanban_eval/rubric.json', import.meta.url)))
const client = String.raw`
const labels={backlog:'Backlog',todo:'To do',in_progress:'In progress',in_review:'In review',done:'Done'};
const el=id=>document.getElementById(id);
function text(tag,value){const node=document.createElement(tag);node.textContent=value;return node}
async function request(path,options){const response=await fetch(path,options);const body=await response.json();if(!response.ok)throw Error(body.error);return body}
let boardGeneration=0;
async function board(){
 const generation=++boardGeneration;
 el('board-status').textContent='Loading tickets…';el('total').textContent='';el('lanes').replaceChildren();el('empty').hidden=true;el('retry').hidden=true;
 try{
  const {tickets}=await request('/api/tickets');if(generation!==boardGeneration)return;
  const count=window.badTotal&&tickets.length===5?15:tickets.length;
  el('total').textContent='Total: '+count+' tickets';el('empty').hidden=tickets.length!==0;el('board-status').textContent='';
  for(const [status,label] of Object.entries(labels)){
   const lane=text('li','');const heading=text('h2',label);const items=tickets.filter(t=>t.status===status);heading.append(text('span',String(items.length)));lane.append(heading);
   for(const ticket of items){const card=text('a','');card.href='#ticket/'+ticket.id;card.setAttribute('aria-label','Open '+ticket.key+': '+ticket.title);card.append(text('span',ticket.key),text('span',ticket.title),text('span',ticket.priority),text('span',ticket.assignee||'Unassigned'));lane.append(card)}
   el('lanes').append(lane);
  }
 }catch(error){el('board-status').textContent='Unable to load tickets: '+error.message;el('retry').hidden=false}
}
async function show(){
 const route=location.hash;el('settings-view').hidden=route!=='#settings';el('board-view').hidden=route==='#settings';el('details').hidden=true;
 if(route==='#settings'){el('data-dir').value=(await request('/api/config')).data_dir;return}
 await board();
 if(route.startsWith('#ticket/')){
  const {ticket}=await request('/api/tickets/'+route.slice(8));if(location.hash!==route)return;
  el('details').replaceChildren(text('h2',ticket.title),text('p',ticket.key));
  for(const value of [ticket.title,ticket.description,labels[ticket.status],ticket.priority[0].toUpperCase()+ticket.priority.slice(1),ticket.assignee||'Unassigned'])el('details').append(text('p',value));
  const status=text('p','');status.role='alert';const remove=text('button','Delete ticket');
  remove.onclick=async()=>{try{await request('/api/tickets/'+ticket.id,{method:'DELETE'});if(location.hash===route)location.hash='#board';await show()}catch(error){status.textContent='Unable to delete: '+error.message}};
  el('details').append(status,remove);el('details').hidden=false;
 }
}
el('refresh').onclick=board;el('retry').onclick=board;
el('settings').onsubmit=async event=>{event.preventDefault();await request('/api/config',{method:'PUT',headers:{'content-type':'application/json'},body:JSON.stringify({data_dir:el('data-dir').value})});el('settings-status').textContent='Settings saved.'};
el('new').onclick=()=>{el('create').reset();el('create-error').textContent='';el('dialog').showModal();(window.noFocus?el('create-description'):el('create-title')).focus()};
el('create').onsubmit=async event=>{
 event.preventDefault();
 try{const {ticket}=await request('/api/tickets',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify(Object.fromEntries(new FormData(el('create'))))});el('dialog').close();location.hash='#ticket/'+ticket.id}
 catch(error){el('create-error').textContent='Unable to create: '+error.message}
};
window.addEventListener('hashchange',()=>{void show()});void show();
`
function markup({ badTotal, noFocus }) {
  return `<!doctype html><meta charset="utf-8"><style>[hidden]{display:none!important}*{box-sizing:border-box}body{margin:12px}#lanes{list-style:none;padding:0}#lanes>li{margin:8px}a span{display:block}h2 span{margin-left:12px}dialog{max-width:95vw}input,textarea,select{max-width:100%;display:block}</style>
  <nav><a href="#board">Board</a><a href="#settings">Settings</a></nav>
  <main><section id="board-view"><h1>Board overview</h1><button id="refresh">Refresh board</button><button id="new">New ticket</button><p id="board-status" role="status"></p><p id="total"></p><p id="empty" hidden>No tickets yet.</p><button id="retry" hidden>Retry</button><ul id="lanes"></ul><aside id="details" hidden></aside></section>
  <section id="settings-view" hidden><form id="settings"><label>Data directory<input id="data-dir"></label><button>Save settings</button><p id="settings-status" role="status"></p></form></section></main>
  <dialog id="dialog" aria-label="New ticket"><form id="create"><label>Title<input id="create-title" name="title"></label><label>Description<textarea id="create-description" name="description"></textarea></label><label>Status<select name="status"><option value="backlog">Backlog</option><option value="in_review">In review</option></select></label><label>Priority<select name="priority"><option value="medium">Medium</option><option value="urgent">Urgent</option></select></label><label>Assignee<input name="assignee"></label><button>Create ticket</button><p id="create-error" role="alert"></p></form></dialog>
  <script>window.badTotal=${!!badTotal};window.noFocus=${!!noFocus};${client}</script>`
}
async function run(caseId, variant = {}) {
  let directory = '/data', counter = 0
  const stores = new Map()
  const records = () => { if (!stores.has(directory)) stores.set(directory, []); return stores.get(directory) }
  const trigger = async (fn, payload = {}) => {
    if (fn.endsWith('::create')) {
      const ticket = { id: `id-${++counter}`, key: `KAN-${counter}`, title: payload.title, description: '', status: 'backlog', priority: 'medium', assignee: null, ...payload }
      records().push(ticket); return structuredClone(ticket)
    }
    const ticket = records().find(t => !t.deleted_at && [t.id, t.key].includes(payload.id))
    if (!ticket) throw Error('Ticket not found')
    if (fn.endsWith('::delete')) ticket.deleted_at = '2026-09-20'
    return structuredClone(ticket)
  }
  const server = createServer(async (req, res) => {
    try {
      const respond = (status, body) => { res.writeHead(status, { 'content-type': 'application/json' }); res.end(JSON.stringify(body)) }
      if (req.url === '/') { res.writeHead(200, { 'content-type': 'text/html' }); res.end(markup(variant)); return }
      const chunks = []; for await (const chunk of req) chunks.push(chunk)
      const bytes = Buffer.concat(chunks).toString('utf8')
      if (req.url === '/api/config') {
        if (req.method === 'PUT') directory = JSON.parse(bytes).data_dir
        respond(200, { data_dir: directory, resolved_data_dir: directory }); return
      }
      if (req.url === '/api/tickets' && req.method === 'GET') { respond(200, { tickets: records().filter(t => !t.deleted_at) }); return }
      if (req.url === '/api/tickets' && req.method === 'POST') {
        if (req.headers['content-type'] !== 'application/json') { respond(415, { error: 'JSON required' }); return }
        const input = JSON.parse(bytes)
        respond(201, { ticket: await trigger('kanban::tickets::create', variant.dropCreatedFields ? { title: input.title } : input) }); return
      }
      const id = decodeURIComponent(req.url.slice('/api/tickets/'.length))
      respond(200, { ticket: await trigger(req.method === 'DELETE' ? 'kanban::tickets::delete' : 'kanban::tickets::get', { id }) })
    } catch (error) { res.writeHead(404, { 'content-type': 'application/json' }); res.end(JSON.stringify({ error: error.message })) }
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  const baseUrl = `http://127.0.0.1:${server.address().port}`
  const browser = await chromium.launch({ headless: true })
  const output = await mkdtemp(join(tmpdir(), 'kanban-browser-'))
  const checks = []
  const recorder = createCheckRecorder(checks, async () => {}, async () => {
    await Promise.all(browser.contexts().map(context => context.close()))
  })
  try {
    await PROBES[caseId]({ api: (path, init) => fetch(baseUrl + path, init), trigger,
      control: async operation => operation === 'read_store' ? JSON.stringify(records()) : undefined,
      browser, baseUrl, output, ...recorder })
    return scoreCriteria(checks, rubric[caseId])
  } finally {
    await browser.close(); await new Promise(resolve => server.close(resolve)); await rm(output, { recursive: true, force: true })
  }
}
const c3 = await run('kanban_c3_board')
assert.deepEqual(c3.filter(c => c.status !== 'passed'), [], JSON.stringify(c3))
const c4 = await run('kanban_c4_ticket_flow')
assert.deepEqual(c4.filter(c => c.status !== 'passed'), [], JSON.stringify(c4))
const wrongCount = await run('kanban_c3_board', { badTotal: true })
assert.deepEqual(wrongCount.filter(c => c.status !== 'passed').map(c => c.id), ['criterion_total'])
const lostFocus = await run('kanban_c4_ticket_flow', { noFocus: true })
assert.deepEqual(lostFocus.filter(c => c.status !== 'passed').map(c => c.id), ['criterion_modal'])
const droppedFields = await run('kanban_c4_ticket_flow', { dropCreatedFields: true })
assert.deepEqual(droppedFields.filter(c => c.status !== 'passed').map(c => c.id), ['criterion_creation'])
console.log(JSON.stringify({ c3: c3.length, c4: c4.length, negativeControls: ['wrong total', 'missing modal focus', 'lost creation fields'] }))
