import json
import pathlib
import sys
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from publish_engineering_endurance import (
    ALLOWED_REPOSITORY,
    PublishError,
    load_report,
    public_projection,
    publish,
)


def report():
    return {
        "scenario_version": 1,
        "initial_head": "a" * 40,
        "accepted_head": "b" * 40,
        "accepted_rungs": 1,
        "total_rungs": 10,
        "terminal_status": "capability_failure",
        "terminal_rung": 2,
        "elapsed_ms": 1234,
        "accepted_checkpoints": 1,
        "rejected_checkpoints": 3,
        "total_changed_lines": 20,
        "accepted_patch": "diff --git a/src/x.py b/src/x.py\n",
        "measurements": [{"id": "max_accepted_rung", "value": 1, "unit": "rungs"}],
        "checkpoints": [
            {
                "rung": 1,
                "ticket_id": "idempotent-submit",
                "attempt": 1,
                "requested_head": "b" * 40,
                "previous_accepted_head": "a" * 40,
                "duration_ms": 100,
                "accepted": True,
                "feedback": "private feedback",
                "evidence": {
                    "public_tests_passed": True,
                    "hidden_probes_passed": True,
                    "worktree_clean": True,
                    "branch_valid": True,
                    "refs_valid": True,
                    "git_config_valid": True,
                    "remotes_valid": True,
                    "ancestry_valid": True,
                    "non_merge_commits": 1,
                    "changed_paths": ["src/durable_queue.py"],
                    "changed_lines": 20,
                    "scope_valid": True,
                    "public_output": "private public log",
                    "hidden_output": "SECRET HIDDEN PROBE DETAIL",
                },
            }
        ],
    }


class PublisherContractTests(unittest.TestCase):
    def test_load_report_rejects_wrong_contract(self):
        value = report()
        value["scenario_version"] = 2
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / "report.json"
            path.write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaisesRegex(PublishError, "unsupported"):
                load_report(path)

    def test_public_projection_excludes_raw_outputs_feedback_and_patch(self):
        projection = public_projection(report(), "123-1")
        encoded = json.dumps(projection, sort_keys=True)
        self.assertNotIn("SECRET HIDDEN PROBE DETAIL", encoded)
        self.assertNotIn("private feedback", encoded)
        self.assertNotIn("accepted_patch", projection)
        self.assertTrue(projection["checkpoints"][0]["evidence"]["hidden_probes_passed"])

    def test_publisher_is_pinned_to_the_fixture_repository_before_network(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / "report.json"
            path.write_text(json.dumps(report()), encoding="utf-8")
            with self.assertRaisesRegex(PublishError, ALLOWED_REPOSITORY):
                publish(
                    repository="iii-hq/not-the-fixture",
                    execution_id="123-1",
                    report_path=path,
                    run_url="",
                )


if __name__ == "__main__":
    unittest.main()
