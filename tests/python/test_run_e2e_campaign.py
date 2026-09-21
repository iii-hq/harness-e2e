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
    FAULT_PROFILES,
    RESULT_CONTRACT_SHA256,
    RESULT_AGGREGATE_COUNT_FIELDS,
    RESULT_AGGREGATE_RATE_FIELDS,
    RESULT_AGGREGATE_TOKEN_FIELDS,
    scenario_catalog,
    aggregate_existing_campaign,
    build_campaign_bundle,
    build_group_command,
    compact_aggregate_artifacts,
    execute_campaign,
    load_campaign,
    main,
    parse_campaign,
    score_campaign,
    validate_campaign_bundle,
)


CAMPAIGN_DIR = ROOT / "config" / "campaigns"


def native_scenario(scenario_id, *, deferred=False):
    aggregate = {field: 0 for field in RESULT_AGGREGATE_COUNT_FIELDS}
    aggregate.update({field: None for field in RESULT_AGGREGATE_RATE_FIELDS})
    aggregate.update({field: None for field in RESULT_AGGREGATE_TOKEN_FIELDS})
    aggregate.update({
        "planned_runs": 1,
        "observed_runs": 0 if deferred else 1,
        "deferred_runs": 1 if deferred else 0,
        "completed_runs": 0 if deferred else 1,
        "technical_valid_runs": 0 if deferred else 1,
        "scored_runs": 0 if deferred else 1,
        "mean_score": None if deferred else 90,
    })
    scenario = {"scenario_id": scenario_id, "aggregate": aggregate}
    if deferred:
        scenario.update({"deferral_reason": "materialization failed", "runs": []})
    else:
        scenario["case"] = {"case_id": f"{scenario_id}@1"}
    return scenario


