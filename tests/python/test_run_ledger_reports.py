"""The run ledger boundary: what this repository must send Release Control.

These tests state the shape of the three reports and of the contract this
repository now assembles for itself. They are the executable half of the
agreement — Release Control reads exactly the fields asserted here.
"""

import importlib.util
import json
import subprocess
import os
import pathlib
import sys
import tempfile
import textwrap
import unittest
from types import SimpleNamespace
from unittest.mock import patch


ROOT = pathlib.Path(__file__).resolve().parents[2]


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


report_execution = load("report_execution")
prepare_execution = load("prepare_execution")


PROFILE_SNAPSHOT = {
    "schema": "harness-e2e-profile-snapshot",
    "plan_id": "harness",
    "version": 1,
    "definition_sha256": "sha256:" + "d" * 64,
    "profile_sha256": "sha256:" + "p" * 64,
    "profile": {"id": "regression", "label": "Regression", "repetitions": 1, "technical_retries": 1},
    "scenario_ids": ["minimal_path"],
    "cases": [],
    "campaigns": [
        {
            "kind": "harness-e2e-campaign",
            "campaign_id": "regression-r01",
            "lane": "local-regression",
            "failure_policy": "advisory",
            "groups": [
                {
                    "id": "case-minimal-path",
                    "execution_kind": "harness_turn",
                    "runs": 1,
                    "technical_retries": 1,
                    "scenarios": ["minimal_path"],
                }
            ],
        }
    ],
    "budget": {"planned_runs": 1},
}

PLAN = {
    "key": "harness-regression",
    "sha256": "b" * 64,
    "profile": {"plan_id": "harness", "id": "regression"},
    "subject": {"provider": "deepseek", "model": "deepseek-v4-flash"},
    "runner": {"revision": "a" * 40},
    "stack": {"policy": "latest"},
}


class Args:
    """The argparse namespace the payload builders read, without argparse."""

    def __init__(self, **fields):
        defaults = {
            "execution_id": "b0607faa-096a-4efe-a4a2-a2a9bc06de83",
            "artifacts": None,
            "campaign_id": None,
            "group_id": None,
            "outcome": None,
            "profile_snapshot": None,
            "plan": None,
            "resolution": None,
            "summary": None,
            "stack_lock_sha256": None,
            "runner_sha": None,
            "cli_version": None,
            "artifact_name": None,
        }
        self.__dict__.update(defaults | fields)


