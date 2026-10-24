"""Independent Kanban evidence regressions, including negative controls."""
import json
import os
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PROBE = ROOT / 'scripts/kanban_eval/probe.mjs'
PLAYWRIGHT = Path(os.environ.get(
    'PLAYWRIGHT_MODULE_PATH', ROOT / 'dashboard/node_modules/playwright/index.mjs'
))


class KanbanScoringRegressions(unittest.TestCase):
    def test_total_accepts_units_after_the_number_without_accepting_another_count(self):
        result = self.node("""
const found=[];
for(const text of ['Total: 5 tickets', 'Total tickets: 5', '5 tickets', 'Total: 15 tickets', 'Backlog 5']){
 await page.setContent(`<p>${text}</p>`);found.push(await probe.boardTicketTotal(page,5).count());
}
console.log(JSON.stringify(found));
""", browser=True)
        self.assertEqual(result, [1, 1, 1, 0, 0])

    def test_detail_header_actions_do_not_hide_sibling_fields(self):
        result = self.node("""
const ticket={key:'KAN-1',title:'Ticket',description:'Details',status:'todo',priority:'urgent',assignee:'Lin'};
await page.setContent('<section id="detail"><header><h2>Ticket</h2><button>Delete ticket</button></header><p>KAN-1</p><p>Details</p><p>To do</p><p>Urgent</p><p>Lin</p></section>');
await probe.assertTicketDetails(page,ticket);
console.log(JSON.stringify(await (await probe.ticketDetail(page,ticket)).getAttribute('id')));
""", browser=True)
        self.assertEqual(result, 'detail')

    def test_detail_title_outside_the_action_block_still_scopes_to_the_detail_region(self):
        # Reference fixture layout: the title heading sits above a content block that
        # carries its own heading ("Description") together with the Delete action.
        result = self.node("""
const ticket={key:'KAN-1',title:'Editable probe',description:'Keep me',status:'todo',priority:'high',assignee:'Initial'};
await page.setContent('<main><section id="ticket-view"><p>KAN-1</p><h1>Editable probe</h1><div id="detail-content"><dl><dt>Status</dt><dd>To do</dd><dt>Priority</dt><dd>high</dd><dt>Assignee</dt><dd>Initial</dd></dl><h2>Description</h2><p>Keep me</p><div><p>Deleting removes this ticket.</p><button>Delete ticket</button></div></div></section></main>');
await probe.assertTicketDetails(page,ticket);
console.log(JSON.stringify(await (await probe.ticketDetail(page,ticket)).getAttribute('id')));
""", browser=True)
        self.assertEqual(result, 'ticket-view')

    def test_detail_validation_never_borrows_matching_fields_from_the_board(self):
        result = self.node("""
const ticket={key:'KAN-1',title:'Ticket',description:'Details',status:'in_review',priority:'urgent',assignee:'Lin'};
await page.setContent('<section id="workspace"><div id="board"><a><span>Ticket</span><span>In review</span><span>Urgent</span><span>Lin</span></a></div><div id="details"><header><h2>Ticket</h2><button>Delete ticket</button></header><p>KAN-1</p><p>Details</p><p>Backlog</p><p>Low</p><p>Wrong assignee</p></div></section>');
let rejected=false;try{await probe.assertTicketDetails(page,ticket)}catch{rejected=true}
console.log(JSON.stringify({scope:await (await probe.ticketDetail(page,ticket)).getAttribute('id'),rejected}));
""", browser=True)
        self.assertEqual(result, {'scope': 'details', 'rejected': True})

    def test_cards_accept_span_titles_and_details_are_scoped_away_from_the_board(self):
        result = self.node("""
const ticket={id:'id-1',key:'KAN-1',title:'Ticket [one]',description:'Details',status:'in_review',priority:'urgent',assignee:'Lin'};
await page.setContent(`<main><section id="board">
<a href="#ticket/id-1" aria-label="Open KAN-1: Ticket [one]"><span>Ticket [one]</span><span>Lin</span><span>Urgent</span></a>
 <a href="#ticket/id-2"><span>Another ticket</span></a>
 </section><aside id="details"><h2>Ticket [one]</h2><p>KAN-1</p><p>Details</p><p>in_review</p><p>Urgent</p><p id="assignee">Lin</p><button>Delete ticket</button></aside></main>`);
const card=probe.ticketCard(page,ticket);
await card.focus();await card.press('Enter');
const activated=await page.evaluate(()=>location.hash);
await probe.assertTicketDetails(page,ticket);
const scope=await probe.ticketDetail(page,ticket);
await page.locator('#assignee').evaluate(element=>element.textContent='Wrong assignee');
let wrongRejected=false;try{await probe.assertTicketDetails(page,ticket)}catch{wrongRejected=true}
await page.locator('#details h2').evaluate(element=>element.textContent='Wrong ticket');
let wrongTicketRejected=false;try{await probe.ticketDetail(page,ticket)}catch{wrongTicketRejected=true}
console.log(JSON.stringify({cards:await card.count(),activated,scope:await scope.getAttribute('id'),wrongRejected,wrongTicketRejected}));
""", browser=True)
        self.assertEqual(result, {'cards': 1, 'activated': '#ticket/id-1', 'scope': 'details',
                                  'wrongRejected': True, 'wrongTicketRejected': True})

    def test_persisted_json_ignores_object_order_but_preserves_values_and_array_order(self):
        result = self.node("""
const ticket={id:'one',comments:[{author:'Alice',body:'hello'},{body:'reply',author:'Bob'}]};
console.log(JSON.stringify([
 probe.sameJson(ticket,{comments:[{body:'hello',author:'Alice'},{author:'Bob',body:'reply'}],id:'one'}),
 probe.sameJson(ticket,{id:'one',comments:[{author:'Alice',body:'changed'},{body:'reply',author:'Bob'}]}),
 probe.sameJson(ticket,{id:'one',comments:[{body:'reply',author:'Bob'},{author:'Alice',body:'hello'}]}),
 probe.sameJson({id:'one'},['one']),
 probe.sameJson(null,{}),
]));
""")
        self.assertEqual(result, [True, False, False, False, False])

    def node(self, body, browser=False):
        if browser and not PLAYWRIGHT.exists():
            self.skipTest('dashboard Playwright is not installed')
        script = f"import * as probe from {json.dumps(PROBE.as_uri())};\n"
        if browser:
            script += f"import {{chromium}} from {json.dumps(PLAYWRIGHT.as_uri())};\n"
            script += "const browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH});const page=await browser.newPage();try {\n"
        script += body
        if browser:
            script += '\n} finally {await browser.close()}'
        result = subprocess.run(['node', '--input-type=module', '--eval', script],
                                capture_output=True, text=True, cwd=ROOT, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def test_live_session_waits_for_the_event_stream_before_counting(self):
        result = self.node("""
const html=(delay)=>`<h1>Board</h1><script>setTimeout(()=>new EventSource('/api/events'),${delay})</script>`;
await page.route('http://kanban.test/api/events',route=>route.fulfill({status:200,headers:{'content-type':'text/event-stream'},body:'event: change\\ndata: {}\\n\\n'}));
await page.route('http://kanban.test/',route=>route.fulfill({contentType:'text/html',body:html(300)}));
const late=await probe.openLiveSession(page,'http://kanban.test/').then(()=>'subscribed',error=>error.message);
await page.route('http://kanban.test/',route=>route.fulfill({contentType:'text/html',body:'<h1>Board without a stream</h1>'}));
const never=await probe.openLiveSession(page,'http://kanban.test/',500).then(()=>'subscribed',error=>error.message);
console.log(JSON.stringify({late,never}));
""", browser=True)
        self.assertEqual(result, {'late': 'subscribed', 'never': 'session did not open the live event stream'})

    def test_list_lanes_are_accepted_without_selecting_outer_board(self):
        result = self.node("""
await page.setContent('<section id="board"><ol><li id="backlog"><h2>Backlog <span>1</span></h2><ul><li>Card</li></ul></li><li><h2>Done <span>1</span></h2></li></ol></section>')
const lane=probe.boardLane(page,'Backlog');
console.log(JSON.stringify({id:await lane.getAttribute('id'),count:await (await probe.boardLaneWithCount(page,'Backlog',1)).count()}));
""", browser=True)
        self.assertEqual(result, {'id': 'backlog', 'count': 1})

    def test_card_metadata_cannot_be_borrowed_from_a_sibling_card(self):
        result = self.node("""
const ticket={title:'Target',key:'KAN-1',priority:'high',assignee:'Grace'};
await page.setContent('<main><section><h2>Backlog <span>2</span></h2><a><span>Target</span></a><a><span>Other</span><p>KAN-1</p><p>high</p><p>Grace</p></a></section><section><h2>Done <span>0</span></h2></section></main>');
const region=await probe.ticketCardRegion(probe.boardLane(page,'Backlog'),ticket);
console.log(JSON.stringify({tag:await region.evaluate(element=>element.tagName),key:await region.getByText('KAN-1',{exact:true}).count(),priority:await region.getByText(/^high$/i).count(),assignee:await region.getByText('Grace',{exact:true}).count()}));
""", browser=True)
        self.assertEqual(result, {'tag': 'SPAN', 'key': 0, 'priority': 0, 'assignee': 0})

    def test_div_card_metadata_is_accepted_without_borrowing_from_its_lane(self):
        result = self.node("""
const ticket={title:'Target',key:'KAN-1',priority:'high',assignee:'Grace'};
await page.setContent('<main><section><h2>Backlog <span>2</span></h2><div class="card"><a href="/tickets/1"><h3>Target</h3></a><p>KAN-1</p><p>high</p><p>Grace</p><button>More actions</button></div><div class="card"><h3>Other</h3></div></section><section><h2>Done <span>0</span></h2></section></main>');
const region=await probe.ticketCardRegion(probe.boardLane(page,'Backlog'),ticket);
console.log(JSON.stringify({tag:await region.evaluate(element=>element.tagName),className:await region.getAttribute('class')}));
""", browser=True)
        self.assertEqual(result, {'tag': 'DIV', 'className': 'card'})

    def test_parent_accessible_label_variant_still_requires_real_navigation(self):
        result = self.node("""
await page.setContent('<ul><li id="parent" tabindex="-1">Alice: first</li><li id="reply"><button id="reply-action" aria-controls="reply-form">Reply</button><button id="delete-action" aria-label="Delete parent comment" aria-controls="parent">×</button><button aria-label="Jump upward">↑</button>second<form id="reply-form"></form></li></ul>');
const entry=page.locator('#reply');const parent=await page.locator('#parent').elementHandle();
await page.evaluate(()=>{window.destructive=0;document.querySelector('#reply-action').onclick=()=>window.destructive++;document.querySelector('#delete-action').onclick=()=>window.destructive++});
const inert=await probe.activateCommentParent(entry,parent,page);
await page.locator('[aria-label="Jump upward"]').evaluate(button=>button.onclick=()=>document.querySelector('#parent').focus());
const navigated=await probe.activateCommentParent(entry,parent,page);
console.log(JSON.stringify({inert,navigated,destructive:await page.evaluate(()=>window.destructive)}));
""", browser=True)
        self.assertEqual(result, {'inert': False, 'navigated': True, 'destructive': 0})

    def test_preexisting_parent_hash_does_not_fake_navigation(self):
        result = self.node("""
await page.setContent('<ul><li id="parent" tabindex="-1">first</li><li id="reply"><a href="#parent">Context</a></li></ul>');
await page.evaluate(()=>{location.hash='#parent'});
const parent=await page.locator('#parent').elementHandle();
console.log(JSON.stringify(await probe.activateCommentParent(page.locator('#reply'),parent,page)));
""", browser=True)
        self.assertFalse(result)

    def test_failed_write_event_before_identified_barrier_is_rejected(self):
        result = self.node("""
const start=1, store='/workspace/barrier';
const valid=[{store:'/workspace/original'},{store}];
const invalid=[{store:'/workspace/original'},{store:'/workspace/original'}];
await new Promise(resolve=>setTimeout(()=>{invalid.push({store});resolve()},10));
console.log(JSON.stringify({valid:probe.barrierEventIndex(valid,start,store)===start,invalid:probe.barrierEventIndex(invalid,start,store)===start}));
""")
        self.assertEqual(result, {'valid': True, 'invalid': False})

    def test_delayed_failed_write_event_cannot_masquerade_as_store_restoration(self):
        if not PLAYWRIGHT.exists():
            self.skipTest('dashboard Playwright is not installed')
        fixture = ROOT / 'tests/fixtures/kanban-evaluator/sse.mjs'
        result = self.node(f"await import({json.dumps(fixture.as_uri())});")
        self.assertEqual(result, {'valid': True, 'beforeBarrier': False, 'afterBarrier': False})

    def test_error_feedback_accepts_failed_creation_and_deletion_not_success(self):
        result = self.node("""
const found=[];
for(const text of ['probe failure','Failed to create ticket','Could not be created','Unable to delete KAN-1 (500)','Could not be deleted','Unable to post comment (500)','Failed to post comment','Creating…','Ticket created','Delete ticket']){
 await page.setContent(`<p role="alert">${text}</p>`);found.push(await probe.mutationFailureFeedback(page).count());
}
console.log(JSON.stringify(found));
""", browser=True)
        self.assertEqual(result, [1, 1, 1, 1, 1, 1, 1, 0, 0, 0])

    def test_save_action_accepts_contract_names_without_matching_settings(self):
        result = self.node("""
const found=[];
for(const name of ['Save','Save changes','Save ticket','Save settings','Autosave']){
 await page.setContent(`<form><label for="title">Title</label><input id="title"><button>${name}</button></form>`);
 found.push(await probe.saveTicketAction(page.locator('form')).count());
}
console.log(JSON.stringify(found));
""", browser=True)
        self.assertEqual(result, [1, 1, 1, 0, 0])

    def test_soft_deleted_ticket_is_found_in_array_or_envelope(self):
        result = self.node("""
const ticket={id:'ticket-1',deleted_at:'now'};
console.log(JSON.stringify([
 probe.findStoredTicket([ticket],'ticket-1')?.deleted_at,
 probe.findStoredTicket({version:1,records:[ticket]},'ticket-1')?.deleted_at,
 probe.findStoredTicket({version:1,records:[]},'ticket-1') ?? null,
]));
""")
        self.assertEqual(result, ['now', 'now', None])

    def test_checkpoints_preserve_earned_evidence_and_do_not_fail_unreached_checks(self):
        result = self.node("""
const records=[];const recorder=probe.createCheckRecorder(records);
await recorder.check('flow',async()=>{
 recorder.stage('creation');
 recorder.stage('feedback');
 throw new Error('no feedback');
});
const rubric=[{id:'criterion_create',description:'Creation',weight:40,checks:['creation']},
{id:'criterion_error',description:'Error feedback',weight:30,checks:['feedback']},
{id:'criterion_delete',description:'Deletion',weight:30,checks:['deletion']}];
console.log(JSON.stringify(probe.scoreCriteria(records,rubric).map(({id,status})=>({id,status}))));
""")
        self.assertEqual(result, [
            {'id': 'criterion_create', 'status': 'passed'},
            {'id': 'criterion_error', 'status': 'failed'},
            {'id': 'criterion_delete', 'status': 'unverified'},
        ])

    def test_visible_lane_drag_handles_both_tall_layouts_and_keeps_broken_drag_broken(self):
        result = self.node("""
const outcomes=[];
for(const style of ['baseline','deepwork']){
 await page.setContent(`<style>
  #board{display:grid;grid-template-columns:repeat(5,220px);gap:${style==='baseline'?'16px':'24px'};width:1180px}
  .lane{height:693px;border:1px solid;padding:${style==='baseline'?'12px':'20px'};box-sizing:border-box}
  .blocker{height:96px;margin-top:8px}.broken{height:693px;border:1px solid}
 </style><main id="board">
  <section class="lane">${'<div class="blocker"></div>'.repeat(5)}</section>
  <section class="lane"><a id="source" draggable="true">Drag probe</a><div class="blocker"></div></section>
  <section class="lane"></section><section class="lane"></section>
  <section id="target" class="lane">${'<div class="blocker"></div>'.repeat(5)}</section>
 </main><section id="broken" class="broken"></section>`);
 let drops=0;const target=page.locator('#target');
 await target.evaluate(node=>{node.addEventListener('dragover',event=>event.preventDefault());node.addEventListener('drop',event=>{event.preventDefault();globalThis.__drops=(globalThis.__drops||0)+1})});
 const height=(await target.boundingBox()).height;
 await probe.dragToVisibleLane(page.locator('#source'),target,page);
 drops=await page.evaluate(()=>globalThis.__drops||0);await page.evaluate(()=>{globalThis.__drops=0});await target.evaluate(node=>node.remove());
 await probe.dragToVisibleLane(page.locator('#source'),page.locator('#broken'),page);
 outcomes.push({style,drops,broken:await page.evaluate(()=>globalThis.__drops||0),height});
}
console.log(JSON.stringify(outcomes));
""", browser=True)
        self.assertEqual(result, [
            {'style': 'baseline', 'drops': 1, 'broken': 0, 'height': 693},
            {'style': 'deepwork', 'drops': 1, 'broken': 0, 'height': 693},
        ])

    def test_missing_fixture_is_unverified_without_losing_other_results(self):
        result = self.node("""
const records=[];const recorder=probe.createCheckRecorder(records);
await recorder.check('deletion',async()=>{
 probe.requireEvidence(undefined,'fixture creation failed');
 recorder.stage('deleted');
});
await recorder.check('utf8',async()=> 'text preserved');
console.log(JSON.stringify(records.map(({id,status})=>({id,status}))));
""")
        self.assertEqual(result, [{'id': 'deletion', 'status': 'unverified'},
                                  {'id': 'utf8', 'status': 'passed'}])

    def test_rubric_has_explicit_weights_without_duplicated_evidence_credit(self):
        rubric = json.loads((ROOT / 'scripts/kanban_eval/rubric.json').read_text())
        self.assertEqual(len(rubric), 7)
        for scenario, criteria in rubric.items():
            with self.subTest(scenario=scenario):
                self.assertEqual(sum(c['weight'] for c in criteria), 100)
                self.assertTrue(all(c['weight'] > 0 for c in criteria))
                self.assertEqual(len({c['id'] for c in criteria}), len(criteria))
                checks = [check for c in criteria for check in c['checks']]
                self.assertEqual(len(checks), len(set(checks)))
                gates = [c['id'] for c in criteria if c.get('gate')]
                self.assertTrue(0 < len(gates) <= 2, gates)

    def test_discussion_preservation_does_not_require_earlier_browser_flows(self):
        result = self.node("""
const tickets=[];const stages=[];
const trigger=async (fn,payload={})=>{
 if(fn==='kanban::tickets::create'){
  const ticket={id:`id-${tickets.length}`,key:`KAN-${tickets.length+1}`,title:payload.title,comments:[]};tickets.push(ticket);return structuredClone(ticket);
 }
 const ticket=tickets.find(({id,key})=>id===payload.id||key===payload.id);
 if(!ticket)throw new Error('fixture not found');
 if(fn==='kanban::tickets::comment')ticket.comments.push({id:`comment-${ticket.comments.length}`,...payload.comment});
 if(fn==='kanban::tickets::update')Object.assign(ticket,payload.changes);
 if(fn==='kanban::tickets::delete')ticket.deleted_at='2026-09-11';
 return structuredClone(ticket);
};
await probe.PROBES.kanban_c6_discussion({trigger,stage:id=>stages.push(id),
 control:async operation=>operation==='read_store'?JSON.stringify(tickets):undefined,
 check:async(id,action)=>{if(id==='discussion_survives_edit_delete_and_restart')await action()},
});
console.log(JSON.stringify({stages,retained:tickets.filter(t=>t.deleted_at&&t.comments.length>0).length}));
""")
        self.assertEqual(result, {'stages': ['discussion_preservation', 'discussion_restart'], 'retained': 1})


if __name__ == '__main__':
    unittest.main()
