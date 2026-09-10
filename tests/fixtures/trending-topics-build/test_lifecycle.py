import importlib.util
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock


MODULE_PATH = Path(__file__).with_name("lifecycle.py")
SPEC = importlib.util.spec_from_file_location("trending_topics_lifecycle", MODULE_PATH)
LIFECYCLE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(LIFECYCLE)


class DeliveryChecksTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.repo = Path(self.temporary.name)
        self.git("init", "--quiet", "--initial-branch=build")
        self.git("config", "user.name", "Harness Test")
        self.git("config", "user.email", "harness@example.test")
        (self.repo / "package.json").write_text('{"private":true}\n')
        (self.repo / "src").mkdir()
        (self.repo / "src/app.js").write_text("export {};\n")
        self.git("add", ".")
        self.git("commit", "--quiet", "-m", "baseline")
        self.initial = self.sha()

    def tearDown(self):
        self.temporary.cleanup()

    def git(self, *args):
        return subprocess.run(
            ["git", "-C", self.repo, *args], check=True, capture_output=True, text=True
        ).stdout.strip()

    def sha(self):
        return self.git("rev-parse", "HEAD")

    def commit(self, path, contents, message):
        destination = self.repo / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(contents)
        self.git("add", path)
        self.git("commit", "--quiet", "-m", message)
        return self.sha()

    def check(self):
        return LIFECYCLE.delivery_checks(self.repo, self.initial, self.sha())

    def test_accepts_nonempty_linear_commits_in_allowed_paths(self):
        self.commit("src/app.js", "export const ready = true;\n", "build app")
        self.commit("public/icon.txt", "icon\n", "add asset")

        self.assertEqual(self.check(), [])

    def test_rejects_empty_commit(self):
        self.git("commit", "--quiet", "--allow-empty", "-m", "empty")

        self.assertEqual(self.check(), [f"Empty commit is not permitted: {self.sha()}"])

    def test_rejects_merge_commit(self):
        self.git("checkout", "--quiet", "-b", "side")
        self.commit("src/side.js", "export {};\n", "side")
        self.git("checkout", "--quiet", "build")
        self.commit("src/main.js", "export {};\n", "main")
        self.git("merge", "--quiet", "--no-ff", "side", "-m", "merge")

        self.assertIn(f"Merge commit is not permitted: {self.sha()}", self.check())

    def test_rejects_protected_change_even_when_later_reverted(self):
        changed = self.commit("package.json", '{"private":false}\n', "change input")
        self.git("revert", "--quiet", "--no-edit", changed)

        reasons = self.check()
        self.assertEqual(len(reasons), 2)
        self.assertTrue(all("Protected path changed" in reason for reason in reasons))
        self.assertTrue(all("package.json" in reason for reason in reasons))


