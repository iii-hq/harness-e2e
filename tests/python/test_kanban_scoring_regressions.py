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

    def test_total_accepts_captured_summaries_without_accepting_another_count_or_sentence(self):
        # 0.38.0 C3 r02 indents "Total <output>N</output> tickets"; 0.36.0 C3 r02 reads "N tickets total".
        # "10 tickets total" fails if the leading tolerance is more than whitespace, and
        # "0 tickets selected" fails if the words after the number are open-ended.
        result = self.node("""
const found=[];
for(const [html,count] of [
 ['<p id="summary">\\n  <span>Total</span>\\n  <output>0</output> <span>tickets</span>\\n</p>',0],
 ['<p id="summary">\\n  <span>Total</span>\\n  <output>0</output> <span>tickets</span>\\n</p>',5],
 ['<p id="summary"><strong>0</strong> tickets total</p>',0],
 ['<p id="summary"><strong>0</strong> tickets total</p>',5],
 ['<p>0 tickets in total</p>',0],
 ['<p>10 tickets total</p>',0],
 ['<p>0 tickets selected</p>',0],
]){await page.setContent(html);found.push(await probe.boardTicketTotal(page,count).count());}
console.log(JSON.stringify(found));
""", browser=True)
        self.assertEqual(result, [1, 0, 1, 0, 1, 0, 0])

    def test_labeled_total_accepts_an_indented_label_without_accepting_another_label(self):
        # getByLabel tests a RegExp against the raw label text, like getByText. The output sits
        # apart from its label so only the labeled form can find it; "Subtotal" fails if the
        # label match is unanchored.
        result = self.node("""
const labeled=label=>`<header>${label}<span>tickets on the board</span></header><footer><output id="t">0</output></footer>`;
const found=[];
for(const [html,count] of [
 [labeled('<label for="t">\\n  Total\\n</label>'),0],
 [labeled('<label for="t">\\n  Total\\n</label>'),5],
 ['<div><output aria-label=" Total tickets ">0</output></div>',0],
 [labeled('<label for="t">\\n  Subtotal\\n</label>'),0],
]){await page.setContent(html);found.push(await probe.boardTicketTotal(page,count).count());}
console.log(JSON.stringify(found));
""", browser=True)
        self.assertEqual(result, [1, 0, 1, 0])

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

    def test_detail_fields_beside_a_shared_title_and_action_block_count_without_borrowing_from_the_board(self):
        # Releases 0.35-0.37 (C4 r01/r03): the title and Delete share one heading block
        # and every field, a Title row included, is its sibling.
        result = self.node("""
const ticket={key:'KAN-1',title:'Persisted details probe',description:'UTF-8: ação e café',status:'in_review',priority:'urgent',assignee:'Lin'};
const lanes=cards=>['Backlog','To do','In progress','In review','Done'].map(l=>`<section><h3>${l}<span>1</span></h3>${l==='In review'?cards:''}</section>`).join('');
const page_=(dl,cards)=>`<main><section id="board-view"><section id="ticket-detail"><div class="detail-heading"><div><p>KAN-1</p><h2>Persisted details probe</h2></div><div><button>Delete ticket</button><button>Close details</button></div></div>${dl}</section><div id="board">${cards}</div></section></main>`;
const dl=description=>`<dl><dt>Title</dt><dd>Persisted details probe</dd><dt>Description</dt><dd>${description}</dd><dt>Status</dt><dd>In review</dd><dt>Priority</dt><dd>Urgent</dd><dt>Assignee</dt><dd>Lin</dd></dl>`;
const cards=lanes('<a href="#t/1"><h4>Persisted details probe</h4><p>Urgent</p><p>Lin</p></a><a href="#t/2"><h4>Browser-created ticket</h4><p>UTF-8: ação e café</p></a>');
const verdict=async()=>probe.assertTicketDetails(page,ticket).then(()=>'accepted',()=>'rejected');
await page.setContent(page_(dl('UTF-8: ação e café'),cards));const sibling=await verdict();
await page.setContent(page_(dl('Stale'),cards));const borrowed=await verdict();
await page.setContent(`<main><div id="board">${['Backlog','In review'].map(l=>`<section><h3>${l}</h3>${l==='In review'?'<div><h4>Persisted details probe</h4><p>UTF-8: ação e café</p><p>Urgent</p><p>Lin</p><button>Delete ticket</button></div>':''}</section>`).join('')}</div></main>`);
const laneHeading=await verdict();
await page.setContent('<main><section id="ticket-detail"><div class="h"><h2>Persisted details probe</h2><button>Delete ticket</button></div><dl><dd>Stale</dd><dd>In review</dd><dd>Urgent</dd><dd>Lin</dd></dl></section><div id="board"><div class="card"><h4>Persisted details probe</h4></div><div class="card"><h4>Other</h4><p>UTF-8: ação e café</p></div></div></main>');
const divCards=await verdict();
const mixed=(key,title)=>`<button><span>${key}</span> ${title}<span>UTF-8: ação e café</span><span>Urgent</span><span>Lin</span></button>`;
await page.setContent(`<main><div id="board"><section><div>Backlog</div></section><section><div>In review</div>${mixed('KAN-2','Browser-created ticket')}${mixed('KAN-1','Persisted details probe')}</section></div><aside><h2>Persisted details probe</h2><button>Delete ticket</button><p>Stale</p><p>In review</p><p>Urgent</p><p>Lin</p></aside></main>`);
const mixedCards=await verdict();
console.log(JSON.stringify({sibling,borrowed,laneHeading,divCards,mixedCards}));
""", browser=True)
        self.assertEqual(result, {'sibling': 'accepted', 'borrowed': 'rejected', 'laneHeading': 'rejected',
                                  'divCards': 'rejected', 'mixedCards': 'rejected'})

    def test_detail_accepts_read_only_form_values_but_not_unselected_options(self):
        # Release 0.37 C4 r02 shows persisted values in read-only inputs and disabled selects.
        result = self.node("""
const ticket={key:'KAN-1',title:'Persisted details probe',description:'UTF-8: ação e café',status:'in_review',priority:'urgent',assignee:'Lin'};
const form=(status,extra='')=>`<aside id="detail"><h2>Persisted details probe</h2><form><label for="d">Description</label><textarea id="d" readonly></textarea><label for="s">Status</label><select id="s" disabled><option value="backlog">Backlog</option><option value="in_review">In review</option></select><label for="p">Priority</label><select id="p" disabled><option value="low">Low</option><option value="urgent">Urgent</option></select><label for="a">Assignee</label><input id="a" readonly>${extra}</form><button>Delete ticket</button></aside><script>d.value='UTF-8: ação e café';s.value='${status}';p.value='urgent';a.value='Lin'</script>`;
const verdict=async()=>probe.assertTicketDetails(page,ticket).then(()=>'accepted',()=>'rejected');
await page.setContent(form('in_review'));const controls=await verdict();
await page.setContent(form('backlog'));const unselected=await verdict();
await page.setContent(form('backlog','<input type="radio" name="r" value="in_review">'));const radio=await verdict();
await page.setContent(form('in_review').replace(/ readonly| disabled/g,''));const editable=await verdict();
console.log(JSON.stringify({controls,unselected,radio,editable}));
""", browser=True)
        self.assertEqual(result, {'controls': 'accepted', 'unselected': 'rejected', 'radio': 'rejected',
                                  'editable': 'rejected'})

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

    def late_get(self, designs):
        return self.node("""
const designs=DESIGNS;
const {createServer}=await import('node:http');
let title;
const server=createServer((request,response)=>{
 const url=new URL(request.url,'http://kanban.test');
 if(url.pathname==='/api/tickets/1'){response.setHeader('content-type','application/json');response.end(JSON.stringify({ticket:{title}}));return}
 response.setHeader('content-type','text/html');
 response.end(`<h1>Ticket</h1><script>const read=()=>fetch('/api/tickets/1').then(response=>response.json()).then(body=>body.ticket.title);const show=(text)=>{document.querySelector('h1').textContent=text};${designs[url.searchParams.get('design')]}</script>`);
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
page.setDefaultTimeout(8_000);
const outcome={};
try{
 for(const design of Object.keys(designs)){
  title='Response ordering probe';
  await page.goto(`http://127.0.0.1:${server.address().port}/?design=${design}`);
  await page.getByRole('heading',{name:title}).waitFor();
  outcome[design]=await probe.assertLateGetKeepsNewerState(page,'**/api/tickets/1',title,'Newer GET state',async()=>{title='Newer GET state';await page.evaluate(()=>change())})
   .then(()=>'kept',error=>error.message);
  await page.unroute('**/api/tickets/1');
 }
}finally{server.closeAllConnections();server.close()}
console.log(JSON.stringify(outcome));
""".replace('DESIGNS', json.dumps(designs)), browser=True)

    def test_late_get_accepts_reads_that_drop_or_queue_behind_the_stale_response(self):
        result = self.late_get({
            'latest-wins': 'let seq=0;const load=async()=>{const mine=++seq;const title=await read();if(mine===seq)show(title)};window.change=load;load()',
            # 0.45.0 C7 r01: one read at a time; a change during a read queues another and drops the late result.
            'serialised': 'let seq=0,loading=false,queued=false;const load=async()=>{if(loading){queued=true;return}loading=true;const mine=seq;'
                          'try{const title=await read();if(mine!==seq){queued=true;return}show(title)}finally{loading=false;if(queued){queued=false;load()}}};'
                          'window.change=()=>{seq++;load()};load()',
        })
        self.assertEqual(result, {'latest-wins': 'kept', 'serialised': 'kept'})

    def test_late_get_rejects_a_stale_response_rendered_over_newer_state(self):
        result = self.late_get({
            'last-response-wins': 'const load=async()=>show(await read());window.change=load;load()',
            # The stale title flashes over newer state until the next poll restores it.
            'polling': 'const load=async()=>show(await read());window.change=load;load();setInterval(load,300)',
            # The stale title lands 200 ms after its response, after a check that looked only right away.
            'deferred-polling': 'const load=async()=>{const title=await read();setTimeout(()=>show(title),200)};window.change=load;load();setInterval(load,300)',
        })
        self.assertEqual(result, {'last-response-wins': 'late GET replaced newer state',
                                  'polling': 'late GET rendered over newer state',
                                  'deferred-polling': 'late GET rendered over newer state'})
        # The stale title lands 700 ms after its response and stays.
        self.assertEqual(self.late_get({'deferred': 'const load=async()=>{const title=await read();setTimeout(()=>show(title),700)};window.change=load;load()'}),
                         {'deferred': 'late GET replaced newer state'})
        # A page that never re-reads is named for that, not for a stale overwrite it never made.
        self.assertEqual(self.late_get({'never-rereads': 'const load=async()=>show(await read());window.change=()=>{};load()'}),
                         {'never-rereads': 'newer state did not render after the late GET'})

    def test_list_lanes_are_accepted_without_selecting_outer_board(self):
        result = self.node("""
await page.setContent('<section id="board"><ol><li id="backlog"><h2>Backlog <span>1</span></h2><ul><li>Card</li></ul></li><li><h2>Done <span>1</span></h2></li></ol></section>')
const lane=probe.boardLane(page,'Backlog');
console.log(JSON.stringify({id:await lane.getAttribute('id'),count:await (await probe.boardLaneWithCount(page,'Backlog',1)).count()}));
""", browser=True)
        self.assertEqual(result, {'id': 'backlog', 'count': 1})

    def test_lane_headings_accept_a_separator_before_the_count_without_accepting_another_column(self):
        # 0.38.0 C3 r01 names each column "Backlog, 0 tickets" and shows the count in its own span.
        # "Backlog archive, 0 tickets" fails if the separator admits words, and an indented
        # "10" fails at count 0 if the count tolerates more than whitespace.
        result = self.node("""
const labels=['Backlog','To do','In progress','In review','Done'];
const separators=[',',':',' ·',' –',' -'];
await page.setContent(`<main><p>0 tickets</p><div id="board">${labels.map((label,index)=>`<section class="column"><h2 aria-label="${label}${separators[index]} 0 tickets"><span>${label}</span><span class="column-count">${index?'\\n  0\\n':'0'}</span></h2><div class="column-cards"></div></section>`).join('')}</div></main>`);
const accepted=[],wrongCount=[];
for(const label of labels){
 accepted.push(await (await probe.boardLaneWithCount(page,label,0)).getAttribute('class'));
 wrongCount.push(await probe.boardLaneCount(page,label,1).count());
}
await page.setContent('<main><section><h2 aria-label="Backlog archive, 0 tickets"><span>Backlog archive</span><span>0</span></h2></section><section><h2>To do</h2></section><section><h2>Done</h2><span>\\n  10\\n</span></section></main>');
const archive=await probe.boardLane(page,'Backlog').count();
const tenAsZero=await probe.boardLaneCount(page,'Done',0).count();
console.log(JSON.stringify({accepted,wrongCount,archive,tenAsZero}));
""", browser=True)
        self.assertEqual(result, {'accepted': ['column'] * 5, 'wrongCount': [0] * 5, 'archive': 0, 'tenAsZero': 0})

    def test_a_count_written_in_the_lane_heading_counts_once_at_its_own_number(self):
        # A heading may carry its count in its own text ("Backlog, 0 tickets", "Backlog (0)"), with no
        # separate badge. The 0.38.0 C3 r01 form (aria-label plus a count span) must still count once,
        # a heading with no count (loading) never reads as zero, and "10" is not "0".
        result = self.node("""
const labels=['Backlog','To do','In progress','In review','Done'];
const board=head=>`<main>${labels.map(label=>`<section>${head(label)}<ul></ul></section>`).join('')}</main>`;
const forms={comma:l=>`<h2>${l}, 0 tickets</h2>`,paren:l=>`<h2>${l} (0)</h2>`,space:l=>`<h2>${l} 0</h2>`,colon:l=>`<h2>${l}: 0</h2>`,
 ariaSpan:l=>`<h2 aria-label="${l}, 0 tickets"><span>${l}</span><span>0</span></h2>`,loading:l=>`<h2>${l}</h2>`,ten:l=>`<h2>${l}, 10 tickets</h2>`,
 ariaOnly:l=>`<h2 aria-label="${l}, 0 tickets">${l}</h2>`,ariaAgainstBadge:l=>`<h2 aria-label="${l}, 0 tickets"><span>${l}</span><span>3</span></h2>`,
 ariaAgainstText:l=>`<h2 aria-label="${l}, 0 tickets">${l} 10 tickets (0 selected)</h2>`};
const out={};
for(const [name,head] of Object.entries(forms)){
 await page.setContent(board(head));
 out[name]=[await probe.boardLaneCount(page,'Backlog',0).count(),await probe.boardLaneCount(page,'Backlog',1).count(),await probe.boardLaneCount(page,'Backlog',10).count()];
}
console.log(JSON.stringify(out));
""", browser=True)
        self.assertEqual(result, {'comma': [1, 0, 0], 'paren': [1, 0, 0], 'space': [1, 0, 0], 'colon': [1, 0, 0],
                                  'ariaSpan': [1, 0, 0], 'loading': [0, 0, 0], 'ten': [0, 0, 1],
                                  # What the heading shows wins over its aria-label: no visible count, another
                                  # badge, or a heading that reads as another count with the number elsewhere.
                                  'ariaOnly': [0, 0, 0], 'ariaAgainstBadge': [0, 0, 0], 'ariaAgainstText': [0, 0, 0]})

    def test_settings_navigation_accepts_a_repeated_link_but_follows_the_first_visible_one(self):
        # 0.39.0 C3 r02 links to /settings from both its header and its board actions, which a strict
        # locator rejects as ambiguous. A broken first visible link is still followed (the form checks
        # then fail it), and a missing or renamed link still fails.
        result = self.node("""
let board='';
await page.route('http://kanban.test/**',route=>{const path=new URL(route.request().url()).pathname;route.fulfill({contentType:'text/html',body:path==='/'?board:`<h1>${path}</h1>`})});
const follow=async html=>{board=html;await page.goto('http://kanban.test/');return probe.settingsNavigation(page).click({timeout:500}).then(()=>new URL(page.url()).pathname,error=>error.name)};
console.log(JSON.stringify({
 repeated:await follow('<nav><a href="/settings">Settings</a></nav><main><a class="button-link" href="/settings">Settings</a></main>'),
 collapsedFirst:await follow('<a href="/elsewhere" style="display:block;width:0;height:0;overflow:hidden">Settings</a><a href="/settings">Settings</a>'),
 brokenFirst:await follow('<a href="/elsewhere">Settings</a><a href="/settings">Settings</a>'),
 missing:await follow('<nav><a href="/">Board</a></nav>'),
 otherName:await follow('<a href="/settings">Settings archive</a>'),
}));
""", browser=True)
        self.assertEqual(result, {'repeated': '/settings', 'collapsedFirst': '/settings', 'brokenFirst': '/elsewhere',
                                  'missing': 'TimeoutError', 'otherName': 'TimeoutError'})

    def test_creation_opener_accepts_a_new_ticket_link_without_accepting_a_card_or_other_action(self):
        # 0.39.0 C4 r02 opens its creation modal from <a href="#/tickets/new">New ticket</a>. A card whose
        # title contains "new ticket" must not become the opener even when it comes first.
        result = self.node("""
await page.setContent(`<a href="#/tickets/new">New ticket</a>
<dialog><h2>Create ticket</h2><label>Title <input></label><button aria-label="Close create ticket dialog">Close</button><button>Create ticket</button></dialog>
<script>addEventListener('hashchange',()=>{if(location.hash==='#/tickets/new')document.querySelector('dialog').showModal()})</script>`);
await probe.ticketCreationOpener(page).click();
await probe.ticketCreationDialog(page).waitFor({timeout:5000});
const routed=await probe.ticketCreationDialog(page).count();
const openers=[];
for (const html of ['<button>New ticket</button>','<button>+ New ticket</button>','<a href="#new">New ticket…</a>',
  '<a href="/tickets/9">KAN-9 Write new ticket docs</a><button>New ticket</button>',
  '<a href="/tickets/9">KAN-9 Write new ticket docs</a>','<button>Edit ticket</button>','<button>Create</button>',
  '<button hidden>New ticket</button>','<a href="/tickets/1">KAN-1 Browser-created ticket</a>']) {
  await page.setContent(html);
  const opener=probe.ticketCreationOpener(page);
  openers.push(await opener.count() && /new ticket/i.test(await opener.textContent()) && !/KAN-9/.test(await opener.textContent()) ? 1 : 0);
}
console.log(JSON.stringify({routed,openers}));
""", browser=True)
        self.assertEqual(result, {'routed': 1, 'openers': [1, 1, 1, 1, 0, 0, 0, 0, 0]})

    def test_card_and_detail_values_accept_surrounding_whitespace_without_accepting_other_words(self):
        # Pretty-printed markup indents text nodes; Playwright tests a RegExp against raw textContent.
        # "High priority" and "Not urgent" fail if the tolerance admits more than whitespace.
        result = self.node("""
const card=priority=>`<main><section><h2>Backlog <span>1</span></h2><div class="card"><h3>Target</h3><p>KAN-1</p><p>\\n  ${priority}\\n</p><p>Grace</p></div></section><section><h2>Done <span>0</span></h2></section></main>`;
const region=async()=>(await probe.ticketCardRegion(probe.boardLane(page,'Backlog'),{title:'Target',key:'KAN-1',priority:'high',assignee:'Grace'})).evaluate(element=>element.tagName);
await page.setContent(card('High'));const indentedCard=await region();
await page.setContent(card('High priority'));const otherCard=await region();
const ticket={key:'KAN-1',title:'Ticket',description:'Details',status:'todo',priority:'urgent',assignee:'Lin'};
const detail=priority=>`<section id="detail"><header><h2>Ticket</h2><button>Delete ticket</button></header><p>KAN-1</p><p>Details</p><p>\\n  To do\\n</p><p>\\n  ${priority}\\n</p><p>Lin</p></section>`;
const verdict=async()=>probe.assertTicketDetails(page,ticket).then(()=>'accepted',()=>'rejected');
await page.setContent(detail('Urgent'));const indentedDetail=await verdict();
await page.setContent(detail('Not urgent'));const otherDetail=await verdict();
console.log(JSON.stringify({indentedCard,otherCard,indentedDetail,otherDetail}));
""", browser=True)
        self.assertEqual(result, {'indentedCard': 'DIV', 'otherCard': 'H3',
                                  'indentedDetail': 'accepted', 'otherDetail': 'rejected'})

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

    def test_card_metadata_accepts_a_field_label_without_accepting_another_value_or_label(self):
        # 0.40.0 C3 r02 writes "Assignee: Grace" in one paragraph. "Graceful" fails if the value
        # is unanchored, and "Reviewer: Grace" if any label is allowed before it.
        result = self.node("""
const ticket={title:'Backlog probe',key:'KAN-1',priority:'high',assignee:'Grace'};
const card=(priority,assignee)=>`<main><section><h2>Backlog <span>1</span></h2><article class="card"><div class="card-meta"><span>KAN-1</span><span>${priority}</span></div><h3>Backlog probe</h3><p>${assignee}</p></article></section><section><h2>Done <span>0</span></h2></section></main>`;
const found={};
for(const [name,priority,assignee] of [['bare','high','Grace'],['labeled','high','Assignee: Grace'],['assignedTo','Priority: high','Assigned to Grace'],['otherValue','high','Assignee: Graceful'],['otherLabel','high','Reviewer: Grace'],['otherPriority','High priority','Grace']]){
 await page.setContent(card(priority,assignee));
 const region=await probe.ticketCardRegion(probe.boardLane(page,'Backlog'),ticket);
 found[name]=await region.evaluate(element=>element.tagName);
}
console.log(JSON.stringify(found));
""", browser=True)
        self.assertEqual(result, {'bare': 'ARTICLE', 'labeled': 'ARTICLE', 'assignedTo': 'ARTICLE',
                                  'otherValue': 'H3', 'otherLabel': 'H3', 'otherPriority': 'H3'})

    def test_card_fields_name_the_missing_field(self):
        result = self.node("""
console.log(JSON.stringify(probe.cardFields({key:'KAN-1',priority:'high',assignee:''}).map(({field})=>field)));
""")
        self.assertEqual(result, ['readable key', 'priority'])

    def test_board_load_failure_accepts_could_not_wording_but_not_loading_or_empty_states(self):
        # 0.40.0 C3 r01 alerts "Could not load tickets (HTTP 500)." and r03's status reads
        # "Tickets could not be loaded."; the failure still has to be announced in an alert or status.
        result = self.node("""
const found=[];
for(const html of ['<p role="alert">Could not load tickets (HTTP 500).</p>','<p role="status">Tickets could not be loaded.</p>',
 '<p role="alert">Couldn’t load tickets</p>',"<p role=\\"status\\">Can't reach the server</p>",'<p role="alert">Cannot load tickets</p>',
 '<p role="alert">Unable to load tickets (500).</p>','<p role="status">Failed to load</p>','<p role="alert">Error 500</p>',
 '<p role="status">Loading tickets…</p>','<p role="status">No tickets yet</p>','<p role="alert">Tickets loaded</p>','<p>Unable to load tickets (500).</p>']){
 await page.setContent(html);found.push(await probe.boardLoadFailure(page).count());
}
console.log(JSON.stringify(found));
""", browser=True)
        self.assertEqual(result, [1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0])

    def test_board_loading_accepts_visible_loading_text_outside_a_status_but_not_other_words(self):
        # 0.41.0 C3 r02 showed "Loading tickets…" in a busy section while its only status stayed empty.
        result = self.node("""
const found=[];
for(const html of ['<p role="status" aria-live="polite"></p><section aria-busy="true"><p>Loading tickets…</p></section>',
 '<p role="status">Loading tickets…</p>','<p role="status">Loaded 2 tickets</p>','<div><span>Loading…</span></div>',
 '<p hidden>Loading tickets…</p>','<p>Tickets appear after loading.</p>','<button>Reload</button>','<p>Loaded 0 tickets</p>',
 '<a href="#">Download</a>','<p>lazy-loading enabled</p>']){
 await page.setContent(html);const indicator=probe.boardLoadingIndicator(page);
 found.push(await indicator.count()?(await indicator.first().textContent()).trim():null);
}
console.log(JSON.stringify(found));
""", browser=True)
        self.assertEqual(result, ['Loading tickets…', 'Loading tickets…', 'Loaded 2 tickets', 'Loading…',
                                  None, None, None, None, None, None])

    def test_loading_text_must_clear_once_the_first_read_lands(self):
        # A stuck spinner (CSS overriding hidden) or static help text is not a loading state.
        result = self.node("""
const found=[];
for(const html of ['<section hidden><p>Loading tickets…</p></section><p role="status">Loaded 0 tickets</p>',
 '<p role="status"></p><p>0 tickets</p>','<p role="status">Board up to date.</p>',
 '<style>.l{display:grid}</style><section class="l" hidden><p>Loading tickets…</p></section>',
 '<p>Loading tickets can take a few seconds after the engine starts.</p>','<p role="status">Loading tickets…</p>']){
 await page.setContent(html);found.push(await probe.loadingText(page).count());
}
console.log(JSON.stringify(found));
""", browser=True)
        self.assertEqual(result, [0, 0, 0, 1, 1, 1])

    def test_comment_body_accepts_the_subjects_own_label_but_never_the_author_or_a_ticket_field(self):
        # 0.41.0 C6 r03 labelled its body "Comment body"; the prompt names no label for it.
        result = self.node("""
const form=(body)=>`<form><label for="author">Author</label><input id="author"><label for="body">${body}</label><textarea id="body"></textarea></form>`;
const ids=async(locator)=>locator.evaluateAll(nodes=>nodes.map(node=>node.id));
const found=[];
for(const html of [form('Comment'),form('Comment body'),form('Your comment'),form('Comment text'),form('Body'),
 form('Add a comment'),form('Write a comment…'),form('Add comment'),form('Comment:'),form('Comment *'),form('Comment (required)'),
 form('Write your comment'),form('Type a comment…'),form('Post a comment'),
 form('Comments'),form('Notes'),form('Description'),form('Comment author'),form('Commenter'),form('Comment by'),
 '<form><label for="d">Description</label><textarea id="d"></textarea><label for="a">Assignee name</label><input id="a"></form>',
 '<div hidden><form><label for="stale">Comment</label><textarea id="stale"></textarea></form></div>'+form('Comment body')]){
 await page.setContent(html);found.push([await ids(probe.commentBody(page)),await ids(probe.commentAuthor(page))]);
}
console.log(JSON.stringify(found));
""", browser=True)
        accepted, rejected = [['body'], ['author']], [[], []]
        self.assertEqual(result, [accepted] * 14 + [rejected] * 7 + [accepted])

    def test_status_only_patch_accepts_a_wrapped_status_but_not_other_fields(self):
        # 0.40.0 C5 r02 sends {"changes":{"status":"done"}} and its server saves only the status.
        result = self.node("""
const bodies=[{status:'done'},{changes:{status:'done'}},{ticket:{status:'done'}},{status:'todo'},{changes:{status:'done',title:'x'}},
 {status:'done',title:'x'},{changes:{status:'done'},title:'x'},{a:{status:'done'},b:{status:'done'}},null,[{status:'done'}],'done'];
console.log(JSON.stringify(bodies.map(body=>probe.statusOnlyPatch(body,'done'))));
""")
        self.assertEqual(result, [True, True, True, False, False, False, False, False, False, False, False])

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

    def test_reply_named_jump_reaches_the_parent_but_a_reply_composer_stays_untouched(self):
        # 0.43.0 C6 r01 named its parent jump "Reply to Alice, posted …. Jump to that comment."
        result = self.node("""
await page.setContent('<ul><li id="parent" tabindex="-1">Alice: first</li><li id="reply"><button id="compose" aria-label="Reply to the original comment" aria-controls="reply-form">↩</button><button id="jump" aria-label="Reply to Alice, posted 10:00. Jump to that comment.">Reply to Alice</button>second<form id="reply-form"></form></li></ul>');
const entry=page.locator('#reply');const parent=await page.locator('#parent').elementHandle();
await page.evaluate(()=>{window.destructive=0;document.querySelector('#compose').onclick=()=>window.destructive++;document.querySelector('#jump').onclick=()=>document.querySelector('#parent').focus()});
const navigated=await probe.activateCommentParent(entry,parent,page);
console.log(JSON.stringify({navigated,destructive:await page.evaluate(()=>window.destructive)}));
""", browser=True)
        self.assertEqual(result, {'navigated': True, 'destructive': 0})

    def test_reply_named_move_to_reaches_the_parent_but_reply_composers_stay_untouched(self):
        # 0.45.0 C6 r01 timeline: the parent jump says "Move to that comment." and each entry has a "Reply" composer.
        timeline = """<ol id="timeline" class="timeline" aria-label="Comments and replies in posting order"><li><article class="comment" id="comment-12e6ef05-5b3f-42e0-b922-d1c6647b126b" data-comment-id="12e6ef05-5b3f-42e0-b922-d1c6647b126b" tabindex="-1"><p class="comment-meta"><span class="comment-author">Alice</span> · <time datetime="2026-11-24T01:12:48.797Z">Nov 24, 2026, 1:12 AM</time></p><p class="comment-body">&lt;img src=x onerror=alert(1)&gt; first</p><button type="button" class="comment-reply" aria-label="Reply to Alice's comment">Reply</button></article></li><li><article class="comment" id="comment-62ea0a03-6719-4129-a909-062fe72924cc" data-comment-id="62ea0a03-6719-4129-a909-062fe72924cc" tabindex="-1"><p class="comment-meta"><span class="comment-author">Bob</span> · <time datetime="2026-11-24T01:12:48.896Z">Nov 24, 2026, 1:12 AM</time></p><p class="comment-parent-line"><button type="button" class="comment-parent-link" aria-label="Reply to Alice's comment from Nov 24, 2026, 1:12 AM. Move to that comment.">↳ Reply to Alice</button></p><p class="comment-body">second</p><button type="button" class="comment-reply" aria-label="Reply to Bob's comment">Reply</button></article></li><li><article class="comment" id="comment-e7d863ed-7be2-4a25-88a1-6a0865e13691" data-comment-id="e7d863ed-7be2-4a25-88a1-6a0865e13691" tabindex="-1"><p class="comment-meta"><span class="comment-author">Carol</span> · <time datetime="2026-11-24T01:12:48.932Z">Nov 24, 2026, 1:12 AM</time></p><p class="comment-parent-line"><button type="button" class="comment-parent-link" aria-label="Reply to Bob's comment from Nov 24, 2026, 1:12 AM. Move to that comment.">↳ Reply to Bob</button></p><p class="comment-body">third</p><button type="button" class="comment-reply" aria-label="Reply to Carol's comment">Reply</button></article></li></ol>"""
        result = self.node("""
await page.setContent(TIMELINE);
await page.evaluate(()=>{window.pressed=[];document.addEventListener('click',event=>{const button=event.target.closest('button');window.pressed.push(button.getAttribute('aria-label'));
 if(button.classList.contains('comment-parent-link'))button.closest('li').previousElementSibling.querySelector('article').focus()})});
const entry=page.getByText('second',{exact:true}).locator('xpath=ancestor::*[self::li or self::article or @role="listitem"][1]');
const parent=await page.locator('article').first().elementHandle();
const link=entry.locator('.comment-parent-link');const name=await link.getAttribute('aria-label');
const outcome={};
// Composer-like names on the very control that would navigate must still never be pressed.
for(const label of ["Reply to Alice's comment",'Reply to the original comment','Reply','Reply to Hugo to add context']){
 await link.evaluate((element,label)=>element.setAttribute('aria-label',label),label);outcome[label]=await probe.activateCommentParent(entry,parent,page);
}
await link.evaluate((element,name)=>element.setAttribute('aria-label',name),name);
outcome.navigated=await probe.activateCommentParent(entry,parent,page);
// Rubric 8 let a bare "jump" through the Reply guard; it still does.
await page.evaluate(()=>document.activeElement.blur());
await link.evaluate(element=>element.setAttribute('aria-label','Reply to Alice (jump)'));
outcome.bareJump=await probe.activateCommentParent(entry,parent,page);
console.log(JSON.stringify({outcome,pressed:await page.evaluate(()=>window.pressed)}));
""".replace('TIMELINE', json.dumps(timeline)), browser=True)
        self.assertEqual(result, {
            'outcome': {"Reply to Alice's comment": False, 'Reply to the original comment': False, 'Reply': False,
                        'Reply to Hugo to add context': False, 'navigated': True, 'bareJump': True},
            'pressed': ["Reply to Alice's comment from Nov 24, 2026, 1:12 AM. Move to that comment.", 'Reply to Alice (jump)'],
        })

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

    def test_error_feedback_accepts_negated_verb_wording_but_not_progress_success_or_drafts(self):
        # 0.45.0 C4 r02 creation dialog after the forced 500.
        dialog = """<dialog id="create-dialog" aria-labelledby="create-heading" open=""><form id="create-form" novalidate=""><h2 id="create-heading">New ticket</h2><div class="field"><label for="new-title">Title</label><input id="new-title" name="title" required="" autofocus="" aria-describedby="create-error" style=""></div><div class="field"><label for="new-description">Description</label><textarea id="new-description" name="description" rows="4" style=""></textarea></div><div class="field-row"><div class="field"><label for="new-status">Status</label><select id="new-status" name="status"><option value="backlog">Backlog</option><option value="todo">To do</option><option value="in_progress">In progress</option><option value="in_review">In review</option><option value="done">Done</option></select></div><div class="field"><label for="new-priority">Priority</label><select id="new-priority" name="priority"><option value="low">Low</option><option value="medium" selected="">Medium</option><option value="high">High</option><option value="urgent">Urgent</option></select></div></div><div class="field"><label for="new-assignee">Assignee</label><input id="new-assignee" name="assignee" autocomplete="off" style=""></div><p id="create-error" class="form-error" role="alert">The ticket was not created (500). Please try again.</p><p id="create-status" role="status" aria-live="polite"></p><div class="actions"><button id="create-submit" type="submit">Create ticket</button><button id="create-cancel" type="button" class="secondary">Cancel</button></div></form></dialog>"""
        result = self.node("""
await page.setContent(DIALOG);
const found=[await probe.mutationFailureFeedback(page).allTextContents()];
// An auxiliary other than was/were also reads as an idle draft or notice, which the callers cannot tell from a failure.
for(const text of ["Ticket wasn't saved.",'Ticket wasn’t saved.','Your changes were not saved.','The ticket could not be moved.',
 'Creating ticket…','Ticket created.','Ticket deleted.','Saving changes…','Ticket not found. It may have been deleted.','Not saved yet','Cannot find that ticket',
 'Changes have not been saved yet.',"Your edits haven't been saved.",'This draft has not been saved',"Comments haven't been posted yet","This ticket can't be deleted right now."]){
 await page.setContent(`<p role="status">${text}</p>`);found.push(await probe.mutationFailureFeedback(page).count());
}
console.log(JSON.stringify(found));
""".replace('DIALOG', json.dumps(dialog)), browser=True)
        self.assertEqual(result, [['The ticket was not created (500). Please try again.'], 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])

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
 check:async(id,...flow)=>{if(id==='discussion_survives_edit_delete_and_restart')await flow.at(-1)()},
});
console.log(JSON.stringify({stages,retained:tickets.filter(t=>t.deleted_at&&t.comments.length>0).length}));
""")
        self.assertEqual(result, {'stages': ['discussion_preservation', 'discussion_restart'], 'retained': 1})

    def test_discussion_store_without_the_fixture_is_unverified_not_a_control_error(self):
        result = self.node("""
