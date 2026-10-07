import importlib.util
import io
import os
import signal
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch


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

    def test_a_signal_during_the_dispatch_cancels_the_run_once_it_is_known(self):
        """The job is cancelled while `gh workflow run` still waits for the run URL."""
        def fake_gh(*args, **_):
            if args[:2] == ("workflow", "run"):
                signal.getsignal(signal.SIGTERM)(signal.SIGTERM, None)
                return "https://github.com/iii-hq/harness-e2e/actions/runs/42\n"
            raise AssertionError(f"nothing should run after the cancel: {args}")

        previous = signal.getsignal(signal.SIGTERM), signal.getsignal(signal.SIGINT)
        try:
            # A run GitHub has only just queued refuses the first cancel (409).
            answers = [SimpleNamespace(returncode=1), SimpleNamespace(returncode=0)]
            with patch.object(verify_stack, "gh", side_effect=fake_gh), \
                    patch.object(verify_stack.subprocess, "run", side_effect=answers) as run, \
                    patch.object(verify_stack.time, "sleep"), \
                    self.assertRaises(SystemExit) as exited:
                verify_stack.measure({"containers": {}}, "pr", "deepseek/deepseek-flash", None)
        finally:
            signal.signal(signal.SIGTERM, previous[0])
            signal.signal(signal.SIGINT, previous[1])
        self.assertEqual(exited.exception.code, 143)
        self.assertEqual(run.call_count, 2)
        self.assertEqual(run.call_args.args[0][:4], ["gh", "run", "cancel", "42"])

    def measure_with(self, views, monotonic=None):
        """Run `measure` against a fake `gh`: `views` answers each `gh run view`
        in turn (an exception is raised, a dict is printed as JSON)."""
        calls = []

        def fake_gh(*args, **_):
            calls.append(args[:2])
            if args[:2] == ("workflow", "run"):
                return "https://github.com/iii-hq/harness-e2e/actions/runs/42\n"
            if args[:2] == ("run", "view"):
                answer = views.pop(0)
                if isinstance(answer, Exception):
                    raise answer
                return verify_stack.json.dumps(answer)
            if args[:2] == ("run", "download"):
                directory = args[args.index("-D") + 1]
                (Path(directory) / "execution-summary.json").write_text(
                    verify_stack.json.dumps(summary(group("minimal_path"))))
                return ""
            raise AssertionError(f"unexpected gh call: {args}")

        previous = signal.getsignal(signal.SIGTERM), signal.getsignal(signal.SIGINT)
        try:
            with patch.object(verify_stack, "gh", side_effect=fake_gh), \
                    patch.object(verify_stack.subprocess, "run", return_value=SimpleNamespace(returncode=0)), \
                    patch.object(verify_stack.time, "sleep"), \
                    patch.object(verify_stack.time, "monotonic", side_effect=monotonic or (lambda: 0)):
                result = verify_stack.measure({"containers": {}}, "pr", "deepseek/deepseek-flash", None)
        finally:
            signal.signal(signal.SIGTERM, previous[0])
            signal.signal(signal.SIGINT, previous[1])
        return result, calls

    def test_a_run_not_readable_yet_or_a_5xx_is_polled_again(self):
        """Right after the dispatch the run 404s for a moment; the API also 5xxs now and then."""
        failed = lambda status: verify_stack.VerifyError(  # noqa: E731
            f"gh run view 42 failed: failed to get run: HTTP {status} "
            "(https://api.github.com/repos/iii-hq/harness-e2e/actions/runs/42)")
        views = [failed("404: Not Found"), failed("500"), {"status": "in_progress", "attempt": 1},
                 {"status": "completed", "attempt": 1, "conclusion": "success"}]
        (result, run_url), calls = self.measure_with(views)
        self.assertEqual((result["status"], result["passed"], result["planned"]), ("passed", 1, 1))
        self.assertEqual(calls.count(("run", "view")), 4)
        self.assertTrue(run_url.endswith("/actions/runs/42"))

    def test_a_lookup_error_that_will_not_pass_still_fails_fast(self):
        denied = verify_stack.VerifyError("gh run view 42 failed: HTTP 401: Bad credentials")
        with self.assertRaises(verify_stack.VerifyError):
            self.measure_with([denied])

    def test_a_run_never_readable_gives_up_at_the_deadline_with_the_last_error(self):
        missing = verify_stack.VerifyError("gh run view 42 failed: failed to get run: HTTP 404: Not Found")
        clock = iter([0, verify_stack.DEADLINE_SECONDS + 1])
        (result, _), _ = self.measure_with([missing], monotonic=lambda: next(clock))
        self.assertEqual(result["status"], "not_measured")
        self.assertIn("did not finish", result["reason"])
        self.assertIn("HTTP 404", result["reason"])

    def test_a_registry_outage_still_leaves_a_one_line_verdict(self):
        """A multi-line error must neither crash the tool nor break GITHUB_OUTPUT."""
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "output"
            argv = ["verify_stack.py", "--pins", '["ade"]', "--commit", COMMIT, "--exit-zero"]
            outage = RuntimeError("https://api.workers.iii.dev/resolve did not answer:\nHTTP 503")
            with patch.object(sys, "argv", argv), patch.dict(os.environ, {"GITHUB_OUTPUT": str(output)}), \
                    patch.object(verify_stack, "graph_workers", side_effect=outage), \
                    patch("sys.stdout", new=io.StringIO()):
                self.assertEqual(verify_stack.main(), 0)
            lines = output.read_text().splitlines()
        self.assertEqual(lines[0], "status=not_measured")
        self.assertTrue(lines[1].startswith("summary=") and "HTTP 503" in lines[1])
        self.assertEqual(len(lines), 3)


if __name__ == "__main__":
    unittest.main()
