import hashlib
import importlib.util
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[2] / "scripts" / "release_worker.py"
SPEC = importlib.util.spec_from_file_location("release_worker", SCRIPT)
assert SPEC and SPEC.loader
release_worker = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release_worker)


class ReleaseAssetsTests(unittest.TestCase):
    def test_declares_each_target_archive_checked_against_its_published_digest(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for target in release_worker.TARGETS:
                name = f"harness-e2e-{target}.tar.gz"
                (root / name).write_bytes(target.encode())
                digest = hashlib.sha256(target.encode()).hexdigest()
                (root / f"harness-e2e-{target}.sha256").write_text(f"{digest}  {name}\n")

            assets = release_worker.release_assets(root)

            self.assertEqual([asset["target"] for asset in assets], list(release_worker.TARGETS))
            self.assertEqual(assets[0], {
                "name": "harness-e2e-x86_64-unknown-linux-gnu.tar.gz",
                "target": "x86_64-unknown-linux-gnu",
                "sha256": hashlib.sha256(b"x86_64-unknown-linux-gnu").hexdigest(),
                "size_bytes": len(b"x86_64-unknown-linux-gnu"),
            })

            (root / "harness-e2e-aarch64-apple-darwin.tar.gz").write_bytes(b"tampered")
            with self.assertRaisesRegex(ValueError, "does not match"):
                release_worker.release_assets(root)


if __name__ == "__main__":
    unittest.main()
