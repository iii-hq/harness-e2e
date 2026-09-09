import json
import os
import subprocess
import tempfile
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


if __name__ == "__main__":
    unittest.main()
