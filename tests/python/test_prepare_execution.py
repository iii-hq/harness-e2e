"""What `iii: latest` resolves to when the newest candidate is not out yet."""
import contextlib
import hashlib
import importlib.util
import io
import json
import sys
import tempfile
import unittest
import urllib.error
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("prepare_execution", ROOT / "scripts" / "prepare_execution.py")
prepare_execution = importlib.util.module_from_spec(SPEC)
sys.modules["prepare_execution"] = prepare_execution
SPEC.loader.exec_module(prepare_execution)

DIGEST = "sha256:" + "c" * 64
ARCHIVE = {"name": prepare_execution.CLI_ASSET, "digest": DIGEST}


class LatestCliTests(unittest.TestCase):
    def resolve(self, releases, selector="latest"):
        """`releases` maps a tag to its assets, or None for a tag whose
        release GitHub does not have (404)."""
        asked = []

        def get(url, token=None):
            if url.endswith("/git/matching-refs/tags/iii/v"):
                return [{"ref": f"refs/tags/iii/v{tag}"} for tag in ["0.24.1", *releases]]
            tag = url.rsplit("/iii/v", 1)[1]
            asked.append(tag)
            if releases.get(tag) is None:
                raise prepare_execution.ResolutionError(f"{url} answered HTTP 404") from urllib.error.HTTPError(
                    url, 404, "Not Found", None, None)
            return {"assets": releases[tag]}

        said = io.StringIO()
        self.skipped = []
        self.get = get
        with patch.object(prepare_execution, "get_json", side_effect=get), contextlib.redirect_stdout(said):
            cli = prepare_execution.resolve_cli(selector, None, self.skipped)
        return cli, asked, said.getvalue()

    def test_the_newest_candidate_runs_when_it_has_the_archive(self):
        cli, asked, said = self.resolve({"0.24.4-rc.1": [ARCHIVE], "0.24.4-rc.2": [ARCHIVE]})
        self.assertEqual((cli["version"], cli["sha256"]), ("0.24.4-rc.2", DIGEST))
        self.assertEqual(asked, ["0.24.4-rc.2"])
        self.assertEqual(said, "")

    def test_a_candidate_still_building_or_unreleased_gives_way_to_the_one_before_it_said_out_loud(self):
        windows = {"name": "iii-x86_64-pc-windows-msvc.zip", "digest": DIGEST}
        for newest in ([windows], [], None):
            with self.subTest(newest=newest):
                cli, asked, said = self.resolve({"0.24.4-rc.1": [ARCHIVE], "0.24.4-rc.2": newest})
                self.assertEqual(cli["version"], "0.24.4-rc.1")
                self.assertEqual(asked, ["0.24.4-rc.2", "0.24.4-rc.1"])
                self.assertEqual(said, f"::warning::iii/v0.24.4-rc.2 publishes no {prepare_execution.CLI_ASSET} yet; "
                                       "iii: latest runs iii/v0.24.4-rc.1, the newest release candidate that does\n")
                reason = "has no release yet (HTTP 404)" if newest is None else f"publishes no {prepare_execution.CLI_ASSET}"
                self.assertEqual(self.skipped, [{"version": "0.24.4-rc.2", "reason": reason}])

    def test_the_fallback_stays_within_the_newest_core_or_the_candidate_right_before_it(self):
        # Any earlier candidate of the newest X.Y.Z, never another X.Y.Z past the one right before.
        cli, asked, _ = self.resolve({"0.24.4-rc.9": [ARCHIVE], "0.25.0-rc.1": [ARCHIVE],
                                      "0.25.0-rc.2": [], "0.25.0-rc.3": None})
        self.assertEqual(cli["version"], "0.25.0-rc.1")
        self.assertEqual(asked, ["0.25.0-rc.3", "0.25.0-rc.2", "0.25.0-rc.1"])
        self.assertEqual([entry["version"] for entry in self.skipped], ["0.25.0-rc.3", "0.25.0-rc.2"])
        # A new X.Y.Z with one candidate gives way to the candidate before it…
        cli, _, _ = self.resolve({"0.24.4-rc.2": [ARCHIVE], "0.25.0-rc.1": []})
        self.assertEqual(cli["version"], "0.24.4-rc.2")
        # …and to nothing older.
        with self.assertRaisesRegex(prepare_execution.ResolutionError,
                                    r"iii/v0.25.0-rc.1 publishes no .*\(iii/v0.24.4-rc.2\)"):
            self.resolve({"0.24.3-rc.1": [ARCHIVE], "0.24.4-rc.2": [], "0.25.0-rc.1": []})

    def test_the_execution_records_what_was_asked_what_runs_and_what_was_passed_over(self):
        releases = {"0.24.4-rc.1": [ARCHIVE], "0.24.4-rc.2": []}
        self.resolve(releases)
        with tempfile.TemporaryDirectory() as directory:
            contract_dir = Path(directory)
            (contract_dir / "execution.json").write_text("{}")
            (contract_dir / "stack.yaml").write_text("containers: {}\n")
            with patch.object(prepare_execution, "get_json", side_effect=self.get), \
                    patch.object(prepare_execution, "resolve_fixtures", return_value=({}, {})), \
                    contextlib.redirect_stdout(io.StringIO()):
                prepare_execution.command_runtime(SimpleNamespace(contract_dir=contract_dir))
            execution = json.loads((contract_dir / "execution.json").read_text())
        self.assertEqual(execution["iii"], "0.24.4-rc.1")
        self.assertEqual(execution["iii_resolution"], {
            "requested": "latest", "resolved": "0.24.4-rc.1",
            "skipped": [{"version": "0.24.4-rc.2", "reason": f"publishes no {prepare_execution.CLI_ASSET}"}],
        })

    def test_the_cli_brings_the_vm_companions_its_release_publishes(self):
        worker, init = prepare_execution.CLI_COMPANIONS
        other = "sha256:" + "d" * 64
        cli, _, _ = self.resolve({"0.24.4-rc.1": [ARCHIVE], "0.24.4-rc.2": [
            ARCHIVE, {"name": worker, "digest": other}, {"name": "iii-worker-x86_64-unknown-linux-musl.tar.gz"},
        ]})
        self.assertEqual(cli["companions"], [{"asset": worker, "sha256": other}])
        with self.assertRaisesRegex(prepare_execution.ResolutionError, f"{init} has no SHA-256 digest"):
            self.resolve({"0.24.4-rc.1": [ARCHIVE], "0.24.4-rc.2": [ARCHIVE, {"name": init}]})

    def test_integrity_and_a_named_version_still_fail(self):
        with self.assertRaisesRegex(prepare_execution.ResolutionError, "nor does any candidate"):
            self.resolve({"0.24.4-rc.1": [], "0.24.4-rc.2": []})
        # A digest is what the groups check the download against.
        with self.assertRaisesRegex(prepare_execution.ResolutionError, "digest"):
            self.resolve({"0.24.4-rc.1": [ARCHIVE], "0.24.4-rc.2": [{"name": prepare_execution.CLI_ASSET}]})
        # A version the stack names is that version or nothing.
        with self.assertRaisesRegex(prepare_execution.ResolutionError, "iii/v0.24.4-rc.2 publishes no"):
            self.resolve({"0.24.4-rc.1": [ARCHIVE], "0.24.4-rc.2": []}, selector="0.24.4-rc.2")


