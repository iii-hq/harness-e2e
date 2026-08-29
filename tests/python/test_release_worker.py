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
        descriptor = {
            "contract": "release-descriptor",
            "worker": "harness-e2e",
            "version": version,
            "source_sha": "a" * 40,
            "release_spec_sha256": "b" * 64,
            "public_manifest_sha256": "c" * 64,
            "registry_projection_sha256": "d" * 64,
            "compiler_digest": "e" * 64,
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
            "runtime": {"exec": ["harness-e2e"], "environment": {}, "resources": {}},
            "validation": {"interface": "required"},
            "publish": True,
            "build_units": [
                {"id": f"harness-e2e-{target}", "kind": "rust-binary", "target": target}
                for target in TARGETS
            ],
            "registry_projection": {
                "worker_name": "harness-e2e",
                "version": version,
                "type": "binary",
                "description": "Harness E2E",
                "license": "MIT",
                "tags": ["e2e"],
                "dependencies": [{"name": "state", "version": "^0.22.2"}],
                "config": {},
                "experimental": "-experimental" in version,
                "readme": "# Harness E2E\n",
            },
        }
        descriptor["descriptor_sha256"] = descriptor_sha256(descriptor)
        return descriptor

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
                descriptor_path=descriptor_path,
                tag="harness-e2e/v0.2.0-experimental",
                repo_url="https://github.com/iii-hq/harness-e2e",
                interface=interface,
                checksums_dir=checksums_dir,
            )

        self.assertEqual(payload["tag"], "next")
        self.assertEqual(
            set(payload),
            {
                "worker_name", "version", "type", "tag", "description", "license",
                "tags", "dependencies", "config", "experimental", "readme", "repo",
                "functions", "triggers", "binaries",
            },
        )
        self.assertEqual(payload["version"], "0.2.0-experimental")
        self.assertNotIn("package_descriptor", payload)
        self.assertNotIn("descriptor_sha256", payload)
        self.assertEqual(len(payload["binaries"]), len(TARGETS))
        self.assertEqual(payload["functions"], interface["functions"])
        self.assertEqual(payload["triggers"], interface["triggers"])


if __name__ == "__main__":
    unittest.main()