def native_report(scenarios, *, partial=False):
    return {
        "result_contract_sha256": RESULT_CONTRACT_SHA256,
        "report_state": "partial" if partial else "complete",
        "objective_outcome": "inconclusive" if partial else "passed",
        "scenarios": scenarios,
    }


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
    return {
        "kind": "harness-e2e-campaign",
        "campaign_id": "test-campaign",
        "lane": "daily",
        "failure_policy": "enforcing",
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
    def test_endurance_retains_its_scheduled_scope_and_policy(self):
        campaign = load_campaign(CAMPAIGN_DIR / "endurance.json")
        self.assertEqual(campaign.failure_policy, "advisory")
        self.assertEqual(len(campaign.groups), 1)
        group = campaign.groups[0]
        self.assertEqual(group.id, "engineering-endurance")
        self.assertEqual(group.scenarios, ("engineering_endurance_ladder",))
        self.assertEqual(group.runs, 1)
        self.assertEqual(group.technical_retries, 0)


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

    def test_a_manifest_that_still_states_a_difficulty_weight_is_rejected(self):
        """Weights are gone from the contract, so they are an unknown field."""
        weighted = manifest()
        weighted["groups"][0]["difficulty_weight"] = 4
        with self.assertRaisesRegex(CampaignError, "difficulty_weight"):
            parse_campaign(weighted)
        profiled = manifest()
        profiled["scoring_profile"] = "difficulty-weighted"
        with self.assertRaisesRegex(CampaignError, "scoring_profile"):
            parse_campaign(profiled)

    def test_only_canonical_fault_profiles_are_accepted(self):
        fault = {
            "id": "fault",
            "execution_kind": "fault_injection",
            "runs": 3,
            "technical_retries": 0,
            "fault_profile": "weekly-l9-recovery",
            "fault_scenario": "stateful.2",
            "soak_minutes": 60,
        }
        with self.assertRaisesRegex(CampaignError, "fault_profile is not canonical"):
            parse_campaign(manifest([fault]))
        fault["fault_profile"] = sorted(FAULT_PROFILES)[0]
        campaign = parse_campaign(manifest([fault]))
        self.assertEqual(campaign.groups[0].fault_profile, fault["fault_profile"])


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
        self.assertNotIn("objective_passed", summary)
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
        self.assertNotIn("objective_passed", summary)
        self.assertEqual(summary["process_exit_code"], 1)

    def test_enforcing_exit_depends_on_execution_not_objective_approval(self):
        for return_code in (0, 5):
            with self.subTest(return_code=return_code), tempfile.TemporaryDirectory() as directory:
                def fake_run(command, *, env, check):
                    output = pathlib.Path(command[command.index("--output") + 1])
                    output.mkdir(parents=True, exist_ok=True)
                    scenario = native_scenario(command[command.index("--scenario") + 1])
                    scenario["aggregate"]["mean_score"] = 65
                    report = native_report([scenario])
                    report["objective_outcome"] = "failed"
                    (output / "results.json").write_text(json.dumps(report), encoding="utf-8")
                    return types.SimpleNamespace(returncode=return_code)

                summary = execute_campaign(
                    self.campaign,
                    e2e_bin=pathlib.Path("bin/harness-e2e"),
                    output_root=pathlib.Path(directory),
                    execution_id="numeric-score",
                    dry_run=False,
                    advisory=False,
                    model="model",
                    provider="provider",
                    environ={},
                    run_process=fake_run,
                )
                self.assertEqual(summary["scoring"]["harness_score"], 65)
                self.assertEqual(summary["process_exit_code"], 0 if return_code == 0 else 1)
                self.assertNotIn("product_passed", summary["scoring"])
                self.assertNotIn("objective_passed", summary)

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

    def test_validate_only_cli_uses_native_catalog_without_models(self):
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(
                main([str(CAMPAIGN_DIR / "endurance.json"), "--validate-only"]),
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
                "import sys, json\n"
                "if sys.argv[1:] == ['test-plan', 'catalog']: print(" + repr(json.dumps({"scenarios": scenario_catalog()})) + "); sys.exit(0)\n"
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
            self.assertNotIn("objective_passed", summary)
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
                        "score": 91.0,
                        "score_availability": "complete",
                        "output": str(group_output),
                    }
                ],
            }
            summary_path.write_text(json.dumps(summary), encoding="utf-8")
            campaign_path = root / "campaign.json"
            campaign_path.write_text(json.dumps(manifest()), encoding="utf-8")
            bundle = build_campaign_bundle(
                summary,
                summary_path=summary_path,
                manifest_path=campaign_path,
            )
            validate_campaign_bundle(bundle, root=root)
            results.write_text('{"native":false}\n', encoding="utf-8")
            with self.assertRaisesRegex(CampaignError, "digest mismatch"):
                validate_campaign_bundle(bundle, root=root)

    def test_bundle_preserves_native_journal_without_results(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            group_output = root / "groups" / "core"
            event = group_output / "native" / "executions" / "run-1" / "journal" / "events" / "00000001.json"
            event.parent.mkdir(parents=True)
            event.write_text('{"event":"RunCommitted"}\n', encoding="utf-8")
            summary_path = root / "campaign-summary.json"
            summary = {
                "campaign_id": "test-campaign",
                "execution_id": "execution-1",
                "lane": "daily",
                "groups": [
                    {
                        "group_id": "core",
                        "execution_kind": "harness_turn",
                        "status": "failed",
                        "score": None,
                        "score_availability": "unavailable",
                        "output": str(group_output),
                    }
                ],
            }
            summary_path.write_text(json.dumps(summary), encoding="utf-8")
            campaign_path = root / "campaign.json"
            campaign_path.write_text(json.dumps(manifest()), encoding="utf-8")
            bundle = build_campaign_bundle(
                summary,
                summary_path=summary_path,
                manifest_path=campaign_path,
            )

            paths = [artifact["path"] for artifact in bundle["groups"][0]["artifacts"]]
            self.assertIn(
                "groups/core/native/executions/run-1/journal/events/00000001.json",
                paths,
            )
            validate_campaign_bundle(bundle, root=root)

    def test_campaign_bundle_rejects_symlink_artifacts(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            group_output = root / "groups" / "core"
            group_output.mkdir(parents=True)
            outside = root / "outside.json"
            outside.write_text("{}\n", encoding="utf-8")
            (group_output / "linked.json").symlink_to(outside)
            summary_path = root / "campaign-summary.json"
            summary_path.write_text("{}\n", encoding="utf-8")
            campaign_path = root / "campaign.json"
            campaign_path.write_text(json.dumps(manifest()), encoding="utf-8")
            summary = {
                "campaign_id": "test-campaign",
                "execution_id": "execution-1",
                "lane": "daily",
                "groups": [{"group_id": "core", "output": str(group_output)}],
            }
            with self.assertRaisesRegex(CampaignError, "contains symlink"):
                build_campaign_bundle(
                    summary,
                    summary_path=summary_path,
                    manifest_path=campaign_path,
                )

    def test_legacy_results_v3_shape_is_rejected(self):
        campaign = parse_campaign(
            manifest(
                [
                    {
                        "id": "core",
                        "execution_kind": "harness_turn",
                        "runs": 1,
                        "technical_retries": 0,
                        "scenarios": ["tool_contract_recovery"],
                    }
                ]
            )
        )
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory) / "core"
            output.mkdir()
            (output / "results.json").write_text(
                json.dumps({"passed": True, "scenarios": []}),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(CampaignError, "result_contract_sha256"):
                score_campaign(campaign, [{"group_id": "core", "output": str(output)}])

    def test_compact_aggregate_keeps_only_bundle_references(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            group = root / "groups" / "core"
            group.mkdir(parents=True)
            results = group / "results.json"
            results.write_text('{"native":true}\n', encoding="utf-8")
            runtime = group / "session.jsonl"
            runtime.write_text("large runtime state\n", encoding="utf-8")
            contract = root / "stack-lock.json"
            contract.write_text("{}\n", encoding="utf-8")
            bundle = {
                "groups": [{"artifacts": [{"path": "groups/core/results.json"}]}]
            }

            compact_aggregate_artifacts(bundle, root=root, group_root=root / "groups")

            self.assertTrue(results.is_file())
            self.assertTrue(contract.is_file())
            self.assertFalse(runtime.exists())

    def test_partial_report_requires_every_planned_slot_explicitly(self):
        campaign = parse_campaign(manifest([{
            "id": "core", "execution_kind": "harness_turn", "runs": 1,
            "technical_retries": 0,
            "scenarios": ["tool_contract_recovery", "persistent_state"],
        }]))
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory)
            document = native_report([native_scenario("tool_contract_recovery")], partial=True)
            results = output / "results.json"
            results.write_text(json.dumps(document), encoding="utf-8")
            with self.assertRaisesRegex(CampaignError, "planned runs do not match campaign"):
                score_campaign(campaign, [{"group_id": "core", "output": str(output)}])

            document["scenarios"].append(native_scenario("persistent_state", deferred=True))
            results.write_text(json.dumps(document), encoding="utf-8")
            scoring = score_campaign(campaign, [{"group_id": "core", "output": str(output)}])
            self.assertEqual(scoring["harness_score"], 90)
            self.assertEqual(scoring["score_coverage"], 0.5)
            self.assertEqual(scoring["score_availability"], "partial")

    def test_unmaterialized_scenario_requires_deferral_reason_without_observations(self):
        campaign = parse_campaign(manifest())
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory)
            deferred = native_scenario("tool_contract_recovery", deferred=True)
            deferred.pop("deferral_reason")
            (output / "results.json").write_text(
                json.dumps(native_report([deferred], partial=True)), encoding="utf-8"
            )
            with self.assertRaisesRegex(CampaignError, "wholly deferred with a reason"):
                score_campaign(campaign, [{"group_id": "core", "output": str(output)}])

    def test_persistence_error_preserves_score_but_invalidates_infrastructure(self):
        campaign = parse_campaign(manifest())
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory)
            document = native_report([native_scenario("tool_contract_recovery")], partial=True)
            document["persistence_errors"] = ["journal append failed"]
            (output / "results.json").write_text(json.dumps(document), encoding="utf-8")
            scoring = score_campaign(campaign, [{"group_id": "core", "output": str(output)}])
            self.assertEqual(scoring["harness_score"], 90)
            self.assertFalse(scoring["infrastructure_valid"])

    def test_harness_score_is_the_plain_mean_of_the_group_scores(self):
        campaign = parse_campaign(
            manifest(
                [
                    {
                        "id": "first",
                        "execution_kind": "harness_turn",
                        "runs": 1,
                        "technical_retries": 0,
                        "scenarios": ["tool_contract_recovery"],
                    },
                    {
                        "id": "second",
                        "execution_kind": "harness_turn",
                        "runs": 1,
                        "technical_retries": 0,
                        "scenarios": ["performance_regression"],
                    },
                ]
            )
        )
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            groups = []
            for group_id, mean_score in [("first", 80.0), ("second", 100.0)]:
                output = root / group_id
                output.mkdir()
                scenario = native_scenario(f"{group_id}_scenario")
                scenario["aggregate"].update(
                    {
                        "mean_score": mean_score,
                        "execution_reliability": 1.0,
                        "completion_evidence_coverage": 1.0,
                        "completion_rate": 1.0,
                        "total_tokens_consumed": 1200,
                        "tokens_completed_p50": 1200.0,
                        "failed_attempt_tokens": 0,
                        "tokens_per_completion": 1200.0,
                    }
                )
                (output / "results.json").write_text(
                    json.dumps(native_report([scenario])), encoding="utf-8"
                )
                groups.append({"group_id": group_id, "output": str(output)})
            scoring = score_campaign(campaign, groups)
        # No weights: an easy group and a hard group count exactly the same.
        self.assertAlmostEqual(scoring["harness_score"], (80 + 100) / 2)
        self.assertEqual(scoring["scored_groups"], 2)
        self.assertEqual(scoring["expected_groups"], 2)
        self.assertEqual(scoring["score_coverage"], 1.0)
        self.assertEqual(scoring["score_availability"], "complete")

    def test_group_score_and_coverage_come_from_the_native_aggregates(self):
        campaign = parse_campaign(manifest([{
            "id": "core", "execution_kind": "harness_turn", "runs": 1,
            "technical_retries": 0,
            "scenarios": ["tool_contract_recovery", "persistent_state"],
        }]))
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory)
            scored = native_scenario("tool_contract_recovery")
            scored["aggregate"]["mean_score"] = 70.0
            unscored = native_scenario("persistent_state")
            unscored["aggregate"].update({"scored_runs": 0, "mean_score": None})
            (output / "results.json").write_text(
                json.dumps(native_report([scored, unscored])), encoding="utf-8"
            )
            group = [{"group_id": "core", "output": str(output)}]
            scoring = score_campaign(campaign, group)
        # The group score is the mean of the scenarios that reported one.
        self.assertEqual(group[0]["score"], 70.0)
        # Coverage is scored_runs over planned_runs across the group.
        self.assertEqual(group[0]["score_coverage"], 0.5)
        self.assertEqual(group[0]["score_availability"], "partial")
        self.assertEqual(scoring["harness_score"], 70.0)

    def test_a_group_without_a_score_leaves_the_mean_and_makes_it_partial(self):
        campaign = parse_campaign(
            manifest(
                [
                    {
                        "id": "scored",
                        "execution_kind": "harness_turn",
                        "runs": 1,
                        "technical_retries": 0,
                        "scenarios": ["tool_contract_recovery"],
                    },
                    {
                        "id": "unscored",
                        "execution_kind": "harness_turn",
                        "runs": 1,
                        "technical_retries": 0,
                        "scenarios": ["performance_regression"],
                    },
                ]
            )
        )
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            groups = []
            for group_id, deferred in [("scored", False), ("unscored", True)]:
                output = root / group_id
                output.mkdir()
                (output / "results.json").write_text(
                    json.dumps(
                        native_report(
                            [native_scenario(f"{group_id}_scenario", deferred=deferred)],
                            partial=deferred,
                        )
                    ),
                    encoding="utf-8",
                )
                groups.append({"group_id": group_id, "output": str(output)})
            scoring = score_campaign(campaign, groups)
        self.assertIsNone(groups[1]["score"])
        self.assertEqual(groups[1]["score_availability"], "unavailable")
        # The unscored group never dilutes the mean, it only withholds it.
        self.assertEqual(scoring["harness_score"], 90)
        self.assertEqual(scoring["scored_groups"], 1)
        self.assertEqual(scoring["expected_groups"], 2)
        self.assertEqual(scoring["score_availability"], "partial")

    def test_fault_infrastructure_is_null_not_zero_and_reduces_coverage(self):
        campaign = parse_campaign(
            manifest(
                [
                    {
                        "id": "fault",
                        "execution_kind": "fault_injection",
                        "runs": 3,
                        "technical_retries": 0,
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
        self.assertAlmostEqual(scoring["score_coverage"], 1 / 3)
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