const tickets=[];const records=[];const recorder=probe.createCheckRecorder(records);
const trigger=async(fn,payload={})=>{
 if(fn==='kanban::tickets::create'){const t={id:`id-${tickets.length}`,key:`KAN-${tickets.length+1}`,title:payload.title,comments:[]};tickets.push(t);return structuredClone(t)}
 const t=tickets.find(({id})=>id===payload.id);
 if(fn==='kanban::tickets::comment')t.comments.push({id:`c-${t.comments.length}`,...payload.comment});
 return structuredClone(t);
};
await probe.PROBES.kanban_c6_discussion({trigger,stage:()=>{},api:async()=>({status:400}),
 control:async op=>op==='read_store'?'{"version":1,"records":[]}':undefined,
 check:async(id,action)=>{if(id==='discussion_invalid_input_and_corrupt_store_fail_closed')await recorder.check(id,action)}});
console.log(JSON.stringify(records));
""")
        self.assertEqual([(c['status'], c['detail'].split(':')[0]) for c in result],
                         [('unverified', 'Cannot plant a malformed discussion')])

    def test_duplicate_planting_follows_the_shape_of_the_persisted_store(self):
        # The task fixes no store shape: one keyed by id cannot hold a duplicate record.
        for shape, status, detail in [
            ('{version:1,records:[]}', 'unverified', 'Cannot plant a duplicate'),
            ('{tickets:Object.fromEntries(tickets.map(t=>[t.id,t]))}', 'passed',
             'no duplicate record could be planted'),
        ]:
            with self.subTest(shape=shape):
                result = self.node("""