class ReportPayloadTests(unittest.TestCase):
    def setUp(self):
        self.tmp = pathlib.Path(
            __import__("tempfile").mkdtemp(prefix="harness-e2e-ledger-")
        )
        self.snapshot = self.tmp / "profile.json"
        self.snapshot.write_text(json.dumps(PROFILE_SNAPSHOT))
        self.plan = self.tmp / "plan.json"
        self.plan.write_text(json.dumps(PLAN))

    def tearDown(self):
        __import__("shutil").rmtree(self.tmp, ignore_errors=True)

    def test_identity_uses_the_effective_model_and_agent_reported_by_the_runtime(self):
        subject = {"provider": "zai", "model": "resolved-model", "agent": {"id": "reviewer", "configuration_sha256": "c" * 64}}
        (self.tmp / "results.json").write_text(json.dumps({"subject": subject}))
        identity = report_execution.identity_of(Args(plan=self.plan), self.tmp)
        self.assertEqual(identity["subject"], subject)

    def test_template_identity_comes_from_resolution_not_the_requested_branch(self):
        template = {"id": "harness", "repository": "iii-hq/templates", "ref": "main", "revision": "f" * 40}
        resolution = self.tmp / "resolution.json"
        resolution.write_text(json.dumps({"template": template}))
        self.assertEqual(report_execution.identity_of(Args(plan=self.plan, resolution=resolution), None)["template"], template)
        self.assertNotIn("template", report_execution.identity_of(Args(plan=self.plan), None))

    def test_identity_names_the_executor_image_the_resolution_recorded(self):
        image = "ghcr.io/iii-hq/harness-e2e@sha256:" + "e" * 64
        resolution = self.tmp / "resolution.json"
        resolution.write_text(json.dumps({"executor_image": image}))
        self.assertEqual(report_execution.identity_of(Args(plan=self.plan, resolution=resolution), None)["executor_image"], image)
        # An execution prepared outside the image states none.
        self.assertNotIn("executor_image", report_execution.identity_of(Args(plan=self.plan), None))

    def test_materialized_states_the_shards_and_planned_runs(self):
        """Release Control reads only these fields; it must find all of them."""
        payload = report_execution.materialized_payload(
            Args(profile_snapshot=self.snapshot, plan=self.plan, runner_sha="a" * 40, cli_version="0.23.1-rc.2")
        )
        self.assertEqual(payload["kind"], "materialized")
        self.assertEqual(payload["profile_snapshot"], PROFILE_SNAPSHOT)
        self.assertEqual(payload["profile"]["id"], "regression")
        self.assertEqual(payload["profile"]["profile_sha256"], PROFILE_SNAPSHOT["profile_sha256"])
        self.assertEqual(payload["profile"]["definition_sha256"], PROFILE_SNAPSHOT["definition_sha256"])
        self.assertEqual(payload["profile"]["repetitions"], 1)
        campaign = payload["campaigns"][0]
        self.assertEqual(campaign["campaign_id"], "regression-r01")
        group = campaign["groups"][0]
        self.assertEqual((group["id"], group["scenarios"], group["runs"]), ("case-minimal-path", ["minimal_path"], 1))
        self.assertEqual(payload["identity"]["plan_sha256"], PLAN["sha256"])
        self.assertEqual(payload["identity"]["subject"], PLAN["subject"])

    def test_bundle_identity_preserves_the_workflow_attempt_without_inventing_uploaded_metadata(self):
        args = Args(artifact_name="e2e-observation-fixture-gh-2")
        with patch.dict("os.environ", {
            "GITHUB_REPOSITORY": "iii-hq/harness-e2e", "GITHUB_RUN_ID": "77", "GITHUB_RUN_ATTEMPT": "2"
        }, clear=True):
            bundle = report_execution.shard_payload(args)["bundle"]
            self.assertEqual(bundle["artifact_name"], args.artifact_name)
            self.assertEqual((bundle["run_id"], bundle["run_attempt"]), (77, 2))
            self.assertIsNone(bundle["artifact_id"])
            self.assertIsNone(bundle["sha256"])
            self.assertIsNone(bundle["size_bytes"])
        with patch.dict("os.environ", {}, clear=True):
            self.assertIsNone(report_execution.summary_payload(args)["bundle"])

    def test_runs_come_from_results_with_the_slot_release_control_recomputes(self):
        results = {
            "scenarios": [
                {
                    "scenario_id": "tool_contract_recovery",
                    "case_id": "tool_contract_recovery@1",
                    "behavior_sha256": "sha256:" + "b" * 64,
                    "case": {"seed": 4404, "inputs_sha256": "sha256:" + "1" * 64},
                    "runs": [{"run_id": "run-a", "status": "passed"}, {"run_id": "run-b", "status": "failed"}],
                }
            ]
        }
        runs = report_execution.runs_from_results(results)
        self.assertEqual([run["repetition"] for run in runs], [0, 1])
        # Seed travels as a decimal string: it is an input to the slot digest.
        self.assertEqual(runs[0]["seed"], "4404")
        # Nothing grades the case: no difficulty travels with the run.
        self.assertEqual(
            sorted(runs[0]),
            [
                "behavior_sha256",
                "case_id",
                "definition_sha256",
                "repetition",
                "run",
                "scenario_id",
                "seed",
            ],
        )
        self.assertEqual(runs[0]["definition_sha256"], "sha256:" + "1" * 64)
        self.assertEqual(runs[0]["run"]["run_id"], "run-a")

    def test_a_group_that_died_still_reports_its_committed_runs(self):
        artifacts = self.tmp / "artifacts"
        events = artifacts / "journal" / "events"
        events.mkdir(parents=True)
        (events / "00000002-slot-inventory-committed.json").write_text(
            json.dumps(
                {
                    "sequence": 2,
                    "at": "2026-09-05T05:17:00Z",
                    "type": "slot_inventory_committed",
                    "slots": [
                        {
                            "slot_id": "slot-2420557511cf4c76c9a21421",
                            "ordinal": 1,
                            "scenario_id": "tool_contract_recovery",
                            "case_id": "tool_contract_recovery@1",
                            "seed": "4404",
                            "repetition": 0,
                        }
                    ],
                }
            )
        )
        checkpoint = artifacts / "journal" / "runs" / "slot-2420557511cf4c76c9a21421"
        checkpoint.mkdir(parents=True)
        (checkpoint / "run-a.json").write_text(
            json.dumps({"schema": "harness-e2e-run-checkpoint", "slot_id": "slot-2420557511cf4c76c9a21421",
                        "run_id": "run-a", "run": {"run_id": "run-a", "status": "failed"}})
        )

        runs, source = report_execution.collect_runs(artifacts)
        self.assertEqual(source, "journal")
        self.assertEqual(runs[0]["slot_id"], "slot-2420557511cf4c76c9a21421")
        self.assertEqual(runs[0]["scenario_id"], "tool_contract_recovery")
        self.assertEqual(runs[0]["seed"], "4404")
        self.assertEqual(runs[0]["run"]["status"], "failed")

    def test_a_group_that_produced_nothing_still_reports_a_shard(self):
        artifacts = self.tmp / "empty"
        artifacts.mkdir()
        (artifacts / "failure.json").write_text(json.dumps({"phase": "bootstrap", "outcome": "infra_failed"}))
        payload = report_execution.shard_payload(
            Args(
                artifacts=artifacts,
                campaign_id="regression-r01",
                group_id="case-minimal-path",
                outcome="failure",
                plan=self.plan,
                profile_snapshot=self.snapshot,
            )
        )
        self.assertEqual(payload["kind"], "shard")
        self.assertEqual(payload["shard"], "regression-r01/case-minimal-path")
        self.assertEqual(payload["runs"], [])
        self.assertEqual(payload["group"]["evidence"], "none")
        self.assertEqual(payload["group"]["failure"]["outcome"], "infra_failed")


