"""Behavior regressions from execution e6f01f0f, including negative controls."""
import json
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PROBE = ROOT / 'scripts/kanban_eval/probe.mjs'
PLAYWRIGHT = ROOT / 'dashboard/node_modules/playwright/index.mjs'


class KanbanScoringRegressions(unittest.TestCase):
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
            script += "const browser=await chromium.launch({headless:true});const page=await browser.newPage();try {\n"
        script += body
        if browser:
            script += '\n} finally {await browser.close()}'
        result = subprocess.run(['node', '--input-type=module', '--eval', script],
                                capture_output=True, text=True, cwd=ROOT, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def test_list_lanes_are_accepted_without_selecting_outer_board(self):
        result = self.node("""
await page.setContent('<section id="board"><ol><li id="backlog"><h2>Backlog <span>1</span></h2><ul><li>Card</li></ul></li><li><h2>Done <span>1</span></h2></li></ol></section>')
const lane=probe.boardLane(page,'Backlog');
console.log(JSON.stringify({id:await lane.getAttribute('id'),count:await (await probe.boardLaneWithCount(page,'Backlog',1)).count()}));
""", browser=True)
        self.assertEqual(result, {'id': 'backlog', 'count': 1})

    def test_parent_accessible_label_variant_still_requires_real_navigation(self):
        result = self.node("""
await page.setContent('<ul><li id="parent" tabindex="-1">Alice: first</li><li id="reply"><button aria-label="Replying to Alice; go to that comment">In reply to Alice</button>second</li></ul>');
const action=probe.commentParentAction(page.locator('#reply'));
const count=await action.count();let inert=false;let navigated=false;
if(count){
 const parent=await page.locator('#parent').elementHandle();const link=await action.elementHandle();
 await action.focus();await action.press('Enter');inert=await page.evaluate(probe.reachedCommentParent,{parent,link});
 await action.evaluate(button=>button.onclick=()=>document.querySelector('#parent').focus());
 await action.press('Enter');navigated=await page.evaluate(probe.reachedCommentParent,{parent,link});
}
console.log(JSON.stringify({count,inert,navigated}));
""", browser=True)
        self.assertEqual(result, {'count': 1, 'inert': False, 'navigated': True})

    def test_error_feedback_accepts_failed_creation_and_deletion_not_success(self):
        result = self.node("""
const found=[];
for(const text of ['probe failure','Failed to create ticket','Unable to delete KAN-1 (500)','Creating…','Ticket created','Delete ticket']){
 await page.setContent(`<p role="alert">${text}</p>`);found.push(await probe.mutationFailureFeedback(page).count());
}
console.log(JSON.stringify(found));
""", browser=True)
        self.assertEqual(result, [1, 1, 1, 0, 0, 0])

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
