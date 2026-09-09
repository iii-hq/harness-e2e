import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SUBJECT = ROOT / "scripts" / "kanban_eval" / "subject.mjs"
CONTAINER = "a" * 64


class KanbanSubjectTest(unittest.TestCase):
    def run_subject(self, *args, env=None):
        return subprocess.run(
            ["node", str(SUBJECT), *args], cwd=ROOT, text=True,
            capture_output=True, check=False, env=env,
        )

    def test_help_needs_no_runtime_dependency(self):
        completed = self.run_subject("--help")
        self.assertEqual(completed.returncode, 0, completed.stderr)
        for value in ("--container", "--prompt-file", "--output", "--engine-url", "--namespace", "--model", "--provider", "III_SDK_MODULE"):
            self.assertIn(value, completed.stdout)

    def test_rejects_non_exact_container_before_loading_sdk(self):
        completed = self.run_subject(
            "--container", "candidate", "--prompt-file", "/prompt", "--output", "/output",
            "--engine-url", "ws://127.0.0.1:1", "--namespace", "my-project",
            "--model", "deepseek-v4-flash", "--provider", "deepseek",
        )
        self.assertEqual(completed.returncode, 2)
        self.assertIn("exact 64-hex Docker ID", completed.stderr)

    def test_mock_run_is_isolated_bounded_and_captures_usage(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "evidence"
            prompt = root / "prompt.txt"
            prompt.write_text("Implement the requested Kanban change.")
            calls = root / "calls.jsonl"
            docker = root / "docker"
            docker.write_text("""#!/bin/sh
printf '%s\\n' \"$*\" >> \"$CALLS\"
if [ \"$1\" = inspect ]; then
  printf '[{"Id":"%s","State":{"Running":true},"Config":{"Labels":{"kanban-eval.role":"candidate"},"User":"1000:1000"},"HostConfig":{"NetworkMode":"none","ReadonlyRootfs":true,"CapDrop":["ALL"],"Tmpfs":{"/workspace":"rw,size=64m","/data":"rw,size=32m"}}}]' \"$2\"
elif [ \"$7\" = pwd ]; then
  printf '/workspace\\n'
elif [ \"$7\" = false ]; then
  if [ -n \"$OVERFLOW\" ]; then
    head -c 262145 /dev/zero
    exit 0
  fi
  printf 'expected test failure\\n' >&2
  exit 1
else
  printf ok
fi
""")
            docker.chmod(0o755)
            sdk = root / "sdk.mjs"
            sdk.write_text("""
import { appendFileSync } from 'node:fs'
export function registerWorker(...worker) {
  appendFileSync(process.env.CALLS, JSON.stringify({worker}) + '\\n')
  let custom
  return {
    registerFunction(id, handler, options) {
      appendFileSync(process.env.CALLS, JSON.stringify({register: {id, options}}) + '\\n')
      custom = {id, handler}
      return {unregister() {}}
    },
    async trigger(request) {
      appendFileSync(process.env.CALLS, JSON.stringify(request) + '\\n')
      if (request.function_id === custom?.id) return custom.handler({...request.payload, _caller_worker_id: 'mock-engine'})
      if (request.function_id === 'router::models::get') return {model: {pricing: {input: 0.1, output: 0.2}}}
      if (request.function_id === 'harness::send') {
        const result = await custom.handler({command: 'false', _caller_worker_id: 'mock-engine'}).catch(error => ({error: error.message}))
        appendFileSync(process.env.CALLS, JSON.stringify({commandResult: result}) + '\\n')
        return {accepted: true, session_id: request.payload.session_id, turn_id: 'turn-1'}
      }
      if (request.function_id === 'harness::status') return {status: 'completed', expects_wake: false}
      if (request.function_id === 'harness::metrics') return {complete: true, totals: {input_tokens: 10, output_tokens: 20, cache_read_tokens: 3, cache_write_tokens: process.env.NO_CACHE ? null : 4, reasoning_tokens: 5, cost_usd: process.env.BAD_METRICS ? null : 0.01}}
      if (request.function_id === 'session::messages') return {messages: [{message: {role: 'assistant', model: process.env.WRONG_MODEL ? 'other' : 'deepseek-v4-flash', provider: 'deepseek'}}]}
      return {}
    },
    async shutdown() {},
  }
}
""")
            env = {**os.environ, "PATH": f"{root}:{os.environ['PATH']}", "CALLS": str(calls), "III_SDK_MODULE": str(sdk)}
            completed = self.run_subject(
                "--container", CONTAINER, "--prompt-file", str(prompt), "--output", str(output),
                "--engine-url", "ws://127.0.0.1:49134", "--namespace", "my-project",
                "--model", "deepseek-v4-flash", "--provider", "deepseek", env=env,
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            result = json.loads((output / "subject.json").read_text())
            self.assertEqual((result["actual_model"], result["actual_provider"]), ("deepseek-v4-flash", "deepseek"))
            self.assertEqual([result[k] for k in ("input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens", "cost_usd")], [10, 20, 3, 4, 0.01])
            records = [json.loads(line) if line.startswith("{") else line.strip() for line in calls.read_text().splitlines()]
            send = next(row for row in records if isinstance(row, dict) and row.get("function_id") == "harness::send")
            functions = send["payload"]["options"]["functions"]
            self.assertEqual(functions["expose"], "agent_trigger")
            self.assertIn(functions["allow"][0], send["payload"]["message"])
            self.assertEqual(len(functions["allow"]), 1)
            self.assertNotIn("engine::functions::list", functions["allow"])
            self.assertEqual(send["payload"]["options"]["max_cost_usd"], 5)
            self.assertEqual(send["payload"]["options"]["max_turns"], 100)
            self.assertEqual(send["payload"]["options"]["max_total_tokens"], 1000000)
            self.assertEqual(send["payload"]["options"]["max_output_tokens"], 65536)
            self.assertEqual(result["cost_cap_usd"], 5)
            self.assertEqual(send["payload"]["session_id"], result["session_id"])
            self.assertTrue(result["send_attempted"])
            self.assertTrue(result["model_invoked"])
            worker = next(row["worker"] for row in records if isinstance(row, dict) and "worker" in row)
            self.assertEqual(worker[1]["namespace"], "my-project")
            preflight = next(row for row in records if isinstance(row, dict) and row.get("function_id") == functions["allow"][0])
            self.assertEqual((preflight["namespace"], preflight["payload"]), ("my-project", {"command": "pwd"}))
            command_result = next(row["commandResult"] for row in records if isinstance(row, dict) and "commandResult" in row)
            self.assertEqual(command_result["exit_code"], 1)
            self.assertIn("expected test failure", command_result["stderr"])
            self.assertTrue(any(isinstance(row, str) and row.startswith(f"exec -w /workspace {CONTAINER} /bin/sh -c pwd") for row in records))
            self.assertEqual((output.stat().st_mode & 0o777), 0o700)

            for name in ("NO_CACHE", "WRONG_MODEL", "BAD_METRICS", "OVERFLOW"):
                failed_output = root / name.lower()
                failed = self.run_subject(
                    "--container", CONTAINER, "--prompt-file", str(prompt), "--output", str(failed_output),
                    "--engine-url", "ws://127.0.0.1:49134", "--namespace", "my-project",
                    "--model", "deepseek-v4-flash", "--provider", "deepseek", env={**env, name: "1"},
                )
                self.assertEqual(failed.returncode, 0 if name == "NO_CACHE" else 2, failed.stderr)
                failure = json.loads((failed_output / "subject.json").read_text())
                self.assertEqual(failure["status"], "completed" if name == "NO_CACHE" else "evaluation_failed")
                if name == "NO_CACHE":
                    self.assertIsNone(failure["cache_write_tokens"])
                if name == "OVERFLOW":
                    self.assertIn("command output exceeded 262144 bytes", failure["result_error"])
                    self.assertIn(f"rm -f {CONTAINER}", calls.read_text())
                self.assertTrue(failure["model_invoked"])

    def test_isolation_failure_precedes_sdk_import(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            prompt = root / "prompt"
            prompt.write_text("prompt")
            docker = root / "docker"
            docker.write_text(f"#!/bin/sh\nprintf '%s' '[{{\"Id\":\"{CONTAINER}\",\"State\":{{\"Running\":true}},\"Config\":{{\"Labels\":{{\"kanban-eval.role\":\"candidate\"}},\"User\":\"1000\"}},\"HostConfig\":{{\"NetworkMode\":\"none\",\"ReadonlyRootfs\":true,\"CapDrop\":[\"ALL\"],\"Tmpfs\":{{}}}}}}]'\n")
            docker.chmod(0o755)
            env = {**os.environ, "PATH": f"{root}:{os.environ['PATH']}", "III_SDK_MODULE": str(root / "missing.mjs")}
            completed = self.run_subject(
                "--container", CONTAINER, "--prompt-file", str(prompt), "--output", str(root / "out"),
                "--engine-url", "ws://127.0.0.1:1", "--namespace", "my-project",
                "--model", "deepseek-v4-flash", "--provider", "deepseek", env=env,
            )
            self.assertEqual(completed.returncode, 2)
            self.assertIn("sized tmpfs", completed.stderr)
            self.assertNotIn("Cannot find module", completed.stderr)


if __name__ == "__main__":
    unittest.main()
