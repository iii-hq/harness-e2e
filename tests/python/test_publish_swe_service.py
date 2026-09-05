import contextlib
import io
import json
import os
import pathlib
import sys
import tempfile
import unittest
from unittest import mock


ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from publish_swe_service import (
    ALLOWED_REPOSITORY,
    PublishError,
    gh,
    load_report,
    public_projection,
    publish,
)


def valid_report(*, accepted=True):
    accepted_tickets = [1] if accepted else []
    accepted_head = "b" * 40 if accepted else "a" * 40
    accepted_patch = (
        "diff --git a/src/profile.py b/src/profile.py\n"
        "--- a/src/profile.py\n"
        "+++ b/src/profile.py\n"
        "@@ -1 +1 @@\n"
        "-OLD = True\n"
        "+OLD = False\n"
        if accepted
        else ""
    )
    checkpoints = (
        [
            {
                "ticket": 1,
                "id": "swe_config_isolation",
                "head_sha": "b" * 40,
                "accepted": True,
                "attempt": 1,
                "feedback": "private checkpoint feedback",
                "hidden_output": "HIDDEN-PROBE-SENTINEL",
                "workspace": "/private/tmp/subject-workspace",
            }
        ]
        if accepted
        else []
    )
    return {
        "schema": "swe-service-report/v1",
        "scenario_id": "swe_service_journey",
        "mode": "journey",
        "fixture_revision": "f" * 40,
        "run_id": "run-20260904-1",
        "initial_head": "a" * 40,
        "accepted_head": accepted_head,
        "accepted_tickets": accepted_tickets,
        "checkpoints": checkpoints,
        "terminal_status": "completed" if accepted else "capability_failure",
        "terminal_ticket": None if accepted else 1,
        "elapsed_ms": 12_345,
        "accepted_patch": accepted_patch,
        "unaccepted_patch": "SECRET-UNACCEPTED-PATCH /private/tmp/subject-workspace",
        "raw_probe_output": "HIDDEN-PROBE-SENTINEL",
        "workspace": "/private/tmp/subject-workspace",
    }


class FakeGitHub:
    def __init__(self):
        self.default_branch = "main"
        self.base_sha = "1" * 40
        self.base_tree = "2" * 40
        self.blobs = {}
        self.trees = {}
        self.commits = {
            self.base_sha: {"sha": self.base_sha, "tree": {"sha": self.base_tree}}
        }
        self.refs = {"heads/main": self.base_sha}
        self.pulls = []
        self.calls = []
        self.next_id = 10
        self.pull_failures = 0

    def _sha(self):
        self.next_id += 1
        return f"{self.next_id:040x}"

    def __call__(self, method, endpoint, payload=None, *, allow_not_found=False):
        self.calls.append((method, endpoint, payload))
        if endpoint == f"repos/{ALLOWED_REPOSITORY}" and method == "GET":
            return {"default_branch": self.default_branch, "owner": {"login": "iii-hq"}}
        prefix = f"repos/{ALLOWED_REPOSITORY}/git/ref/"
        if endpoint.startswith(prefix) and method == "GET":
            ref = endpoint[len(prefix) :]
            sha = self.refs.get(ref)
            if sha is None:
                if allow_not_found:
                    return None
                raise PublishError("not found")
            return {"ref": f"refs/{ref}", "object": {"sha": sha}}
        prefix = f"repos/{ALLOWED_REPOSITORY}/git/commits/"
        if endpoint.startswith(prefix) and method == "GET":
            return self.commits[endpoint[len(prefix) :]]
        if endpoint == f"repos/{ALLOWED_REPOSITORY}/git/blobs" and method == "POST":
            import base64

            sha = self._sha()
            self.blobs[sha] = base64.b64decode(payload["content"])
            return {"sha": sha}
        if endpoint == f"repos/{ALLOWED_REPOSITORY}/git/trees" and method == "POST":
            sha = self._sha()
            self.trees[sha] = payload
            return {"sha": sha}
        if endpoint == f"repos/{ALLOWED_REPOSITORY}/git/commits" and method == "POST":
            sha = self._sha()
            self.commits[sha] = {"sha": sha, "tree": {"sha": payload["tree"]}}
            return {"sha": sha}
        if endpoint == f"repos/{ALLOWED_REPOSITORY}/git/refs" and method == "POST":
            self.refs[payload["ref"].removeprefix("refs/")] = payload["sha"]
            return {"ref": payload["ref"], "object": {"sha": payload["sha"]}}
        if endpoint.startswith(f"repos/{ALLOWED_REPOSITORY}/contents/") and method == "GET":
            path, _, query = endpoint.partition("?ref=")
            ref = query
            commit_sha = self.refs[f"heads/{ref}"]
            tree_sha = self.commits[commit_sha]["tree"]["sha"]
            wanted = path.split("/contents/", 1)[1]
            for item in self.trees[tree_sha]["tree"]:
                if item["path"] == wanted:
                    import base64

                    return {
                        "encoding": "base64",
                        "content": base64.encodebytes(self.blobs[item["sha"]]).decode(),
                    }
            if allow_not_found:
                return None
            raise PublishError("content not found")
        if endpoint.startswith(f"repos/{ALLOWED_REPOSITORY}/pulls?") and method == "GET":
            return list(self.pulls)
        if endpoint == f"repos/{ALLOWED_REPOSITORY}/pulls" and method == "POST":
            if self.pull_failures:
                self.pull_failures -= 1
                raise PublishError("temporary pull request failure")
            pull = {
                "number": len(self.pulls) + 1,
                "html_url": f"https://github.test/{ALLOWED_REPOSITORY}/pull/{len(self.pulls) + 1}",
                "draft": payload["draft"],
                "head": {"ref": payload["head"]},
                "base": {"ref": payload["base"]},
            }
            self.pulls.append(pull)
            return pull
        raise AssertionError(f"unexpected fake GitHub call: {method} {endpoint} {payload}")


