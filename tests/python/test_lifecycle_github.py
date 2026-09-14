import importlib.util
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
MODULE = ROOT / "src/scenarios/swe_service/github_ops.py"
SPEC = importlib.util.spec_from_file_location("github_ops", MODULE)
github = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(github)
HEAD = "a" * 40
NEXT = "b" * 40
TOKEN = "c" * 32


def state(ticket=1):
    return {"mode": "lifecycle", "ownership_token": TOKEN, "current_ticket": ticket,
            "initial_head": HEAD, "objects": "/trusted/repository.git", "assets": "/tmp",
            "canary_revealed": False}


class GitHubBridgeTests(unittest.TestCase):
    def test_external_issue_closure_does_not_credit_subject_operation(self):
        current = state(8)
        current["github"] = {"issue": {"state": "closed"}, "journal": []}
        closed = lambda: next(item for item in github.checks(current, 8, NEXT)
                              if item["id"] == "github.issue_closed")["passed"]
        self.assertFalse(closed())
        current["github"]["journal"].append({"operation": "close_issue", "origin": "subject",
                                            "ticket": 8, "status": "completed"})
        self.assertTrue(closed())

    def test_local_commands_do_not_inherit_github_credentials(self):
        spec = importlib.util.spec_from_file_location("lifecycle_controller", MODULE.with_name("controller.py"))
        controller = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(controller)
        names = ("GH_TOKEN", "GITHUB_TOKEN", "GH_CONFIG_DIR", "HOME")
        with patch.dict(os.environ, {name: "private-credential-control" for name in names}):
            observed = controller.run([sys.executable, "-I", "-c",
                "import json,os;print(json.dumps([k for k in " + repr(names) + " if k in os.environ]))"])
        self.assertEqual(json.loads(observed), [])

    def test_validation_failure_is_journaled_before_any_remote_call(self):
        current = state(4)
        saved = []

        def save(value):
            saved.append(json.loads(json.dumps(value)))

        def reject():
            self.assertEqual(saved[-1]["github"]["journal"][-1]["status"], "intent")
            raise github.BridgeError("protected tree changed")

        with patch.object(github, "initialize") as initialize:
            result = github.operate(current, {"operation": "ci", "head": NEXT}, save, validate=reject)
            initialize.assert_not_called()
        self.assertEqual(result["status"], "failed")
        self.assertEqual(current["github"]["journal"][-1]["status"], "failed")
        self.assertIn("protected tree changed", current["github"]["journal"][-1]["error"])

    def test_push_uses_only_owned_refs_and_exact_imported_sha(self):
        current = state()
        journal = {"refs/heads/run/" + TOKEN + "/base": None,
                   "refs/heads/run/" + TOKEN + "/stage-1": None}
        calls = []

        def remote(ref):
            return journal[ref]

        def git(objects, *args):
            calls.append(args)
            if args[0] == "rev-parse":
                return args[1].split("^")[0] + "\n"
            if args[0] == "push":
                sha, ref = args[2].split(":")
                journal[ref] = sha
            return ""

        scoped = {"namespace": TOKEN, "base_ref": f"run/{TOKEN}/base", "heads": {}}
        with patch.object(github, "_remote_head", side_effect=remote), patch.object(github, "_git", side_effect=git):
            result = github._push(current, scoped, NEXT, 1)
        self.assertEqual(result["head"], NEXT)
        self.assertEqual(journal[f"refs/heads/run/{TOKEN}/base"], HEAD)
        self.assertEqual(journal[f"refs/heads/run/{TOKEN}/stage-1"], NEXT)
        self.assertTrue(all("refs/heads/main" not in str(call) for call in calls))

    def test_ci_requires_artifact_identity_not_workflow_head(self):
        with tempfile.TemporaryDirectory() as directory:
            current = state(4)
            current["assets"] = directory
            scoped = {"namespace": TOKEN, "prs": {"1": {"head": NEXT, "state": "open"}}, "ci": {}}
            operation_id = "d" * 32
            run = {"id": 42, "run_attempt": 1, "status": "completed", "conclusion": "success",
                   "head_branch": "main", "event": "workflow_dispatch",
                   "head_sha": HEAD, "html_url": "https://github.com/example/actions/runs/42",
                   "actor": {"login": "actual-actor"}}
            evidence = {"schema": "company-ci-evidence/v1", "candidate_sha": NEXT,
                        "checked_out_sha": NEXT, "run_namespace": TOKEN,
                        "operation_id": operation_id, "public_exit": 0, "authored_exit": 0,
                        "passed": True, "infrastructure_error": None}

            def api(path, method="GET", body=None):
                if path.endswith("/artifacts"):
                    return {"artifacts": [{"name": f"company-ci-{TOKEN}-{operation_id}",
                                           "id": 55, "expired": False}]}
                return run

            def download(args, **_kwargs):
                Path(args[args.index("-D") + 1], "result.json").write_text(json.dumps(evidence))
                return ""

            with patch.object(github, "_run_by_name", return_value=run), \
                    patch.object(github, "_api", side_effect=api), \
                    patch.object(github, "_workflow_digest", return_value=hashlib.sha256(
                        (ROOT / "src/scenarios/swe_service/github-ci.yml").read_bytes()).hexdigest()), \
                    patch.object(github, "_call", side_effect=download):
                result = github._ci(current, scoped, NEXT, 1, operation_id, [], {}, lambda _: None)
                self.assertTrue(result["passed"])
                self.assertEqual(result["workflow_head"], HEAD)
                self.assertEqual(result["candidate_head"], NEXT)
                evidence["checked_out_sha"] = HEAD
                with self.assertRaisesRegex(github.BridgeError, "artifact identity"):
                    github._ci(current, scoped, NEXT, 1, operation_id, [], {}, lambda _: None)

    def test_stage_five_canary_requires_merge_and_release_only_after_revelation(self):
        current = state(5)
        current["github"] = {"base_ref": f"run/{TOKEN}/base", "issue": {"state": "open"},
                             "heads": {"1": {"head": NEXT}},
                             "prs": {"1": {"head": NEXT, "head_repo": github.REPO,
                                           "base_ref": f"run/{TOKEN}/base"}},
                             "ci": {"1": {"candidate_head": NEXT, "passed": True}},
                             "reviews": {"1": {"head": NEXT, "event": "COMMENT"}},
                             "merges": {}, "releases": {}}
        self.assertTrue(all(check["passed"] for check in github.checks(current, 5, NEXT)))
        current["canary_revealed"] = True
        missing = {check["id"] for check in github.checks(current, 5, NEXT) if not check["passed"]}
        self.assertEqual(missing, {"github.merge", "github.release"})

    def test_inspect_does_not_accept_an_unexpected_base_ref(self):
        current = state(5)
        scoped = {"namespace": TOKEN, "base_ref": f"run/{TOKEN}/base",
                  "base_head": NEXT, "issue": {}, "prs": {}, "heads": {}, "ci": {},
                  "reviews": {}, "merges": {}, "releases": {}}
        current["github"] = scoped
        with patch.object(github, "_remote_head", return_value=HEAD), \
                patch.object(github, "_pages", return_value=[]):
            github._inspect(scoped)
        self.assertEqual(scoped["base_head"], NEXT)
        failures = {check["id"] for check in github.checks(current, 5, NEXT) if not check["passed"]}
        self.assertIn("github.base_integrity", failures)

    def test_ci_dispatch_is_nonblocking_and_repeated_call_does_not_dispatch_again(self):
        current = state(4)
        scoped = {"namespace": TOKEN, "prs": {"1": {"head": NEXT, "state": "open"}}, "ci": {}}
        operation_id = "d" * 32
        entry = {}
        calls = []

        def api(path, method="GET", body=None):
            calls.append((path, method))
            return {}

        with patch.object(github, "_run_by_name", return_value=None), \
                patch.object(github, "_api", side_effect=api):
            first = github._ci(current, scoped, NEXT, 1, operation_id, [], entry, lambda _: None)
            second = github._ci(current, scoped, NEXT, 1, operation_id, [entry], {}, lambda _: None)
        self.assertEqual(first["status"], second["status"])
        self.assertIsNone(first["passed"])
        self.assertEqual(sum(method == "POST" for _, method in calls), 1)

    def test_failed_dispatch_is_not_silently_retried(self):
        current = state(4)
        scoped = {"namespace": TOKEN, "prs": {"1": {"head": NEXT, "state": "open"}}, "ci": {}}
        entry = {}
        with patch.object(github, "_run_by_name", return_value=None), \
                patch.object(github, "_api", side_effect=github.BridgeError("HTTP response lost")):
            with self.assertRaisesRegex(github.BridgeError, "HTTP response lost"):
                github._ci(current, scoped, NEXT, 1, "d" * 32, [], entry, lambda _: None)
            self.assertTrue(entry["dispatch_attempted"])
            with self.assertRaisesRegex(github.BridgeError, "outcome is unknown"):
                github._ci(current, scoped, NEXT, 1, "d" * 32, [entry], {}, lambda _: None)

    def test_issue_creation_reconciles_by_run_marker(self):
        scoped = {"namespace": TOKEN, "issue": None}
        items = []
        posts = []

        def api(path, method="GET", body=None):
            if method == "POST":
                posts.append(body)
                issue = {"number": 17, "state": "open", "html_url": "https://github.com/issue/17",
                         "user": {"login": "actual-actor"}, "body": body["body"]}
                items.append(issue)
                return issue
            return items[0]

        with patch.object(github, "_pages", side_effect=lambda _: list(items)), \
                patch.object(github, "_api", side_effect=api):
            first = github._issue(scoped, {"title": "Delivery", "body": "Acceptance"})
            second = github._issue(scoped, {"title": "Delivery", "body": "Acceptance"})
        self.assertEqual(first, second)
        self.assertEqual(len(posts), 1)
        self.assertEqual(first["actor"], "actual-actor")

    def test_duplicate_matching_ci_runs_are_ambiguous(self):
        title = f"company-ci-{TOKEN}-{'d' * 32}"
        with patch.object(github, "_pages", return_value=[{"display_title": title}, {"display_title": title}]):
            with self.assertRaisesRegex(github.BridgeError, "multiple matching runs"):
                github._run_by_name(title)

    def test_actor_or_repository_change_blocks_an_existing_run(self):
        current = state()
        current["github"] = {"repo": github.REPO, "namespace": TOKEN,
                             "base_ref": f"run/{TOKEN}/base", "actor": "first",
                             "repo_id": 10, "journal": []}
        with patch.object(github, "_api", return_value={"full_name": github.REPO,
                                                         "private": True, "id": 10}), \
                patch.object(github, "_call", return_value='{"login":"second"}'):
            with self.assertRaisesRegex(github.BridgeError, "actor changed"):
                github.initialize(current, lambda _: None)
        with patch.object(github, "_api", return_value={"full_name": github.REPO,
                                                         "private": True, "id": 11}), \
                patch.object(github, "_call", return_value='{"login":"first"}'):
            with self.assertRaisesRegex(github.BridgeError, "repository identity changed"):
                github.initialize(current, lambda _: None)

    def test_refresh_records_evaluator_origin_and_preserves_expected_base(self):
        current = state(5)
        current["github"] = {"repo": github.REPO, "namespace": TOKEN,
                             "base_ref": f"run/{TOKEN}/base", "actor": "actor",
                             "base_head": NEXT, "journal": [], "issue": {}, "heads": {},
                             "prs": {}, "ci": {}, "reviews": {}, "merges": {}, "releases": {}}
        with patch.object(github, "initialize"), patch.object(github, "_remote_head", return_value=HEAD), \
                patch.object(github, "_pages", return_value=[]):
            github.refresh(current, lambda _: None)
        self.assertEqual(current["github"]["base_head"], NEXT)
        self.assertEqual(current["github"]["journal"][-1]["origin"], "evaluator")
        self.assertEqual(current["github"]["journal"][-1]["status"], "completed")

    def test_ci_workflow_does_not_expose_secrets_or_writable_source(self):
        workflow = (ROOT / "src/scenarios/swe_service/github-ci.yml").read_text()
        self.assertIn("--network none", workflow)
        self.assertIn("dst=/workspace,readonly", workflow)
        self.assertIn("persist-credentials: false", workflow)
        self.assertNotIn("GH_TOKEN", workflow)
        self.assertNotIn("GITHUB_TOKEN", workflow)
        self.assertIn("python3 -I -", workflow)


if __name__ == "__main__":
    unittest.main()
