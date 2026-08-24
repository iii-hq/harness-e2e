import copy
import contextlib
import io
import json
import pathlib
import sys
import tempfile
import types
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from run_e2e_campaign import (
    CampaignError,
    build_group_command,
    execute_campaign,
    load_campaign,
    main,
    parse_campaign,
)


CAMPAIGN_DIR = ROOT / "config" / "campaigns"


def manifest(groups=None):
    return {
        "kind": "harness-e2e-campaign",
        "campaign_id": "test-campaign",
        "lane": "daily",
        "failure_policy": "enforcing",
        "groups": groups
        or [
            {
                "id": "core",
                "execution_kind": "harness_turn",
                "runs": 1,
                "technical_retries": 1,
                "scenarios": ["tool_contract_recovery"],
            }
        ],
    }


def contains_seed_field(value):
    if isinstance(value, dict):
        return any(
            key in {"seed", "seeds", "rotating_seed", "rotating_seeds"}
            or contains_seed_field(child)
            for key, child in value.items()
        )
    if isinstance(value, list):
        return any(contains_seed_field(child) for child in value)
    return False


class CanonicalManifestTests(unittest.TestCase):
    def test_every_checked_in_campaign_is_valid_and_seedless(self):
        paths = sorted(CAMPAIGN_DIR.glob("*.json"))
        self.assertEqual(
            [path.name for path in paths],
            ["daily.json", "endurance.json", "post-release.json", "weekly.json"],
        )
        for path in paths:
            raw = json.loads(path.read_text(encoding="utf-8"))
            self.assertFalse(contains_seed_field(raw), path)
            campaign = load_campaign(path)
            self.assertTrue(campaign.groups)

    def test_endurance_is_single_run_advisory_and_not_technically_retried(self):
        campaign = load_campaign(CAMPAIGN_DIR / "endurance.json")
        self.assertEqual(campaign.failure_policy, "advisory")
        self.assertEqual(len(campaign.groups), 1)
        group = campaign.groups[0]
        self.assertEqual(group.id, "engineering-endurance")
        self.assertEqual(group.scenarios, ("engineering_endurance_ladder",))
        self.assertEqual(group.runs, 1)
        self.assertEqual(group.technical_retries, 0)

    def test_post_release_has_core_and_isolated_adaptive_scenarios(self):
        campaign = load_campaign(CAMPAIGN_DIR / "post-release.json")
        selected = {
            scenario
            for group in campaign.groups
            for scenario in group.scenarios
        }
        self.assertEqual(
            selected,
            {
                "tool_contract_recovery",
                "policy_bound_action",
                "cross_app_transaction",
                "database_migration_recovery",
                "incident_response",
                "release_train_recovery",
                "cross_repo_contract_migration",
            },
        )
        policy_group = next(
            group
            for group in campaign.groups
            if "policy_bound_action" in group.scenarios
        )
        self.assertEqual(policy_group.execution_kind, "scripted_dialogue")
        self.assertEqual(policy_group.technical_retries, 0)
        adaptive = [
            group for group in campaign.groups if group.execution_kind == "adaptive_flow"
        ]
        self.assertEqual(len(adaptive), 3)
        for group in adaptive:
            self.assertEqual(group.runs, 1)
            self.assertEqual(group.technical_retries, 0)
            self.assertEqual(len(group.scenarios), 1)

    def test_daily_has_core_research_and_isolated_policy(self):
        campaign = load_campaign(CAMPAIGN_DIR / "daily.json")
        core = next(group for group in campaign.groups if group.id == "daily-core")
        self.assertEqual(
            set(core.scenarios),
            {
                "tool_contract_recovery",
                "cross_app_transaction",
                "database_migration_recovery",
                "research_pipeline",
                "moving_target",
            },
        )
        policy = next(
            group for group in campaign.groups if group.id == "daily-policy-dialogue"
        )
        self.assertEqual(policy.scenarios, ("policy_bound_action",))
        self.assertEqual(policy.technical_retries, 0)
        engineering = next(
            group
            for group in campaign.groups
            if group.id == "daily-engineering-comparison"
        )
        self.assertEqual(
            engineering.scenarios,
            ("engineering_ticket", "engineering_ticket_git_handoff"),
        )
        self.assertEqual(engineering.execution_kind, "harness_turn")
        self.assertEqual(engineering.runs, 1)
        self.assertEqual(engineering.technical_retries, 1)

    def test_weekly_separates_repeatability_performance_and_browser(self):
        campaign = load_campaign(CAMPAIGN_DIR / "weekly.json")
        groups = {group.id: group for group in campaign.groups}
        self.assertEqual(groups["weekly-repeatability"].runs, 5)
        self.assertEqual(
            groups["weekly-performance"].scenarios,
            ("performance_regression",),
        )
        self.assertEqual(
            groups["weekly-browser-smoke"].scenarios,
            ("browser_cross_site",),
        )
        self.assertEqual(
            groups["weekly-browser-smoke"].execution_kind, "harness_turn"
        )
        self.assertEqual(groups["weekly-browser-smoke"].technical_retries, 0)
        adaptive = [
            group for group in campaign.groups if group.execution_kind == "adaptive_flow"
        ]
        self.assertEqual(len(adaptive), 3)
        self.assertTrue(all(group.runs == 1 for group in adaptive))
        self.assertTrue(all(len(group.scenarios) == 1 for group in adaptive))


