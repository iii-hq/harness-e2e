import importlib.util
import hashlib
import json
import os
from pathlib import Path
import subprocess
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
            run = {"id": 42, "display_title": f"company-ci-{TOKEN}-{operation_id}",
                   "run_attempt": 1, "status": "completed", "conclusion": "success",
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

    def test_known_ci_run_is_polled_directly_with_operation_identity(self):
        current = state(4)
        operation_id = "d" * 32
        name = f"company-ci-{TOKEN}-{operation_id}"
        scoped = {"namespace": TOKEN, "prs": {"1": {"head": NEXT, "state": "open"}},
                  "ci": {"1": {"candidate_head": NEXT, "run_id": 42}}}
        run = {"id": 42, "display_title": name, "status": "in_progress"}
        with patch.object(github, "_run_by_name", side_effect=AssertionError("must use known ID")), \
                patch.object(github, "_api", return_value=run) as api:
            result = github._ci(current, scoped, NEXT, 1, operation_id, [], {}, lambda _: None)
            self.assertEqual(result["status"], "pending")
            self.assertEqual(result["run_id"], 42)
            api.assert_called_once_with("actions/runs/42")
            run["display_title"] = "another operation"
            with self.assertRaisesRegex(github.BridgeError, "Stored CI run ID"):
                github._ci(current, scoped, NEXT, 1, operation_id, [], {}, lambda _: None)

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
        self.assertTrue(posts[0]["title"].startswith(f"[lifecycle {TOKEN[:8]}] "))
        self.assertEqual(first["actor"], "actual-actor")

    def test_new_pr_and_release_have_visible_and_full_run_markers(self):
        base = f"run/{TOKEN}/base"
        branch = f"run/{TOKEN}/stage-1"
        scoped = {"namespace": TOKEN, "base_ref": base, "issue": {"number": 11},
                  "heads": {"1": {"head": NEXT, "ref": f"refs/heads/{branch}"}},
                  "prs": {}, "merges": {"1": {"candidate_head": NEXT,
                                                "merge_head": HEAD, "merged": True}}, "releases": {}}
        posted = []
        created = {}

        def api(path, method="GET", body=None):
            if method == "POST":
                posted.append((path, body))
                if path == "pulls":
                    created["pr"] = {"number": 12, "body": body["body"], "state": "open",
                                     "merged": False, "html_url": "https://github.com/pull/12",
                                     "user": {"login": "actor"},
                                     "head": {"sha": NEXT, "ref": branch, "repo": {"full_name": github.REPO}},
                                     "base": {"ref": base}}
                    return created["pr"]
                created["release"] = {"id": 13, "tag_name": body["tag_name"],
                                      "body": body["body"], "prerelease": True,
                                      "author": {"login": "actor"}, "html_url": "https://github.com/release/13"}
                return created["release"]
            if path == "pulls/12":
                return created["pr"]
            if path == "releases/13":
                return created["release"]
            if path.startswith("git/ref/tags/"):
                return {"object": {"sha": HEAD}}
            raise AssertionError((path, method))

        with patch.object(github, "_remote_head", return_value=NEXT), \
                patch.object(github, "_pages", return_value=[]), \
                patch.object(github, "_optional_api", return_value=None), \
                patch.object(github, "_api", side_effect=api):
            github._pr(scoped, {"title": "Candidate", "body": "Review"}, NEXT, 1)
            github._release(scoped, {"version": "1"}, NEXT, 1)
        self.assertEqual([path for path, _ in posted], ["pulls", "releases"])
        for _, body in posted:
            self.assertTrue((body.get("title") or body.get("name")).startswith(f"[lifecycle {TOKEN[:8]}]"))
            self.assertIn(f"<!-- company-lifecycle:{TOKEN} -->", body["body"])

    def test_releasing_another_version_of_same_cycle_reconciles_its_own_tag(self):
        tag = f"run/{TOKEN}/v1.0.1"
        scoped = {"namespace": TOKEN, "merges": {"1": {"candidate_head": NEXT,
                                                    "merge_head": HEAD, "merged": True}},
                  "releases": {"1": {"id": 13, "tag": f"run/{TOKEN}/v1.0.0"}}}
        existing = {"id": 14, "tag_name": tag, "body": f"<!-- company-lifecycle:{TOKEN} -->",
                    "author": {"login": "actor"}, "html_url": "https://github.com/release/14"}
        def api(path, method="GET", body=None):
            if path == "releases/14":
                return existing
            if path.startswith("git/ref/tags/"):
                return {"object": {"sha": HEAD}}
            raise AssertionError((path, method))
        with patch.object(github, "_optional_api", return_value=existing), \
                patch.object(github, "_api", side_effect=api):
            result = github._release(scoped, {"version": "1.0.1"}, NEXT, 1)
        self.assertEqual(result["id"], 14)
        self.assertEqual(result["tag"], tag)

    def cleanup_remote(self):
        current = state(8)
        merged = "d" * 40
        base = f"run/{TOKEN}/base"
        branch = f"run/{TOKEN}/stage-1"
        tag = f"run/{TOKEN}/v1"
        marker = f"<!-- company-lifecycle:{TOKEN} -->"
        issue = {"number": 11, "state": "open", "body": marker, "user": {"login": "actor", "id": 7}}
        pr = {"number": 12, "state": "open", "body": marker, "user": {"login": "actor", "id": 7},
              "head": {"ref": branch, "repo": {"full_name": github.REPO}}, "base": {"ref": base}}
        release = {"id": 13, "tag_name": tag, "prerelease": True, "body": marker,
                   "author": {"login": "actor", "id": 7}}
        current["github"] = {
            "repo": github.REPO, "namespace": TOKEN, "base_ref": base, "actor": "actor", "actor_id": 7,
            "journal": [
                {"operation": "issue", "cycle": 1, "intent_at": 1, "result": {"number": 11}},
                {"operation": "push", "cycle": 1, "head": NEXT, "status": "completed"},
                {"operation": "pr", "cycle": 1, "intent_at": 2, "result": {"number": 12}},
                {"operation": "merge", "cycle": 1, "status": "completed", "result": {"merge_head": merged}},
                {"operation": "release", "cycle": 1, "version": "1", "origin": "subject", "intent_at": 3}],
            "issue": {"number": 11, "state": "open"},
            "heads": {"1": {"ref": f"refs/heads/{branch}", "head": NEXT}},
            "prs": {"1": {"number": 12}}, "ci": {"1": {"run_id": 99}},
            "merges": {"1": {"merge_head": merged}},
            "releases": {"1": {"id": 13, "tag": tag}},
        }
        remote = {"issue": issue, "pr": pr, "release": release,
                  "refs/heads/" + branch: NEXT, "refs/heads/" + base: merged,
                  "refs/tags/" + tag: merged, "refs/heads/main": HEAD}
        writes = []
        fail = {"release_delete": False}

        def api(path, method="GET", body=None):
            if path.startswith("releases/tags/") and path != f"releases/tags/{tag}":
                raise github.BridgeError("HTTP 404")
            if path in (f"releases/tags/{tag}", "releases/13"):
                if method == "DELETE":
                    writes.append("release-delete")
                    if fail["release_delete"]:
                        raise github.BridgeError("release deletion failed")
                    remote["release"] = None
                    return {}
                if remote["release"] is None:
                    raise github.BridgeError("HTTP 404")
                return remote["release"]
            if path == f"git/ref/tags/{tag}":
                return {"object": {"sha": remote[f"refs/tags/{tag}"]}}
            if path == "pulls/12":
                if method == "PATCH":
                    writes.append("pr-close")
                    remote["pr"]["state"] = body["state"]
                return remote["pr"]
            if path == "issues/11":
                if method == "PATCH":
                    writes.append("issue-close")
                    remote["issue"]["state"] = body["state"]
                return remote["issue"]
            raise AssertionError((path, method))

        def git(objects, *args):
            if args[0] == "rev-parse":
                return args[1].split("^")[0] + "\n"
            self.assertTrue((Path(objects) / "HEAD").is_file(), "ref deletion needs a temporary bare repository")
            ref = args[1].split(":", 1)[0].removeprefix("--force-with-lease=")
            self.assertEqual(args[-1], f":{ref}")
            writes.append("tag-delete" if ref.startswith("refs/tags/") else "branch-delete:" + ref)
            remote[ref] = None
            return ""

        def pages(path, key=None):
            if path.startswith("issues?state=all&since="):
                return [remote["issue"]]
            if path.startswith("pulls?state=all&head=iii-hq:"):
                return [remote["pr"]]
            raise AssertionError("cleanup performed an unscoped list: " + path)

        return current, remote, writes, fail, api, git, pages

    def test_cleanup_owned_resources_is_ordered_idempotent_and_keeps_history(self):
        current, remote, writes, _, api, git, pages = self.cleanup_remote()
        journal = json.loads(json.dumps(current["github"]["journal"]))
        saves = []
        with patch.object(github, "initialize"), patch.object(github, "_api", side_effect=api), \
                patch.object(github, "_git", side_effect=git), \
                patch.object(github, "_remote_head", side_effect=lambda ref: remote[ref]), \
                patch.object(github, "_pages", side_effect=pages):
            receipt = github.cleanup(current, lambda value: saves.append(json.loads(json.dumps(value))))
            self.assertEqual(receipt["status"], "completed", receipt)
            self.assertLess(writes.index("release-delete"), writes.index("tag-delete"))
            self.assertLess(writes.index("pr-close"),
                            writes.index(f"branch-delete:refs/heads/run/{TOKEN}/stage-1"))
            self.assertEqual(current["github"]["journal"], journal)
            self.assertEqual(current["github"]["ci"]["1"]["run_id"], 99)
            self.assertTrue(any(snapshot["github"]["cleanup_receipt"]["resources"] for snapshot in saves))
            before = list(writes)
            current.pop("objects")
            self.assertEqual(github.cleanup(current, lambda _: None)["status"], "completed")
            self.assertEqual(writes, before, "repeated cleanup must not mutate remote history")
        self.assertEqual(remote["refs/heads/main"], HEAD)

    def test_cleanup_release_failure_preserves_tag_then_retries_without_local_assets(self):
        current, remote, writes, fail, api, git, pages = self.cleanup_remote()
        fail["release_delete"] = True
        with patch.object(github, "initialize"), patch.object(github, "_api", side_effect=api), \
                patch.object(github, "_git", side_effect=git), \
                patch.object(github, "_remote_head", side_effect=lambda ref: remote[ref]), \
                patch.object(github, "_pages", side_effect=pages):
            first = github.cleanup(current, lambda _: None)
            self.assertEqual(first["status"], "partial", first)
            self.assertEqual(remote[f"refs/tags/run/{TOKEN}/v1"], "d" * 40)
            self.assertNotIn("tag-delete", writes)
            fail["release_delete"] = False
            current.pop("objects")
            second = github.cleanup(current, lambda _: None)
            self.assertEqual(second["status"], "completed", second)
            self.assertIsNone(remote[f"refs/tags/run/{TOKEN}/v1"])
            self.assertEqual(len(second["resources"][f"release:run/{TOKEN}/v1"]["attempts"]), 2)

    def test_cleanup_authentication_failure_still_persists_ref_plan_for_retry(self):
        current, remote, _, _, api, git, pages = self.cleanup_remote()
        with patch.object(github, "initialize", side_effect=github.BridgeError("auth unavailable")), \
                patch.object(github, "_git", side_effect=git):
            first = github.cleanup(current, lambda _: None)
        self.assertEqual(first["status"], "failed")
        self.assertEqual(first["trusted_refs"]["1"], [NEXT])
        current.pop("objects")
        with patch.object(github, "initialize"), patch.object(github, "_api", side_effect=api), \
                patch.object(github, "_git", side_effect=git), \
                patch.object(github, "_remote_head", side_effect=lambda ref: remote[ref]), \
                patch.object(github, "_pages", side_effect=pages):
            second = github.cleanup(current, lambda _: None)
        self.assertEqual(second["status"], "completed", second)
        self.assertNotIn("error", second)

    def test_cleanup_does_not_remove_unowned_resources(self):
        for kind in ("issue", "pr", "release"):
            with self.subTest(kind=kind):
                current, remote, writes, _, api, git, pages = self.cleanup_remote()
                remote[kind]["body"] = "another run"
                with patch.object(github, "initialize"), patch.object(github, "_api", side_effect=api), \
                        patch.object(github, "_git", side_effect=git), \
                        patch.object(github, "_remote_head", side_effect=lambda ref: remote[ref]), \
                        patch.object(github, "_pages", side_effect=pages):
                    receipt = github.cleanup(current, lambda _: None)
                self.assertIn(receipt["status"], ("partial", "failed"), receipt)
                self.assertEqual(remote[kind]["body"], "another run")
                if kind == "issue":
                    self.assertNotIn("issue-close", writes)
                elif kind == "pr":
                    self.assertNotIn("pr-close", writes)
                    self.assertNotIn(f"branch-delete:refs/heads/run/{TOKEN}/stage-1", writes)
                else:
                    self.assertNotIn("release-delete", writes)
                    self.assertNotIn("tag-delete", writes)

    def test_cleanup_reconciles_creation_intents_with_scoped_lists(self):
        current, remote, writes, _, api, git, pages = self.cleanup_remote()
        current["github"]["issue"] = None
        current["github"]["prs"] = {}
        current["github"]["releases"] = {}
        for entry in current["github"]["journal"]:
            if entry["operation"] in ("issue", "pr"):
                entry.pop("result", None)
        with patch.object(github, "initialize"), patch.object(github, "_api", side_effect=api), \
                patch.object(github, "_git", side_effect=git), \
                patch.object(github, "_remote_head", side_effect=lambda ref: remote[ref]), \
                patch.object(github, "_pages", side_effect=pages):
            receipt = github.cleanup(current, lambda _: None)
        self.assertEqual(receipt["status"], "completed", receipt)
        self.assertIn("release-delete", writes)
        self.assertIn("pr-close", writes)
        self.assertIn("issue-close", writes)

    def test_cleanup_includes_older_journal_release_intents_and_conflicting_refs_are_preserved(self):
        current, remote, writes, _, api, git, pages = self.cleanup_remote()
        extra_tag = f"run/{TOKEN}/v2"
        remote[f"refs/tags/{extra_tag}"] = "d" * 40
        remote[f"refs/heads/run/{TOKEN}/stage-1"] = "e" * 40
        current["github"]["journal"].append({"operation": "release", "cycle": 1,
                                               "version": "2", "origin": "subject", "intent_at": 4})
        with patch.object(github, "initialize"), patch.object(github, "_api", side_effect=api), \
                patch.object(github, "_git", side_effect=git), \
                patch.object(github, "_remote_head", side_effect=lambda ref: remote[ref]), \
                patch.object(github, "_pages", side_effect=pages):
            receipt = github.cleanup(current, lambda _: None)
        self.assertEqual(receipt["status"], "partial", receipt)
        self.assertIsNone(remote[f"refs/tags/{extra_tag}"], "journal-only tag must be cleaned")
        self.assertEqual(remote[f"refs/heads/run/{TOKEN}/stage-1"], "e" * 40)
        self.assertNotIn(f"branch-delete:refs/heads/run/{TOKEN}/stage-1", writes)

    def test_ref_deletion_works_outside_checkout_and_rejects_stale_lease(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            remote, source, outside = (root / name for name in ("remote.git", "source", "outside"))
            outside.mkdir()
            subprocess.run(["git", "init", "--bare", "--quiet", str(remote)], check=True, capture_output=True)
            subprocess.run(["git", "init", "--quiet", str(source)], check=True, capture_output=True)
            for key, value in (("user.name", "Fixture"), ("user.email", "fixture@example.invalid")):
                subprocess.run(["git", "-C", str(source), "config", key, value], check=True, capture_output=True)
            (source / "file.txt").write_text("candidate")
            subprocess.run(["git", "-C", str(source), "add", "file.txt"], check=True, capture_output=True)
            subprocess.run(["git", "-C", str(source), "commit", "--quiet", "-m", "candidate"],
                           check=True, capture_output=True)
            head = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
            branch = f"refs/heads/run/{TOKEN}/stage-1"
            tag = f"refs/tags/run/{TOKEN}/v1"
            for ref in (branch, tag):
                subprocess.run(["git", "-C", str(source), "push", "--quiet", str(remote), f"{head}:{ref}"],
                               check=True, capture_output=True)
            with patch.dict(os.environ, {"GIT_CEILING_DIRECTORIES": str(root)}):
                self.assertNotEqual(subprocess.run(["git", "-C", str(outside), "rev-parse", "--git-dir"],
                                                  capture_output=True).returncode, 0)
                cwd = Path.cwd()
                try:
                    os.chdir(outside)
                    with patch.object(github, "REMOTE", str(remote)):
                        github._delete_ref(branch, head)
                        with self.assertRaises(github.BridgeError):
                            github._delete_ref(tag, "e" * 40)
                        github._delete_ref(tag, head)
                finally:
                    os.chdir(cwd)
            for ref in (branch, tag):
                self.assertNotEqual(subprocess.run(["git", "--git-dir", str(remote), "show-ref", "--verify", ref],
                                                  capture_output=True).returncode, 0)

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
