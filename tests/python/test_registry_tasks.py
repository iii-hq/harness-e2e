"""Patch handoff and input-boundary checks for the Registry task lifecycle."""
import importlib.util
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from concurrent.futures import ThreadPoolExecutor
from unittest.mock import patch

ASSETS = Path(__file__).resolve().parents[2] / "tests/fixtures/registry-version-comparison"
spec = importlib.util.spec_from_file_location("registry_lifecycle", ASSETS / "lifecycle.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class RegistryDeliveryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / "source"
        self.source.mkdir()
        self.git("init", "-q")
        self.git("config", "user.email", "test@example.invalid")
        self.git("config", "user.name", "Test")
        (self.source / "existing.txt").write_text("before\n")
        (self.source / "deleted.txt").write_text("delete me\n")
        self.git("add", ".")
        self.git("commit", "-qm", "Initial content")
        self.base = self.git("rev-parse", "HEAD").decode().strip()
        # The controller's copy, as prepare takes it before the subject starts.
        self.git_dir = self.root / "registry.git"
        shutil.copytree(self.source / ".git", self.git_dir)
        self.delivery = self.root / "delivery"
        self.delivery.mkdir()

    def git(self, *args):
        return module.run(["git", *args], cwd=self.source)

    def bundle(self):
        artifact = self.delivery / "implementation.patch"
        artifact.write_bytes(module.git_patch(self.source, self.git_dir, self.base))
        module.write_json(self.delivery / "manifest.json", {
            "base_registry_sha": self.base, "patch_sha256": module.digest(artifact),
        })

    def test_patch_preserves_index_and_replays_all_delivered_source(self):
        (self.source / "existing.txt").write_text("committed update\n")
        self.git("commit", "-qam", "Committed change")
        (self.source / "new.bin").write_bytes(bytes(range(256)))
        (self.source / "deleted.txt").unlink()
        before = self.git("diff", "--cached", "--binary")
        self.bundle()
        self.assertEqual(before, self.git("diff", "--cached", "--binary"))
        replay = self.root / "replay"
        module.run(["git", "clone", "-q", self.source, replay])
        module.run(["git", "checkout", "--detach", self.base], cwd=replay)
        with patch.object(module, "REGISTRY_SHA", self.base):
            module.apply_delivery(replay, self.delivery)
        self.assertEqual((replay / "existing.txt").read_text(), "committed update\n")
        self.assertEqual((replay / "new.bin").read_bytes(), bytes(range(256)))
        self.assertFalse((replay / "deleted.txt").exists())

    def test_handoff_rejects_changed_patch_or_base_without_mutating_source(self):
        self.bundle()
        with patch.object(module, "REGISTRY_SHA", "0" * 40):
            with self.assertRaisesRegex(ValueError, "checksum"):
                module.apply_delivery(self.source, self.delivery)
        (self.delivery / "implementation.patch").write_text("tampered")
        with patch.object(module, "REGISTRY_SHA", self.base):
            with self.assertRaisesRegex(ValueError, "checksum"):
                module.apply_delivery(self.source, self.delivery)
        self.assertEqual(self.git("status", "--porcelain"), b"")

    def test_handoff_rejects_dirty_target(self):
        self.bundle()
        (self.source / "unrelated.txt").write_text("preserve")
        with patch.object(module, "REGISTRY_SHA", self.base):
            with self.assertRaisesRegex(ValueError, "clean"):
                module.apply_delivery(self.source, self.delivery)
        self.assertEqual((self.source / "unrelated.txt").read_text(), "preserve")

    def test_failed_empty_delivery_is_blocked_but_partial_work_is_labelled(self):
        run_root = self.attempt("run", 2)
        with patch.object(module, "fixture_action", side_effect=RuntimeError("not running")):
            self.finish(run_root, "timed_out")
            self.assertFalse((run_root / "delivery").exists())
            (run_root / "workspace/registry/existing.txt").write_text("partial implementation")
            self.finish(run_root, "timed_out")
        manifest = json.loads((run_root / "delivery/manifest.json").read_text())
        self.assertEqual(manifest["subject_status"], "timed_out")
        self.assertTrue((run_root / "delivery/implementation.patch").stat().st_size)

    def test_finish_leaves_the_subject_runner_as_the_subject_left_it(self):
        # The truth is measured in a second runner; finish only seals the subject's source.
        (self.source / "existing.txt").write_text("delivered\n")
        for test in (2, 4):
            with self.subTest(test=test):
                # Unchanged since prepare: no scope deviation, so test 4 reaches what used to restart the stack.
                run_root = self.attempt(f"run-{test}", test)
                untouchable = AssertionError("finish must not touch the subject runner")
                with patch.object(module, "container_exec", side_effect=untouchable), \
                     patch.object(module, "fixture_action", side_effect=untouchable):
                    result = self.finish(run_root)
                self.assertNotIn("runtime_ready", result)
                self.assertEqual((run_root / "delivery/manifest.json").is_file(), test == 2)

    def attempt(self, name, test):
        """An attempt root as prepare leaves it: the subject's tree and the controller's git directory."""
        root = self.root / name
        source = root / "workspace/registry"
        shutil.copytree(self.source, source, symlinks=True)
        shutil.copytree(self.git_dir, root / "registry.git")
        initial = module.git_patch(source, root / "registry.git", self.base)
        module.write_json(root / "state.json", {
            "test": test, "container": "registry-task-x", "fixture_files": {}, "input_files": {},
            "initial_patch_sha256": hashlib.sha256(initial).hexdigest(),
        })
        return root

    def plant(self, source):
        """What a root subject can write: git config, hooks and attributes that each run a command."""
        marker = self.root / "marker"
        git_dir = source / ".git"

        def command(name):
            return f"echo {name} >> '{marker}'"

        for path, name in ((git_dir / "planted-hooks/post-index-change", "hook"),
                           (git_dir / "fsmonitor", "fsmonitor")):
            path.parent.mkdir(exist_ok=True)
            path.write_text(f"#!/bin/sh\n{command(name)}\n")
            path.chmod(0o755)
        # git config -f quotes the values: a hand-written value is cut at ";".
        for file, key, value in ((git_dir / "config", "filter.x.clean", command("clean") + "; cat"),
                                 (git_dir / "config", "filter.p.process", command("process") + "; cat"),
                                 (git_dir / "config", "diff.d.command", command("diff") + "; true"),
                                 (git_dir / "config", "core.fsmonitor", git_dir / "fsmonitor"),
                                 (git_dir / "config", "core.hooksPath", git_dir / "planted-hooks"),
                                 (git_dir / "config", "include.path", git_dir / "included"),
                                 (git_dir / "included", "filter.inc.clean", command("include") + "; cat")):
            module.run(["git", "config", "-f", file, key, value])
        (source / ".gitattributes").write_text(
            "* filter=x diff=d\n*.bin filter=p\n*.md filter=inc\n*.txt filter=controller\n")
        return marker

    def finish(self, root, subject_status="finished"):
        """finish against a fake subject runner: every host git call falls between its pause and unpause."""
        import argparse
        calls = []
        real_run = module.run

        def fake_run(argv, **kwargs):
            argv = [str(x) for x in argv]
            calls.append(argv)
            if argv[0] == "docker":
                return b"true true\n" if argv[1] == "inspect" else b""
            return real_run(argv, **kwargs)

        try:
            with patch.object(module, "REGISTRY_SHA", self.base), patch.object(module, "run", fake_run):
                return module.finish(argparse.Namespace(root=root, assets=ASSETS, subject_status=subject_status))
        finally:
            docker = [i for i, argv in enumerate(calls) if argv[0] == "docker"]
            self.assertEqual([(calls[i][1], calls[i][-1]) for i in docker],
                             [("pause", "registry-task-x"), ("inspect", "registry-task-x"),
                              ("unpause", "registry-task-x")])
            self.assertTrue(all(docker[1] < i < docker[2] for i, argv in enumerate(calls) if argv[0] == "git"))

    def test_git_the_subject_configured_runs_nothing_on_the_controller(self):
        root = self.attempt("run", 2)
        source = root / "workspace/registry"
        marker = self.plant(source)
        (source / "existing.txt").write_text("after\n")
        (source / "deleted.txt").unlink()
        (source / "new.bin").write_bytes(bytes(range(256)))
        (source / "new.md").write_text("notes\n")
        # A driver in the controller's global or system config (git-lfs, say) must not shape the patch either.
        controller = self.root / "controller.gitconfig"
        module.run(["git", "config", "-f", controller, "filter.controller.clean",
                    f"echo controller >> '{marker}'; cat"])
        with patch.dict(os.environ, {"GIT_CONFIG_GLOBAL": str(controller), "GIT_CONFIG_SYSTEM": str(controller)}):
            for args in (["add", "-A"], ["status"], ["diff", "--cached", "HEAD"]):
                subprocess.run(["git", *args], cwd=source, capture_output=True, timeout=60)
            ran = set(marker.read_text().split())
            self.assertLessEqual({"clean", "process", "include", "hook", "fsmonitor", "diff", "controller"},
                                 ran, "plain git in this tree runs what was planted")
            marker.unlink()
            self.finish(root)
        self.assertFalse(marker.exists())
        delivered = (root / "source.patch").read_bytes()
        self.assertEqual((root / "delivery/implementation.patch").read_bytes(), delivered)
        self.assertEqual((root / "source-status.txt").read_bytes(),
                         b" D deleted.txt\n M existing.txt\n?? .gitattributes\n?? new.bin\n?? new.md\n")
        replay = self.root / "replay"
        module.run(["git", "clone", "-q", self.source, replay])
        module.run(["git", "apply", root / "source.patch"], cwd=replay)
        self.assertEqual((replay / "existing.txt").read_text(), "after\n")
        self.assertEqual((replay / "new.bin").read_bytes(), bytes(range(256)))
        self.assertEqual((replay / "new.md").read_text(), "notes\n")
        self.assertEqual((replay / ".gitattributes").read_bytes(), (source / ".gitattributes").read_bytes())
        self.assertFalse((replay / "deleted.txt").exists())

    def test_a_repository_nested_in_the_subject_tree_runs_nothing_on_the_controller(self):
        root = self.attempt("run", 2)
        nested = root / "workspace/registry/nested"
        nested.mkdir()
        marker = self.root / "marker"

        def git(*args):
            return module.run(["git", *args], cwd=nested)

        git("init", "-q")
        (nested / "tracked.txt").write_text("committed\n")
        git("add", ".")
        git("-c", "user.email=t@example.invalid", "-c", "user.name=T", "commit", "-qm", "nested")
        (nested / "tracked.txt").write_text("different\n")  # same size: status must hash it, through the filter
        fsmonitor = nested / ".git/fsmonitor"
        fsmonitor.write_text(f"#!/bin/sh\necho nested-fsmonitor >> '{marker}'\n")
        fsmonitor.chmod(0o755)
        git("config", "filter.n.clean", f"echo nested-clean >> '{marker}'; cat")
        git("config", "core.fsmonitor", fsmonitor)
        (nested / ".git/info/attributes").write_text("* filter=n\n")
        subprocess.run(["git", "status"], cwd=nested, capture_output=True, timeout=60)
        self.assertLessEqual({"nested-clean", "nested-fsmonitor"}, set(marker.read_text().split()))
        marker.unlink()
        self.finish(root)
        self.assertFalse(marker.exists())
        self.assertIn(b"?? nested/\n", (root / "source-status.txt").read_bytes())

    def test_a_base_that_tracks_a_submodule_is_refused(self):
        self.git("update-index", "--add", "--cacheinfo", f"160000,{self.base},vendored")
        self.git("commit", "-qm", "Track a submodule")
        git_dir = self.root / "submodule.git"
        shutil.copytree(self.source / ".git", git_dir)
        with self.assertRaisesRegex(ValueError, "submodule"):
            module.git_patch(self.source, git_dir, self.git("rev-parse", "HEAD").decode().strip())

    def test_the_subject_s_git_directory_is_never_opened(self):
        for swap in ("gitfile", "symlink", "removed"):
            with self.subTest(swap=swap):
                root = self.attempt(swap, 2)
                source = root / "workspace/registry"
                marker = self.plant(source)
                (source / "existing.txt").write_text("after\n")
                before = module.git_patch(source, root / "registry.git", self.base)
                hostile = root / "hostile.git"
                (source / ".git").rename(hostile)
                if swap == "gitfile":
                    (source / ".git").write_text(f"gitdir: {hostile}\n")
                elif swap == "symlink":
                    (source / ".git").symlink_to(hostile, target_is_directory=True)
                self.assertEqual(module.git_patch(source, root / "registry.git", self.base), before)
                self.assertFalse(marker.exists())

    def test_a_registry_tree_replaced_by_a_symlink_is_not_read(self):
        root = self.attempt("run", 2)
        outside = self.root / "controller-files"
        outside.mkdir()
        (outside / "secret.txt").write_text("controller secret\n")
        shutil.rmtree(root / "workspace/registry")
        (root / "workspace/registry").symlink_to(outside, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "real directory"):
            self.finish(root)
        self.assertFalse((root / "source.patch").exists())
        self.assertFalse((root / "delivery").exists())

    def test_preparation_fetches_fresh_fixture_and_limits_inputs(self):
        fixture = self.root / "fixture"
        fixture.mkdir()
        (fixture / "artifacts").mkdir()
        (fixture / "artifacts/a.txt").write_text("artifact")
        (fixture / "seed.sql").write_text("SELECT 1;")
        (fixture / "Dockerfile").write_text("environment solution")
        (self.source / "existing.txt").write_text("delivered\n")
        self.bundle()
        self.git("checkout", "--", ".")
        revision = "f" * 40
        for test in (1, 2, 3, 4):
            root = self.root / f"test-{test}"
            calls = []
            real_run = module.run

            def fake_run(argv, **kwargs):
                calls.append([str(x) for x in argv])
                if argv[:3] == ["git", "clone", "--depth"]:
                    import shutil
                    shutil.copytree(fixture, Path(argv[-1]) / "registry-version-comparison")
                    return b""
                if argv[:2] == ["git", "fetch"]:
                    import shutil
                    shutil.copytree(fixture, Path(kwargs["cwd"]) / "registry-version-comparison")
                    shutil.copytree(self.delivery, Path(kwargs["cwd"]) / "registry-verification-delivery")
                    return b""
                if argv[:2] == ["git", "checkout"] and argv[-1] == "FETCH_HEAD":
                    return b""
                if argv[:3] == ["git", "clone", "--no-checkout"]:
                    return real_run(["git", "clone", "-q", self.source, argv[-1]])
                if argv[0] == "docker":
                    return b"image-id\n"
                return real_run(argv, **kwargs)

            import argparse
            args = argparse.Namespace(root=root, test=test, web_port=45000 + test * 2,
                                      api_port=45001 + test * 2, assets=ASSETS,
                                      fixture_revision=revision)
            with patch.object(module, "REGISTRY_SHA", self.base), patch.object(module, "run", fake_run), patch.object(module, "git_patch", return_value=b""):
                module.prepare(args)
            inputs = root / "workspace/inputs"
            self.assertTrue((root / "registry.git/HEAD").is_file())
            self.assertEqual((inputs / "reference-plan.md").exists(), test == 2)
            self.assertFalse((inputs / "Dockerfile").exists())
            self.assertEqual(["git", "clone", "--depth", "1", module.FIXTURE_URL, str(root / "fixture-checkout")] in calls, test in (2, 3))
            # Verification checks a fixed delivery: the pinned commit, never the head.
            self.assertEqual(["git", "fetch", "--quiet", "--depth=1", module.FIXTURE_URL, revision] in calls, test == 4)
            if test == 4:
                state = json.loads((root / "state.json").read_text())
                self.assertEqual(state["implementation"],
                                 json.loads((self.delivery / "manifest.json").read_text()))
                self.assertEqual((root / "workspace/registry/existing.txt").read_text(), "delivered\n")
            self.assertEqual((inputs / "seed.sql").exists(), test != 1)
            environment = json.loads((inputs / "environment.json").read_text())
            if test == 1:
                self.assertIn("Planning only", environment["runtime_requirements"])
            # Implementation and verification open the fixture's Next dev server, which hydrates only on localhost.
            host = "localhost" if test in (2, 4) else "127.0.0.1"
            self.assertEqual(environment["web_url"], f"http://{host}:{45000 + test * 2}")
            self.assertEqual("localhost" in environment["build_note"], test in (2, 4))
            # The API and the seeded artifact URLs need no hydrated page and stay where fixture.sh puts them.
            self.assertEqual(environment["api_url"], f"http://127.0.0.1:{45001 + test * 2}")
            self.assertEqual("artifact_origin=http://127.0.0.1:<web_port>" in environment["runtime_requirements"], test != 1)
            launch = next(c for c in calls if c[:3] == ["docker", "run", "-d"])
            self.assertFalse(any("/var/run/docker.sock" in c and "src=" in c for c in launch))
            self.assertEqual("--privileged" in launch, test != 1)
            self.assertEqual(any("dst=/fixture," in c for c in launch), test in (2, 4))
            self.assertFalse(any("controller-assets" in c for c in launch))
            # iii telemetry off in the runner and, through it, the fixture's services.
            self.assertIn(["-e", "III_TELEMETRY_ENABLED=false"],
                          [launch[i:i + 2] for i in range(len(launch) - 1)])
            after = [" ".join(call) for call in calls[calls.index(launch) + 1:]]
            expected = (["docker inspect", "apk add"]
                        + (["docker info", "TCP-LISTEN:8080", "TCP-LISTEN:8081"] if test != 1 else [])
                        + (["fixture.sh up", "fixture.sh check"] if test in (2, 4) else []))
            self.assertEqual(len(after), len(expected), after)
            for call, marker in zip(after, expected):
                self.assertIn(marker, call)

    def subject_root(self, test, name="run"):
        """A prepared subject root whose fixture checkout is a real repository."""
        import shutil
        root = self.root / name
        checkout = root / "fixture-checkout"
        fixture = checkout / "registry-version-comparison"
        fixture.mkdir(parents=True)
        (fixture / "seed.sql").write_text("SELECT 1;")
        (fixture / "fixture.sh").write_text("#!/bin/sh\n")
        (fixture / "fixture.sh").chmod(0o755)
        shutil.copytree(self.delivery, checkout / "registry-verification-delivery")
        for args in (["init", "-q"], ["add", "."],
                     ["-c", "user.email=t@example.invalid", "-c", "user.name=T", "commit", "-qm", "fixture"]):
            module.run(["git", *args], cwd=checkout)
        state = {"test": test, "container": "registry-task-subject",
                 "fixture_files": module.fixture_hashes(fixture)}
        if test == 4:
            state["implementation"] = json.loads((self.delivery / "manifest.json").read_text())
        module.write_json(root / "state.json", state)
        return root

    def measure(self, root, inspect="true true", up=lambda count: b""):
        """measure's Docker calls and fixture actions; each action with existing.txt as it was."""
        import argparse
        calls, actions = [], []
        real_run = module.run
        source = root / "measurement/workspace/registry"

        def fake_run(argv, **kwargs):
            argv = [str(x) for x in argv]
            calls.append(argv)
            if argv[:3] == ["git", "clone", "--no-checkout"]:
                return real_run(["git", "clone", "-q", self.source, argv[-1]])
            if argv[:2] == ["docker", "pause"] and inspect != "true true":
                raise RuntimeError("docker failed (1): container is not running")
            if argv[:2] == ["docker", "inspect"]:
                return f"{inspect}\n".encode() if "{{.State.Paused}}" in argv[3] else b"image-id\n"
            if argv[0] == "docker":
                if argv[-1].startswith("/fixture/fixture.sh "):
                    action = argv[-1].split(" ", 1)[1]
                    actions.append((action, (source / "existing.txt").read_text()))
                    if action == "up":
                        return up(sum(name == "up" for name, _ in actions))
                return b""
            return real_run(argv, **kwargs)

        args = argparse.Namespace(root=root, web_port=2222, api_port=2223)
        with patch.object(module, "REGISTRY_SHA", self.base), patch.object(module, "run", fake_run):
            try:
                return module.measure(args), calls, actions
            except Exception as error:
                error.calls = calls
                raise

    def delivered(self):
        (self.source / "existing.txt").write_text("delivered\n")
        self.bundle()
        self.git("checkout", "--", ".")

    def test_measurement_of_the_pinned_delivery_starts_once(self):
        self.delivered()
        root = self.subject_root(4)
        result, calls, actions = self.measure(root)
        measured = root / "measurement"
        self.assertEqual(result, {"runtime_ready": True, "subject_paused": True})
        self.assertEqual(actions, [("up", "delivered\n")])
        launch = next(c for c in calls if c[:3] == ["docker", "run", "-d"])
        mounts = [launch[i + 1] for i, value in enumerate(launch) if value == "--mount"]
        self.assertEqual(len(mounts), 2)
        self.assertFalse(any(f"{root}/workspace," in m or "fixture-checkout" in m for m in mounts))
        self.assertIn(["git", "clone", "--no-checkout", module.REGISTRY_URL,
                       str(measured / "workspace/registry")], calls)
        state = json.loads((measured / "state.json").read_text())
        self.assertEqual((state["web_port"], state["api_port"]), (2222, 2223))
        self.assertIs(state["subject_paused"], True)
        self.assertNotIn("patch_application", state)

        changed = self.subject_root(4, "changed")
        subject = json.loads((changed / "state.json").read_text())
        subject["implementation"]["patch_sha256"] = "0" * 64
        module.write_json(changed / "state.json", subject)
        with self.assertRaisesRegex(RuntimeError, "pinned delivery changed") as raised:
            self.measure(changed)
        self.assertFalse(any(c[:2] == ["docker", "run"] for c in raised.exception.calls))

    def test_a_tampered_fixture_tree_does_not_reach_the_measurement(self):
        # The subject had the working tree mounted and, as privileged root, can remount it rw.
        self.delivered()
        root = self.subject_root(4)
        tree = root / "fixture-checkout/registry-version-comparison"
        (tree / "seed.sql").write_text("SELECT 1; INSERT INTO forged VALUES (1);")
        (tree / "fixture.sh").chmod(0o644)
        self.assertEqual(self.measure(root)[0], {"runtime_ready": True, "subject_paused": True})
        measured = root / "measurement/registry-version-comparison"
        self.assertEqual((measured / "seed.sql").read_text(), "SELECT 1;")
        self.assertTrue(module.os.access(measured / "fixture.sh", module.os.X_OK))

    def test_a_subject_runner_that_cannot_be_paused_is_not_measured(self):
        self.delivered()
        with self.subTest("running, not paused"):
            with self.assertRaisesRegex(RuntimeError, "could not be paused") as raised:
                self.measure(self.subject_root(4, "running"), inspect="true false")
            self.assertFalse(any(c[:2] == ["docker", "run"] for c in raised.exception.calls))
        with self.subTest("exited"):
            root = self.subject_root(4, "exited")
            _result, calls, _actions = self.measure(root, inspect="false false")
            state = json.loads((root / "measurement/state.json").read_text())
            self.assertIs(state["subject_paused"], False)
            self.assertTrue(any(c[:2] == ["docker", "run"] for c in calls))
            named = {c[1] for c in calls if c[0] == "docker"
                     and any("registry-task-subject" in part for part in c)}
            self.assertEqual(named, {"pause", "inspect"})

    def test_only_a_failure_after_the_patch_is_the_delivery_s(self):
        import shutil
        self.delivered()

        def implementation(name, patch_text=None):
            root = self.subject_root(2, name)
            if patch_text is None:
                shutil.copytree(self.delivery, root / "delivery")
            elif patch_text is not False:
                (root / "delivery").mkdir()
                (root / "delivery/implementation.patch").write_text(patch_text)
                module.write_json(root / "delivery/manifest.json", {
                    "base_registry_sha": self.base,
                    "patch_sha256": module.digest(root / "delivery/implementation.patch")})
            return root

        def applied(root):
            return json.loads((root / "measurement/state.json").read_text())["patch_application"]

        def recorded(root, **result):
            # The delivery status carries the record into the archived evidence.
            return {**result, "subject_paused": True, "patch_application": applied(root)}

        with self.subTest("good patch"):
            root = implementation("good")
            result, _calls, actions = self.measure(root)
            self.assertEqual(actions, [("up", "before\n"), ("up", "delivered\n")])
            self.assertEqual(applied(root)["exit_code"], 0)
            self.assertEqual(result, recorded(root, runtime_ready=True))
            self.assertTrue((root / "final-start.log").is_file())  # top level: archived
        with self.subTest("patch that does not apply"):
            root = implementation("stale", "diff --git a/existing.txt b/existing.txt\n"
                                  "--- a/existing.txt\n+++ b/existing.txt\n"
                                  "@@ -1 +1 @@\n-not the base\n+delivered\n")
            result, _calls, actions = self.measure(root)
            self.assertEqual([name for name, _ in actions], ["up"])
            self.assertNotEqual(applied(root)["exit_code"], 0)
            self.assertIn("patch failed", applied(root)["stderr"])
            self.assertEqual(result, recorded(root, runtime_ready=False,
                                              runtime_error="delivered patch does not apply to the pinned base"))
        with self.subTest("patched start fails"):
            root = implementation("broken")

            def up(count):
                if count == 2:
                    raise RuntimeError("app exited with code 1")
                return b""
            result, _calls, _actions = self.measure(root, up=up)
            self.assertEqual(result, recorded(root, runtime_ready=False, runtime_error="app exited with code 1"))
            self.assertEqual(applied(root)["exit_code"], 0)
        with self.subTest("baseline start fails"):
            def baseline(_count):
                raise RuntimeError("pull rate limit")
            with self.assertRaisesRegex(RuntimeError, "pull rate limit"):
                self.measure(implementation("baseline"), up=baseline)
        for name, patch_text in (("empty", ""), ("missing", False)):
            with self.subTest(name):
                root = implementation(name, patch_text)
                result, _calls, actions = self.measure(root)
                self.assertEqual([action for action, _ in actions], ["up"])
                self.assertEqual(result, recorded(root, runtime_ready=True))
                self.assertEqual(applied(root)["exit_code"], 1)

    def test_cleanup_removes_the_measurement_runner_then_releases_the_subject(self):
        import argparse

        def cleaned(root, measurement_exit=0):
            calls = []

            def fake_run(argv, **_kwargs):
                calls.append([str(x) for x in argv])
                return b"id\n" if argv[:2] == ["docker", "ps"] else b"log\n"

            def fake_subprocess(argv, **_kwargs):
                calls.append(list(argv))
                code = measurement_exit if argv == ["docker", "rm", "-fv", "measure"] else 0
                if isinstance(code, Exception):
                    raise code
                return module.subprocess.CompletedProcess(argv, code, b"", b"rm failed" if code else b"")

            with patch.object(module, "run", fake_run), \
                 patch.object(module.subprocess, "run", side_effect=fake_subprocess):
                return module.cleanup(argparse.Namespace(root=root)), calls

        root = self.root / "both"
        (root / "measurement").mkdir(parents=True)
        module.write_json(root / "state.json", {"test": 4, "container": "subject"})
        module.write_json(root / "measurement/state.json", {"test": 4, "container": "measure"})
        result, calls = cleaned(root)
        order = [calls.index(call) for call in (
            ["docker", "rm", "-fv", "measure"], ["docker", "unpause", "subject"],
            ["docker", "exec", "-i", "-w", "/workspace", "subject", "sh", "-lc", "/fixture/fixture.sh logs"],
            ["docker", "rm", "-fv", "subject"])]
        self.assertEqual(order, sorted(order))
        self.assertEqual(result["cleanup_exit_code"], 0)
        self.assertTrue((root / "measurement/compose.log").is_file())
        self.assertTrue((root / "measurement/container.log").is_file())
        # A measurement runner that fails to go never keeps the subject paused.
        for failure, stderr in ((1, "rm failed"),
                                (module.subprocess.TimeoutExpired(["docker", "rm"], 60), "timed out")):
            with self.subTest(measurement_failure=failure):
                result, calls = cleaned(root, measurement_exit=failure)
                self.assertEqual(result["cleanup_exit_code"], 1)
                self.assertIn(stderr, result["stderr"])
                self.assertIn(["docker", "unpause", "subject"], calls)
                self.assertIn(["docker", "rm", "-fv", "subject"], calls)

        subject_only = self.root / "subject-only"
        subject_only.mkdir()
        module.write_json(subject_only / "state.json", {"test": 4, "container": "subject"})
        result, calls = cleaned(subject_only)
        self.assertEqual([c for c in calls if c[:2] == ["docker", "rm"]], [["docker", "rm", "-fv", "subject"]])
        self.assertEqual(result["cleanup_exit_code"], 0)

    def test_measure_takes_its_ports(self):
        import contextlib
        import io
        argv = ["lifecycle.py", "measure", "--root", str(self.root)]
        with patch.object(module.sys, "argv", argv + ["--web-port", "1", "--api-port", "2"]), \
             patch.object(module, "measure", return_value={"runtime_ready": True}) as measure, \
             contextlib.redirect_stdout(io.StringIO()) as output:
            module.main()
        args = measure.call_args.args[0]
        self.assertEqual((args.root, args.web_port, args.api_port), (self.root.resolve(), 1, 2))
        self.assertEqual(json.loads(output.getvalue()), {"runtime_ready": True})
        with patch.object(module.sys, "argv", argv), \
             contextlib.redirect_stderr(io.StringIO()) as error, self.assertRaises(SystemExit):
            module.main()
        self.assertIn("measure requires", error.getvalue())

    def test_evidence_collection_rejects_symlinks_and_bounds(self):
        cases = []

        target = self.root / "target"
        target.mkdir()
        root_link = self.root / "root-link"
        root_link.symlink_to(target, target_is_directory=True)
        cases.append(("root symlink", root_link, {}, "real directory"))

        nested_link = self.root / "nested-link"
        nested_link.mkdir()
        (nested_link / "escape").symlink_to(target, target_is_directory=True)
        cases.append(("nested symlink", nested_link, {}, "real directories"))

        count = self.root / "count"
        count.mkdir()
        (count / "a").write_bytes(b"a")
        (count / "b").write_bytes(b"b")
        cases.append(("count", count, {"max_files": 1}, "too many"))

        directories = self.root / "directories"
        directories.mkdir()
        (directories / "a").mkdir()
        (directories / "b").mkdir()
        cases.append(("directory count", directories, {"max_entries": 1},
                      "too many evidence entries"))

        individual = self.root / "individual"
        individual.mkdir()
        (individual / "a").write_bytes(b"ab")
        cases.append(("individual size", individual, {"max_file_bytes": 1}, "size limit"))

        total = self.root / "total"
        total.mkdir()
        (total / "a").write_bytes(b"ab")
        (total / "b").write_bytes(b"cd")
        cases.append(("total size", total, {"max_total_bytes": 3}, "total size"))

        for name, directory, limits, message in cases:
            with self.subTest(name=name, implementation="controller"):
                with self.assertRaisesRegex(ValueError, message):
                    module.artifact_manifest(directory, **limits)
            with self.subTest(name=name, implementation="container"):
                with self.assertRaisesRegex(RuntimeError, message):
                    module.run([module.sys.executable, "-c",
                                module.manifest_script(**limits), directory])

        valid = self.root / "valid"
        valid.mkdir()
        proof = valid / "proof.json"
        proof.write_bytes(b"{}")
        expected = module.artifact_manifest(valid)
        payload = json.loads(module.run([module.sys.executable, "-c",
                                         module.manifest_script(), valid]))
        proof.write_bytes(b"changed after collection")
        self.assertEqual(module.base64.b64decode(payload[0].pop("data_base64")), b"{}")
        self.assertEqual(expected, payload)

    def test_exec_limit_failure_cleans_stage_without_receipt(self):
        import argparse
        root = self.root / "limited-run"
        (root / "workspace/output").mkdir(parents=True)
        module.write_json(root / "state.json", {"container": "private-runner"})
        calls = []

        def fake_run(argv, **_kwargs):
            argv = [str(value) for value in argv]
            calls.append(argv)
            if argv[:3] == ["docker", "exec", "private-runner"] and "python3" in argv:
                raise RuntimeError("too many evidence files")
            return b""

        completed = module.subprocess.CompletedProcess([], 0, b"", b"")
        args = argparse.Namespace(root=root, command="python verify.py", timeout_ms=1000)
        with patch.object(module, "run", side_effect=fake_run), \
             patch.object(module.subprocess, "run", return_value=completed):
            with self.assertRaisesRegex(RuntimeError, "too many evidence files"):
                module.execute(args)
        self.assertTrue(any("rm" in call and "--" in call for call in calls))
        self.assertFalse((root / "commands").exists())
        self.assertFalse((root / "execution-evidence").exists())

    def test_concurrent_execs_materialize_only_private_staged_evidence(self):
        import argparse
        root = self.root / "run"
        (root / "workspace/output").mkdir(parents=True)
        module.write_json(root / "state.json", {"container": "private-runner"})
        proof = b'{"check_id":"example","status":"fail","assertions":[{"id":"http","passed":false,"expected":{"status":200},"observed":{"status":500}}]}'
        staged = [{"path": "proof.json", "sha256": hashlib.sha256(proof).hexdigest(),
                   "size_bytes": len(proof),
                   "data_base64": module.base64.b64encode(proof).decode()}]
        calls = []

        def fake_run(argv, **_kwargs):
            argv = [str(value) for value in argv]
            calls.append(argv)
            if argv[:3] == ["docker", "exec", "private-runner"] and "python3" in argv:
                return json.dumps(staged).encode()
            return b""

        completed = module.subprocess.CompletedProcess([], 1, b"observed failure", b"")
        args = argparse.Namespace(root=root, command="python verify.py", timeout_ms=1000)
        with patch.object(module, "run", side_effect=fake_run), \
             patch.object(module.subprocess, "run", return_value=completed):
            with ThreadPoolExecutor(max_workers=2) as pool:
                results = list(pool.map(lambda _: module.execute(args), range(2)))

        self.assertEqual([result["exit_code"] for result in results], [1, 1])
        receipts = [json.loads(path.read_text()) for path in (root / "commands").glob("*.json")]
        self.assertEqual(len(receipts), 2)
        paths = {receipt["artifacts"][0]["path"] for receipt in receipts}
        self.assertEqual(len(paths), 2)
        self.assertTrue(all(path.startswith("execution-evidence/") for path in paths))
        self.assertTrue(all(receipt["artifacts"][0]["sha256"] == hashlib.sha256(proof).hexdigest()
                            for receipt in receipts))
        self.assertTrue(all((root / path).read_bytes() == proof for path in paths))
        self.assertFalse(any(call[:2] == ["docker", "cp"] for call in calls))

    def test_exec_materializes_snapshot_when_stage_changes_after_collection(self):
        import argparse
        root = self.root / "stage-race"
        (root / "workspace/output").mkdir(parents=True)
        module.write_json(root / "state.json", {"container": "private-runner"})
        stage = self.root / "simulated-stage"
        stage.mkdir()
        proof = stage / "proof.json"
        original = b'{"observed":"bounded"}'
        proof.write_bytes(original)
        calls = []

        def fake_run(argv, **_kwargs):
            argv = [str(value) for value in argv]
            calls.append(argv)
            if argv[:3] == ["docker", "exec", "private-runner"] and "python3" in argv:
                snapshot = module.artifact_manifest(stage, include_data=True)
                proof.write_bytes(b"changed and grown after collection" * 20)
                return json.dumps(snapshot).encode()
            return b""

        completed = module.subprocess.CompletedProcess([], 0, b"", b"")
        args = argparse.Namespace(root=root, command="python verify.py", timeout_ms=1000)
        with patch.object(module, "run", side_effect=fake_run), \
             patch.object(module.subprocess, "run", return_value=completed):
            result = module.execute(args)
        materialized = root / result["artifacts"][0]["path"]
        self.assertEqual(materialized.read_bytes(), original)
        self.assertFalse(any(call[:2] == ["docker", "cp"] for call in calls))

    def test_exec_rejects_payload_that_does_not_match_the_manifest(self):
        import argparse
        root = self.root / "tampered-run"
        (root / "workspace/output").mkdir(parents=True)
        module.write_json(root / "state.json", {"container": "private-runner"})
        expected = b'{"expected":true}'
        staged = [{"path": "proof.json", "sha256": hashlib.sha256(expected).hexdigest(),
                   "size_bytes": len(expected),
                   "data_base64": module.base64.b64encode(b'{"tampered":true}').decode()}]

        def fake_run(argv, **_kwargs):
            argv = [str(value) for value in argv]
            if argv[:3] == ["docker", "exec", "private-runner"] and "python3" in argv:
                return json.dumps(staged).encode()
            return b""

        completed = module.subprocess.CompletedProcess([], 0, b"", b"")
        args = argparse.Namespace(root=root, command="python verify.py", timeout_ms=1000)
        with patch.object(module, "run", side_effect=fake_run), \
             patch.object(module.subprocess, "run", return_value=completed):
            with self.assertRaisesRegex(ValueError, "does not match its manifest"):
                module.execute(args)
        self.assertFalse((root / "commands").exists())
        self.assertFalse(any((root / "execution-evidence").glob("*")))


if __name__ == "__main__":
    unittest.main()
