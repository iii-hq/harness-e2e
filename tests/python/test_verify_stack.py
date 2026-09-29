import importlib.util
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[2] / "scripts" / "verify_stack.py"
SPEC = importlib.util.spec_from_file_location("verify_stack", SCRIPT)
assert SPEC and SPEC.loader
verify_stack = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(verify_stack)

GRAPH = {"harness", "ade", "state", "queue"}
COMMIT = "c46ead4ffb5051ae922888139849dcdb47e07805"


def group(scenario, outcome="passed", valid=True, state="complete"):
    """One group of an execution-summary.json, as the finalizer writes it."""
    return {
        "group_id": f"case-{scenario}",
        "scenarios": [scenario],
        "objective_outcome": outcome,
        "infrastructure_valid": valid,
        "report_state": state,
        "status": "passed",
    }


def summary(*groups, expected=None):
    return {"campaigns": [{"groups": list(groups), "scoring": {"expected_groups": expected or len(groups)}}]}


class BuildStackTests(unittest.TestCase):
    def test_a_changed_worker_is_built_from_the_commit_together_with_the_harness(self):
        stack = verify_stack.build_stack(["ade"], COMMIT, "", GRAPH)
        pin = {"repository": "iii-hq/workers", "commit": COMMIT}
        self.assertEqual(stack["containers"]["ade"], {"worker": "package://ade", **pin})
        # The harness leaves `version: latest` behind: a pin has no version.
        self.assertEqual(stack["containers"]["harness"], {"worker": "package://harness", **pin})
        # What the measurement itself needs stays as the default stack has it.
        self.assertIn("harness-e2e", stack["containers"])

    def test_workers_outside_the_harness_graph_are_not_pinned(self):
        stack = verify_stack.build_stack(["ade", "billing"], COMMIT, "", GRAPH)
        self.assertNotIn("billing", stack["containers"])

    def test_nothing_in_the_graph_means_nothing_to_verify(self):
        self.assertIsNone(verify_stack.build_stack(["billing"], COMMIT, "", GRAPH))

    def test_an_iii_release_changes_only_the_cli(self):
        default = verify_stack.load_yaml(verify_stack.DEFAULT_STACK.read_text())
        stack = verify_stack.build_stack([], "", "0.24.3-rc.1", set())
        self.assertEqual(stack["iii"], "0.24.3-rc.1")
        self.assertEqual(stack["containers"], default["containers"])


class VerdictTests(unittest.TestCase):
    def test_every_group_passing_is_passed(self):
        result = verify_stack.verdict(summary(group("minimal_path"), group("persistent_state")))
        self.assertEqual((result["status"], result["passed"], result["planned"]), ("passed", 2, 2))

    def test_a_measured_failure_is_failed_and_named(self):
        result = verify_stack.verdict(summary(group("minimal_path"), group("shell_coder_sandbox", "failed")))
        self.assertEqual(result["status"], "failed")
        self.assertEqual(result["failed"], ["shell_coder_sandbox"])

    def test_an_inconclusive_group_was_not_measured(self):
        result = verify_stack.verdict(summary(group("minimal_path"), group("timer_wake", "inconclusive", state="partial")))
        self.assertEqual((result["status"], result["not_measured"]), ("not_measured", ["timer_wake"]))

    def test_invalid_infrastructure_is_not_a_product_failure(self):
        result = verify_stack.verdict(summary(group("minimal_path", "failed", valid=False)))
        self.assertEqual((result["status"], result["failed"]), ("not_measured", []))

    def test_a_group_that_never_reported_is_not_measured(self):
        result = verify_stack.verdict(summary(group("minimal_path"), expected=4))
        self.assertEqual((result["status"], result["passed"], result["planned"]), ("not_measured", 1, 4))

    def test_a_failure_is_reported_even_when_another_group_was_not_measured(self):
        result = verify_stack.verdict(summary(group("a", "failed"), group("b", "inconclusive", state="partial")))
        self.assertEqual(result["status"], "failed")

    def test_an_empty_summary_is_not_measured(self):
        self.assertEqual(verify_stack.verdict({"campaigns": []})["status"], "not_measured")


class RunTests(unittest.TestCase):
    def test_the_run_id_comes_from_the_url_gh_prints(self):
        out = "✓ Created workflow_dispatch event\nhttps://github.com/iii-hq/harness-e2e/actions/runs/36251783742\n"
        self.assertEqual(verify_stack.run_id_of(out), 36251783742)

    def test_no_url_is_an_error_not_a_guess(self):
        with self.assertRaises(verify_stack.VerifyError):
            verify_stack.run_id_of("Created workflow_dispatch event")

    def test_the_comment_carries_the_marker_and_names_what_failed(self):
        result = verify_stack.verdict(summary(group("minimal_path"), group("shell_coder_sandbox", "failed")))
        body = verify_stack.describe(result, "ade, harness @ c46ead4", "https://example/run")
        self.assertTrue(body.startswith(verify_stack.MARKER))
        self.assertIn("1/2 passed", body)
        self.assertIn("failed: shell_coder_sandbox", body)


if __name__ == "__main__":
    unittest.main()
