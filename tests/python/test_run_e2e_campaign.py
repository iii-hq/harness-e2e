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
    FAULT_PROFILE_WEIGHT,
    SCENARIO_DIFFICULTY_WEIGHT,
    aggregate_existing_campaign,
    attach_markdown_group,
    build_campaign_bundle,
    build_group_command,
    discover_markdown_scenarios,
    execute_campaign,
    load_campaign,
    main,
    parse_campaign,
    score_campaign,
    validate_campaign_bundle,
)


CAMPAIGN_DIR = ROOT / "config" / "campaigns"


def manifest(groups=None):
    selected = copy.deepcopy(
        groups
        or [
            {
                "id": "core",
                "execution_kind": "harness_turn",
                "runs": 1,
                "technical_retries": 1,
                "scenarios": ["tool_contract_recovery"],
            }
        ]
    )
    for group in selected:
        if "difficulty_weight" in group:
            continue
        if group.get("execution_kind") == "fault_injection":
            group["difficulty_weight"] = FAULT_PROFILE_WEIGHT.get(
                group.get("fault_profile"), 1
            )
        else:
            group["difficulty_weight"] = max(
                (
                    SCENARIO_DIFFICULTY_WEIGHT.get(scenario, 1)
                    for scenario in group.get("scenarios", [])
                ),
                default=1,
            )
    return {
        "kind": "harness-e2e-campaign",
        "campaign_id": "test-campaign",
        "lane": "daily",
        "failure_policy": "enforcing",
        "scoring_profile": "difficulty-weighted-v1",
        "groups": selected,
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
    def test_markdown_plan_membership_materializes_a_campaign_group(self):
        daily = attach_markdown_group(
            load_campaign(CAMPAIGN_DIR / "daily.json"), ROOT / "scenarios"
        )
        group = daily.groups[-1]
        self.assertEqual(group.id, "daily-markdown")
        self.assertEqual(group.execution_kind, "harness_turn")
        self.assertEqual(group.runs, 1)
        self.assertEqual(group.technical_retries, 1)
        self.assertEqual(group.difficulty_weight, 2)
        self.assertIn("insert_record", group.scenarios)
        self.assertIn("database_migration_recovery", group.scenarios)
        self.assertIn("minimal_path", group.scenarios)
        self.assertIn("sequential_pipeline", group.scenarios)

        weekly = attach_markdown_group(
            load_campaign(CAMPAIGN_DIR / "weekly.json"), ROOT / "scenarios"
        )
        self.assertEqual(weekly.groups[-1].runs, 3)

    def test_markdown_discovery_reads_only_the_plans_section(self):
        scenarios = discover_markdown_scenarios(ROOT / "scenarios", "daily")
        self.assertIn("insert_record", scenarios)
        self.assertIn("persistent_state", scenarios)
        self.assertIn("database_migration_recovery", scenarios)
        self.assertIn("minimal_path", scenarios)
        self.assertIn("sequential_pipeline", scenarios)

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

    def test_post_release_covers_canary_code_integration_and_release_recovery(self):
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
                "shell_coder_sandbox",
                "performance_regression",
                "chess_engine_build",
                "engineering_ticket_git_handoff",
                "cross_app_transaction",
                "browser_cross_site",
                "release_train_recovery",
            },
        )
        adaptive = [
            group for group in campaign.groups if group.execution_kind == "adaptive_flow"
        ]
        self.assertEqual(len(adaptive), 1)
        for group in adaptive:
            self.assertEqual(group.runs, 1)
            self.assertEqual(group.technical_retries, 0)
            self.assertEqual(len(group.scenarios), 1)

    def test_daily_prioritizes_code_with_a_hard_code_group(self):
        campaign = load_campaign(CAMPAIGN_DIR / "daily.json")
        groups = {group.id: group for group in campaign.groups}
        self.assertEqual(
            set(groups["daily-canary"].scenarios),
            {
                "tool_contract_recovery",
            },
        )
        self.assertEqual(
            groups["daily-code"].scenarios,
            (
                "shell_coder_sandbox",
                "performance_regression",
                "engineering_ticket_git_handoff",
            ),
        )
        self.assertEqual(
            groups["daily-hard-code"].scenarios,
            ("chess_engine_build", "git_regression_forensics"),
        )
        self.assertTrue(all(group.runs == 1 for group in campaign.groups))
        self.assertTrue(all(group.technical_retries == 1 for group in campaign.groups))

    def test_weekly_repeats_hard_code_and_isolates_complex_flows(self):
        campaign = load_campaign(CAMPAIGN_DIR / "weekly.json")
        groups = {group.id: group for group in campaign.groups}
        self.assertEqual(groups["weekly-shell-parity"].runs, 3)
        self.assertEqual(groups["weekly-performance"].runs, 5)
        self.assertEqual(groups["weekly-chess-build"].runs, 3)
        self.assertEqual(groups["weekly-git-forensics"].runs, 3)
        self.assertEqual(groups["weekly-engineering-handoff"].runs, 3)
        self.assertEqual(groups["weekly-contention"].runs, 3)
        self.assertEqual(
            groups["weekly-performance"].scenarios,
            ("performance_regression",),
        )
        self.assertEqual(
            groups["weekly-security-review"].scenarios,
            ("security_review",),
        )
        self.assertEqual(
            groups["weekly-security-review"].execution_kind, "composite_flow"
        )
        self.assertEqual(groups["weekly-security-review"].technical_retries, 0)
        adaptive = [
            group for group in campaign.groups if group.execution_kind == "adaptive_flow"
        ]
        self.assertEqual(len(adaptive), 2)
        self.assertTrue(all(group.runs == 1 for group in adaptive))
        self.assertTrue(all(len(group.scenarios) == 1 for group in adaptive))
        faults = [
            group for group in campaign.groups if group.execution_kind == "fault_injection"
        ]
        self.assertEqual(len(faults), 3)
        self.assertEqual([group.difficulty_weight for group in faults], [2, 3, 4])
        self.assertTrue(all(group.runs == 3 for group in faults))
        self.assertTrue(all(group.soak_minutes == 60 for group in faults))

    def test_revised_plans_exclude_removed_scenarios(self):
        removed = {
            "validation_scope_enforcement",
            "cleanup_under_failure",
            "policy_bound_action",
            "engineering_ticket",
        }
        for name in ("daily.json", "weekly.json", "post-release.json"):
            campaign = load_campaign(CAMPAIGN_DIR / name)
            selected = {
                scenario
                for group in campaign.groups
                for scenario in group.scenarios
            }
            self.assertTrue(removed.isdisjoint(selected), name)


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

    def test_markdown_groups_require_and_freeze_an_explicit_auxiliary_model(self):
        markdown_campaign = parse_campaign(manifest())
        markdown_campaign = type(markdown_campaign)(
            campaign_id=markdown_campaign.campaign_id,
            lane=markdown_campaign.lane,
            failure_policy=markdown_campaign.failure_policy,
            scoring_profile=markdown_campaign.scoring_profile,
            groups=(
                type(markdown_campaign.groups[0])(
                    id=f"{markdown_campaign.campaign_id}-markdown",
                    execution_kind="harness_turn",
                    runs=1,
                    technical_retries=1,
                    difficulty_weight=2,
                    scenarios=("insert_record",),
                ),
            ),
        )
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(CampaignError, "explicit judge"):
                execute_campaign(
                    markdown_campaign,
                    e2e_bin=pathlib.Path("bin/harness-e2e"),
                    output_root=pathlib.Path(directory),
                    execution_id="markdown-no-judge",
                    dry_run=False,
                    advisory=True,
                    model="model",
                    provider="provider",
                    environ={},
                )

            summary = execute_campaign(
                markdown_campaign,
                e2e_bin=pathlib.Path("bin/harness-e2e"),
                output_root=pathlib.Path(directory),
                execution_id="markdown-with-judge",
                dry_run=False,
                advisory=True,
                model="model",
                provider="provider",
                judge_model="judge-model",
                judge_provider="judge-provider",
                environ={},
                run_process=lambda *_args, **_kwargs: types.SimpleNamespace(returncode=0),
            )
        command = summary["groups"][0]["command"]
        self.assertIn("--judge-model", command)
        self.assertIn("judge-model", command)
        self.assertTrue(summary["groups"][0]["materialized_group_sha256"].startswith("sha256:"))

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

    def test_bundle_preserves_native_bytes_and_rejects_tampering(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            group_output = root / "groups" / "core"
            group_output.mkdir(parents=True)
            results = group_output / "results.json"
            results.write_text('{"native":true}\n', encoding="utf-8")
            summary_path = root / "campaign-summary.json"
            summary = {
                "campaign_id": "test-campaign",
                "execution_id": "execution-1",
                "lane": "daily",
                "groups": [
                    {
                        "group_id": "core",
                        "execution_kind": "harness_turn",
                        "status": "passed",
                        "difficulty_weight": 4,
                        "objective_score": 91.0,
                        "score_availability": "complete",
                        "output": str(group_output),
                    }
                ],
            }
            summary_path.write_text(json.dumps(summary), encoding="utf-8")
            campaign_path = root / "campaign.json"
            campaign_path.write_text(json.dumps(manifest()), encoding="utf-8")
            scoring_path = ROOT / "config" / "scoring" / "difficulty-weighted-v1.json"
            bundle = build_campaign_bundle(
                summary,
                summary_path=summary_path,
                manifest_path=campaign_path,
                scoring_profile_path=scoring_path,
            )
            validate_campaign_bundle(bundle, root=root)
            results.write_text('{"native":false}\n', encoding="utf-8")
            with self.assertRaisesRegex(CampaignError, "digest mismatch"):
                validate_campaign_bundle(bundle, root=root)

    def test_difficulty_weighted_score_uses_native_scenario_medians(self):
        campaign = parse_campaign(
            manifest(
                [
                    {
                        "id": "l4",
                        "execution_kind": "harness_turn",
                        "runs": 1,
                        "technical_retries": 0,
                        "difficulty_weight": 4,
                        "scenarios": ["tool_contract_recovery"],
                    },
                    {
                        "id": "l2",
                        "execution_kind": "harness_turn",
                        "runs": 1,
                        "technical_retries": 0,
                        "difficulty_weight": 2,
                        "scenarios": ["performance_regression"],
                    },
                ]
            )
        )
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            groups = []
            for group_id, tier, median in [
                ("l4", "l4_coordinated", 80.0),
                ("l2", "l2_stateful", 100.0),
            ]:
                output = root / group_id
                output.mkdir()
                (output / "results.json").write_text(
                    json.dumps(
                        {
                            "passed": True,
                            "scenarios": [
                                {
                                    "case": {"complexity": {"tier": tier}},
                                    "aggregate": {
                                        "median_score": median,
                                        "scored_runs": 1,
                                        "technical_failures": 0,
                                    },
                                }
                            ],
                        }
                    ),
                    encoding="utf-8",
                )
                groups.append({"group_id": group_id, "output": str(output)})
            scoring = score_campaign(campaign, groups)
        self.assertAlmostEqual(scoring["harness_score"], (80 * 4 + 100 * 2) / 6)
        self.assertEqual(scoring["coverage"], 1.0)
        self.assertEqual(scoring["score_availability"], "complete")

    def test_fault_infrastructure_is_null_not_zero_and_reduces_coverage(self):
        campaign = parse_campaign(
            manifest(
                [
                    {
                        "id": "fault",
                        "execution_kind": "fault_injection",
                        "runs": 3,
                        "technical_retries": 0,
                        "difficulty_weight": 2,
                        "fault_profile": "weekly-l2-recovery",
                        "fault_scenario": "stateful.2",
                        "soak_minutes": 60,
                    }
                ]
            )
        )
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory) / "fault"
            for index, classification in enumerate(
                ["correct_recovery", "infrastructure_failure"], start=1
            ):
                run = output / f"run-{index}"
                run.mkdir(parents=True)
                (run / "fault-evaluation.json").write_text(
                    json.dumps({"classification": classification}), encoding="utf-8"
                )
            scoring = score_campaign(
                campaign, [{"group_id": "fault", "output": str(output)}]
            )
        self.assertEqual(scoring["harness_score"], 100.0)
        self.assertAlmostEqual(scoring["coverage"], 1 / 3)
        self.assertFalse(scoring["infrastructure_valid"])
        self.assertEqual(scoring["score_availability"], "partial")

    def test_aggregate_existing_campaign_keeps_missing_group_as_infrastructure(self):
        campaign = parse_campaign(manifest())
        with tempfile.TemporaryDirectory() as directory:
            summary = aggregate_existing_campaign(
                campaign,
                group_root=pathlib.Path(directory),
                execution_id="workflow-1",
            )
        self.assertIsNone(summary["scoring"]["harness_score"])
        self.assertFalse(summary["scoring"]["infrastructure_valid"])
        self.assertEqual(summary["process_exit_code"], 0)

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
