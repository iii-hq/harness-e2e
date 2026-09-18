"""The run ledger boundary: what this repository must send Release Control.

These tests state the shape of the three reports and of the contract this
repository now assembles for itself. They are the executable half of the
agreement — Release Control reads exactly the fields asserted here.
"""

import importlib.util
import base64
import json
import pathlib
import sys
import tempfile
import unittest
from unittest.mock import patch


ROOT = pathlib.Path(__file__).resolve().parents[2]


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


report_execution = load("report_execution")
resolve_stack_lock = load("resolve_stack_lock")


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
            "contract_sha256": None,
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


class StackResolutionTests(unittest.TestCase):
    def test_runner_release_version_pins_the_runner_unless_the_stack_does(self):
        plan = {"runner": {"version": "0.11.2-experimental"}}
        self.assertEqual(resolve_stack_lock.runner_selector(plan, {}), "0.11.2-experimental")
        self.assertEqual(
            resolve_stack_lock.runner_selector(plan, {"harness-e2e": "0.11.1-experimental"}),
            "0.11.1-experimental",
        )
        self.assertEqual(resolve_stack_lock.runner_selector({}, {}), "latest")
        with self.assertRaises(resolve_stack_lock.ResolutionError):
            resolve_stack_lock.runner_selector({"runner": {"version": "latest"}}, {})

    def test_template_catalog_and_dependencies_are_read_at_one_main_commit(self):
        files = {
            "template.yaml": "templates: [harness, new-template]\n",
            "new-template/template.yaml": "files: [worker-compose.yaml]\noptional: []\n",
            "new-template/worker-compose.yaml": "containers:\n  board:\n    worker: package://kanban\n    version: latest\n  local:\n    worker: path://./local\n",
        }
        urls = []
        def get(url, **kwargs):
            urls.append(url)
            if url.endswith("/commits/main"):
                return {"sha": "a" * 40}
            path, ref = url.split("/contents/iii/", 1)[1].split("?ref=")
            self.assertEqual(ref, "a" * 40)
            return {"content": base64.b64encode(files[path].encode()).decode()}
        with patch.object(resolve_stack_lock, "get_json", side_effect=get):
            identity, packages = resolve_stack_lock.resolve_template("new-template", None)
        self.assertEqual(identity["revision"], "a" * 40)
        self.assertEqual(identity["id"], "new-template")
        self.assertEqual(packages, {"kanban": "latest"})
        self.assertEqual(sum(url.endswith("/commits/main") for url in urls), 1)
        for invalid in ("../harness", {}, ""):
            with self.subTest(invalid=invalid), self.assertRaises(resolve_stack_lock.ResolutionError):
                resolve_stack_lock.resolve_template(invalid, None)
        self.assertEqual(resolve_stack_lock.resolve_template(None, None), (None, {}))

    def test_unsupported_or_interactive_templates_fail_before_execution(self):
        for metadata in ("files: [config.yaml]", "files: [worker-compose.yaml]\noptional: [python]"):
            responses = [
                {"sha": "a" * 40},
                {"content": base64.b64encode(b"templates: [starter]").decode()},
                {"content": base64.b64encode(metadata.encode()).decode()},
            ]
            with self.subTest(metadata=metadata), patch.object(resolve_stack_lock, "get_json", side_effect=responses):
                with self.assertRaises(resolve_stack_lock.ResolutionError):
                    resolve_stack_lock.resolve_template("starter", None)

    def graph(self, worker, version, nodes=(), edges=()):
        return {"root": {"worker": worker, "version": version}, "nodes": list(nodes), "edges": list(edges)}

    def test_the_contract_states_the_profile_the_runner_materialized(self):
        contract = resolve_stack_lock.build_contract(
            PROFILE_SNAPSHOT["campaigns"][0],
            execution_id="b0607faa-096a-4efe-a4a2-a2a9bc06de83",
            snapshot=PROFILE_SNAPSHOT,
            plan=PLAN,
            cli={"version": "0.23.1-rc.2", "target": "t", "asset": "iii-t.tar.gz", "sha256": "sha256:" + "c" * 64},
            stack={"harness": "1.8.15"},
            oidc_audience="release-control-harness-e2e",
        )
        self.assertEqual(contract["schema"], resolve_stack_lock.CONTRACT_SCHEMA)
        self.assertEqual(contract["suite"]["id"], "regression-r01")
        self.assertEqual(contract["suite"]["subject"], PLAN["subject"])
        agent = "console-ui"
        with_agent = resolve_stack_lock.build_contract(
            PROFILE_SNAPSHOT["campaigns"][0], execution_id=contract["execution_id"],
            snapshot=PROFILE_SNAPSHOT, plan={**PLAN, "agent_profile": agent},
            cli=contract["runtime"]["cli"],
            stack=contract["runtime"]["stack"], oidc_audience="release-control-harness-e2e",
        )
        self.assertEqual(with_agent["suite"]["agent_profile"], agent)
        self.assertNotEqual(with_agent["idempotency_key"], contract["idempotency_key"])
        # Absent: each scenario keeps the canonical seed it was materialized
        # with, so the same slot stays the same slot across executions.
        self.assertIsNone(contract["suite"]["seed"])
        group = contract["suite"]["groups"][0]
        # No difficulty weight travels: every case counts the same.
        self.assertEqual(
            sorted(group),
            ["execution_kind", "id", "runs", "scenarios", "technical_retries"],
        )
        self.assertEqual(group["scenarios"], ["minimal_path"])
        self.assertRegex(contract["idempotency_key"], r"^rc:e2e:[0-9a-f]{64}$")

    def test_a_selector_that_stays_mutable_never_reaches_a_contract(self):
        """`latest` is a question. A contract may only contain the answer."""
        source = (ROOT / "scripts/resolve_stack_lock.py").read_text()
        self.assertIn("EXACT_VERSION.fullmatch(version)", source)
        with self.assertRaises(resolve_stack_lock.ResolutionError):
            resolve_stack_lock.resolve_cli("latest", None)


if __name__ == "__main__":
    unittest.main()
