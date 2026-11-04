import hashlib
import importlib.util
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


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


TYPED = {"type": "object", "properties": {"ok": {"type": "boolean"}}}
UNTYPED = {"$schema": "http://json-schema.org/draft-07/schema#", "title": "AnyValue"}


def engine_0_26(details):
    """`iii trigger` against a 0.26 engine: functions only; trigger types are gone.

    `details` maps each harness-e2e function to its `engine::functions::info`
    row, which states `args_format`, `private` and `observable` only when they
    are not the defaults."""
    calls = []

    def answer(function_id, payload):
        calls.append(function_id)
        if function_id == "engine::workers::list":
            return {"workers": [{"name": "database", "id": "w-db"}, {"name": "harness-e2e", "id": "w-e2e"}]}
        if function_id == "engine::functions::list":
            rows = [{"function_id": fid, "worker_name": "harness-e2e"} for fid in details]
            return {"functions": [*rows, {"function_id": "database::query", "worker_name": "database"}]}
        if function_id == "engine::functions::info":
            return {"function": {"function_id": payload["function_id"], **details[payload["function_id"]]}}
        raise subprocess.CalledProcessError(1, ["iii", "trigger", function_id],
                                            stderr=f"Function {function_id} not found in namespace default.")

    return answer, calls


class CollectInterfaceTests(unittest.TestCase):
    BASELINE = {"workers": [{"name": "database", "id": "w-db"}]}

    def collect(self, details):
        answer, calls = engine_0_26(details)
        with patch.object(release_worker, "iii_trigger", answer):
            interface = release_worker.collect_interface(
                worker="harness-e2e", prefix="e2e::", baseline_workers=self.BASELINE, wait_seconds=5)
        return interface, calls

    def test_collects_every_function_with_how_it_is_observed_and_reads_no_trigger_types(self):
        interface, calls = self.collect({
            "e2e::scenarios-list": {"request_schema": TYPED, "response_schema": TYPED},
            "e2e::dashboard::changed": {"request_schema": TYPED, "response_schema": TYPED, "private": True},
            "e2e::sealed": {"request_schema": TYPED, "response_schema": TYPED, "observable": False,
                            "args_format": {"type": "object", "properties": {"in_ms": {"type": "integer"}}}},
        })
        self.assertEqual(list(interface), ["functions"])
        rows = {row["name"]: row for row in interface["functions"]}
        self.assertEqual(sorted(rows), ["e2e::dashboard::changed", "e2e::scenarios-list", "e2e::sealed"])
        observed = {name: (row["args_format"], row["private"], row["observable"]) for name, row in rows.items()}
        self.assertEqual(observed, {
            "e2e::scenarios-list": (None, False, True),
            "e2e::dashboard::changed": (None, True, True),
            "e2e::sealed": ({"type": "object", "properties": {"in_ms": {"type": "integer"}}}, False, False),
        })
        self.assertFalse([call for call in calls if call.startswith("engine::triggers::")])

    def test_an_untyped_function_still_fails_the_collection_private_or_not(self):
        with self.assertRaisesRegex(RuntimeError, r"e2e::dashboard::changed\.request_schema"):
            self.collect({
                "e2e::scenarios-list": {"request_schema": TYPED, "response_schema": TYPED},
                "e2e::dashboard::changed": {"request_schema": UNTYPED, "response_schema": TYPED, "private": True},
            })

    def test_an_args_format_that_is_not_a_schema_fails_the_collection(self):
        with self.assertRaisesRegex(RuntimeError, r"e2e::sealed\.args_format"):
            self.collect({"e2e::sealed": {"request_schema": TYPED, "response_schema": TYPED, "args_format": "in_ms"}})


class BuildPayloadTests(unittest.TestCase):
    ROOT = Path(__file__).parents[2]

    def test_a_0_26_release_publishes_functions_with_how_they_are_observed_and_no_trigger_types(self):
        function = {"name": "e2e::dashboard::changed", "description": "", "request_schema": TYPED,
                    "response_schema": TYPED, "args_format": None, "private": True, "observable": True,
                    "metadata": {}}
        payload = release_worker.build_payload(root=self.ROOT, version="0.24.0", tag="harness-e2e/v0.24.0",
                                               repo_url="https://github.com/iii-hq/harness-e2e",
                                               interface={"functions": [function]})
        self.assertNotIn("triggers", payload)
        self.assertEqual(payload["functions"], [function])
        with self.assertRaisesRegex(ValueError, "untyped function schemas"):
            release_worker.build_payload(root=self.ROOT, version="0.24.0", tag="harness-e2e/v0.24.0",
                                         repo_url="https://github.com/iii-hq/harness-e2e",
                                         interface={"functions": [{**function, "request_schema": UNTYPED}]})

    def test_the_payload_carries_the_observe_v2_engine_floor_and_never_publishes_without_one(self):
        function = {"name": "e2e::run", "description": "", "request_schema": TYPED, "response_schema": TYPED}
        payload = release_worker.build_payload(root=self.ROOT, version="0.24.1", tag="harness-e2e/v0.24.1",
                                               repo_url="https://github.com/iii-hq/harness-e2e",
                                               interface={"functions": [function]})
        # iii 0.25 must resolve an older runner, not this Observe v2 one.
        self.assertEqual(payload["iii_engine_version"], ">=0.26.0")

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shutil.copy(self.ROOT / "config.yaml", root)
            shutil.copy(self.ROOT / "README.md", root)
            manifest = (self.ROOT / "iii.worker.yaml").read_text()
            (root / "iii.worker.yaml").write_text(
                "\n".join(line for line in manifest.splitlines() if not line.startswith("iii_engine_version:")))
            with self.assertRaisesRegex(ValueError, "iii_engine_version"):
                release_worker.build_payload(root=root, version="0.24.1", tag="harness-e2e/v0.24.1",
                                             repo_url="https://github.com/iii-hq/harness-e2e",
                                             interface={"functions": [function]})


class ReleaseWorkflowTests(unittest.TestCase):
    ROOT = Path(__file__).parents[2]

    def test_the_interface_job_installs_what_the_manifest_declares(self):
        import yaml

        workflow = yaml.safe_load((self.ROOT / ".github/workflows/release.yml").read_text())
        steps = {step.get("name"): step["run"] for step in workflow["jobs"]["registry"]["steps"] if "run" in step}
        project = steps["Start isolated iii engine"].split("<<'PYCONFIG'\n", 1)[1].split("\nPYCONFIG", 1)[0]
        declared = yaml.safe_load((self.ROOT / "iii.worker.yaml").read_text())["dependencies"]
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / "interface-project").mkdir()
            shutil.copy(self.ROOT / "iii.worker.yaml", directory)
            subprocess.run([sys.executable, "-c", project], cwd=directory, check=True)
            compose = yaml.safe_load((Path(directory) / "interface-project/worker-compose.yaml").read_text())
            listed = (Path(directory) / "interface-project/dependencies.txt").read_text().split()
        self.assertEqual(compose["containers"]["database"]["version"], declared["database"])
        self.assertEqual(listed, [f"{name}@{version}" for name, version in declared.items()])
        self.assertNotRegex("\n".join(steps.values()), r"worker=[\w-]+@\d")


if __name__ == "__main__":
    unittest.main()
