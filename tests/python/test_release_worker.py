import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

from scripts.release_worker import (
    TARGETS,
    TAG_RE,
    build_payload,
    release_maturity,
    schema_is_typed,
    tag_metadata,
)


class ReleaseWorkerTest(unittest.TestCase):
    def test_release_tag_supports_workers_maturity_ladder(self):
        self.assertIsNotNone(TAG_RE.fullmatch("harness-e2e/v1.2.3"))
        self.assertIsNotNone(TAG_RE.fullmatch("harness-e2e/v1.2.3-experimental"))
        self.assertIsNotNone(TAG_RE.fullmatch("harness-e2e/v1.2.3-alpha"))
        self.assertIsNotNone(TAG_RE.fullmatch("harness-e2e/v1.2.3-beta"))
        for invalid in (
            "v1.2.3",
            "harness-e2e/1.2.3",
            "harness-e2e/v1.2.3-rc.1",
            "harness-e2e/v1.2.3-alpha.1",
            "harness-e2e/v01.2.3",
        ):
            self.assertIsNone(TAG_RE.fullmatch(invalid))

        self.assertEqual(release_maturity("1.2.3"), "stable")
        self.assertEqual(release_maturity("1.2.3-experimental"), "experimental")
        self.assertEqual(release_maturity("1.2.3-alpha"), "alpha")
        self.assertEqual(release_maturity("1.2.3-beta"), "beta")

    def test_release_matrix_matches_registry_binary_contract(self):
        self.assertEqual(len(TARGETS), 9)
        self.assertIn("x86_64-unknown-linux-gnu", TARGETS)
        self.assertEqual(len(TARGETS), len(set(TARGETS)))

    def test_interface_schema_must_define_a_shape(self):
        self.assertTrue(schema_is_typed({"type": "object", "properties": {}}))
        self.assertTrue(schema_is_typed({"$ref": "#/definitions/Request"}))
        self.assertFalse(schema_is_typed({}))
        self.assertFalse(schema_is_typed({"title": "AnyValue"}))

    def test_missing_tag_has_no_metadata(self):
        self.assertEqual(tag_metadata(Path("."), "missing"), {})

    def test_payload_preserves_registry_channel_and_experimental_flag(self):
        root = Path(__file__).parents[2]
        interface = {
            "functions": [
                {
                    "name": "e2e::run",
                    "request_schema": {"type": "object", "properties": {}},
                    "response_schema": {"type": "object", "properties": {}},
                }
            ],
            "triggers": [],
        }
        with TemporaryDirectory() as temporary_directory:
            checksums_dir = Path(temporary_directory)
            for target in TARGETS:
                (checksums_dir / f"harness-e2e-{target}.sha256").write_text(
                    f"{'0' * 64}  harness-e2e-{target}\n",
                    encoding="utf-8",
                )

            payload = build_payload(
                root=root,
                version="0.1.0-experimental",
                tag="harness-e2e/v0.1.0-experimental",
                registry_tag="latest",
                repo_url="https://github.com/iii-hq/harness-e2e",
                interface=interface,
                checksums_dir=checksums_dir,
                experimental=True,
            )

        self.assertEqual(payload["tag"], "latest")
        self.assertTrue(payload["experimental"])
        self.assertIn(
            "/releases/download/harness-e2e/v0.1.0-experimental/",
            payload["binaries"]["x86_64-unknown-linux-gnu"]["url"],
        )


if __name__ == "__main__":
    unittest.main()