class RuntimeBoundaryTest(unittest.TestCase):
    @staticmethod
    def completed(returncode=0, stdout=b"", stderr=b""):
        return subprocess.CompletedProcess([], returncode, stdout=stdout, stderr=stderr)

    def test_container_is_run_scoped_and_restricted(self):
        state = {"token": "a" * 32}

        argv = LIFECYCLE.container_args(state, "subject", "container:git-service")

        self.assertIn("ttb-" + state["token"] + "-subject", argv)
        self.assertIn("harness.trending-topics.run=" + state["token"], argv)
        for expected in ("container:git-service", "--read-only", "--cap-drop=ALL",
                         "no-new-privileges", "256", "2g", "2"):
            self.assertIn(expected, argv)

    def test_execute_rejects_timeout_outside_contract_before_docker(self):
        for timeout_ms in (0, 120001):
            with self.subTest(timeout_ms=timeout_ms), mock.patch.object(LIFECYCLE, "run") as run:
                with self.assertRaisesRegex(ValueError, "1..=120000"):
                    LIFECYCLE.execute(SimpleNamespace(root=Path("/unused"), timeout_ms=timeout_ms, command="true"))
                run.assert_not_called()

    def test_cleanup_removes_only_containers_with_attempt_label(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            token = "b" * 32
            (root / "state.json").write_text(json.dumps({"token": token}))
            responses = [
                subprocess.CompletedProcess([], 0, stdout=b"container-one\ncontainer-two\n", stderr=b""),
                subprocess.CompletedProcess([], 0, stdout=b"", stderr=b""),
            ]
            with mock.patch.object(LIFECYCLE, "run", side_effect=responses) as run:
                result = LIFECYCLE.cleanup(SimpleNamespace(root=root))

        self.assertEqual(result["removed"], ["container-one", "container-two"])
        self.assertEqual(
            run.call_args_list,
            [
                mock.call(["docker", "ps", "-aq", "--filter", f"label=harness.trending-topics.run={token}"]),
                mock.call(["docker", "rm", "--force", "--volumes", "container-one", "container-two"]),
            ],
        )

    def test_cleanup_rejects_invalid_attempt_identity_before_docker(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "state.json").write_text(json.dumps({"token": "../other-run"}))
            with mock.patch.object(LIFECYCLE, "run") as run:
                with self.assertRaisesRegex(ValueError, "Invalid attempt resource identity"):
                    LIFECYCLE.cleanup(SimpleNamespace(root=root))
                run.assert_not_called()

    def test_build_failure_is_product_failure_but_docker_failure_is_infrastructure(self):
        for exit_code, expected in ((1, "product_error"), (125, "infrastructure_error")):
            with self.subTest(exit_code=exit_code), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                (root / "verifier").mkdir()
                feed = root / "feed.json"
                feed.write_text('{"edition":"test","topics":[]}\n')
                args = SimpleNamespace(root=root)
                state = {"token": "c" * 32, "image": "sha256:image"}
                with mock.patch.object(LIFECYCLE, "run", return_value=self.completed(exit_code)):
                    receipt = LIFECYCLE.evaluate_app(args, state, "original", "a" * 40, feed)

                self.assertIn(expected, receipt)
                self.assertNotIn("infrastructure_error" if expected == "product_error" else "product_error", receipt)

    def test_build_timeout_removes_exact_container_and_preserves_partial_receipt(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "verifier").mkdir()
            feed = root / "feed.json"
            feed.write_text('{"edition":"test","topics":[]}\n')
            token = "c" * 32
            responses = [subprocess.TimeoutExpired(["docker"], 300), self.completed()]
            with mock.patch.object(LIFECYCLE, "run", side_effect=responses) as run:
                receipt = LIFECYCLE.evaluate_app(
                    SimpleNamespace(root=root), {"token": token, "image": "sha256:image"},
                    "original", "a" * 40, feed)

        self.assertEqual(receipt["build_exit_code"], 124)
        self.assertEqual(receipt["infrastructure_error"], "Build container exceeded its bounded deadline")
        self.assertEqual(run.call_args_list[-1],
                         mock.call(["docker", "rm", "--force", f"ttb-{token}-build-original"], check=False))

    def test_evaluator_timeout_removes_it_before_preview_capture_and_removal(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "verifier").mkdir()
            feed = root / "feed.json"
            feed.write_text('{"edition":"test","topics":[]}\n')
            token = "d" * 32
            responses = [self.completed(), self.completed(), self.completed(),
                         subprocess.TimeoutExpired(["docker"], 900), self.completed(),
                         self.completed(), self.completed()]
            with mock.patch.object(LIFECYCLE, "run", side_effect=responses) as run:
                receipt = LIFECYCLE.evaluate_app(
                    SimpleNamespace(root=root), {"token": token, "image": "sha256:image"},
                    "original", "a" * 40, feed)

        self.assertEqual(receipt["test_exit_code"], 124)
        self.assertIn("infrastructure_error", receipt)
        cleanup_calls = run.call_args_list[-3:]
        self.assertEqual(cleanup_calls[0], mock.call(
            ["docker", "rm", "--force", f"ttb-{token}-evaluate-original"], check=False))
        self.assertEqual(cleanup_calls[1].args[0], ["docker", "logs", f"ttb-{token}-preview-original"])
        self.assertEqual(cleanup_calls[2], mock.call(
            ["docker", "rm", "--force", f"ttb-{token}-preview-original"], check=False))

    def test_preview_readiness_failure_is_product_failure(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "verifier").mkdir()
            feed = root / "feed.json"
            feed.write_text('{"edition":"test","topics":[]}\n')
            responses = [self.completed(), self.completed(), self.completed(1), self.completed(), self.completed()]
            with mock.patch.object(LIFECYCLE, "run", side_effect=responses):
                receipt = LIFECYCLE.evaluate_app(
                    SimpleNamespace(root=root), {"token": "d" * 32, "image": "sha256:image"},
                    "original", "a" * 40, feed)

        self.assertEqual(receipt["server_ready"], False)
        self.assertIn("product_error", receipt)
        self.assertNotIn("infrastructure_error", receipt)

    def test_protected_input_failure_does_not_become_infrastructure_error(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            state = {"token": "e" * 32, "image": "sha256:image", "subject": "subject", "git_service": "git"}
            (root / "state.json").write_text(json.dumps(state))
            delivered = "d" * 40

            def fake_git(_directory, *args, **_kwargs):
                if args[:2] == ("rev-parse", "--verify"):
                    return self.completed(stdout=(delivered + "\n").encode())
                if args[0] == "show":
                    contents = b"changed" if args[1] == f"{delivered}:package.json" else b"baseline"
                    return self.completed(stdout=contents)
                return self.completed()

            workspace = {"head": delivered, "branch": "build", "origin": LIFECYCLE.REMOTE_URL,
                         "tracked_clean": True, "index_clean": True, "untracked": []}
            with mock.patch.object(LIFECYCLE, "run", return_value=self.completed()), \
                    mock.patch.object(LIFECYCLE, "git", side_effect=fake_git), \
                    mock.patch.object(LIFECYCLE, "delivery_checks", return_value=[]), \
                    mock.patch.object(LIFECYCLE, "inspect_workspace", return_value=workspace), \
                    mock.patch.object(LIFECYCLE, "evaluate_app") as evaluate:
                result = LIFECYCLE.finish(SimpleNamespace(root=root, assets=MODULE_PATH.parent))

        self.assertEqual(result["criteria"][0]["status"], "failed")
        self.assertIn("Protected execution inputs changed", result["criteria"][0]["reason"])
        self.assertEqual(result["infrastructure_errors"], [])
        self.assertTrue(all(item["status"] == "unverified" for item in result["criteria"][1:]))
        evaluate.assert_not_called()

    def test_dirty_workspace_does_not_hide_build_or_runtime_results(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            state = {"token": "f" * 32, "image": "sha256:image", "subject": "subject", "git_service": "git"}
            (root / "state.json").write_text(json.dumps(state))
            delivered = "d" * 40
            feed = json.dumps({"edition": "test", "topics": []}).encode()

            def fake_git(_directory, *args, **_kwargs):
                if args[:2] == ("rev-parse", "--verify"):
                    return self.completed(stdout=(delivered + "\n").encode())
                if args[0] == "show":
                    return self.completed(stdout=feed if args[1].endswith(":content/feed.json") else b"baseline")
                return self.completed()

            summary = {"criteria": [{"id": f"B{index:02}", "status": "unverified", "reason": "not run"}
                                     for index in range(3, 11)],
                       "infrastructure_errors": [], "evidence": [], "evidence_complete": False}

            def fake_run(argv, **_kwargs):
                if "--feed-variant" in argv:
                    Path(argv[-1]).write_bytes(feed)
                if "--summarize" in argv:
                    return self.completed(stdout=json.dumps(summary).encode())
                return self.completed()

            workspace = {"head": delivered, "branch": "build", "origin": LIFECYCLE.REMOTE_URL,
                         "tracked_clean": False, "index_clean": True, "untracked": []}
            evaluations = [
                {"build_exit_code": 1, "product_error": "Frozen install or build failed", "report": "missing"},
                {"build_exit_code": 125, "infrastructure_error": "Build container failed to execute", "report": "missing"},
            ]
            with mock.patch.object(LIFECYCLE, "run", side_effect=fake_run), \
                    mock.patch.object(LIFECYCLE, "git", side_effect=fake_git), \
                    mock.patch.object(LIFECYCLE, "delivery_checks", return_value=[]), \
                    mock.patch.object(LIFECYCLE, "inspect_workspace", return_value=workspace), \
                    mock.patch.object(LIFECYCLE, "evaluate_app", side_effect=evaluations):
                result = LIFECYCLE.finish(SimpleNamespace(root=root, assets=MODULE_PATH.parent))

        self.assertEqual(next(item for item in result["criteria"] if item["id"] == "B01")["status"], "failed")
        self.assertEqual(next(item for item in result["criteria"] if item["id"] == "B02")["status"], "failed")
        self.assertTrue(all(item["reason"].startswith("B02 failed") for item in result["criteria"]
                            if item["id"] not in ("B01", "B02")))
        self.assertEqual(result["infrastructure_errors"], [
            {"kind": "runtime_infrastructure", "message": "Build container failed to execute"}
        ])


if __name__ == "__main__":
    unittest.main()