class CampaignValidationTests(unittest.TestCase):
    def test_unknown_scenario_id_is_rejected(self):
        value = manifest()
        value["groups"][0]["scenarios"] = ["typo_contract_recovery"]
        with self.assertRaisesRegex(CampaignError, "unknown scenario id"):
            parse_campaign(value)

    def test_seed_fields_are_rejected_at_any_depth(self):
        for field, value in [("seed", 7), ("rotating_seeds", [7, 8])]:
            candidate = manifest()
            candidate["groups"][0][field] = value
            with self.assertRaisesRegex(CampaignError, "forbidden"):
                parse_campaign(candidate)

    def test_scripted_dialogue_must_be_isolated_and_non_retryable(self):
        policy = {
            "id": "policy",
            "execution_kind": "scripted_dialogue",
            "runs": 1,
            "technical_retries": 1,
            "scenarios": ["policy_bound_action"],
        }
        with self.assertRaisesRegex(CampaignError, "technical_retries=0"):
            parse_campaign(manifest([policy]))

        policy["technical_retries"] = 0
        policy["scenarios"].append("tool_contract_recovery")
        with self.assertRaisesRegex(CampaignError, "not scripted_dialogue"):
            parse_campaign(manifest([policy]))

    def test_adaptive_flow_is_single_scenario_single_run_and_non_retryable(self):
        adaptive = {
            "id": "adaptive",
            "execution_kind": "adaptive_flow",
            "runs": 1,
            "technical_retries": 1,
            "scenarios": ["incident_response"],
        }
        with self.assertRaisesRegex(CampaignError, "technical_retries=0"):
            parse_campaign(manifest([adaptive]))

        adaptive["technical_retries"] = 0
        adaptive["runs"] = 2
        with self.assertRaisesRegex(CampaignError, "exactly one scenario with runs=1"):
            parse_campaign(manifest([adaptive]))

        adaptive["runs"] = 1
        adaptive["scenarios"].append("release_train_recovery")
        with self.assertRaisesRegex(CampaignError, "exactly one scenario with runs=1"):
            parse_campaign(manifest([adaptive]))

    def test_scenario_cannot_appear_in_multiple_groups(self):
        first = manifest()["groups"][0]
        second = copy.deepcopy(first)
        second["id"] = "again"
        with self.assertRaisesRegex(CampaignError, "more than once"):
            parse_campaign(manifest([first, second]))

    def test_unknown_and_missing_schema_fields_are_rejected(self):
        unknown = manifest()
        unknown["extra"] = True
        with self.assertRaisesRegex(CampaignError, "unsupported field"):
            parse_campaign(unknown)
        missing = manifest()
        del missing["lane"]
        with self.assertRaisesRegex(CampaignError, "missing required field"):
            parse_campaign(missing)