class PublisherContractTests(unittest.TestCase):
    def write_report(self, directory, value):
        path = pathlib.Path(directory) / "report.json"
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def test_projection_preserves_baseline_and_excludes_private_fields(self):
        value = valid_report()
        projection = public_projection(value, "execution-123")
        encoded = json.dumps(projection, sort_keys=True)

        self.assertEqual(projection["fixture_revision"], "f" * 40)
        self.assertEqual(projection["initial_head"], "a" * 40)
        self.assertEqual(projection["accepted_head"], "b" * 40)
        self.assertEqual(
            projection["checkpoints"],
            [
                {
                    "ticket": 1,
                    "id": "swe_config_isolation",
                    "head_sha": "b" * 40,
                    "head_status": "valid",
                    "accepted": True,
                    "attempt": 1,
                }
            ],
        )
        self.assertNotIn("HIDDEN-PROBE-SENTINEL", encoded)
        self.assertNotIn("private checkpoint feedback", encoded)
        self.assertNotIn("SECRET-UNACCEPTED-PATCH", encoded)
        self.assertNotIn("/private/tmp", encoded)
        self.assertNotIn("accepted_patch", projection)
        self.assertRegex(projection["report_digest"], r"^sha256:[0-9a-f]{64}$")
        self.assertRegex(projection["accepted_patch_digest"], r"^sha256:[0-9a-f]{64}$")
        self.assertIn("publication_digest", projection)

    def test_empty_accepted_prefix_is_valid_and_publishes_empty_patch(self):
        fake = FakeGitHub()
        with tempfile.TemporaryDirectory() as directory:
            path = self.write_report(directory, valid_report(accepted=False))
            with mock.patch("publish_swe_service.gh", side_effect=fake):
                result = publish(
                    repository=ALLOWED_REPOSITORY,
                    execution_id="empty-run",
                    report_path=path,
                    run_url="https://harness.test/runs/empty-run",
                )

        self.assertEqual(result["accepted_tickets"], [])
        self.assertEqual(result["branch"], "feat/swe-result-empty-run")
        tree = next(iter(fake.trees.values()))["tree"]
        self.assertEqual(
            [item["path"] for item in tree],
            [
                "benchmark-runs/swe/empty-run/report.json",
                "benchmark-runs/swe/empty-run/accepted.patch",
            ],
        )
        patch_entry = tree[1]
        self.assertEqual(fake.blobs[patch_entry["sha"]], b"")

    def test_repeat_invocation_reuses_matching_branch_and_draft_pull_request(self):
        fake = FakeGitHub()
        with tempfile.TemporaryDirectory() as directory:
            path = self.write_report(directory, valid_report())
            with mock.patch("publish_swe_service.gh", side_effect=fake):
                first = publish(
                    repository=ALLOWED_REPOSITORY,
                    execution_id="repeat-run",
                    report_path=path,
                    run_url="https://harness.test/runs/repeat-run",
                )
                second = publish(
                    repository=ALLOWED_REPOSITORY,
                    execution_id="repeat-run",
                    report_path=path,
                    run_url="https://harness.test/runs/repeat-run",
                )

        self.assertFalse(first["reused"])
        self.assertTrue(second["reused"])
        self.assertEqual(first["commit_sha"], second["commit_sha"])
        self.assertEqual(len(fake.pulls), 1)
        self.assertEqual(
            sum(
                method == "POST" and endpoint.endswith("/git/refs")
                for method, endpoint, _ in fake.calls
            ),
            1,
        )

    def test_retry_finishes_pull_creation_for_an_exact_existing_branch(self):
        fake = FakeGitHub()
        fake.pull_failures = 1
        with tempfile.TemporaryDirectory() as directory:
            path = self.write_report(directory, valid_report())
            with mock.patch("publish_swe_service.gh", side_effect=fake):
                with self.assertRaisesRegex(PublishError, "temporary"):
                    publish(
                        repository=ALLOWED_REPOSITORY,
                        execution_id="recover-pr",
                        report_path=path,
                        run_url="https://harness.test/runs/recover-pr",
                    )
                recovered = publish(
                    repository=ALLOWED_REPOSITORY,
                    execution_id="recover-pr",
                    report_path=path,
                    run_url="https://harness.test/runs/recover-pr",
                )
                repeated = publish(
                    repository=ALLOWED_REPOSITORY,
                    execution_id="recover-pr",
                    report_path=path,
                    run_url="https://harness.test/runs/recover-pr",
                )
                created_pull_count = len(fake.pulls)
                fake.pulls.append(dict(fake.pulls[0]))
                with self.assertRaisesRegex(PublishError, "more than one"):
                    publish(
                        repository=ALLOWED_REPOSITORY,
                        execution_id="recover-pr",
                        report_path=path,
                        run_url="https://harness.test/runs/recover-pr",
                    )

        self.assertFalse(recovered["reused"])
        self.assertTrue(repeated["reused"])
        self.assertEqual(recovered["commit_sha"], repeated["commit_sha"])
        self.assertEqual(created_pull_count, 1)
        self.assertEqual(len(fake.pulls), 2)
        self.assertEqual(
            sum(
                method == "POST" and endpoint.endswith("/git/refs")
                for method, endpoint, _ in fake.calls
            ),
            1,
        )

    def test_rejected_invalid_head_is_marked_without_poisoning_later_acceptance(self):
        value = valid_report()
        value["checkpoints"].insert(
            0,
            {
                "ticket": 1,
                "id": "swe_config_isolation",
                "head_sha": "../../unsafe-SECRET-CHECKPOINT",
                "accepted": False,
                "attempt": 1,
                "feedback": "invalid submitted head",
            },
        )
        value["checkpoints"][1]["attempt"] = 2

        fake = FakeGitHub()
        with tempfile.TemporaryDirectory() as directory:
            path = self.write_report(directory, value)
            loaded = load_report(path)
            with mock.patch("publish_swe_service.gh", side_effect=fake):
                result = publish(
                    repository=ALLOWED_REPOSITORY,
                    execution_id="rejected-head",
                    report_path=path,
                    run_url="",
                )
            tree = next(iter(fake.trees.values()))["tree"]
            projection = json.loads(fake.blobs[tree[0]["sha"]])

        self.assertEqual(loaded["checkpoints"][0]["head_sha"], None)
        self.assertFalse(result["reused"])
        self.assertEqual(len(fake.pulls), 1)
        self.assertEqual(
            projection["checkpoints"][0],
            {
                "ticket": 1,
                "id": "swe_config_isolation",
                "head_sha": None,
                "head_status": "invalid",
                "accepted": False,
                "attempt": 1,
            },
        )
        self.assertEqual(projection["checkpoints"][1]["head_sha"], "b" * 40)
        self.assertEqual(projection["checkpoints"][1]["head_status"], "valid")
        self.assertNotIn("unsafe-SECRET-CHECKPOINT", json.dumps(projection))
        self.assertNotIn(
            b"unsafe-SECRET-CHECKPOINT", b"".join(fake.blobs.values())
        )

    def test_accepted_checkpoint_still_requires_an_exact_head(self):
        value = valid_report()
        value["checkpoints"][0]["head_sha"] = "abcd123"
        value["accepted_head"] = "a" * 40
        with tempfile.TemporaryDirectory() as directory:
            path = self.write_report(directory, value)
            with self.assertRaisesRegex(PublishError, "accepted.*full lowercase Git SHA"):
                load_report(path)

    def test_existing_branch_with_conflicting_evidence_fails_closed(self):
        fake = FakeGitHub()
        with tempfile.TemporaryDirectory() as directory:
            path = self.write_report(directory, valid_report())
            with mock.patch("publish_swe_service.gh", side_effect=fake):
                publish(
                    repository=ALLOWED_REPOSITORY,
                    execution_id="conflict-run",
                    report_path=path,
                    run_url="",
                )
                changed = valid_report()
                changed["elapsed_ms"] = 99_999
                self.write_report(directory, changed)
                with self.assertRaisesRegex(PublishError, "conflicting evidence"):
                    publish(
                        repository=ALLOWED_REPOSITORY,
                        execution_id="conflict-run",
                        report_path=path,
                        run_url="",
                    )

        self.assertEqual(len(fake.pulls), 1)

    def test_validation_rejects_wrong_scenario_unsafe_patch_and_invalid_run_url(self):
        cases = []
        wrong_scenario = valid_report()
        wrong_scenario["scenario_id"] = "swe_config_isolation"
        cases.append((wrong_scenario, "scenario"))

        traversal = valid_report()
        traversal["accepted_patch"] = "diff --git a/../../secret b/../../secret\n"
        cases.append((traversal, "unsafe patch path"))

        secret = valid_report()
        secret["accepted_patch"] += "+api_token = 'ghp_abcdefghijklmnopqrstuvwxyz1234567890'\n"
        cases.append((secret, "secret"))

        for index, (value, message) in enumerate(cases):
            with self.subTest(index=index), tempfile.TemporaryDirectory() as directory:
                path = self.write_report(directory, value)
                with self.assertRaisesRegex(PublishError, message):
                    load_report(path)

        with tempfile.TemporaryDirectory() as directory:
            path = self.write_report(directory, valid_report())
            for run_url in (
                "file:///private/tmp/secret",
                "https://harness.test/runs/1?token=sensitive-value",
            ):
                with self.subTest(run_url=run_url), self.assertRaisesRegex(
                    PublishError, "run URL"
                ):
                    publish(
                        repository=ALLOWED_REPOSITORY,
                        execution_id="valid-id",
                        report_path=path,
                        run_url=run_url,
                    )

    def test_large_private_native_envelope_is_accepted_for_sanitization(self):
        value = valid_report()
        value["transcript"] = "x" * (5 * 1024 * 1024)
        with tempfile.TemporaryDirectory() as directory:
            loaded = load_report(self.write_report(directory, value))
            projection = public_projection(loaded, "large-envelope")

        self.assertEqual(loaded["schema"], "swe-service-report/v1")
        self.assertNotIn("transcript", projection)

    def test_repository_and_execution_id_are_rejected_before_network(self):
        with tempfile.TemporaryDirectory() as directory:
            path = self.write_report(directory, valid_report())
            with mock.patch("publish_swe_service.gh") as network:
                with self.assertRaisesRegex(PublishError, ALLOWED_REPOSITORY):
                    publish(
                        repository="iii-hq/other",
                        execution_id="valid-id",
                        report_path=path,
                        run_url="",
                    )
                with self.assertRaisesRegex(PublishError, "execution id"):
                    publish(
                        repository=ALLOWED_REPOSITORY,
                        execution_id="../escape",
                        report_path=path,
                        run_url="",
                    )
                network.assert_not_called()

    def test_gh_sends_json_on_stdin_without_shell_interpolation(self):
        with tempfile.TemporaryDirectory() as directory:
            directory_path = pathlib.Path(directory)
            capture = directory_path / "capture.json"
            executable = directory_path / "gh"
            executable.write_text(
                "#!/usr/bin/env python3\n"
                "import json, os, pathlib, sys\n"
                "pathlib.Path(os.environ['FAKE_GH_CAPTURE']).write_text(json.dumps({"
                "'argv': sys.argv[1:], 'stdin': sys.stdin.read()}))\n"
                "print(json.dumps({'sha': 'c' * 40}))\n",
                encoding="utf-8",
            )
            executable.chmod(0o755)
            env = {
                "PATH": f"{directory}{os.pathsep}{os.environ.get('PATH', '')}",
                "FAKE_GH_CAPTURE": str(capture),
            }
            with mock.patch.dict(os.environ, env, clear=False):
                response = gh(
                    "POST",
                    "repos/iii-hq/e2e-fixture/git/blobs",
                    {"content": "$(touch /tmp/not-executed)", "encoding": "utf-8"},
                )

            observed = json.loads(capture.read_text(encoding="utf-8"))
            self.assertEqual(response, {"sha": "c" * 40})
            self.assertEqual(
                observed["argv"],
                [
                    "api",
                    "--method",
                    "POST",
                    "repos/iii-hq/e2e-fixture/git/blobs",
                    "--input",
                    "-",
                ],
            )
            self.assertEqual(
                json.loads(observed["stdin"]),
                {"content": "$(touch /tmp/not-executed)", "encoding": "utf-8"},
            )


if __name__ == "__main__":
    unittest.main()
