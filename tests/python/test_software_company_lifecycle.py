"""Qualify local product stages with real Git and the pinned reference product.

GitHub evidence is mocked here; its remote contract has separate tests. The real
verifier runs directly, while isolation tests cover the OS boundary.
"""
from contextlib import redirect_stdout
from io import StringIO
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
ASSETS = ROOT / "src/scenarios/swe_service"
sys.path.insert(0, str(ASSETS))
import controller


class CompanyLifecycle(unittest.TestCase):
    def setUp(self):
        self.github_checks = []
        temporary = tempfile.TemporaryDirectory(prefix="company-curriculum-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.fixture = self.root / "fixture"
        subprocess.run(["git", "clone", "--quiet", str(ROOT / "tests/fixtures/campaign/swe-service.bundle"),
                        str(self.fixture)], check=True, capture_output=True)
        self.workspace = self.root / "workspace"
        self.state = self.root / "trusted/state.json"
        isolation = self.root / "test-isolation.py"
        isolation.write_text("import argparse,subprocess,sys\n"
            "p=argparse.ArgumentParser();p.add_argument('--probes');a,rest=p.parse_known_args()\n"
            "raise SystemExit(subprocess.call([sys.executable,'-I',a.probes,*rest]))\n")
        self.invoke("prepare", "--fixture-root", self.fixture, "--workspace", self.workspace,
            "--state-file", self.state, "--probes", ASSETS / "probes.py", "--isolation", isolation,
            "--mode", "lifecycle", "--ticket", 1, "--fixture-revision",
            "ab373b11ae167ef853f5b5c5184cdcd431a444ea")

    def invoke(self, *args):
        output = StringIO()
        with patch.object(controller.github_ops, "refresh", return_value={}), \
             patch.object(controller.github_ops, "checks", return_value=self.github_checks), \
             patch.object(controller.github_ops, "refresh"), \
             patch.object(sys, "argv", [str(ASSETS / "controller.py"), *map(str, args)]), \
             redirect_stdout(output):
            self.assertEqual(controller.main(), 0, output.getvalue())
        return json.loads(output.getvalue())

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.workspace), *args], text=True).strip()

    def commit(self):
        self.git("add", "-A")
        self.git("commit", "--allow-empty", "-qm", "Company checkpoint")
        return self.git("rev-parse", "HEAD")

    def document(self, name, value):
        (self.workspace / "docs" / f"{name}.json").write_text(json.dumps(value))

    def submit(self, stage, revision=None):
        args = ["checkpoint", "--state-file", self.state, "--ticket", stage, "--head", self.commit()]
        if revision:
            args += ["--revision-id", revision]
        return self.invoke(*args)

    def product(self, stage):
        source = self.fixture / f"swe-service/snapshots/{stage:02}/src"
        shutil.rmtree(self.workspace / "src")
        shutil.copytree(source, self.workspace / "src")

    def request(self):
        self.document("request", {"goal": "Reliable customer data", "stakeholders": ["support"],
            "acceptance": [{"id": key, "behavior": behavior} for key, behavior in [
                ("config", "Each instance retains explicit overrides"),
                ("cache", "Reads observe committed revisions"),
                ("replay", "All supplied events are persisted in order")]]})
        return self.submit(1)

    def plan(self):
        self.assertEqual(self.request()["status"], "accepted")
        self.document("plan", {"work_items": [{"id": "service", "owner": "engineering",
            "requirements": ["config", "cache", "replay"],
            "source_paths": ["src/profile_service/config.py"],
            "test_paths": ["tests/agent/test_configuration.py"], "depends_on": []}],
            "interfaces": ["CLI, HTTP and Python service"], "risks": ["Retain configuration precedence"]})
        self.assertEqual(self.submit(2)["status"], "accepted")

    def test_full_reference_lifecycle_with_red_build_and_late_incompatible_canary(self):
        self.plan()
        rejected = self.submit(3)
        self.assertEqual(rejected["status"], "rejected", "defective base must not satisfy implementation")
        tests = self.workspace / "tests/agent/test_configuration.py"
        tests.write_text("import json,tempfile,unittest\nfrom pathlib import Path\n"
            "from profile_service.service import Service\n"
            "class Configuration(unittest.TestCase):\n"
            " def test_environment_overrides_file(self):\n"
            "  with tempfile.TemporaryDirectory() as temporary:\n"
            "   root=Path(temporary);config=root/'config.json'\n"
            "   config.write_text(json.dumps({'greeting':'file'}))\n"
            "   service=Service(root/'db.sqlite',config,{'PROFILE_GREETING':'environment'})\n"
            "   self.assertEqual(service.settings()['greeting'],'environment')\n")
        self.product(3)
        build = self.submit(3)
        self.assertEqual(build["status"], "accepted")
        self.product(4)
        self.document("review", {"reviewed_head": build["accepted_head"], "decision": "approve", "findings": []})
        reviewed = self.submit(4)
        self.assertEqual(reviewed["status"], "accepted")
        self.product(5)
        client = self.workspace / "src/profile_service/client.py"
        original_client = client.read_text()
        client.write_text(original_client.replace('return payload["name"]', 'return payload["display_name"]'))
        self.document("release", {"version": "1.0.0", "entrypoint": "src/profile_service/__main__.py",
                                  "rollback_head": reviewed["accepted_head"]})
        canary = self.submit(5)
        self.assertEqual(canary["status"], "revision_required")
        self.assertFalse(canary["canary_observation"]["passed"])
        client.write_text(original_client)
        released = self.submit(5, canary["revision_id"])
        self.assertEqual(released["status"], "accepted")
        self.product(6)
        self.assertEqual(self.submit(6)["status"], "accepted")
        self.product(7)
        self.document("incident", {"release_head": released["accepted_head"], "symptom": "Replay work grows",
            "cause": "Repeated full-history reads", "mitigation": "Indexed acknowledgement lookup",
            "regression_test": "tests/agent/test_configuration.py"})
        self.assertEqual(self.submit(7)["status"], "accepted")
        self.product(8)
        (self.workspace / "docs/delivery.md").write_text("Operations owns rollout, rollback, API compatibility and replay recovery.\n")
        self.document("handoff", {"release_head": released["accepted_head"], "owner": "operations",
            "regression_tests": ["tests/agent/test_configuration.py"], "runbook": "docs/delivery.md"})
        self.assertEqual(self.submit(8)["status"], "completed")
        report = self.invoke("capture", "--state-file", self.state)
        self.assertEqual(report["accepted_tickets"], list(range(1, 9)))
        self.assertEqual(report["lifecycle"]["release_head"], released["accepted_head"])
        self.assertEqual(report["lifecycle"]["rejected_checkpoints"], 1)
        self.assertTrue(all(stage["score"] == 1 for stage in report["lifecycle"]["stages"]), report["lifecycle"])
        review = next(item for item in report["checkpoints"] if item["ticket"] == 4)
        regression = next(item for item in review["lifecycle_checks"] if item["id"] == "regressions_detect_original_defect")
        self.assertTrue(regression["green"]["passed"])
        self.assertGreater(regression["red"]["failures"], 0)
        self.assertEqual(regression["red"]["errors"], 0)
        self.assertTrue(self.invoke("cleanup", "--state-file", self.state)["cleaned"])
        self.assertFalse(self.workspace.exists())
        self.assertEqual(self.invoke("capture", "--state-file", self.state)["accepted_head"], report["accepted_head"])

    def test_missing_required_demand_is_rejected_without_closing_execution(self):
        for _ in range(5):
            result = self.submit(1)
            self.assertEqual(result["status"], "rejected")
            self.assertIn("request.json", result["feedback"])
        self.plan()

    def test_missing_authored_tests_rejects_implementation_without_infrastructure_error(self):
        self.plan()
        self.product(3)
        shutil.rmtree(self.workspace / "tests/agent")
        build = self.submit(3)
        self.assertEqual(build["status"], "rejected")
        self.assertIn("authored_regressions", build["feedback"])
        report = self.invoke("capture", "--state-file", self.state)
        self.assertEqual(report["accepted_tickets"], [1, 2])
        self.assertEqual(report["checkpoints"][-1]["verification"]["passed"], True)
        self.assertEqual(report["lifecycle"]["stages"][2]["score"], 0)

    def test_demand_and_plan_cannot_change_source(self):
        source = self.workspace / "src/profile_service/config.py"
        source.write_text(source.read_text() + "\n# premature implementation\n")
        rejected = self.submit(1)
        self.assertEqual(rejected["status"], "rejected")
        self.assertIn("source", rejected["feedback"].lower())

        self.git("restore", "--source", "HEAD^", "--", "src/profile_service/config.py")
        self.assertEqual(self.request()["status"], "accepted")
        source.write_text(source.read_text() + "\n# premature implementation\n")
        self.document("plan", {"work_items": [{"id": "service", "owner": "engineering",
            "requirements": ["config", "cache", "replay"],
            "source_paths": ["src/profile_service/config.py"],
            "test_paths": ["tests/agent/test_configuration.py"], "depends_on": []}],
            "interfaces": ["CLI, HTTP and Python service"], "risks": ["Retain configuration precedence"]})
        rejected = self.submit(2)
        self.assertEqual(rejected["status"], "rejected")
        self.assertIn("source", rejected["feedback"].lower())

    def test_same_commit_can_retry_after_new_github_evidence(self):
        self.github_checks = [{"id": "github.mock", "passed": False, "reason": "Remote evidence pending"}]
        self.document("request", {"goal": "Reliable customer data", "stakeholders": ["support"],
            "acceptance": [{"id": key, "behavior": key} for key in ("config", "cache", "replay")]})
        head = self.commit()
        args = ("checkpoint", "--state-file", self.state, "--ticket", 1, "--head", head)
        self.assertEqual(self.invoke(*args)["status"], "rejected")
        state = json.loads(self.state.read_text())
        state.setdefault("github", {}).setdefault("journal", []).append({"observation": "remote changed"})
        self.state.write_text(json.dumps(state))
        self.github_checks = [{"id": "github.mock", "passed": True, "reason": "Remote evidence observed"}]
        accepted = self.invoke(*args)
        self.assertEqual(accepted["status"], "accepted")
        self.assertEqual(accepted["accepted_head"], head)


if __name__ == "__main__":
    unittest.main()