class CampaignRunnerTests(unittest.TestCase):
    def setUp(self):
        self.campaign = parse_campaign(
            manifest(
                [
                    {
                        "id": "core",
                        "execution_kind": "harness_turn",
                        "runs": 1,
                        "technical_retries": 1,
                        "scenarios": ["tool_contract_recovery"],
                    },
                    {
                        "id": "policy",
                        "execution_kind": "scripted_dialogue",
                        "runs": 1,
                        "technical_retries": 0,
                        "scenarios": ["policy_bound_action"],
                    },
                ]
            )
        )

    def test_group_command_has_explicit_scope_and_no_seed_arguments(self):
        group = self.campaign.groups[0]
        command = build_group_command(
            self.campaign,
            group,
            e2e_bin=pathlib.Path("bin/harness-e2e"),
            output=pathlib.Path("out/core"),
            model="model",
            provider="provider",
            url="ws://stack",
            progress_interval_seconds=0,
        )
        self.assertEqual(command[:2], ["bin/harness-e2e", "run"])
        self.assertIn("--scenario", command)
        self.assertIn("tool_contract_recovery", command)
        self.assertIn("--technical-retries", command)
        self.assertNotIn("--seed", command)
        self.assertNotIn("--rotating-seed", command)

    def test_advisory_runs_every_group_and_returns_zero_with_failed_objective(self):
        calls = []
        return_codes = iter([9, 0])

        def fake_run(command, *, env, check):
            calls.append((command, env, check))
            return types.SimpleNamespace(returncode=next(return_codes))

        with tempfile.TemporaryDirectory() as directory:
            summary = execute_campaign(
                self.campaign,
                e2e_bin=pathlib.Path("bin/harness-e2e"),
                output_root=pathlib.Path(directory),
                execution_id="execution-1",
                dry_run=False,
                advisory=True,
                model="model",
                provider="provider",
                environ={"EXISTING": "preserved"},
                run_process=fake_run,
            )
        self.assertEqual(len(calls), 2, "advisory mode must execute every group")
        self.assertFalse(summary["objective_passed"])
        self.assertEqual(summary["process_exit_code"], 0)
        self.assertEqual([group["exit_code"] for group in summary["groups"]], [9, 0])
        for _, environment, check in calls:
            self.assertEqual(environment["HARNESS_E2E_LANE"], "daily")
            self.assertEqual(environment["EXISTING"], "preserved")
            self.assertFalse(check)

    def test_enforcing_also_preserves_the_full_summary_but_returns_failure(self):
        calls = []

        def fake_run(command, *, env, check):
            calls.append(command)
            return types.SimpleNamespace(returncode=5 if len(calls) == 1 else 0)

        with tempfile.TemporaryDirectory() as directory:
            summary = execute_campaign(
                self.campaign,
                e2e_bin=pathlib.Path("bin/harness-e2e"),
                output_root=pathlib.Path(directory),
                execution_id="execution-2",
                dry_run=False,
                advisory=False,
                model="model",
                provider="provider",
                environ={},
                run_process=fake_run,
            )
        self.assertEqual(len(calls), 2)
        self.assertFalse(summary["objective_passed"])
        self.assertEqual(summary["process_exit_code"], 1)

    def test_dry_run_builds_every_command_without_starting_a_process(self):
        def should_not_run(*_args, **_kwargs):
            self.fail("dry-run must not start a subprocess")

        with tempfile.TemporaryDirectory() as directory:
            summary = execute_campaign(
                self.campaign,
                e2e_bin=pathlib.Path("missing-harness-e2e"),
                output_root=pathlib.Path(directory),
                execution_id="dry-run",
                dry_run=True,
                advisory=True,
                environ={},
                run_process=should_not_run,
            )
        self.assertEqual(summary["process_exit_code"], 0)
        self.assertEqual(
            [group["status"] for group in summary["groups"]],
            ["dry_run", "dry_run"],
        )

    def test_validate_only_cli_needs_no_model_or_binary(self):
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(
                main([str(CAMPAIGN_DIR / "post-release.json"), "--validate-only"]),
                0,
            )

    def test_advisory_cli_persists_the_complete_summary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            campaign_path = root / "campaign.json"
            campaign_path.write_text(json.dumps(manifest(self.campaign_groups())), encoding="utf-8")
            fake_binary = root / "fake-harness-e2e"
            fake_binary.write_text(
                "#!/usr/bin/env python3\n"
                "import sys\n"
                "sys.exit(7 if 'tool_contract_recovery' in sys.argv else 0)\n",
                encoding="utf-8",
            )
            fake_binary.chmod(0o755)
            output_root = root / "output"
            with contextlib.redirect_stdout(io.StringIO()):
                exit_code = main(
                    [
                        str(campaign_path),
                        "--advisory",
                        "--e2e-bin",
                        str(fake_binary),
                        "--output-root",
                        str(output_root),
                        "--execution-id",
                        "persisted-summary",
                        "--model",
                        "model",
                        "--provider",
                        "provider",
                    ]
                )
            self.assertEqual(exit_code, 0)
            summary_path = (
                output_root
                / "test-campaign"
                / "persisted-summary"
                / "campaign-summary.json"
            )
            summary = json.loads(summary_path.read_text(encoding="utf-8"))
            self.assertFalse(summary["objective_passed"])
            self.assertEqual(summary["process_exit_code"], 0)
            self.assertEqual(
                [group["exit_code"] for group in summary["groups"]], [7, 0]
            )

    @staticmethod
    def campaign_groups():
        return [
            {
                "id": "core",
                "execution_kind": "harness_turn",
                "runs": 1,
                "technical_retries": 1,
                "scenarios": ["tool_contract_recovery"],
            },
            {
                "id": "policy",
                "execution_kind": "scripted_dialogue",
                "runs": 1,
                "technical_retries": 0,
                "scenarios": ["policy_bound_action"],
            },
        ]


if __name__ == "__main__":
    unittest.main()