const tickets=[];const records=[];const recorder=probe.createCheckRecorder(records);let store;
const trigger=async(fn,payload={})=>{
 const now=new Date().toISOString();
 if(fn==='kanban::tickets::create'){const t={status:'backlog',priority:'medium',description:'',assignee:null,...payload,id:crypto.randomUUID(),key:`KAN-${tickets.length+1}`,title:payload.title.trim(),created_at:now,updated_at:now};tickets.push(t);return structuredClone(t)}
 if(fn==='kanban::tickets::list'){if(store?.startsWith('{broken'))throw new Error('corrupt');return {tickets:structuredClone(tickets)}}
 return structuredClone(tickets.find(({id,key})=>id===payload.id||key===payload.id));
};
await probe.PROBES.kanban_c2_persistence({trigger,stage:()=>{},api:async()=>({status:200,json:async()=>({})}),
 control:async(op,p)=>{if(op==='read_store')return store??JSON.stringify(SHAPE);if(op==='write_store')store=p.value},
 check:async(id,...flow)=>id==='persistence_iii_crud'?(await flow.at(-1)(),true)
  :(id==='persistence_corrupt_and_duplicate_store_fail_closed'&&recorder.check(id,...flow))});
console.log(JSON.stringify(records));
""".replace('SHAPE', shape))
                self.assertEqual([c['status'] for c in result], [status])
                self.assertIn(detail, result[0]['detail'])


if __name__ == '__main__':
    unittest.main()
