from __future__ import annotations

import hashlib
import io
import json
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import verify_subject_artifact as verifier


REVISION = "1" * 40


def digest(data: bytes) -> str:
    return f"sha256:{hashlib.sha256(data).hexdigest()}"


class SubjectArtifactVerificationTests(unittest.TestCase):
    def build_archive(self, root: Path, *, malicious: bool = False) -> tuple[Path, dict]:
        archive_path = root / "subject.tar"
        payload = b"#!/bin/sh\nexit 0\n"
        with tarfile.open(archive_path, "w") as archive:
            info = tarfile.TarInfo("bin/harness")
            info.mode = 0o755
            info.size = len(payload)
            archive.addfile(info, io.BytesIO(payload))
            if malicious:
                escape = tarfile.TarInfo("../escape")
                escape.size = 1
                archive.addfile(escape, io.BytesIO(b"x"))
        archive_bytes = archive_path.read_bytes()
        manifest = {
            "schema_version": 1,
            "repository": "iii-hq/workers",
            "revision": REVISION,
            "created_at": "2026-08-12T00:00:00Z",
            "archive": {
                "sha256": digest(archive_bytes),
                "size_bytes": len(archive_bytes),
                "media_type": "application/x-tar",
            },
            "files": [
                {
                    "path": "bin/harness",
                    "sha256": digest(payload),
                    "size_bytes": len(payload),
                    "executable": True,
                }
            ],
            "entrypoints": [
                {
                    "worker": "harness",
                    "path": "bin/harness",
                    "args": ["--url", "{iii_url}"],
                    "readiness_functions": ["harness::send"],
                }
            ],
        }
        return archive_path, manifest

    def test_verifies_and_extracts_every_declared_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive, manifest = self.build_archive(root)
            manifest_path = root / "manifest.json"
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

            loaded = verifier.load_manifest(manifest_path)
            verifier.verify_archive(loaded, archive, root / "extracted")

            extracted = root / "extracted/bin/harness"
            self.assertEqual(extracted.read_bytes(), b"#!/bin/sh\nexit 0\n")
            self.assertTrue(extracted.stat().st_mode & 0o100)

    def test_rejects_archive_digest_changes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive, manifest = self.build_archive(root)
            manifest["archive"]["sha256"] = "sha256:" + "0" * 64
            with self.assertRaisesRegex(verifier.VerificationError, "SHA-256"):
                verifier.verify_archive(manifest, archive, root / "extracted")

    def test_rejects_archive_path_traversal(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive, manifest = self.build_archive(root, malicious=True)
            with self.assertRaisesRegex(verifier.VerificationError, "unsafe archive path"):
                verifier.verify_archive(manifest, archive, root / "extracted")
            self.assertFalse((root / "escape").exists())

    def test_rejects_undeclared_manifest_fields(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _, manifest = self.build_archive(root)
            manifest["mutable_ref"] = "latest"
            path = root / "manifest.json"
            path.write_text(json.dumps(manifest), encoding="utf-8")
            with self.assertRaisesRegex(verifier.VerificationError, "fields"):
                verifier.load_manifest(path)


if __name__ == "__main__":
    unittest.main()