class LedgerDeliveryTests(unittest.TestCase):
    """The artifact is the report; posting it only makes the ledger current."""

    def test_a_report_that_cannot_be_delivered_stays_in_the_artifacts(self):
        with tempfile.TemporaryDirectory() as directory:
            artifacts = pathlib.Path(directory)
            script = pathlib.Path(report_execution.__file__)
            result = subprocess.run(
                [sys.executable, str(script), "shard",
                 "--execution-id", "exec-41", "--campaign-id", "r01", "--group-id", "core",
                 "--outcome", "success", "--artifacts", str(artifacts),
                 "--oidc-audience", "rc"],
                capture_output=True, text=True,
                env={**os.environ,
                     "RELEASE_CONTROL_API_URL": "http://127.0.0.1:9/unreachable",
                     "ACTIONS_ID_TOKEN_REQUEST_URL": "http://127.0.0.1:9/token",
                     "ACTIONS_ID_TOKEN_REQUEST_TOKEN": "x",
                     "GITHUB_RUN_ATTEMPT": "1"},
            )
            # Release Control being unreachable says nothing about the run.
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("not delivered", result.stderr)
            report = json.loads((artifacts / "ledger-shard.json").read_text())
            self.assertEqual(report["kind"], "shard")
            self.assertEqual(report["shard"], "r01/core")

    def test_the_report_key_separates_a_retry_from_a_rerun(self):
        first = report_execution.report_key("shard", Args(campaign_id="r01", group_id="core"))
        with patch.dict(os.environ, {"GITHUB_RUN_ATTEMPT": "2"}):
            rerun = report_execution.report_key("shard", Args(campaign_id="r01", group_id="core"))
        self.assertEqual(first, report_execution.report_key("shard", Args(campaign_id="r01", group_id="core")))
        self.assertNotEqual(first, rerun)
        self.assertTrue(rerun.endswith(":2"))


