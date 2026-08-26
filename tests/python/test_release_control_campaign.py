import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).parents[2]
SCRIPT = ROOT / "scripts" / "release_control_campaign.py"
RUNNER_SCRIPT = ROOT / "scripts" / "run_release_control_group.sh"
SPEC = importlib.util.spec_from_file_location("release_control_campaign", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def campaign_contract(versions: dict[str, str] | None = None):
    versions = versions or {"harness": "1.9.0", "state": "0.22.1"}
    stack_digest = MODULE.canonical_sha256(versions)
    catalog_digest = MODULE.canonical_sha256(catalog())
    provenance = [
        {"worker": worker, "version": version}
        for worker, version in sorted(versions.items())
    ]
    harness = next(item for item in provenance if item["worker"] == "harness")
    harness.update(
        {
            "source_sha": "b" * 40,
            "operation_id": "33333333-3333-4333-8333-333333333333",
        }
    )
    return {
        "schema_version": 3,
        "campaign_id": "11111111-1111-4111-8111-111111111111",
        "execution_id": "22222222-2222-4222-8222-222222222222",
        "attempt": 1,
        "idempotency_key": f"rc:e2e:{'a' * 64}",
        "target": {
            "application": "harness",
            "version": versions["harness"],
            "source_sha": "b" * 40,
            "deployment_id": "33333333-3333-4333-8333-333333333333",
            "stack_versions": versions,
            "stack_digest": stack_digest,
            "origin": None,
            "base": {
                "kind": "deployment",
                "id": "33333333-3333-4333-8333-333333333333",
            },
            "stack": {
                "requested_versions": versions,
                "resolved_versions": versions,
                "resolution_sha256": stack_digest,
                "provenance": provenance,
            },
        },
        "plan": {
            "id": "44444444-4444-4444-8444-444444444444",
            "revision": 3,
            "sha256": f"sha256:{'c' * 64}",
            "definition": {
                "mode": "campaign",
                "entrypoint": "e2e::run",
                "label": "Daily campaign",
                "lane": "daily",
                "failurePolicy": "advisory",
                "subject": {"provider": "deepseek", "model": "deepseek-v4-flash"},
                "judge": {"provider": "zai", "model": "glm-5.3"},
                "manifest": {"id": "daily", "sha256": f"sha256:{'3' * 64}"},
                "scoring": {
                    "profile": "difficulty-weighted-v1",
                    "sha256": f"sha256:{'4' * 64}",
                },
                "catalog": {
                    "revision": "catalog-1",
                    "sha256": catalog_digest,
                    "seed": 4404,
                },
                "groups": [
                    {
                        "id": "daily-core",
                        "executionKind": "harness_turn",
                        "scenarios": ["direct_answer"],
                        "runs": 1,
                        "technicalRetries": 1,
                        "difficultyTier": "L4",
                        "difficultyWeight": 4,
                    },
                    {
                        "id": "weekly-fault-l2",
                        "executionKind": "fault_injection",
                        "scenarios": [],
                        "runs": 3,
                        "technicalRetries": 0,
                        "difficultyTier": "L2",
                        "difficultyWeight": 2,
                        "faultProfile": "weekly-l2-recovery",
                        "faultScenario": "stateful.2",
                        "soakMinutes": 60,
                    },
                ],
            },
        },
        "runner": {
            "registry_worker": "harness-e2e",
            "registry_ref": "0.2.1-experimental",
            "revision": "e" * 40,
            "catalog_sha256": catalog_digest,
            "manifest_sha256": f"sha256:{'3' * 64}",
            "scoring_profile_sha256": f"sha256:{'4' * 64}",
            "assets_sha256": f"sha256:{'5' * 64}",
        },
        "workflow": {
            "repository": "iii-hq/harness-e2e",
            "file": "release-control-campaign.yml",
            "ref": "main",
        },
        "runtime": {
            "cli": {"version": "0.22.1"},
            "stack_versions": {"fp": "0.2.6"},
            "stack_digest": MODULE.canonical_sha256({"fp": "0.2.6"}),
        },
        "security": {"oidc_audience": "release-control-harness-e2e"},
    }


def catalog():
    return {
        "schema": "e2e-scenario-catalog/v2",
        "runner": {
            "name": "harness-e2e",
            "version": "0.2.1-experimental",
            "revision": "e" * 40,
        },
        "catalog_sha256": f"sha256:{'f' * 64}",
        "scenarios": [
            {
                "scenario_id": "direct_answer",
                "scenario_version": 2,
                "case_id": "direct_answer:4404",
                "seed": 4404,
                "inputs_sha256": f"sha256:{'1' * 64}",
                "contract_sha256": f"sha256:{'2' * 64}",
            }
        ],
    }


def write_lock(path: Path, workers: dict[str, dict[str, str]]):
    path.write_text(json.dumps({"version": 1, "workers": workers}))


class ReleaseControlCampaignTest(unittest.TestCase):
    def test_common_runner_contains_only_the_exact_v3_path(self):
        runner = RUNNER_SCRIPT.read_text()
        self.assertIn('export III_CONFIG_PATH="$project_config"', runner)
        self.assertIn(
            'install_exact_stack stack-bootstrap "$exact_stack_versions" false',
            runner,
        )
        self.assertIn(
            'install_exact_stack stack-repin "$exact_stack_versions" false',
            runner,
        )
        self.assertIn("Release Control campaign schema v3 is required", runner)
        self.assertNotIn("verify_registry_lock.py", runner)
        self.assertNotIn("III_CLI_CHANNEL", runner)

    def test_common_runner_waits_for_a_stable_persistence_plane(self):
        runner = RUNNER_SCRIPT.read_text()
        self.assertIn("HARNESS_E2E_STACK_SETTLE_SECONDS", runner)
        self.assertIn("HARNESS_E2E_ADMISSION_TIMEOUT_SECONDS", runner)
        self.assertIn("state::list state::get state::set", runner)
        self.assertIn(
            "storage::putObject storage::getObject database::execute database::query",
            runner,
        )
        self.assertIn("verify_target_harness_runtime", runner)

    def test_rejects_legacy_contracts(self):
        value = campaign_contract()
        value["schema_version"] = 2
        with self.assertRaisesRegex(ValueError, "schema_version must be 3"):
            MODULE.validate_contract(value)

    def test_materializes_one_observe_only_group(self):
        self.assertNotEqual(
            catalog()["catalog_sha256"],
            campaign_contract()["runner"]["catalog_sha256"],
        )
        request = MODULE.materialize_request(
            campaign_contract(), catalog(), group_id="daily-core"
        )
        self.assertEqual(request["scenarios"], ["direct_answer"])
        self.assertEqual(
            set(request["run_contract"]["plan"]),
            {"id", "revision", "sha256", "catalog_sha256"},
        )
        self.assertEqual(request["run_contract"]["mode"]["decision"], "observe_only")
        self.assertEqual(
            request["idempotency_key"], MODULE.observation_idempotency_key(request)
        )

    def test_keeps_legacy_v1_catalogs_readable(self):
        legacy = catalog()
        legacy["schema"] = "e2e-scenario-catalog/v1"
        contract = campaign_contract()
        digest = MODULE.canonical_sha256(legacy)
        contract["plan"]["definition"]["catalog"]["sha256"] = digest
        contract["runner"]["catalog_sha256"] = digest
        request = MODULE.materialize_request(
            contract, legacy, group_id="daily-core"
        )
        self.assertEqual(request["scenarios"], ["direct_answer"])

    def test_rejects_unknown_catalog_schema(self):
        changed = catalog()
        changed["schema"] = "e2e-scenario-catalog/v3"
        with self.assertRaisesRegex(ValueError, "unsupported scenario catalog schema"):
            MODULE.materialize_request(
                campaign_contract(), changed, group_id="daily-core"
            )

    def test_preserves_catalog_owned_canonical_seed(self):
        changed = catalog()
        canonical_seed = 0x746F6F6C00000001
        changed["scenarios"][0]["seed"] = canonical_seed
        changed["scenarios"][0]["case_id"] = (
            "direct_answer:v2:seed-746f6f6c00000001"
        )
        contract = campaign_contract()
        digest = MODULE.canonical_sha256(changed)
        contract["plan"]["definition"]["catalog"]["sha256"] = digest
        contract["runner"]["catalog_sha256"] = digest
        request = MODULE.materialize_request(
            contract, changed, group_id="daily-core"
        )
        self.assertEqual(request["seed"], 4404)
        self.assertEqual(
            request["run_contract"]["selected_cases"][0]["seed"], canonical_seed
        )

    def test_rejects_invalid_catalog_seed(self):
        changed = catalog()
        changed["scenarios"][0]["seed"] = -1
        contract = campaign_contract()
        digest = MODULE.canonical_sha256(changed)
        contract["plan"]["definition"]["catalog"]["sha256"] = digest
        contract["runner"]["catalog_sha256"] = digest
        with self.assertRaisesRegex(ValueError, "invalid seed"):
            MODULE.materialize_request(
                contract, changed, group_id="daily-core"
            )

    def test_accepts_exact_registry_prerelease_versions(self):
        MODULE.validate_contract(
            campaign_contract({"harness": "1.9.0-next.5", "state": "0.22.1"})
        )

    def test_campaign_matrix_isolates_faults_on_the_trusted_runner(self):
        matrix = MODULE.campaign_matrix(campaign_contract())
        self.assertEqual(len(matrix["include"]), 2)
        self.assertEqual(matrix["include"][0]["runs_on"], ["ubuntu-latest"])
        self.assertEqual(
            matrix["include"][1]["runs_on"], ["self-hosted", "harness-e2e"]
        )

    def test_rejects_a_foreign_executor_repository(self):
        value = campaign_contract()
        value["workflow"]["repository"] = "iii-hq/workers"
        with self.assertRaisesRegex(ValueError, "workflow.repository"):
            MODULE.validate_contract(value)

    def test_rejects_fault_retries_and_weight_drift(self):
        retries = campaign_contract()
        retries["plan"]["definition"]["groups"][1]["technicalRetries"] = 1
        with self.assertRaisesRegex(ValueError, "fault injection requires"):
            MODULE.validate_contract(retries)

        weight = campaign_contract()
        weight["plan"]["definition"]["groups"][0]["difficultyWeight"] = 3
        with self.assertRaisesRegex(ValueError, "does not match L4"):
            MODULE.validate_contract(weight)

    def test_rejects_a_tampered_stack_digest(self):
        changed = campaign_contract()
        changed["target"]["stack"]["resolution_sha256"] = f"sha256:{'0' * 64}"
        with self.assertRaisesRegex(ValueError, "resolution_sha256 does not match"):
            MODULE.validate_contract(changed)

    def test_verify_lock_records_engine_managed_workers(self):
        value = campaign_contract(
            {
                "harness": "1.9.0",
                "iii-observability": "0.22.1",
                "state": "0.22.1",
            }
        )
        with tempfile.TemporaryDirectory() as directory:
            lock_path = Path(directory) / "iii.lock"
            write_lock(
                lock_path,
                {
                    "harness": {"version": "1.9.0", "type": "binary"},
                    "iii-observability": {
                        "version": "0.21.8",
                        "type": "engine",
                    },
                    "state": {"version": "0.22.1", "type": "binary"},
                    "fp": {"version": "0.2.6", "type": "binary"},
                    "harness-e2e": {
                        "version": "0.2.1-experimental",
                        "type": "binary",
                    },
                },
            )
            manifest = MODULE.verify_lock(value, lock_path)
        observed = manifest["verification"]["engine_managed"]["workers"][
            "iii-observability"
        ]
        self.assertEqual(observed["declared_version"], "0.22.1")
        self.assertEqual(observed["observed_version"], "0.21.8")

    def test_verify_lock_rejects_registry_version_drift(self):
        value = campaign_contract()
        with tempfile.TemporaryDirectory() as directory:
            lock_path = Path(directory) / "iii.lock"
            write_lock(
                lock_path,
                {
                    "harness": {"version": "1.9.0", "type": "binary"},
                    "state": {"version": "0.22.0", "type": "binary"},
                    "fp": {"version": "0.2.6", "type": "binary"},
                    "harness-e2e": {
                        "version": "0.2.1-experimental",
                        "type": "binary",
                    },
                },
            )
            with self.assertRaisesRegex(ValueError, "stack_version_mismatch: state"):
                MODULE.verify_lock(value, lock_path)

    def test_packages_raw_file_digests(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "results.json").write_text("{}\n")
            manifest = MODULE.package_bundle(
                root, campaign_contract(), {"run_id": 7, "run_attempt": 1}
            )
        self.assertEqual(manifest["terminal_payload"], "results.json")
        self.assertRegex(manifest["files"][0]["sha256"], r"^sha256:[0-9a-f]{64}$")


if __name__ == "__main__":
    unittest.main()
