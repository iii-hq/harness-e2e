import json
import http.client
import os
import re
import subprocess
import tempfile
import threading
import time
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
PROBE = ROOT / "scripts" / "kanban_eval" / "probe.mjs"
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