class DispatchTests(unittest.TestCase):
    """One execution from a dispatch."""

    def test_a_new_dispatch_names_the_suite_stack_model_and_profile(self):
        dispatch = prepare_execution.read_dispatch(
            {"suite": "pr", "model": "zai/glm-5.1", "profile": "tech-lead", "execution_id": ""}
        )
        self.assertEqual(dispatch["execution"], {
            "execution_id": None, "suite": "pr", "stack": "default",
            "model": "zai/glm-5.1", "profile": "tech-lead",
        })
        self.assertEqual(dispatch["stack"]["iii"], "latest")
        self.assertEqual(dispatch["stack"]["containers"]["harness"]["worker"], "package://harness")
        # The plan shape older Console imports and the ledger reports read.
        self.assertEqual(dispatch["plan"], {
            "profile": {"id": "pr"}, "subject": {"provider": "zai", "model": "glm-5.1"},
            "agent_profile": "tech-lead",
        })

    def test_a_suite_and_a_stack_may_be_stated_whole(self):
        dispatch = prepare_execution.read_dispatch({
            "suite": '{"id": "smoke", "scenarios": ["minimal_path"]}',
            "stack": "iii: 0.24.2\ntemplate: harness\ncontainers:\n  harness:\n    worker: package://harness\n",
            "model": "deepseek/deepseek-v4-flash",
        })
        self.assertEqual(dispatch["execution"]["suite"], {"id": "smoke", "scenarios": ["minimal_path"]})
        self.assertEqual(dispatch["execution"]["stack"], "inline")
        self.assertEqual(dispatch["plan"]["profile"], {"id": "smoke"})
        self.assertEqual(prepare_execution.compose_of(dispatch["stack"]),
                         {"containers": {"harness": {"worker": "package://harness"}}})

    def test_a_dispatch_without_suite_or_model_says_what_is_missing(self):
        for inputs, missing in (({"model": "zai/glm-5.1"}, "suite"), ({"suite": "pr", "model": "glm-5.1"}, "model")):
            with self.subTest(missing=missing), self.assertRaisesRegex(prepare_execution.ResolutionError, missing):
                prepare_execution.read_dispatch(inputs)

    def test_template_packages_take_the_versions_the_execution_locked(self):
        spec = importlib.util.spec_from_file_location("exact_stack_campaign", ROOT / "scripts/exact_stack_campaign.py")
        campaign = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(campaign)
        contract = {"suite": {"groups": [{"id": "g", "scenarios": ["minimal_path"]}],
                              "subject": {"provider": "deepseek", "model": "m"}},
                    "runtime": {"lock": {"containers": {
                        "ade": {"worker": "package://api.workers.iii.dev/ade", "resolved": {"version": "1.9.41"}},
                        "harness": {"worker": "package://api.workers.iii.dev/harness", "resolved": {"version": "1.8.31"}},
                    }}}}
        template = {"containers": {"console": {"worker": "package://ade"}, "harness": {"worker": "package://harness"}}}
        project = campaign.project_scaffold(contract, "e2e-group", pathlib.Path("/data"), None, {}, template)
        self.assertEqual(project["containers"]["console"]["version"], "1.9.41")
        self.assertEqual(project["containers"]["harness"]["version"], "1.8.31")


