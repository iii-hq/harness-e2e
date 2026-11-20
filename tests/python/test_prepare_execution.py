"""What `iii: latest` resolves to when the newest candidate is not out yet."""
import contextlib
import hashlib
import importlib.util
import io
import json
import os
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
                    patch.object(prepare_execution, "resolve_commits", wraps=prepare_execution.resolve_commits) as pins, \
                    contextlib.redirect_stdout(io.StringIO()):
                prepare_execution.command_runtime(SimpleNamespace(contract_dir=contract_dir))
            execution = json.loads((contract_dir / "execution.json").read_text())
            self.assertEqual(pins.call_args.args[2], "0.24.4-rc.1")
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
    def kit_dir(self, root, skills=False):
        kit = Path(root) / "deepwork"
        (kit / "agents").mkdir(parents=True)
        (kit / "iii.kit.yaml").write_text("name: iii/deepwork\nversion: 0.9.0\n")
        (kit / "agents/deepwork.md").write_text("# lead\n")
        if skills:
            (kit / "skills").mkdir()
        return kit

    def test_path_kits_expand_the_environment_and_must_be_kits(self):
        import tempfile
        from unittest import mock
        with tempfile.TemporaryDirectory() as root:
            kit = self.kit_dir(root)
            with mock.patch.dict(os.environ, {"KITS_ROOT": root}):
                self.assertEqual(prepare_execution.stack_kits({"kits": ["path:${KITS_ROOT}/deepwork", "iii/x@1"]}),
                                 [f"path:{kit}", "iii/x@1"])
            os.environ.pop("KITS_ROOT_UNSET", None)
            for bad in ("path:${KITS_ROOT_UNSET}/deepwork", f"path:{root}/missing", "path:relative/dir"):
                with self.subTest(bad=bad), self.assertRaises(prepare_execution.ResolutionError):
                    prepare_execution.stack_kits({"kits": [bad]})
            (kit / "iii.kit.yaml").unlink()
            with self.assertRaises(prepare_execution.ResolutionError):
                prepare_execution.stack_kits({"kits": [f"path:{kit}"]})
        with tempfile.TemporaryDirectory() as root:
            with self.assertRaises(prepare_execution.ResolutionError):
                prepare_execution.stack_kits({"kits": [f"path:{self.kit_dir(root, skills=True)}"]})

    def test_commit_kits_name_a_folder_at_a_full_commit(self):
        sha = "a" * 40
        entry = f"commit:iii-hq/workers/kits/deepwork@{sha}"
        self.assertEqual(prepare_execution.stack_kits({"kits": [entry, "iii/x@1"]}), [entry, "iii/x@1"])
        self.assertEqual(prepare_execution.kit_source(Path("/c"), entry), Path(f"/c/kit-sources/{sha}/iii-hq/workers/kits/deepwork"))
        for bad in (f"commit:iii-hq/workers/kits/deepwork@{sha[:12]}", f"commit:iii-hq/workers/../x@{sha}",
                    f"commit:../../workers/x@{sha}", f"commit:owner/../x/y@{sha}",
                    f"commit:iii-hq/workers@{sha}", f"commit:iii-hq/workers/kits//deepwork@{sha}"):
            with self.subTest(bad=bad), self.assertRaises(prepare_execution.ResolutionError):
                prepare_execution.stack_kits({"kits": [bad]})

    def test_commit_kits_fetch_only_the_kit_at_its_commit(self):
        import subprocess
        import tempfile
        from unittest import mock

        def git(*args, cwd):
            return subprocess.run(["git", "-C", str(cwd), *args], check=True, capture_output=True, text=True).stdout.strip()

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            work = root / "work"
            kit = self.kit_dir(work / "kits")
            (work / "other.txt").write_text("not a kit\n")
            (work / "kits/broken/agents").mkdir(parents=True)
            (work / "kits/broken/iii.kit.yaml").write_text("name: iii/broken\nversion: 1\n")
            (work / "kits/broken/agents/a.md").symlink_to("/etc/hostname")
            (work / "kits/linked").symlink_to("deepwork")
            (work / "kits/outside").mkdir()
            (work / "kits/outside/iii.kit.yaml").write_text("name: iii/outside\nversion: 1\n")
            (work / "kits/outside/agents").symlink_to("../deepwork/agents")
            (work / "kits/dangling/agents").mkdir(parents=True)
            (work / "kits/dangling/iii.kit.yaml").write_text("name: iii/dangling\nversion: 1\n")
            (work / "kits/dangling/agents/a.md").write_text("# a\n")
            (work / "kits/dangling/skills").symlink_to("../shared-skills")
            git("init", "-q", cwd=work)
            git("add", ".", cwd=work)
            git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "kit", cwd=work)
            first = git("rev-parse", "HEAD", cwd=work)
            (kit / "agents/deepwork.md").write_text("# lead, changed\n")
            git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qam", "change", cwd=work)
            second = git("rev-parse", "HEAD", cwd=work)
            bare = root / "remote/owner/repo.git"
            subprocess.run(["git", "clone", "-q", "--bare", str(work), str(bare)], check=True)
            git("config", "uploadpack.allowFilter", "true", cwd=bare)
            git("config", "uploadpack.allowAnySHA1InWant", "true", cwd=bare)
            contract = root / "contract"
            entries = [f"commit:owner/repo/kits/deepwork@{first}", f"commit:owner/repo/kits/deepwork@{second}"]
            with mock.patch.object(prepare_execution, "GIT_URL", f"file://{root / 'remote'}"):
                prepare_execution.fetch_commit_kits(["iii/x@1", *entries], contract)
                with self.assertRaisesRegex(prepare_execution.ResolutionError, "regular files"):
                    prepare_execution.fetch_commit_kits([f"commit:owner/repo/kits/broken@{first}"], contract)
                for folder in ("linked", "outside"):
                    with self.subTest(folder=folder), self.assertRaisesRegex(prepare_execution.ResolutionError,
                                                                             "must not be symlinks"):
                        prepare_execution.fetch_commit_kits([f"commit:owner/repo/kits/{folder}@{first}"], contract)
                with self.assertRaisesRegex(prepare_execution.ResolutionError, "has skills/"):
                    prepare_execution.fetch_commit_kits([f"commit:owner/repo/kits/dangling@{first}"], contract)
                with self.assertRaises(subprocess.CalledProcessError):
                    prepare_execution.fetch_commit_kits([f"commit:owner/repo/kits/missing@{first}"], contract)
                # A fetch cut short before its manifest is fetched again, whole.
                partial = prepare_execution.kit_source(contract, entries[0])
                (partial / "iii.kit.yaml").unlink()
                (partial / "agents/deepwork.md").write_text("trunc")
                prepare_execution.fetch_commit_kits(entries[:1], contract)
            old, new = (prepare_execution.kit_source(contract, entry) for entry in entries)
            self.assertEqual(sorted(str(path.relative_to(old)) for path in old.rglob("*")),
                             ["agents", "agents/deepwork.md", "iii.kit.yaml"])
            self.assertEqual((old / "agents/deepwork.md").read_text(), "# lead\n")
            self.assertEqual((new / "agents/deepwork.md").read_text(), "# lead, changed\n")
            self.assertEqual(sorted(path.name for path in (contract / "kit-sources").iterdir()), sorted([first, second]))

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
