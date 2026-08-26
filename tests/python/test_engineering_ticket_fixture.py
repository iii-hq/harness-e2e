import pathlib
import subprocess
import sys
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from engineering_ticket_fixture import (
    LauncherError,
    REPOSITORY_ENV,
    ROOT_ENV,
    cleanup,
    prepare,
)


class EngineeringTicketFixtureLauncherTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.temporary.name)
        self.repository = self.root / "reviewed"
        self.repository.mkdir()
        self.git("init", "-q")
        (self.repository / "task.txt").write_text("reviewed fixture\n", encoding="utf-8")
        self.git("add", ".")
        self.git(
            "-c",
            "user.name=Fixture Author",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "fixture",
        )
        self.revision = self.git("rev-parse", "HEAD")
        self.git("branch", "source-extra")
        self.git("tag", "source-tag")
        self.environ = {
            REPOSITORY_ENV: str(self.repository.resolve()),
            ROOT_ENV: str((self.root / "leases-root").resolve()),
        }

    def tearDown(self):
        self.temporary.cleanup()

    def git(self, *args):
        return subprocess.run(
            ["git", "-C", str(self.repository), *args],
            check=True,
            stdout=subprocess.PIPE,
            text=True,
        ).stdout.strip()

    def test_prepare_creates_an_offline_clean_committable_clone(self):
        lease = prepare("run-123", self.revision, environ=self.environ)
        path = pathlib.Path(lease["path"])
        self.assertTrue(path.is_dir())
        self.assertEqual(
            subprocess.run(
                ["git", "-C", str(path), "rev-parse", "HEAD"],
                check=True,
                stdout=subprocess.PIPE,
                text=True,
            ).stdout.strip(),
            self.revision,
        )
        self.assertEqual(
            subprocess.run(
                ["git", "-C", str(path), "remote"],
                check=True,
                stdout=subprocess.PIPE,
                text=True,
            ).stdout.strip(),
            "",
        )
        self.assertEqual(
            subprocess.run(
                [
                    "git",
                    "-C",
                    str(path),
                    "for-each-ref",
                    "--format=%(refname)",
                ],
                check=True,
                stdout=subprocess.PIPE,
                text=True,
            ).stdout.strip(),
            "refs/heads/e2e/run-123",
        )
        self.assertEqual(
            subprocess.run(
                ["git", "-C", str(path), "config", "--local", "user.name"],
                check=True,
                stdout=subprocess.PIPE,
                text=True,
            ).stdout.strip(),
            "Harness E2E",
        )
        result = cleanup(lease["lease_id"], environ=self.environ)
        self.assertTrue(result["removed"])
        self.assertFalse(path.exists())

    def test_cleanup_requires_the_opaque_lease_not_a_path(self):
        with self.assertRaisesRegex(LauncherError, "lease id"):
            cleanup("../../reviewed", environ=self.environ)

    def test_prepare_accepts_an_immutable_git_bundle(self):
        bundle = self.root / "reviewed.bundle"
        self.git("bundle", "create", str(bundle), "refs/heads/master")
        bundle_environ = {
            REPOSITORY_ENV: str(bundle.resolve()),
            ROOT_ENV: str((self.root / "bundle-leases").resolve()),
        }

        lease = prepare("bundle-run", self.revision, environ=bundle_environ)
        path = pathlib.Path(lease["path"])
        self.assertEqual(
            subprocess.run(
                ["git", "-C", str(path), "rev-parse", "HEAD"],
                check=True,
                stdout=subprocess.PIPE,
                text=True,
            ).stdout.strip(),
            self.revision,
        )
        self.assertEqual(
            subprocess.run(
                ["git", "-C", str(path), "remote"],
                check=True,
                stdout=subprocess.PIPE,
                text=True,
            ).stdout.strip(),
            "",
        )
        cleanup(lease["lease_id"], environ=bundle_environ)

    def test_prepare_rejects_non_exact_revisions_and_unsafe_ids(self):
        with self.assertRaisesRegex(LauncherError, "full 40-character"):
            prepare("run", "main", environ=self.environ)
        with self.assertRaisesRegex(LauncherError, "execution id"):
            prepare("../run", self.revision, environ=self.environ)


if __name__ == "__main__":
    unittest.main()
