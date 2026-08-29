import json
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

from scripts.release_worker import (
    TARGETS,
    build_payload,
    descriptor_sha256,
    frontend_specs,
    load_release_descriptor,
    package_binary,
    schema_is_typed,
)


class ReleaseWorkerTest(unittest.TestCase):
    def test_release_matrix_matches_registry_binary_contract(self):
        self.assertEqual(len(TARGETS), 9)
        self.assertIn("x86_64-unknown-linux-gnu", TARGETS)
        self.assertEqual(len(TARGETS), len(set(TARGETS)))

    def descriptor(self, version: str = "0.5.0-experimental"):
        package = {
            "name": "harness-e2e",
            "version": version,
            "source": {"path": ".", "package_manifest": "Cargo.toml"},
            "artifact": {
                "kind": "rust-binary",
                "binary": "harness-e2e",
                "targets": list(TARGETS),
                "toolchain": {"name": "rust", "version": "1.97.1"},
                "frontends": [
                    {
                        "workspace_root": "dashboard",
                        "source_path": "dashboard",
                        "runtime": {"name": "node", "version": "22"},
                        "package_manager": {"name": "pnpm", "version": "11.13.1"},
                        "lockfile": "pnpm-lock.yaml",
                        "install_command": ["pnpm", "install", "--frozen-lockfile"],
                        "build_command": ["pnpm", "build"],
                        "outputs": ["dist", "dist-console"],
                    }
                ],
            },
            "runtime": {"exec": ["harness-e2e"]},
            "registry": {
                "description": "Harness E2E",
                "license": "MIT",
                "tags": ["e2e"],
                "dependencies": {"state": "^0.22.2"},
                "publish": True,
            },
            "validation": {"interface": "required"},
        }
        digest = descriptor_sha256(package)
        return {
            "contract": "release-descriptor",
            "worker": "harness-e2e",
            "version": version,
            "source_sha": "a" * 40,
            "descriptor_sha256": digest,
            "package": package,
            "build_units": [],
        }

    def test_compiled_descriptor_is_the_only_release_metadata_input(self):
        with TemporaryDirectory() as temporary_directory:
            path = Path(temporary_directory) / "release-descriptor.json"
            descriptor = self.descriptor()
            path.write_text(json.dumps(descriptor), encoding="utf-8")
            loaded = load_release_descriptor(
                path,
                expected_source_sha="a" * 40,
                expected_digest=descriptor["descriptor_sha256"],
                expected_version="0.5.0-experimental",
            )
        self.assertEqual(loaded, descriptor)

    def test_frontend_build_is_explicit_and_descriptor_owned(self):
        specs = frontend_specs(self.descriptor())
        self.assertEqual(len(specs), 1)
        self.assertEqual(specs[0]["workspace_root"], Path("dashboard"))
        self.assertEqual(specs[0]["source_path"], Path("dashboard"))
        self.assertEqual(specs[0]["install_command"], ("pnpm", "install", "--frozen-lockfile"))
        self.assertEqual(specs[0]["outputs"], [Path("dist"), Path("dist-console")])

    def test_binary_archives_are_reproducible(self):
        with TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            binary = root / "harness-e2e"
            binary.write_bytes(b"reproducible harness executable")
            first = package_binary(binary, "x86_64-unknown-linux-gnu", root / "first")
            second = package_binary(binary, "x86_64-unknown-linux-gnu", root / "second")
            self.assertEqual(first["sha256"], second["sha256"])

            first = package_binary(binary, "x86_64-pc-windows-msvc", root / "windows-first")
            second = package_binary(binary, "x86_64-pc-windows-msvc", root / "windows-second")
            self.assertEqual(first["sha256"], second["sha256"])

    def test_interface_schema_must_define_a_shape(self):
        self.assertTrue(schema_is_typed({"type": "object", "properties": {}}))
        self.assertTrue(schema_is_typed({"$ref": "#/definitions/Request"}))
        self.assertFalse(schema_is_typed({}))
        self.assertFalse(schema_is_typed({"title": "AnyValue"}))

    def test_experimental_version_marks_registry_payload(self):
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
            descriptor_path = checksums_dir / "release-descriptor.json"
            descriptor = self.descriptor("0.2.0-experimental")
            descriptor_path.write_text(json.dumps(descriptor), encoding="utf-8")
            for target in TARGETS:
                (checksums_dir / f"harness-e2e-{target}.sha256").write_text(
                    f"{'0' * 64}  harness-e2e-{target}\\n",
                    encoding="utf-8",
                )

            payload = build_payload(
                root=root,
                descriptor_path=descriptor_path,
                tag="harness-e2e/v0.2.0-experimental",
                repo_url="https://github.com/iii-hq/harness-e2e",
                interface=interface,
                checksums_dir=checksums_dir,
            )

        self.assertEqual(payload["channel"], "next")
        self.assertEqual(
            set(payload),
            {
                "package_descriptor",
                "descriptor_sha256",
                "channel",
                "readme",
                "repo",
                "interface",
                "artifacts",
            },
        )
        self.assertEqual(payload["package_descriptor"]["version"], "0.2.0-experimental")
        self.assertRegex(payload["descriptor_sha256"], r"^[0-9a-f]{64}$")
        self.assertEqual(payload["artifacts"]["kind"], "rust-binary")
        self.assertEqual(len(payload["artifacts"]["binaries"]), len(TARGETS))
        self.assertEqual(payload["interface"], interface)


if __name__ == "__main__":
    unittest.main()
