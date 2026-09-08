"""Patch handoff and input-boundary checks for the Registry task lifecycle."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
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
        self.delivery = self.root / "delivery"
        self.delivery.mkdir()

    def git(self, *args):
        return module.run(["git", *args], cwd=self.source)

    def bundle(self):
        artifact = self.delivery / "implementation.patch"
        artifact.write_bytes(module.git_patch(self.source, self.base))
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
        import argparse
        run_root = self.root / "run"
        (run_root / "workspace").mkdir(parents=True)
        (run_root / "workspace/registry").symlink_to(self.source, target_is_directory=True)
        module.write_json(run_root / "state.json", {
            "test": 2, "initial_patch_sha256": module.hashlib.sha256(b"").hexdigest(),
            "fixture_files": {}, "input_files": {},
        })
        args = argparse.Namespace(root=run_root, assets=ASSETS, subject_status="timed_out")
        with patch.object(module, "REGISTRY_SHA", self.base), patch.object(module, "fixture_action", side_effect=RuntimeError("not running")):
            # git_patch's default base is bound at definition time; use the test repo base.
            original_patch = module.git_patch
            with patch.object(module, "git_patch", side_effect=lambda source: original_patch(source, self.base)):
                module.finish(args)
                self.assertFalse((run_root / "delivery").exists())
                (self.source / "existing.txt").write_text("partial implementation")
                module.finish(args)
        manifest = json.loads((run_root / "delivery/manifest.json").read_text())
        self.assertEqual(manifest["subject_status"], "timed_out")
        self.assertTrue((run_root / "delivery/implementation.patch").stat().st_size)

    def test_preparation_fetches_fresh_fixture_and_limits_inputs(self):
        fixture = self.root / "fixture"
        fixture.mkdir()
        (fixture / "artifacts").mkdir()
        (fixture / "artifacts/a.txt").write_text("artifact")
        (fixture / "seed.sql").write_text("SELECT 1;")
        (fixture / "Dockerfile").write_text("environment solution")
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
                if argv[:3] == ["git", "clone", "--no-checkout"]:
                    return real_run(["git", "clone", "-q", self.source, argv[-1]])
                if argv[0] == "docker":
                    return b"image-id\n"
                return real_run(argv, **kwargs)

            self.bundle()
            import argparse
            args = argparse.Namespace(root=root, test=test, web_port=45000 + test * 2,
                                      api_port=45001 + test * 2, assets=ASSETS,
                                      implementation=self.delivery)
            with patch.object(module, "REGISTRY_SHA", self.base), patch.object(module, "run", fake_run), patch.object(module, "git_patch", return_value=b""):
                module.prepare(args)
            inputs = root / "workspace/inputs"
            self.assertEqual((inputs / "reference-plan.md").exists(), test == 2)
            self.assertFalse((inputs / "Dockerfile").exists())
            self.assertIn(["git", "clone", "--depth", "1", module.FIXTURE_URL, str(root / "fixture-checkout")], calls)
            launch = next(c for c in calls if c[:3] == ["docker", "run", "-d"])
            self.assertFalse(any("/var/run/docker.sock" in c and "src=" in c for c in launch))
            self.assertEqual("--privileged" in launch, test != 1)
            self.assertEqual(any("dst=/fixture," in c for c in launch), test in (2, 4))
            self.assertFalse(any("controller-assets" in c for c in launch))


if __name__ == "__main__":
    unittest.main()
