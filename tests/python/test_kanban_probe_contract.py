import json
import http.client
import os
import re
import subprocess
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
PROBE = ROOT / "scripts" / "kanban_eval" / "probe.mjs"
PLAYWRIGHT = ROOT / "dashboard" / "node_modules" / "playwright" / "index.mjs"
CASE_IDS = [
    "kanban_c1_foundation",
    "kanban_c2_persistence",
    "kanban_c3_board",
    "kanban_c4_ticket_flow",
    "kanban_c5_edit_move",
    "kanban_c6_discussion",
    "kanban_c7_live",
]


class KanbanProbeContractTest(unittest.TestCase):
    def test_run_34596086686_equivalent_controls_and_real_accessibility_failure(self):
        if not PLAYWRIGHT.exists():
            self.skipTest("dashboard Playwright is not installed")
        script = f"""
import {{ chromium }} from {json.dumps(PLAYWRIGHT.as_uri())}
import {{ boardTicketTotal, ticketEditor, commentParentAction, reachedCommentParent,
  assertAccessibleFormControls }} from {json.dumps(PROBE.as_uri())}
const browser = await chromium.launch({{headless:true}})
try {{
  const page = await browser.newPage()
  const totals = []
  for (const markup of ['5 tickets', 'Total tickets <output>5</output>', 'Total tickets: 5']) {{
    await page.setContent(`<p>${{markup}}</p><h2>Backlog <span>5</span></h2>`)
    totals.push([await boardTicketTotal(page, 5).count(), await boardTicketTotal(page, 0).count()])
  }}
  const form = '<form><label for="title">Title</label><input id="title" value="Before"><button>Save changes</button><button type="button">Cancel</button></form>'
  await page.setContent(form)
  let edit = await ticketEditor(page)
  await edit.getByLabel('Title').fill('Direct editor')
  const direct = await edit.getByLabel('Title').inputValue()
  await edit.getByRole('button', {{name:'Cancel'}}).click()
  let inertCancelRejected = false
  try {{ await ticketEditor(page, 'Before') }} catch {{ inertCancelRejected = true }}
  await edit.getByRole('button', {{name:'Cancel'}}).evaluate(button => button.onclick = () => document.querySelector('#title').value = 'Before')
  await edit.getByRole('button', {{name:'Cancel'}}).click()
  await ticketEditor(page, 'Before')
  await page.setContent('<button id="open">Edit ticket</button>'+form.replace('<form>', '<form hidden>'))
  await page.locator('#open').evaluate(button => button.onclick = () => document.querySelector('form').hidden = false)
  edit = await ticketEditor(page)
  await edit.getByLabel('Title').fill('Explicit editor')
  const explicit = await edit.getByLabel('Title').inputValue()
  await page.setContent('<ul><li id="parent" tabindex="-1">Alice: first</li><li id="reply"><button aria-label="Show the comment from Alice that this reply answers">↳ Reply to Alice</button>second</li></ul>')
  const action = commentParentAction(page.locator('#reply'))
  await action.evaluate(button => button.onclick = () => document.querySelector('#parent').focus())
  const parent = await page.locator('#parent').elementHandle()
  const link = await action.elementHandle()
  await action.focus()
  await action.press('Enter')
  const navigated = await page.evaluate(reachedCommentParent, {{parent,link}})
  await page.setContent('<p id="status">Settings</p><form><label for="status">Status</label><select id="status"><option>Todo</option></select></form>')
  let duplicate = ''
  try {{ await assertAccessibleFormControls(page.locator('form')) }} catch (error) {{ duplicate = error.message }}
  await page.setContent('<p id="settings-status">Settings</p><form><label for="status">Status</label><select id="status"><option>Todo</option></select></form>')
  await assertAccessibleFormControls(page.locator('form'))
  console.log(JSON.stringify({{totals,direct,explicit,navigated,duplicate,inertCancelRejected}}))
}} finally {{ await browser.close() }}
"""
        completed = subprocess.run(["node", "--input-type=module", "--eval", script],
                                   cwd=ROOT, text=True, capture_output=True, timeout=25)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        result = json.loads(completed.stdout)
        self.assertEqual(result['totals'], [[1, 0], [1, 0], [1, 0]])
        self.assertEqual(result['direct'], 'Direct editor')
        self.assertTrue(result['inertCancelRejected'])
        self.assertEqual(result['explicit'], 'Explicit editor')
        self.assertTrue(result['navigated'])
        self.assertIn('status', result['duplicate'])
        self.assertIn('duplicate', result['duplicate'])

    def test_rejected_foreign_parent_accepts_400_or_404_but_requires_no_write(self):
        script = f"""
import {{ rejectCommentWithoutWrite }} from {json.dumps(PROBE.as_uri())}
const observed = []
for (const [status, changed] of [[400,false],[404,false],[201,false],[400,true],[500,false]]) {{
  let reads = 0
  const control = async () => ++reads === 1 || !changed ? 'original' : 'modified'
  const api = async () => ({{status}})
  try {{
    await rejectCommentWithoutWrite(api, control, '/api/tickets/id/comments', {{parent_id:'foreign'}}, 'foreign parent')
    observed.push('passed')
  }} catch(error) {{ observed.push(error.message) }}
}}
console.log(JSON.stringify(observed))
"""
        completed = subprocess.run(["node", "--input-type=module", "--eval", script],
                                   cwd=ROOT, text=True, capture_output=True)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        result = json.loads(completed.stdout)
        self.assertEqual(result[:2], ['passed', 'passed'])
        self.assertIn('201', result[2])
        self.assertIn('store', result[3])
        self.assertIn('500', result[4])

    def run_probe(self, *args, env=None):
        return subprocess.run(
            ["node", str(PROBE), *args],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
            env=env,
        )

    def test_lists_exact_catalog_cases_without_loading_runtime_dependencies(self):
        completed = self.run_probe("--list-cases")
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(json.loads(completed.stdout), CASE_IDS)

    def test_criterion_dependencies_keep_pass_failure_and_missing_distinct(self):
        script = (
            f"import {{criterionDependencyStatus}} from {json.dumps(PROBE.as_uri())};"
            "const checks=[{id:'passed',status:'passed'},{id:'failed',status:'failed'},{id:'unknown',status:'unverified'}];"
            "console.log(JSON.stringify({"
            "independent:criterionDependencyStatus(checks,['passed']),"
            "failed:criterionDependencyStatus(checks,['failed']),"
            "failedAndMissing:criterionDependencyStatus(checks,['failed','missing']),"
            "unverified:criterionDependencyStatus(checks,['unknown']),"
            "missing:criterionDependencyStatus(checks,['passed','missing'])}))"
        )
        completed = subprocess.run(
            ["node", "--input-type=module", "--eval", script],
            cwd=ROOT, text=True, capture_output=True, check=False,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(json.loads(completed.stdout), {
            "independent": "passed",
            "failed": "failed",
            "failedAndMissing": "failed",
            "unverified": "unverified",
            "missing": "unverified",
        })

    def test_board_semantics_accept_equivalent_dom_without_accepting_wrong_state(self):
        if not PLAYWRIGHT.exists():
            self.skipTest("dashboard Playwright is not installed")
        script = f"""
import {{ chromium }} from {json.dumps(PLAYWRIGHT.as_uri())}
import {{ boardLaneWithCount, boardRefreshButton, boardTicketTotal }} from {json.dumps(PROBE.as_uri())}
const browser = await chromium.launch({{ headless: true }})
try {{
  const page = await browser.newPage()
  await page.setContent(`
    <main>
      <p>5 tickets</p>
      <button id="refresh">Refresh</button>
      <section id="combined"><h2>Backlog <span>1</span></h2><article><h3>Card</h3></article></section>
      <section id="sibling"><header><h2>To do</h2><span aria-label="1 ticket">1</span></header></section>
    </main>`)
  await page.locator('#refresh').evaluate((button) => button.addEventListener('click', () => window.refreshed = true))
  await boardRefreshButton(page).click()
  const valid = {{
    combined: await (await boardLaneWithCount(page, 'Backlog', 1)).count(),
    sibling: await (await boardLaneWithCount(page, 'To do', 1)).count(),
    total: await boardTicketTotal(page, 5).count(),
    refreshed: await page.evaluate(() => window.refreshed === true),
  }}
  await page.locator('#sibling span').evaluate((element) => {{ element.textContent = '2'; element.setAttribute('aria-label', '2 tickets') }})
  let wrongLaneCountRejected = false
  try {{ await boardLaneWithCount(page, 'To do', 1) }} catch {{ wrongLaneCountRejected = true }}
  await page.setContent('<p role="status">Loading tickets…</p><p>…</p><button>Reload</button>')
  const loading = {{ zero: await boardTicketTotal(page, 0).count(), refresh: await boardRefreshButton(page).count() }}
  await page.setContent('<p role="status">Loading tickets…</p><p>0 tickets</p>')
  const fabricatedZero = await boardTicketTotal(page, 0).count()
  await page.setContent('<main></main>')
  await page.locator('main').evaluate((main) => {{
    const title = document.createElement('h3')
    title.textContent = '<img src=x onerror=alert(1)> literal title'
    main.append(title)
  }})
  const safeText = {{
    heading: await page.getByRole('heading', {{ name: '<img src=x onerror=alert(1)> literal title' }}).count(),
    images: await page.locator('img[src="x"]').count(),
  }}
  console.log(JSON.stringify({{ valid, wrongLaneCountRejected, loading, fabricatedZero, safeText }}))
}} finally {{
  await browser.close()
}}
"""
        completed = subprocess.run(
            ["node", "--input-type=module", "--eval", script],
            cwd=ROOT, text=True, capture_output=True, check=False, timeout=20,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(json.loads(completed.stdout), {
            "valid": {"combined": 1, "sibling": 1, "total": 1, "refreshed": True},
            "wrongLaneCountRejected": True,
            "loading": {"zero": 0, "refresh": 0},
            "fabricatedZero": 1,
            "safeText": {"heading": 1, "images": 0},
        })

    def test_store_duplication_preserves_envelopes_and_compose_rejects_console(self):
        script = f"""
import {{ duplicateStoredTicket, standaloneCompose }} from {json.dumps(PROBE.as_uri())}
const array = [{{id:'one', key:'KAN-1'}}]
const envelope = {{version:1, records:[{{id:'one', key:'KAN-1'}}]}}
const duplicated = [array, envelope].map(value => duplicateStoredTicket(value, 'one'))
let unavailable = false
try {{ standaloneCompose({{}}) }} catch {{ unavailable = true }}
console.log(JSON.stringify({{
  duplicated, array, envelope, missing:duplicateStoredTicket({{}}, 'one'), unavailable,
  standalone:standaloneCompose({{containers:[{{container:'kanban'}}]}}),
  console:standaloneCompose({{containers:[{{container:'kanban'}},{{container:'console'}}]}}),
}}))
"""
        completed = subprocess.run(
            ["node", "--input-type=module", "--eval", script],
            cwd=ROOT, text=True, capture_output=True, check=False,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        result = json.loads(completed.stdout)
        self.assertEqual(result["duplicated"], [True, True])
        self.assertEqual(result["envelope"], {"version": 1, "records": result["array"]})
        self.assertEqual(len(result["array"]), 2)
        self.assertFalse(result["missing"])
        self.assertTrue(result["standalone"])
        self.assertFalse(result["console"])
        self.assertTrue(result["unavailable"])

    def test_parent_navigation_does_not_accept_the_back_reference_label(self):
        if not PLAYWRIGHT.exists():
            self.skipTest("dashboard Playwright is not installed")
        script = f"""
import {{ chromium }} from {json.dumps(PLAYWRIGHT.as_uri())}
import {{ reachedCommentParent }} from {json.dumps(PROBE.as_uri())}
const browser = await chromium.launch({{headless:true}})
try {{
  const page = await browser.newPage()
  await page.setContent('<ul><li id="parent" tabindex="-1">first</li><li><button id="back">Reply to Alice: first</button></li></ul>')
  const parent = await page.locator('#parent').elementHandle()
  const link = await page.locator('#back').elementHandle()
  await link.focus()
  const unchanged = await page.evaluate(reachedCommentParent, {{parent,link}})
  await parent.focus()
  const focused = await page.evaluate(reachedCommentParent, {{parent,link}})
  await link.focus()
  await page.evaluate(() => {{ location.hash = '#parent' }})
  const anchored = await page.evaluate(reachedCommentParent, {{parent,link}})
  console.log(JSON.stringify({{unchanged,focused,anchored}}))
}} finally {{ await browser.close() }}
"""
        completed = subprocess.run(
            ["node", "--input-type=module", "--eval", script],
            cwd=ROOT, text=True, capture_output=True, check=False, timeout=20,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(json.loads(completed.stdout), {
            "unchanged": False, "focused": True, "anchored": True,
        })

    def test_invalid_case_writes_a_versioned_evaluation_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            completed = self.run_probe(
                "--case", "unknown",
                "--base-url", "http://127.0.0.1:3000",
                "--engine-url", "ws://127.0.0.1:50179",
                "--output", directory,
            )
            self.assertEqual(completed.returncode, 2)
            result = json.loads((Path(directory) / "result.json").read_text())
            self.assertEqual(result["schema"], "kanban-evaluation/v1")
            self.assertEqual(result["status"], "evaluation_failed")
            self.assertIsNone(result["functional_status"])
            self.assertEqual(result["checks"], [])

    def test_help_documents_required_arguments_and_trusted_modules(self):
        completed = self.run_probe("--help")
        self.assertEqual(completed.returncode, 0)
        for text in ("--case", "--base-url", "--engine-url", "--output", "III_SDK_MODULE", "PLAYWRIGHT_MODULE"):
            self.assertIn(text, completed.stdout)

    def test_trusted_dependency_failure_has_no_functional_verdict_or_complete_coverage(self):
        with tempfile.TemporaryDirectory() as directory:
            env = {**os.environ, "III_SDK_MODULE": "/missing/iii.mjs", "PLAYWRIGHT_MODULE": "/missing/playwright.mjs"}
            completed = self.run_probe(
                "--case", CASE_IDS[0],
                "--base-url", "http://127.0.0.1:3000",
                "--engine-url", "ws://127.0.0.1:50179",
                "--output", directory,
                env=env,
            )
            self.assertEqual(completed.returncode, 0)
            result = json.loads((Path(directory) / "result.json").read_text())
            coverage = json.loads((Path(directory) / "coverage.json").read_text())
            self.assertEqual(result["status"], "evaluation_failed")
            self.assertIsNone(result["functional_status"])
            self.assertFalse(coverage["complete"])

    def test_missing_persistence_functions_remain_a_functional_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sdk = root / "iii.mjs"
            sdk.write_text("""
export function registerWorker() {
  return {
    trigger: async () => { throw new Error('function_not_found') },
    shutdown: async () => {},
  }
}
""")
            playwright = root / "playwright.mjs"
            playwright.write_text("""
export const chromium = {
  launch: async () => ({ contexts: () => [], close: async () => {} }),
}
""")
            output = root / "evidence"
            completed = self.run_probe(
                "--case", "kanban_c2_persistence",
                "--base-url", "http://127.0.0.1:1",
                "--engine-url", "ws://127.0.0.1:1",
                "--output", str(output),
                env={
                    **os.environ,
                    "III_SDK_MODULE": str(sdk),
                    "PLAYWRIGHT_MODULE": str(playwright),
                },
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            result = json.loads((output / "result.json").read_text())
            self.assertEqual(result["status"], "failed")
            self.assertEqual(result["functional_status"], "failed")
            self.assertNotIn("error", result)
            self.assertFalse((output / "control-request.json").exists())
            corrupt = next(check for check in result["checks"] if check["id"] == "persistence_corrupt_and_duplicate_store_fail_closed")
            self.assertEqual(corrupt["status"], "failed")
            self.assertIn("require working create/list/get", corrupt["detail"])

    def test_direct_configuration_waits_for_the_resolved_path(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            state = root / "config.json"
            state.write_text(json.dumps({"data_dir": "./data"}))
            direct_reads = 0

            class Handler(BaseHTTPRequestHandler):
                def log_message(self, *_):
                    pass

                def respond(self, status, value=None, content_type="application/json"):
                    body = json.dumps(value).encode() if value is not None else b""
                    self.send_response(status)
                    self.send_header("content-type", content_type)
                    self.send_header("content-length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)

                def do_GET(self):
                    nonlocal direct_reads
                    if self.path == "/":
                        self.respond(200, None, "text/html")
                    elif self.path == "/api/config":
                        value = json.loads(state.read_text())
                        resolved = f"/workspace/{value['data_dir'].removeprefix('./')}"
                        if value["data_dir"] == "./direct-probe-data":
                            direct_reads += 1
                            if direct_reads == 1:
                                resolved = "/workspace/probe-browser-data"
                        self.respond(200, {**value, "resolved_data_dir": resolved})
                    else:
                        self.respond(404, {"error": "not found"})

                def do_PUT(self):
                    value = json.loads(self.rfile.read(int(self.headers["content-length"])))
                    if not value.get("data_dir"):
                        self.respond(400, {"error": "invalid"})
                        return
                    current = json.loads(state.read_text())
                    current.update(value)
                    state.write_text(json.dumps(current))
                    self.respond(200, {
                        **current,
                        "resolved_data_dir": f"/workspace/{current['data_dir'].removeprefix('./')}",
                    })

            server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
            server_thread = threading.Thread(target=server.serve_forever)
            server_thread.start()
            sdk = root / "iii.mjs"
            sdk.write_text("""
import { readFileSync, writeFileSync } from 'node:fs'
export function registerWorker() {
  return {
    trigger: async ({ function_id, payload }) => {
      if (function_id === 'compose::status') return { containers: [{container:'kanban'}] }
      const value = JSON.parse(readFileSync(process.env.CONFIG_STATE, 'utf8'))
      if (function_id === 'configuration::get') return { value }
      if (function_id === 'configuration::set') {
        writeFileSync(process.env.CONFIG_STATE, JSON.stringify(payload.value))
        return {}
      }
      throw new Error(`unexpected function: ${function_id}`)
    },
    shutdown: async () => {},
  }
}
""")
            playwright = root / "playwright.mjs"
            playwright.write_text("""
export const chromium = {
  launch: async () => ({
    contexts: () => [],
    newContext: async () => { throw new Error('browser omitted by configuration test') },
    close: async () => {},
  }),
}
""")
            output = root / "evidence"
            stop = threading.Event()

            def respond_to_controls():
                seen = set()
                while not stop.wait(.01):
                    request_path = output / "control-request.json"
                    if not request_path.exists():
                        continue
                    try:
                        request = json.loads(request_path.read_text())
                    except (json.JSONDecodeError, OSError):
                        continue
                    if request["id"] in seen:
                        continue
                    seen.add(request["id"])
                    response = {
                        "id": request["id"],
                        "ok": True,
                        "value": {"observed": True} if request["operation"] == "hot_reload" else {},
                    }
                    temporary = output / "control-response.tmp"
                    temporary.write_text(json.dumps(response))
                    temporary.replace(output / "control-response.json")

            control_thread = threading.Thread(target=respond_to_controls)
            control_thread.start()
            try:
                completed = self.run_probe(
                    "--case", "kanban_c1_foundation",
                    "--base-url", f"http://127.0.0.1:{server.server_port}",
                    "--engine-url", "ws://127.0.0.1:1",
                    "--output", str(output),
                    env={
                        **os.environ,
                        "CONFIG_STATE": str(state),
                        "III_SDK_MODULE": str(sdk),
                        "PLAYWRIGHT_MODULE": str(playwright),
                    },
                )
            finally:
                stop.set()
                control_thread.join()
                server.shutdown()
                server.server_close()
                server_thread.join()

            self.assertEqual(completed.returncode, 0, completed.stderr)
            result = json.loads((output / "result.json").read_text())
            configuration = next(check for check in result["checks"] if check["id"] == "foundation_configuration_contract")
            self.assertEqual(configuration["status"], "passed", configuration)
            self.assertGreaterEqual(direct_reads, 2)
            criteria = {
                check["id"]: check["status"]
                for check in result["checks"] if check["id"].startswith("criterion_")
            }
            self.assertEqual(criteria, {
                "criterion_1": "passed",
                "criterion_2": "passed",
                "criterion_3": "failed",
                "criterion_4": "passed",
                "criterion_5": "failed",
            })

    def test_control_request_is_atomic_correlated_and_has_object_payload(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            failures = []

            def respond():
                try:
                    request_path = root / "control-request.json"
                    for _ in range(100):
                        if request_path.exists():
                            request = json.loads(request_path.read_text())
                            break
                        time.sleep(.01)
                    else:
                        raise AssertionError("control request was not published")
                    self.assertEqual(request["operation"], "restart")
                    self.assertEqual(request["payload"], {})
                    (root / "control-response.json").write_text(json.dumps({"id": "stale", "ok": True, "value": "wrong"}))
                    time.sleep(.05)
                    temporary = root / "response.tmp"
                    temporary.write_text(json.dumps({"id": request["id"], "ok": True, "value": {"ready": True}}))
                    temporary.replace(root / "control-response.json")
                except Exception as error:
                    failures.append(error)

            thread = threading.Thread(target=respond)
            thread.start()
            script = f"import {{control}} from {json.dumps(PROBE.as_uri())}; console.log(JSON.stringify(await control({json.dumps(directory)}, 'restart')))"
            completed = subprocess.run(
                ["node", "--input-type=module", "--eval", script],
                cwd=ROOT, text=True, capture_output=True, check=False, timeout=5,
            )
            thread.join()
            self.assertFalse(failures, failures)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assertEqual(json.loads(completed.stdout), {"ready": True})

    def test_inspector_counts_open_collectible_and_deliberately_leaked_sse_responses(self):
        server = r"""
const http = require('node:http')
global.leakedSseResponses = []
const server = http.createServer((request, response) => {
  response.writeHead(200, {'content-type': 'text/event-stream; charset=utf-8', 'cache-control': 'no-cache'})
  response.write(': open\n\n')
  if (request.url === '/leak') request.once('close', () => global.leakedSseResponses.push(response))
})
server.listen(0, '127.0.0.1', () => console.log(server.address().port))
setInterval(() => {}, 1000)
"""
        child = subprocess.Popen(
            ["node", "--inspect=127.0.0.1:0", "--expose-gc", "--eval", server],
            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        connections = []
        try:
            websocket_url = None
            for _ in range(10):
                match = re.search(r"ws://\S+", child.stderr.readline())
                if match:
                    websocket_url = match.group(0)
                    break
            self.assertIsNotNone(websocket_url, "Node did not publish an inspector URL")
            port = int(child.stdout.readline())

            def count():
                script = (
                    f"import {{inspectorClient,countSseServerResponses}} from {json.dumps(PROBE.as_uri())};"
                    f"const client=await inspectorClient({json.dumps(websocket_url)});"
                    "try{console.log(await countSseServerResponses(client))}finally{client.close()}"
                    "setTimeout(()=>process.exit(0),20)"
                )
                completed = subprocess.run(
                    ["node", "--input-type=module", "--eval", script],
                    text=True, capture_output=True, check=False, timeout=8,
                )
                self.assertEqual(completed.returncode, 0, completed.stderr)
                return int(completed.stdout)

            def connect(path):
                connection = http.client.HTTPConnection("127.0.0.1", port, timeout=2)
                connection.request("GET", path)
                response = connection.getresponse()
                self.assertEqual(response.status, 200)
                connections.append((connection, response))

            baseline = count()
            for _ in range(6):
                connect("/clean")
            self.assertGreaterEqual(count(), baseline + 6)
            while connections:
                connection, response = connections.pop()
                response.close()
                connection.close()
            for _ in range(30):
                if count() == baseline:
                    break
                time.sleep(.05)
            else:
                self.fail("closed SSE responses were not collectible")

            connect("/leak")
            connection, response = connections.pop()
            response.close()
            connection.close()
            for _ in range(30):
                if count() >= baseline + 1:
                    break
                time.sleep(.05)
            else:
                self.fail("deliberately retained SSE response was not detected")
        finally:
            for connection, response in connections:
                response.close()
                connection.close()
            child.terminate()
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=3)
            child.stdout.close()
            child.stderr.close()


if __name__ == "__main__":
    unittest.main()
