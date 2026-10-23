"""Running some slots of a suite again: the dispatch's `slots` cut the
materialized suite down before anything runs, and each slot that runs reports
the shard and slot identity it had in the execution that ran the whole suite."""

import importlib.util
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from types import SimpleNamespace


ROOT = pathlib.Path(__file__).resolve().parents[2]


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


report_execution = load("report_execution")
prepare_execution = load("prepare_execution")


def group(*scenarios):
    return {"id": "case-" + scenarios[0].replace("_", "-"), "execution_kind": "harness_turn", "runs": 1,
            "technical_retries": 0, "scenarios": list(scenarios)}


# What the runner materializes for three repetitions: three identical rounds.
SUITE = {
    "schema": "harness-e2e-profile-snapshot",
    "profile_sha256": "sha256:" + "p" * 64,
    "profile": {"id": "software-engineering", "label": "Builds", "repetitions": 3},
    "scenario_ids": ["timer_wake", "registry_implementation", "registry_verification"],
    "campaigns": [
        {"kind": "harness-e2e-campaign", "campaign_id": f"software-engineering-r0{n}", "lane": "local-se",
         "groups": [group("timer_wake"), group("registry_implementation", "registry_verification")]}
        for n in (1, 2, 3)
    ],
    "budget": {"scenario_runs": 9, "planned_runs": 9, "session_turn_limit_sum": 90},
}

SLOTS = [{"scenario_id": "timer_wake", "repetition": 2},
         {"scenario_id": "registry_verification", "repetition": 0},
         {"scenario_id": "no_such_test", "repetition": 1}]


class SlotInputTests(unittest.TestCase):
    def test_slots_are_a_json_list_of_scenario_and_repetition(self):
        read = prepare_execution.read_slots
        self.assertIsNone(read(""))
        self.assertIsNone(read("  "))
        # Other keys a dispatcher keeps (its slot id, say) are ignored.
        self.assertEqual(read('[{"scenario_id": "timer_wake", "repetition": 2, "slot_id": "slot-0"}]'),
                         [{"scenario_id": "timer_wake", "repetition": 2}])
        for value in ("timer_wake#2", "[]", '{"scenario_id": "timer_wake", "repetition": 2}',
                      '[{"scenario_id": "timer_wake"}]', '[{"scenario_id": "", "repetition": 0}]',
                      '[{"scenario_id": "timer_wake", "repetition": -1}]',
                      '[{"scenario_id": "timer_wake", "repetition": true}]',
                      '[{"scenario_id": "timer_wake", "repetition": "2"}]'):
            with self.subTest(value=value), self.assertRaises(prepare_execution.ResolutionError):
                read(value)

    def test_without_slots_the_dispatch_is_what_it_was(self):
        dispatch = prepare_execution.read_dispatch({"suite": "pr", "model": "zai/glm-5.1"})
        self.assertNotIn("slots", dispatch["execution"])
        dispatch = prepare_execution.read_dispatch(
            {"suite": "pr", "model": "zai/glm-5.1", "slots": json.dumps(SLOTS[:1])})
        self.assertEqual(dispatch["execution"]["slots"], SLOTS[:1])
        # The plan the ledger reports read does not change.
        self.assertEqual(dispatch["plan"], {"profile": {"id": "pr"}, "subject": {"provider": "zai", "model": "glm-5.1"}})