class StackResolutionTests(unittest.TestCase):
    def test_latest_iii_is_the_newest_release_candidate_as_release_control_reads_it(self):
        newest = prepare_execution.newest_release_candidate
        # Today's iii-hq/iii: a stable 0.24.2 does not displace its candidate.
        self.assertEqual(newest(["0.24.0", "0.24.1", "0.24.2", "0.24.2-rc.1", "0.24.2-rc.2"]), "0.24.2-rc.2")
        self.assertEqual(newest(["0.24.2-rc.2", "0.24.2-rc.10", "0.24.1"]), "0.24.2-rc.10")
        self.assertEqual(newest(["0.24.2-rc.9", "0.25.0-rc.1", "0.26.0"]), "0.25.0-rc.1")
        # Only X.Y.Z-rc.N counts: not alpha, beta, next, rc.0 or a leading zero.
        self.assertIsNone(newest(["0.24.2", "0.24.2-alpha.1", "0.24.2-beta", "0.25.0-next.1",
                                  "0.24.2-rc.0", "01.2.3-rc.1", "0.24.2-rc.1.1", "main", ""]))

    def test_iii_resolves_to_a_release_and_the_digest_the_groups_check(self):
        urls = []

        def get(url, token=None):
            urls.append(url)
            if url.endswith("/git/matching-refs/tags/iii/v"):
                return [{"ref": f"refs/tags/iii/v{tag}"} for tag in ("0.24.1", "0.24.2", "0.24.2-rc.2")]
            return {"assets": [{"name": prepare_execution.CLI_ASSET, "digest": "sha256:" + "c" * 64}]}

        with patch.object(prepare_execution, "get_json", side_effect=get):
            cli = prepare_execution.resolve_cli("latest", None)
            self.assertEqual(cli["version"], "0.24.2-rc.2")
            self.assertEqual(cli["sha256"], "sha256:" + "c" * 64)
            self.assertTrue(urls[-1].endswith("/releases/tags/iii/v0.24.2-rc.2"))
            self.assertEqual(prepare_execution.resolve_cli("0.23.1", None)["version"], "0.23.1")
        with patch.object(prepare_execution, "get_json", return_value={"assets": [{"name": prepare_execution.CLI_ASSET}]}):
            with self.assertRaisesRegex(prepare_execution.ResolutionError, "digest"):
                prepare_execution.resolve_cli("0.24.2", None)

    def test_a_template_is_pinned_to_one_commit_of_its_revision(self):
        urls = []

        def get(url, token=None):
            urls.append(url)
            return {"sha": "a" * 40}

        with patch.object(prepare_execution, "get_json", side_effect=get):
            self.assertEqual(prepare_execution.resolve_template("harness", None), {
                "id": "harness", "repository": prepare_execution.TEMPLATES_REPOSITORY,
                "ref": "main", "revision": "a" * 40,
            })
            self.assertEqual(prepare_execution.resolve_template("harness@v2", None)["ref"], "v2")
        self.assertTrue(urls[0].endswith("/commits/main") and urls[1].endswith("/commits/v2"))
        self.assertIsNone(prepare_execution.resolve_template(None, None))

    def test_the_suite_is_materialized_by_the_runner_the_stack_resolves_and_pins(self):
        """`iii compose build` resolves the stack's own runner declaration; its
        exact release is pinned in the stack, so the assembly installs the
        binary that materialized the suite."""
        import yaml

        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            contract = root / "contract"
            contract.mkdir()
            (contract / "execution.json").write_text(json.dumps({"cli": {"version": "0.24.2-rc.2"}}))
            stack = {"iii": "0.24.2-rc.2", "containers": {
                "runner": {"worker": "package://api.workers.iii.dev/harness-e2e", "version": "latest", "env_file": ["./x"]},
                "harness": {"worker": "package://harness", "version": "latest"},
            }}
            (contract / "stack.yaml").write_text(yaml.safe_dump(stack, sort_keys=False))
            iii = root / "iii"
            iii.write_text(
                "#!/usr/bin/env python3\n"
                "import pathlib, sys\n"
                "compose = pathlib.Path(sys.argv[sys.argv.index('-f') + 1])\n"
                "assert 'harness:' not in compose.read_text(), 'only the runner is built'\n"
                "compose.with_name('worker-compose.lock').write_text("
                "'version: 1\\ncontainers:\\n  runner:\\n    worker: package://api.workers.iii.dev/harness-e2e\\n'"
                "'    requested: latest\\n    resolved:\\n      version: 0.12.3\\n')\n"
            )
            runner = root / "harness-e2e"
            runner.write_text("#!/bin/sh\necho '{\"runner\":{\"name\":\"harness-e2e\",\"version\":\"0.12.3\",\"revision\":\"" + "c" * 40 + "\"}}'\n")
            for executable in (iii, runner):
                executable.chmod(0o755)
            args = SimpleNamespace(contract_dir=contract, work_dir=root / "work")
            (root / "work").mkdir()
            with patch.object(prepare_execution, "install_cli", return_value=iii), \
                 patch.object(prepare_execution, "fetch_runner", return_value=runner) as fetched, \
                 patch("sys.stdout", new_callable=__import__("io").StringIO) as printed:
                prepare_execution.command_runner(args)
            self.assertEqual(printed.getvalue().strip(), str(runner))
            self.assertEqual(fetched.call_args.args[0]["containers"]["runner"]["resolved"]["version"], "0.12.3")
            pinned = yaml.safe_load((contract / "stack.yaml").read_text())
            self.assertEqual(pinned["containers"]["runner"]["version"], "0.12.3")
            self.assertEqual(pinned["containers"]["harness"]["version"], "latest")
            self.assertEqual(json.loads((contract / "runner.json").read_text()),
                             {"name": "harness-e2e", "version": "0.12.3", "revision": "c" * 40})
            # The reports name the runner by the revision it reports.
            self.assertEqual(json.loads((contract / "execution.json").read_text())["runner_revision"], "c" * 40)

    def test_a_download_survives_a_transient_failure_but_never_a_wrong_digest(self):
        import hashlib
        import io
        import urllib.error

        body = b"archive"

        class Response(io.BytesIO):
            def __enter__(self):
                return self

            def __exit__(self, *exc):
                return False

        answers = [urllib.error.HTTPError("u", 502, "bad gateway", {}, None), OSError("reset"), Response(body)]
        with tempfile.TemporaryDirectory() as directory, \
                patch("urllib.request.urlopen", side_effect=answers), \
                patch("time.sleep"):
            path = prepare_execution.download("https://example.invalid/a", hashlib.sha256(body).hexdigest(),
                                              pathlib.Path(directory) / "a")
            self.assertEqual(path.read_bytes(), body)
        with tempfile.TemporaryDirectory() as directory, \
                patch("urllib.request.urlopen", side_effect=urllib.error.HTTPError("u", 404, "missing", {}, None)), \
                self.assertRaises(urllib.error.HTTPError):
            prepare_execution.download("https://example.invalid/a", "0" * 64, pathlib.Path(directory) / "a")

    def test_yaml_integers_are_read_as_compose_reads_them(self):
        import yaml

        text = "octal_looking: 010\nclock: 1:30\nseparated: 1_000\nhex: 0x1f\noctal: 0o17\nplain: -3\n"
        values = prepare_execution.load_yaml(text)
        self.assertEqual(values, {"octal_looking": 10, "clock": "1:30", "separated": "1_000",
                                  "hex": 31, "octal": 15, "plain": -3})
        self.assertEqual(prepare_execution.load_yaml(yaml.safe_dump(values)), values)

    def test_the_runner_a_lock_resolved_is_fetched_by_name_and_checked_against_its_digest(self):
        import hashlib
        import io
        import tarfile

        payload = io.BytesIO()
        with tarfile.open(fileobj=payload, mode="w:gz") as bundle:
            body = b"#!/bin/sh\n"
            info = tarfile.TarInfo("harness-e2e")
            info.size = len(body)
            bundle.addfile(info, io.BytesIO(body))
        archive = payload.getvalue()
        lock = {"containers": {"e2e": {"worker": "package://api.workers.iii.dev/harness-e2e", "resolved": {
            "version": "0.12.3", "artifacts": {prepare_execution.CLI_TARGET: {
                "url": "https://example.invalid/harness-e2e.tar.gz",
                "sha256": hashlib.sha256(archive).hexdigest(),
            }},
        }}}}

        class Response(io.BytesIO):
            def __enter__(self):
                return self

            def __exit__(self, *exc):
                return False

        with tempfile.TemporaryDirectory() as directory, \
                patch("urllib.request.urlopen", side_effect=lambda *a, **k: Response(archive)):
            binary = prepare_execution.fetch_runner(lock, pathlib.Path(directory))
            self.assertEqual(binary.read_bytes(), b"#!/bin/sh\n")
            lock["containers"]["e2e"]["resolved"]["artifacts"][prepare_execution.CLI_TARGET]["sha256"] = "0" * 64
            with self.assertRaisesRegex(prepare_execution.ResolutionError, "digest"):
                prepare_execution.fetch_runner(lock, pathlib.Path(directory))

    def test_the_contract_states_the_suite_the_runner_materialized(self):
        execution = {"model": "deepseek/deepseek-v4-flash", "profile": None}
        cli = {"version": "0.24.2", "target": "t", "asset": "iii-t.tar.gz", "sha256": "sha256:" + "c" * 64}
        compose = {"containers": {"harness": {"worker": "package://harness", "version": "latest"}}}
        contract = prepare_execution.build_contract(
            PROFILE_SNAPSHOT["campaigns"][0], execution_key="b0607faa-096a-4efe-a4a2-a2a9bc06de83",
            snapshot=PROFILE_SNAPSHOT, execution=execution, cli=cli, compose=compose,
            oidc_audience="release-control-harness-e2e",
        )
        self.assertEqual(contract["schema"], prepare_execution.CONTRACT_SCHEMA)
        self.assertEqual(contract["suite"]["id"], "regression-r01")
        self.assertEqual(contract["suite"]["subject"], PLAN["subject"])
        self.assertEqual(contract["runtime"], {"cli": cli, "compose": compose})
        with_agent = prepare_execution.build_contract(
            PROFILE_SNAPSHOT["campaigns"][0], execution_key=contract["execution_id"],
            snapshot=PROFILE_SNAPSHOT, execution={**execution, "profile": "console-ui"}, cli=cli,
            compose=compose, oidc_audience="release-control-harness-e2e",
        )
        self.assertEqual(with_agent["suite"]["agent_profile"], "console-ui")
        self.assertNotEqual(with_agent["idempotency_key"], contract["idempotency_key"])
        # Absent: each scenario keeps the canonical seed it was materialized
        # with, so the same slot stays the same slot across executions.
        self.assertIsNone(contract["suite"]["seed"])
        group = contract["suite"]["groups"][0]
        # No difficulty weight travels: every case counts the same.
        self.assertEqual(sorted(group), ["execution_kind", "id", "runs", "scenarios", "technical_retries"])
        self.assertRegex(contract["idempotency_key"], r"^rc:e2e:[0-9a-f]{64}$")

    def test_the_resolution_names_the_executor_image_the_execution_was_prepared_in(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / "suite.json").write_text(json.dumps(PROFILE_SNAPSHOT))
            (root / "execution.json").write_text(json.dumps({
                "model": "deepseek/deepseek-v4-flash", "cli": {"version": "0.24.2"}, "template": None}))
            (root / "stack.yaml").write_text("containers: {}\n")
            args = SimpleNamespace(contract_dir=root, execution_key="42", oidc_audience="release-control-harness-e2e")
            image = "ghcr.io/iii-hq/harness-e2e@sha256:" + "e" * 64
            with patch.dict(os.environ, {"HARNESS_E2E_EXECUTOR_IMAGE": image}):
                prepare_execution.command_contracts(args)
            resolution = json.loads((root / "contracts/resolution.json").read_text())
            self.assertEqual(resolution["executor_image"], image)
            self.assertEqual(resolution["cli_version"], "0.24.2")
            # Prepared outside the image, it names none.
            with patch.dict(os.environ):
                os.environ.pop("HARNESS_E2E_EXECUTOR_IMAGE", None)
                prepare_execution.command_contracts(args)
            self.assertNotIn("executor_image", json.loads((root / "contracts/resolution.json").read_text()))

    def test_every_contract_carries_the_stack_assembled_once_and_its_lock(self):
        import yaml

        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / "contract/contracts").mkdir(parents=True)
            (root / "assembled").mkdir()
            compose = {"namespace": "e2e-prepare", "containers": {
                "harness": {"worker": "package://harness", "version": "latest"},
                "state": {"worker": "package://state", "version": "0.22.8"},
            }}
            # As Compose writes it, with the registry host; and read as YAML
            # 1.2, where `on`, `no` and an unquoted date are strings, not
            # booleans and a datetime.
            lock_text = textwrap.dedent("""\
                version: 1
                containers:
                  harness:
                    worker: package://api.workers.iii.dev/harness
                    requested: latest
                    resolved:
                      name: harness
                      version: 1.9.3
                  state:
                    worker: package://api.workers.iii.dev/state
                    requested: latest
                    resolved:
                      name: state
                      version: 0.22.8
                      default_config:
                        mode: on
                        audit: no
                        since: 2026-09-24
                        strict: true
                """)
            (root / "assembled/worker-compose.yaml").write_text(yaml.safe_dump(compose))
            (root / "assembled/worker-compose.lock").write_text(lock_text)
            (root / "contract/execution.json").write_text(json.dumps({"iii": "0.24.2", "template": None}))
            (root / "contract/contracts/resolution.json").write_text(json.dumps({"cli_version": "0.24.2"}))
            before = {"runtime": {"cli": {}, "compose": {"containers": {}}}, "suite": {"id": "regression-r01"}}
            (root / "contract/contracts/regression-r01.json").write_text(json.dumps(before))
            args = SimpleNamespace(contract_dir=root / "contract", assembled=root / "assembled")
            prepare_execution.command_lock(args)
            contract = json.loads((root / "contract/contracts/regression-r01.json").read_text())
            resolution = json.loads((root / "contract/contracts/resolution.json").read_text())
            stack = yaml.safe_load((root / "contract/stack.yaml").read_text())
            self.assertEqual((root / "contract/worker-compose.lock").read_text(), lock_text)
        self.assertEqual(contract["runtime"]["compose"], compose)
        self.assertEqual(contract["runtime"]["lock"]["containers"]["state"]["resolved"]["default_config"],
                         {"mode": "on", "audit": "no", "since": "2026-09-24", "strict": True})
        # Written back for a group, a 1.2 reader gets the same strings.
        rewritten = yaml.safe_dump(contract["runtime"]["lock"], sort_keys=False)
        self.assertEqual(prepare_execution.load_yaml(rewritten), contract["runtime"]["lock"])
        self.assertRegex(contract["idempotency_key"], r"^rc:e2e:[0-9a-f]{64}$")
        self.assertEqual(resolution["stack_versions"], {"harness": "1.9.3", "state": "0.22.8"})
        self.assertEqual(stack, {"iii": "0.24.2", **compose})


if __name__ == "__main__":
    unittest.main()