class KitStackTests(unittest.TestCase):
    def test_kits_are_executor_inputs_not_compose_keys(self):
        stack = {"iii": "latest", "kits": ["iii/deepwork@0.4.3"], "containers": {}}
        self.assertEqual(prepare_execution.stack_kits(stack), ["iii/deepwork@0.4.3"])
        self.assertEqual(prepare_execution.compose_of(stack), {"containers": {}})
        for invalid in ("iii/deepwork", ["deepwork"], ["iii/deepwork", "iii/deepwork"]):
            with self.subTest(invalid=invalid), self.assertRaises(prepare_execution.ResolutionError):
                prepare_execution.stack_kits({"kits": invalid})

    def test_lock_freezes_the_native_kit_lock_and_assets_into_every_contract(self):
        import yaml

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            contracts = root / "contracts"
            assembled = root / "assembled"
            contracts.mkdir()
            (assembled / "kit-assets/agents").mkdir(parents=True)
            profile = b"---\nid: deepwork\n---\n"
            (assembled / "kit-assets/agents/deepwork.md").write_bytes(profile)
            kits_lock = b"version: 1\nkits:\n  iii/deepwork:\n    requested: 0.4.3\n    version: 0.4.3\n"
            (assembled / "kit-assets/kits.lock").write_bytes(kits_lock)
            manifest = {
                "lock_sha256": "sha256:" + hashlib.sha256(kits_lock).hexdigest(),
                "files": [{"path": "agents/deepwork.md", "sha256": "sha256:" + hashlib.sha256(profile).hexdigest()}],
            }
            (assembled / "kit-assets/manifest.json").write_text(json.dumps(manifest))
            compose = {"containers": {"kanban": {"worker": "package://kanban", "version": "^1.4"}}}
            lock = {"containers": {"kanban": {"worker": "package://kanban", "resolved": {"version": "1.4.2"}}}}
            (assembled / "worker-compose.yaml").write_text(yaml.safe_dump(compose))
            (assembled / "worker-compose.lock").write_text(yaml.safe_dump(lock))
            contract = {"runtime": {"kits": {"requested": ["iii/deepwork@0.4.3"]}}}
            (contracts / "deepwork.json").write_text(json.dumps(contract))
            (contracts / "resolution.json").write_text("{}")
            (root / "execution.json").write_text(json.dumps({"iii": "0.25.0-rc.1"}))

            prepare_execution.command_lock(SimpleNamespace(contract_dir=root, assembled=assembled))

            frozen = json.loads((contracts / "deepwork.json").read_text())["runtime"]["kits"]
            self.assertEqual(frozen["requested"], ["iii/deepwork@0.4.3"])
            self.assertEqual(frozen["lock"]["kits"]["iii/deepwork"]["version"], "0.4.3")
            self.assertEqual(frozen["assets"], manifest["files"])
            self.assertEqual((root / "kit-assets/agents/deepwork.md").read_bytes(), profile)
            self.assertEqual(json.loads((contracts / "resolution.json").read_text())["kits"], frozen)
            self.assertEqual(yaml.safe_load((root / "stack.yaml").read_text())["kits"], ["iii/deepwork@0.4.3"])


if __name__ == "__main__":
    unittest.main()