class SlotSelectionTests(unittest.TestCase):
    def test_only_the_groups_running_a_slot_stay_and_every_round_keeps_its_place(self):
        selected, missing = prepare_execution.select_slots(SUITE, SLOTS)
        self.assertEqual(missing, [SLOTS[2]])
        self.assertEqual([campaign["campaign_id"] for campaign in selected["campaigns"]],
                         [campaign["campaign_id"] for campaign in SUITE["campaigns"]])
        self.assertEqual([[g["id"] for g in campaign["groups"]] for campaign in selected["campaigns"]],
                         [["case-registry-implementation"], [], ["case-timer-wake"]])
        # A paired scenario runs with the one it needs.
        self.assertEqual(selected["campaigns"][0]["groups"][0]["scenarios"],
                         ["registry_implementation", "registry_verification"])
        self.assertEqual(selected["budget"], {**SUITE["budget"], "scenario_runs": 3, "planned_runs": 3})
        # The suite is the same suite: its identity does not move.
        for key in ("profile_sha256", "profile", "scenario_ids", "schema"):
            self.assertEqual(selected[key], SUITE[key])

    def test_a_slot_reports_the_shard_and_repetition_it_had_when_the_whole_suite_ran(self):
        selected, _ = prepare_execution.select_slots(SUITE, SLOTS)
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            artifacts = root / "artifacts"
            artifacts.mkdir()

            def reported(snapshot, campaign_id, group_id, scenarios):
                (root / "profile.json").write_text(json.dumps(snapshot))
                (artifacts / "results.json").write_text(json.dumps({"scenarios": [
                    {"scenario_id": scenario, "case": {"seed": 7}, "runs": [{"run_id": "r"}]}
                    for scenario in scenarios]}))
                payload = report_execution.shard_payload(SimpleNamespace(
                    execution_id="e", artifacts=artifacts, campaign_id=campaign_id, group_id=group_id,
                    outcome="success", profile_snapshot=root / "profile.json", plan=None, resolution=None,
                    stack_lock_sha256=None, runner_sha=None, cli_version=None, artifact_name=None))
                # What Release Control keys a slot by: the shard, and the
                # scenario, seed and repetition its slot id is derived from.
                return [(payload["shard"], run["scenario_id"], run["seed"], run["repetition"])
                        for run in payload["runs"]]

            for campaign in selected["campaigns"]:
                for kept in campaign["groups"]:
                    with self.subTest(campaign=campaign["campaign_id"], group=kept["id"]):
                        args = (campaign["campaign_id"], kept["id"], kept["scenarios"])
                        self.assertEqual(reported(selected, *args), reported(SUITE, *args))
            self.assertEqual(reported(selected, "software-engineering-r03", "case-timer-wake", ["timer_wake"]),
                             [("software-engineering-r03/case-timer-wake", "timer_wake", "7", 2)])

    def test_the_contracts_and_the_matrix_hold_only_what_runs(self):
        selected, _ = prepare_execution.select_slots(SUITE, SLOTS)
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / "suite.json").write_text(json.dumps(selected))
            (root / "execution.json").write_text(json.dumps({
                "model": "deepseek/deepseek-v4-flash", "cli": {"version": "0.24.2"}, "template": None}))
            (root / "stack.yaml").write_text("containers: {}\n")
            prepare_execution.command_contracts(
                SimpleNamespace(contract_dir=root, execution_key="42", oidc_audience="release-control-harness-e2e"))
            resolution = json.loads((root / "contracts/resolution.json").read_text())
            self.assertEqual(resolution["campaign_ids"], ["software-engineering-r01", "software-engineering-r03"])
            self.assertEqual([(entry["campaign_id"], entry["group_id"]) for entry in resolution["matrix"]["include"]],
                             [("software-engineering-r01", "case-registry-implementation"),
                              ("software-engineering-r03", "case-timer-wake")])
            self.assertEqual(sorted(path.name for path in (root / "contracts").iterdir()),
                             ["resolution.json", "software-engineering-r01.json", "software-engineering-r03.json"])

    def test_the_slots_step_cuts_the_suite_and_leaves_it_whole_without_slots(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            suite = json.dumps(SUITE, indent=2) + "\n"

            def step(execution):
                (root / "suite.json").write_text(suite)
                (root / "execution.json").write_text(json.dumps(execution))
                return subprocess.run(["python3", "scripts/prepare_execution.py", "slots", "--contract-dir", str(root)],
                                      cwd=ROOT, env=os.environ, capture_output=True, text=True)

            result = step({"suite": "software-engineering"})
            self.assertEqual((result.returncode, (root / "suite.json").read_text()), (0, suite))
            result = step({"suite": "software-engineering", "slots": SLOTS})
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("::warning::the suite has no slot no_such_test repetition 1", result.stderr)
            self.assertEqual(json.loads((root / "suite.json").read_text()), prepare_execution.select_slots(SUITE, SLOTS)[0])
            # Slots none of the suite runs leave nothing to run: said, not run.
            result = step({"suite": "software-engineering", "slots": SLOTS[2:]})
            self.assertEqual(result.returncode, 2)
            self.assertIn("slots names no slot of the suite", result.stderr)


if __name__ == "__main__":
    unittest.main()
