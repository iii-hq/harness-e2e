from __future__ import annotations

import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/kanban_eval"))

import snapshot


class KanbanSnapshotTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.repository = self.root / "source"
        self.repository.mkdir()
        self.git("init", "--quiet", "--initial-branch=main")

        (self.repository / "app.txt").write_text("base\n", encoding="utf-8")
        self.base = self.commit("base", "2001-01-01T00:00:00Z")
        (self.repository / "app.txt").write_text("reference\n", encoding="utf-8")
        self.reference = self.commit("reference", "2001-01-02T00:00:00Z")
        (self.repository / "future-canary.txt").write_text("must stay private\n", encoding="utf-8")
        (self.repository / "scenarios").mkdir()
        (self.repository / "scenarios/private.md").write_text("private evaluator\n", encoding="utf-8")
        self.future = self.commit("future", "2001-01-03T00:00:00Z")

        self.catalog = self.root / "catalog.json"
        self.write_catalog(self.base, self.reference)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def git(self, *args: str, check: bool = True, cwd: Path | None = None) -> str:
        result = subprocess.run(
            ["git", "-C", str(cwd or self.repository), *args],
            check=check,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        return result.stdout.strip()

    def commit(self, message: str, date: str) -> str:
        self.git("add", "--all")
        self.git(
            "-c",
            "user.name=Fixture Author",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--quiet",
            "--date",
            date,
            "-m",
            message,
        )
        return self.git("rev-parse", "HEAD")

    def write_catalog(self, base: str, reference: str) -> None:
        self.catalog.write_text(
            json.dumps(
                {
                    "schema": "kanban-scenarios/v1",
                    "shared_prompt": "Work only in the supplied snapshot.",
                    "cases": [
                        {
                            "id": "kanban_c1_test",
                            "base_commit": base,
                            "reference_commit": reference,
                            "prompt": "Implement the next behavior.",
                            "criteria": ["The behavior works."],
                        }
                    ],
                }
            ),
            encoding="utf-8",
        )

    def test_exports_only_pinned_tree_into_fresh_deterministic_repository(self) -> None:
        destination = self.root / "subject"
        digest = hashlib.sha256(self.catalog.read_bytes()).hexdigest()
        metadata = snapshot.prepare(
            self.repository,
            self.catalog,
            "kanban_c1_test",
            "base",
            destination,
            expected_catalog_sha256=f"sha256:{digest}",
        )

        self.assertEqual((destination / "app.txt").read_text(), "base\n")
        self.assertFalse((destination / "future-canary.txt").exists())
        self.assertFalse((destination / "scenarios").exists())
        self.assertFalse((destination / "prompt.md").exists())
        self.assertEqual(self.git("remote", cwd=destination), "")
        self.assertEqual(self.git("rev-list", "--count", "--all", cwd=destination), "1")
        self.assertEqual(self.git("status", "--porcelain", cwd=destination), "")
        self.assertNotEqual(metadata["snapshot_git_head"], self.base)
        self.assertEqual(metadata["source_sha"], self.base)
        self.assertEqual(metadata["base_sha"], self.base)
        self.assertEqual(metadata["reference_sha"], self.reference)
        self.assertEqual(metadata["catalog_sha256"], digest)
        self.assertIn("Implement the next behavior.", metadata["prompt"])
        self.assertNotIn(self.reference, metadata["prompt"])
        self.assertNotEqual(
            subprocess.run(
                ["git", "-C", str(destination), "cat-file", "-e", self.future],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            ).returncode,
            0,
        )

        second = self.root / "subject-again"
        second_metadata = snapshot.prepare(
            self.repository, self.catalog, "kanban_c1_test", "base", second
        )
        self.assertEqual(second_metadata["snapshot_git_head"], metadata["snapshot_git_head"])

    def test_rejects_digest_mismatch_and_non_parent_reference(self) -> None:
        empty = self.root / 'empty'
        empty.mkdir()
        link = self.root / 'destination-link'
        link.symlink_to(empty, target_is_directory=True)
        with self.assertRaisesRegex(snapshot.SnapshotError, 'symlink'):
            snapshot.prepare(self.repository, self.catalog, 'kanban_c1_test', 'base', link)
        self.assertEqual(list(empty.iterdir()), [])
        with self.assertRaisesRegex(snapshot.SnapshotError, "SHA-256"):
            snapshot.prepare(
                self.repository,
                self.catalog,
                "kanban_c1_test",
                "base",
                self.root / "digest-failure",
                expected_catalog_sha256="0" * 64,
            )

        self.write_catalog(self.base, self.future)
        with self.assertRaisesRegex(snapshot.SnapshotError, "exactly the base as parent"):
            snapshot.prepare(
                self.repository,
                self.catalog,
                "kanban_c1_test",
                "reference",
                self.root / "parent-failure",
            )

    def test_rejects_escaping_symlink_before_extraction(self) -> None:
        unsafe = self.root / "unsafe-source"
        unsafe.mkdir()
        self.git("init", "--quiet", "--initial-branch=main", cwd=unsafe)
        (unsafe / "alias").symlink_to(".")
        (unsafe / "escape").symlink_to("alias/../outside")
        self.repository = unsafe
        unsafe_base = self.commit("unsafe base", "2002-01-01T00:00:00Z")
        (unsafe / "safe.txt").write_text("reference\n", encoding="utf-8")
        unsafe_reference = self.commit("unsafe reference", "2002-01-02T00:00:00Z")
        self.write_catalog(unsafe_base, unsafe_reference)
        destination = self.root / "unsafe-subject"

        with self.assertRaisesRegex(snapshot.SnapshotError, "symlink escapes"):
            snapshot.prepare(
                unsafe,
                self.catalog,
                "kanban_c1_test",
                "base",
                destination,
            )
        self.assertFalse(destination.exists())
        self.assertFalse((self.root / "outside").exists())


if __name__ == "__main__":
    unittest.main()
