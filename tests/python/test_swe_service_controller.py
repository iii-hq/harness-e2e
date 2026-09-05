"""Behavior tests for the trusted checkpoint protocol, using real Git repositories."""
import concurrent.futures
import json
import os
import signal
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest


CONTROLLER = Path(__file__).resolve().parents[2] / "src/scenarios/swe_service/controller.py"


class ControllerTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.fixture = self.root / "fixture"
        for stage in range(9):
            snap = self.fixture / "swe-service/snapshots" / f"{stage:02}"
            for directory in ("src", "docs", "tests/reference", "tests/agent"):
                (snap / directory).mkdir(parents=True)
            (snap / "src/stage").write_text(str(stage))
            (snap / "README.md").write_text("Public contract\n")
            (snap / "tests/reference/test_public.py").write_text("assert True\n")
            (snap / "tests/agent/.gitkeep").touch()
            (snap / "docs/.gitkeep").touch()
        tickets = [{"number": n, "id": f"ticket-{n}", "title": f"Ticket {n}",
                    "prompt": f"Solve ticket {n}"} for n in range(1, 9)]
        tickets[4]["canary_prompt"] = "Legacy canary requires legacy-ok in src/legacy."
        (self.fixture / "swe-service/curriculum.json").write_text(json.dumps({"tickets": tickets}))
        self.probes = self.root / "probes.py"
        self.probes.write_text('''import argparse,json,pathlib,time
p=argparse.ArgumentParser();p.add_argument('--workspace');p.add_argument('--through',type=int);p.add_argument('--canary',action='store_true');a=p.parse_args()
w=pathlib.Path(a.workspace)
control=pathlib.Path(__file__).parent
with (control/'probe-log').open('a') as f:f.write(str(w)+'\\n')
if a.canary:
 with (control/'canary-log').open('a') as f:f.write(str(w)+'\\n')
if (control/'slow').exists():
 (control/'started').touch()
 time.sleep(0.6)
passed=int((w/'src/stage').read_text())>=a.through
if a.canary:passed=passed and (w/'src/legacy').exists() and (w/'src/legacy').read_text()=='legacy-ok'
print(json.dumps({'passed':passed,'checks':[{'id':'contract','passed':passed,'reason':'contract failed' if not passed else ''}]}))
''')
        self.isolation = self.root / "test_isolator.py"
        self.isolation.write_text('''# Test-only forwarding fixture; production has no unisolated fallback.
import argparse,subprocess,sys
p=argparse.ArgumentParser();p.add_argument('--probes');a,rest=p.parse_known_args()
raise SystemExit(subprocess.call([sys.executable,'-I',a.probes,*rest]))
''')
        self.workspace = self.root / "subject"
        self.state = self.root / "trusted/state.json"

    def invoke(self, *args, ok=True):
        result = subprocess.run([sys.executable, "-I", str(CONTROLLER), *map(str, args)],
                                text=True, capture_output=True)
        if ok:
            self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        return json.loads(result.stdout)

    def prepare(self, mode="journey", ticket=1):
        return self.invoke("prepare", "--fixture-root", self.fixture, "--workspace", self.workspace,
                           "--state-file", self.state, "--probes", self.probes, "--mode", mode,
                           "--isolation", self.isolation, "--ticket", ticket,
                           "--fixture-revision", "a" * 40, "--run-id", "test-run")

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.workspace), *args], text=True).strip()

    def commit(self, stage=None, name="candidate"):
        if stage is not None:
            (self.workspace / "src/stage").write_text(str(stage))
        self.git("add", "-A")
        self.git("-c", "user.name=Subject", "-c", "user.email=subject@example.invalid", "commit", "--allow-empty", "-qm", name)
        return self.git("rev-parse", "HEAD")

    def checkpoint(self, ticket, head, revision_id=None):
        args = ["checkpoint", "--state-file", self.state, "--ticket", ticket, "--head", head]
        if revision_id is not None:
            args += ["--revision-id", revision_id]
        return self.invoke(*args)

    def test_prepare_exports_only_selected_snapshot_without_future_history(self):
        result = self.prepare("isolated", 4)
        self.assertEqual((self.workspace / "src/stage").read_text(), "3")
        self.assertEqual(self.git("rev-list", "--all", "--count"), "1")
        self.assertEqual(self.git("remote"), "")
        self.assertEqual(result["current_ticket"], 4)
        self.assertNotIn("Legacy canary", json.dumps(result))
        self.assertFalse((self.workspace / "snapshots").exists())

    def test_journey_progresses_in_same_repository_and_capture_keeps_unaccepted_edits(self):
        initial = self.prepare()["initial_head"]
        first = self.commit(1)
        a = self.checkpoint(1, first)
        self.assertEqual(a["status"], "accepted")
        self.assertEqual(a["current_ticket"], 2)
        self.assertEqual(a["accepted_tickets"], [1])
        self.assertEqual(a["next_ticket"]["number"], 2)
        (self.workspace / "docs/notes.md").write_text("Retain journey work\n")
        second = self.commit(2)
        self.assertEqual(self.checkpoint(2, second)["accepted_tickets"], [1, 2])
        (self.workspace / "src/stage").write_text("unfinished")
        (self.workspace / "tests/agent/new.py").write_text("pending test\n")
        report = self.invoke("capture", "--state-file", self.state, "--terminal-status", "cancelled")
        self.assertEqual(report["schema"], "swe-service-report/v1")
        self.assertEqual(report["initial_head"], initial)
        self.assertEqual(report["accepted_head"], second)
        self.assertIn("Retain journey work", report["accepted_patch"])
        self.assertNotIn("unfinished", report["accepted_patch"])
        self.assertIn("unfinished", report["unaccepted_patch"])
        self.assertIn("pending test", report["unaccepted_patch"])

    def test_duplicate_checkpoint_is_idempotent_even_after_progression(self):
        self.prepare()
        head = self.commit(1)
        original = self.checkpoint(1, head)
        self.assertEqual(self.checkpoint(1, head), original)
        report = self.invoke("capture", "--state-file", self.state)
        self.assertEqual(len(report["checkpoints"]), 1)

    def test_three_distinct_failed_candidates_end_task_but_duplicates_do_not(self):
        self.prepare()
        first = self.commit(0)
        rejection = self.checkpoint(1, first)
        self.assertEqual(rejection["status"], "rejected")
        self.assertEqual(self.checkpoint(1, first), rejection)
        self.assertEqual(self.checkpoint(1, self.commit(0, "second"))["status"], "rejected")
        result = self.checkpoint(1, self.commit(0, "third"))
        self.assertEqual(result["status"], "capability_failure")
        self.assertEqual(self.checkpoint(1, self.commit(1, "too late"))["status"], "capability_failure")

    def test_each_journey_ticket_has_its_own_three_rejection_budget(self):
        self.prepare()
        for ticket in (1, 2):
            rejection = self.checkpoint(ticket, self.commit(ticket - 1, f"ticket {ticket} rejected"))
            self.assertEqual(rejection["status"], "rejected")
            accepted = self.checkpoint(ticket, self.commit(ticket, f"ticket {ticket} accepted"))
            self.assertEqual(accepted["status"], "accepted")
        accepted_head = accepted["accepted_head"]
        for attempt in (1, 2, 3):
            head = self.commit(2, f"ticket 3 rejection {attempt}")
            result = self.checkpoint(3, head)
            self.assertEqual(result["status"], "capability_failure" if attempt == 3 else "rejected")
            self.assertEqual(result["accepted_tickets"], [1, 2])
            self.assertEqual(result["accepted_head"], accepted_head)
            self.assertEqual(self.checkpoint(3, head), result)
        report = self.invoke("capture", "--state-file", self.state)
        self.assertEqual([(item["ticket"], item["attempt"]) for item in report["checkpoints"]],
                         [(1, 1), (1, 2), (2, 1), (2, 2), (3, 1), (3, 2), (3, 3)])

    def test_protected_test_and_root_control_mutations_are_rejected(self):
        for path in ("tests/reference/test_public.py", "README.md", "new-control.json"):
            with self.subTest(path=path):
                if not self.state.exists():
                    self.prepare()
                (self.workspace / path).write_text("tampered")
                result = self.checkpoint(1, self.commit(1, path))
                self.assertIn(result["status"], ("rejected", "capability_failure"))
                self.assertEqual(result["accepted_tickets"], [])

    def test_dirty_head_wrong_head_and_new_refs_are_rejected(self):
        self.prepare()
        head = self.commit(1)
        (self.workspace / "docs/dirty").write_text("not committed")
        self.assertEqual(self.checkpoint(1, head)["status"], "rejected")
        (self.workspace / "docs/dirty").unlink()
        head = self.commit(1, "new clean candidate")
        self.git("branch", "other")
        self.assertEqual(self.checkpoint(1, head)["status"], "rejected")
        self.git("branch", "-D", "other")
        self.assertEqual(self.checkpoint(1, "f" * 40)["status"], "capability_failure")

    def test_history_rewrite_cannot_replace_accepted_prefix(self):
        initial = self.prepare()["initial_head"]
        accepted = self.commit(1)
        self.checkpoint(1, accepted)
        self.git("reset", "--hard", initial)
        result = self.checkpoint(2, self.commit(2))
        self.assertEqual(result["status"], "rejected")
        self.assertEqual(result["accepted_head"], accepted)

    def test_late_canary_only_revealed_after_valid_migration_and_not_counted_as_failure(self):
        self.prepare("isolated", 5)
        failed = self.checkpoint(5, self.commit(4))
        self.assertEqual(failed["status"], "rejected")
        self.assertNotIn("legacy-ok", json.dumps(failed))
        candidate = self.commit(5)
        reveal = self.checkpoint(5, candidate)
        self.assertEqual(reveal["status"], "revision_required")
        self.assertIn("legacy-ok", reveal["feedback"])
        self.assertEqual(self.checkpoint(5, candidate), reveal)
        self.assertEqual(self.checkpoint(5, self.commit(5, "missing legacy"))["status"], "rejected")
        (self.workspace / "src/legacy").write_text("legacy-ok")
        self.assertEqual(self.checkpoint(5, self.commit(5, "legacy fixed"))["status"], "completed")

    def test_probes_use_immutable_export_and_recheck_live_worktree(self):
        self.prepare()
        head = self.commit(1)
        (self.root / "slow").touch()
        with concurrent.futures.ThreadPoolExecutor() as pool:
            pending = pool.submit(self.checkpoint, 1, head)
            deadline = time.monotonic() + 5
            while not (self.root / "started").exists() and time.monotonic() < deadline:
                time.sleep(0.02)
            self.assertTrue((self.root / "started").exists())
            (self.workspace / "src/stage").write_text("0")
            result = pending.result()
        self.assertEqual(result["status"], "rejected")
        paths = (self.root / "probe-log").read_text().splitlines()
        self.assertTrue(all(Path(path) != self.workspace for path in paths))
        self.assertEqual(result["accepted_tickets"], [])

    def test_concurrent_duplicate_callbacks_are_serialized(self):
        self.prepare()
        head = self.commit(1)
        (self.root / "slow").touch()
        before = len((self.root / "probe-log").read_text().splitlines()) if (self.root / "probe-log").exists() else 0
        with concurrent.futures.ThreadPoolExecutor() as pool:
            a, b = list(pool.map(lambda _: self.checkpoint(1, head), range(2)))
        self.assertEqual(a, b)
        report = self.invoke("capture", "--state-file", self.state)
        self.assertEqual(len(report["checkpoints"]), 1)
        self.assertEqual(len((self.root / "probe-log").read_text().splitlines()) - before, 1)

    def test_cleanup_without_accepted_prefix_captures_and_preserves_evidence(self):
        self.prepare()
        (self.workspace / "src/stage").write_text("unfinished")
        result = self.invoke("cleanup", "--state-file", self.state)
        self.assertTrue(result["cleaned"])
        self.assertFalse(self.workspace.exists())
        report = self.invoke("capture", "--state-file", self.state)
        self.assertEqual(report["accepted_tickets"], [])
        self.assertEqual(report["accepted_patch"], "")
        self.assertIn("unfinished", report["unaccepted_patch"])
        self.assertTrue(self.invoke("cleanup", "--state-file", self.state)["cleaned"])

    def test_cleanup_refuses_replacement_directory(self):
        self.prepare()
        self.workspace.rename(self.root / "original-subject")
        self.workspace.mkdir()
        valuable = self.workspace / "valuable"
        valuable.write_text("keep")
        result = self.invoke("cleanup", "--state-file", self.state, ok=False)
        self.assertFalse(result.get("cleaned", False))
        self.assertEqual(valuable.read_text(), "keep")

    def test_prepare_refuses_nonempty_or_overlapping_workspace(self):
        self.workspace.mkdir()
        (self.workspace / "valuable").write_text("keep")
        result = self.invoke("prepare", "--fixture-root", self.fixture, "--workspace", self.workspace,
                             "--state-file", self.state, "--probes", self.probes, "--mode", "journey",
                             "--isolation", self.isolation, "--ticket", 1, "--fixture-revision", "a" * 40, ok=False)
        self.assertEqual(result["status"], "infrastructure_error")
        self.assertEqual((self.workspace / "valuable").read_text(), "keep")

    def test_revision_ack_accepts_already_compatible_same_commit_idempotently(self):
        self.prepare("isolated", 5)
        (self.workspace / "src/legacy").write_text("legacy-ok")
        head = self.commit(5)
        reveal = self.checkpoint(5, head)
        self.assertEqual(reveal["status"], "revision_required")
        self.assertEqual(reveal.get("canary_observation"), {"passed": True})
        self.assertTrue(reveal.get("revision_id"))
        self.assertEqual(self.checkpoint(5, head), reveal)
        self.assertEqual(len((self.root / "canary-log").read_text().splitlines()), 1)
        accepted = self.checkpoint(5, head, reveal["revision_id"])
        self.assertEqual(accepted["status"], "completed")
        self.assertEqual(accepted["accepted_head"], head)
        self.assertEqual(self.checkpoint(5, head, reveal["revision_id"]), accepted)

    def test_first_canary_runs_and_reports_failure_without_spending_rejection_budget(self):
        self.prepare("isolated", 5)
        head = self.commit(5)
        reveal = self.checkpoint(5, head)
        self.assertEqual(reveal["status"], "revision_required")
        self.assertEqual(reveal.get("canary_observation"), {"passed": False})
        self.assertEqual(len((self.root / "canary-log").read_text().splitlines()), 1)
        self.assertEqual(self.checkpoint(5, head), reveal)
        self.assertEqual(len((self.root / "canary-log").read_text().splitlines()), 1)
        failed_ack = self.checkpoint(5, head, reveal["revision_id"])
        self.assertEqual(failed_ack["status"], "rejected")
        self.assertEqual(self.checkpoint(5, self.commit(5, "still missing legacy"))["status"], "rejected")
        (self.workspace / "src/legacy").write_text("legacy-ok")
        self.assertEqual(self.checkpoint(5, self.commit(5, "legacy repaired"))["status"], "completed")

    def test_first_canary_infrastructure_failure_is_not_revelation_or_rejection(self):
        self.probes.write_text(self.probes.read_text().replace(
            "if a.canary:\n with", "if a.canary:raise SystemExit(2)\nif a.canary:\n with"))
        self.prepare("isolated", 5)
        result = self.invoke("checkpoint", "--state-file", self.state, "--ticket", 5,
                             "--head", self.commit(5), ok=False)
        self.assertEqual(result["status"], "infrastructure_error")
        self.assertEqual(self.invoke("capture", "--state-file", self.state)["checkpoints"], [])

    def test_invalid_revision_ack_is_rejected_without_revealing_canary(self):
        self.prepare("isolated", 5)
        result = self.checkpoint(5, self.commit(5), "another-run")
        self.assertEqual(result["status"], "rejected")
        self.assertNotIn("legacy-ok", json.dumps(result))

    def test_detached_head_is_candidate_rejection(self):
        self.prepare()
        head = self.commit(1)
        self.git("checkout", "--detach", "-q", head)
        self.assertEqual(self.checkpoint(1, head)["status"], "rejected")

    def test_capture_retains_pending_files_even_when_git_directory_is_removed(self):
        self.prepare()
        accepted = self.commit(1)
        self.checkpoint(1, accepted)
        import shutil
        shutil.rmtree(self.workspace / ".git")
        (self.workspace / "src/stage").write_text("work after git damage\n")
        report = self.invoke("capture", "--state-file", self.state, "--terminal-status", "cancelled")
        self.assertIn("work after git damage", report["unaccepted_patch"])
        self.assertIn("+1", report["accepted_patch"])

    def test_capture_retains_empty_files_symlink_targets_and_mode_changes(self):
        self.prepare()
        (self.workspace / "src/empty").touch()
        (self.workspace / "src/link").symlink_to("/some/private/path")
        (self.workspace / "src/stage").chmod(0o755)
        report = self.invoke("capture", "--state-file", self.state)
        self.assertIn("src/empty", report["unaccepted_patch"])
        self.assertIn("/some/private/path", report["unaccepted_patch"])
        self.assertIn("100755", report["unaccepted_patch"])

    def test_cleanup_terminates_only_processes_in_owned_workspace(self):
        self.prepare()
        child = subprocess.Popen([sys.executable, "-c", "import time;time.sleep(60)"], cwd=self.workspace)
        other = subprocess.Popen([sys.executable, "-c", "import time;time.sleep(60)"], cwd=self.root)
        self.addCleanup(lambda: other.poll() is None and other.kill())
        self.addCleanup(lambda: child.poll() is None and child.kill())
        time.sleep(0.1)
        self.invoke("cleanup", "--state-file", self.state)
        try:
            child.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self.fail("Cleanup left an owned application process running")
        self.assertIsNone(other.poll())
        other.terminate()
        other.wait(timeout=2)

    def test_fresh_workspace_can_commit_without_external_git_identity(self):
        self.prepare()
        (self.workspace / "src/stage").write_text("1")
        self.git("add", "src/stage")
        env = dict(os.environ, GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull)
        result = subprocess.run(["git", "-C", str(self.workspace), "commit", "-qm", "Delivery"],
                                env=env, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_subject_cannot_mutate_export_during_probe_and_gain_acceptance(self):
        self.probes.write_text(self.probes.read_text() + "\n(w/'src/stage').write_text('changed during verification')\n")
        self.prepare()
        result = self.checkpoint(1, self.commit(1))
        self.assertEqual(result["status"], "rejected")

    def test_isolated_later_ticket_preserves_previously_accepted_legacy_contract(self):
        self.prepare("isolated", 6)
        result = self.checkpoint(6, self.commit(6))
        self.assertEqual(result["status"], "rejected")
        (self.workspace / "src/legacy").write_text("legacy-ok")
        self.assertEqual(self.checkpoint(6, self.commit(6, "legacy preserved"))["status"], "completed")

    def test_capture_patch_preserves_literal_path_like_content(self):
        self.prepare()
        (self.workspace / "src/stage").write_text("literal a/a/keep and b/b/keep\n")
        report = self.invoke("capture", "--state-file", self.state)
        self.assertIn("+literal a/a/keep and b/b/keep\n", report["unaccepted_patch"])

    def test_state_and_probe_paths_inside_subject_are_rejected(self):
        for target, value in (("--state-file", self.workspace / "private/state.json"),
                              ("--probes", self.workspace / "private/probes.py")):
            args = ["prepare", "--fixture-root", self.fixture, "--workspace", self.workspace,
                    "--state-file", self.state, "--probes", self.probes, "--mode", "journey",
                    "--isolation", self.isolation, "--ticket", 1, "--fixture-revision", "a" * 40]
            args[args.index(target) + 1] = value
            result = self.invoke(*args, ok=False)
            self.assertEqual(result["status"], "infrastructure_error")
            self.assertFalse((self.workspace / "src").exists())

    def test_changed_trusted_verifier_is_infrastructure_failure_not_candidate_rejection(self):
        self.prepare()
        head = self.commit(1)
        self.probes.write_text("print('replacement verifier')")
        result = self.invoke("checkpoint", "--state-file", self.state, "--ticket", 1, "--head", head, ok=False)
        self.assertEqual(result["status"], "infrastructure_error")
        report = self.invoke("capture", "--state-file", self.state)
        self.assertEqual(report["checkpoints"], [])

    def test_isolation_backend_failure_is_not_a_capability_rejection(self):
        self.isolation.write_text("raise SystemExit(2)")
        self.prepare()
        result = self.invoke("checkpoint", "--state-file", self.state, "--ticket", 1,
                             "--head", self.commit(1), ok=False)
        self.assertEqual(result["status"], "infrastructure_error")
        self.assertEqual(self.invoke("capture", "--state-file", self.state)["checkpoints"], [])

    def test_command_deadline_kills_slow_subprocess_and_bounds_cleanup(self):
        import importlib.util
        spec = importlib.util.spec_from_file_location("swe_controller_under_test", CONTROLLER)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        module.OPERATION_DEADLINE = time.monotonic() + 0.15
        started = time.monotonic()
        with self.assertRaises(subprocess.TimeoutExpired):
            module.run([sys.executable, "-c", "import time;time.sleep(2)"], timeout=3)
        self.assertLess(time.monotonic() - started, 1.5)

    def test_missing_git_ownership_marker_is_a_candidate_rejection(self):
        self.prepare()
        head = self.commit(1)
        (self.workspace / ".git/swe-controller-owner").unlink()
        self.assertEqual(self.checkpoint(1, head)["status"], "rejected")

    def test_final_ticket_requires_nonempty_delivery_document_without_keyword_grading(self):
        self.prepare("isolated", 8)
        (self.workspace / "src/legacy").write_text("legacy-ok")
        self.assertEqual(self.checkpoint(8, self.commit(8, "missing handoff"))["status"], "rejected")
        (self.workspace / "docs/delivery.md").write_text("  \n")
        self.assertEqual(self.checkpoint(8, self.commit(8, "empty handoff"))["status"], "rejected")
        (self.workspace / "docs/delivery.md").write_text("A short useful handoff.\n")
        self.assertEqual(self.checkpoint(8, self.commit(8, "documented delivery"))["status"], "completed")

    def test_quiesce_preserves_workspace_stops_owned_processes_and_captures_shutdown_writes(self):
        self.prepare()
        app = subprocess.Popen([sys.executable, "-c", '''
import pathlib,signal,sys,time
def shutdown(signum,frame):
 pathlib.Path('docs/shutdown.md').write_text('Final shutdown write\\n')
 sys.exit(0)
signal.signal(signal.SIGTERM,shutdown)
pathlib.Path('docs/ready').touch()
while True:time.sleep(0.1)
'''], cwd=self.workspace)
        other = subprocess.Popen([sys.executable, "-c", "import time;time.sleep(60)"], cwd=self.root)
        try:
            deadline = time.monotonic() + 5
            while not (self.workspace / "docs/ready").exists() and time.monotonic() < deadline:
                time.sleep(0.02)
            self.assertTrue((self.workspace / "docs/ready").exists())
            receipt = self.invoke("quiesce", "--state-file", self.state)
            self.assertTrue(receipt["quiesced"])
            app.wait(timeout=2)
            self.assertIsNone(other.poll())
            self.assertTrue((self.workspace / "src/stage").exists())
            self.assertTrue(self.state.exists())
            self.assertEqual(self.invoke("quiesce", "--state-file", self.state), receipt)
            report = self.invoke("capture", "--state-file", self.state, "--terminal-status", "cancelled")
            self.assertIn("Final shutdown write", report["unaccepted_patch"])
            result = self.checkpoint(1, self.commit(1))
            self.assertEqual(result["status"], "capability_failure")
            self.assertEqual(result["accepted_tickets"], [])
        finally:
            for process in (app, other):
                if process.poll() is None:
                    process.kill()
                process.wait(timeout=3)

    def test_controller_termination_propagates_to_new_session_command_group(self):
        self.isolation.write_text('''import os,pathlib,signal,subprocess,sys,time
root=pathlib.Path(__file__).parent
child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)'])
def stopped(signum,frame):
 (root/'isolator-stopped').write_text('graceful')
 child.wait(timeout=2)
 sys.exit(0)
signal.signal(signal.SIGTERM,stopped)
(root/'isolator-pid').write_text(str(os.getpid()))
(root/'isolator-ready').touch()
while True:time.sleep(0.1)
''')
        self.prepare()
        head = self.commit(1)
        for signum in (signal.SIGTERM, signal.SIGINT):
            with self.subTest(signal=signum):
                for name in ("isolator-stopped", "isolator-ready", "isolator-pid"):
                    (self.root / name).unlink(missing_ok=True)
                process = subprocess.Popen([sys.executable, "-I", str(CONTROLLER), "checkpoint",
                                            "--state-file", str(self.state), "--ticket", "1", "--head", head],
                                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                try:
                    deadline = time.monotonic() + 5
                    while not (self.root / "isolator-ready").exists() and time.monotonic() < deadline:
                        time.sleep(0.02)
                    self.assertTrue((self.root / "isolator-ready").exists())
                    process.send_signal(signum)
                    try:
                        output, error = process.communicate(timeout=4)
                    except subprocess.TimeoutExpired:
                        self.fail("Controller interruption left its separate command session running")
                    self.assertTrue((self.root / "isolator-stopped").exists(), error + output)
                    self.assertNotEqual(process.returncode, 0)
                    self.assertEqual(json.loads(output)["status"], "infrastructure_error")
                finally:
                    pid_file = self.root / "isolator-pid"
                    if pid_file.exists():
                        try:
                            os.killpg(int(pid_file.read_text()), signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                    if process.poll() is None:
                        process.kill()
                    process.communicate(timeout=3)


if __name__ == "__main__":
    unittest.main()
