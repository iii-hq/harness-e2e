import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).parents[2]
SCRIPT = ROOT / "scripts" / "exact_stack_campaign.py"
RUNNER_SCRIPT = ROOT / "scripts" / "run_exact_stack_group.sh"
SPEC = importlib.util.spec_from_file_location("exact_stack_campaign", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def campaign_contract(versions: dict[str, str] | None = None):
    versions = versions or {"harness": "1.9.0", "state": "0.22.1"}
    catalog_digest = MODULE.canonical_sha256(catalog())
    target_workers = sorted(versions)
    roots = [
        {"worker": "harness-e2e", "version": "0.5.0-experimental", "role": "runner"},
        {"worker": "fp", "version": "0.2.6", "role": "runtime"},
        {"worker": "harness", "version": versions["harness"], "role": "target"},
    ]
    nodes = []
    all_versions = {
        **versions,
        "fp": "0.2.6",
        "harness-e2e": "0.5.0-experimental",
    }
    for worker, version in sorted(all_versions.items()):
        nodes.append(
            {
                "worker": worker,
                "version": version,
                "kind": "binary",
                "artifact": {
                    "target": "x86_64-unknown-linux-gnu",
                    "url": f"https://registry.example/{worker}/{version}",
                    "sha256": f"sha256:{len(nodes) + 1:064x}",
                },
            }
        )
    edges = sorted(
        [{"from": "harness", "to": worker} for worker in target_workers if worker != "harness"]
        + [{"from": "harness-e2e", "to": "state"}],
        key=lambda edge: (edge["from"], edge["to"]),
    )
    graph = {"roots": roots, "nodes": nodes, "edges": edges}
    return {
        "campaign_id": "11111111-1111-4111-8111-111111111111",
        "execution_id": "22222222-2222-4222-8222-222222222222",
        "attempt": 1,
        "idempotency_key": f"rc:e2e:{'a' * 64}",
        "target": {
            "application": "harness",
            "version": versions["harness"],
            "source_sha": "b" * 40,
            "deployment_id": "33333333-3333-4333-8333-333333333333",
            "origin": None,
            "base": {
                "kind": "deployment",
                "id": "33333333-3333-4333-8333-333333333333",
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
                "progressIntervalSeconds": 15,
                "retentionClass": "longitudinal",
                "executor": {
                    "provider": "github_actions",
                    "repository": "iii-hq/harness-e2e",
                    "workflow": "exact-stack-e2e.yml",
                    "ref": "main",
                    "oidcAudience": "release-control-harness-e2e",
                },
                "runner": {
                    "registryWorker": "harness-e2e",
                    "registryRef": "0.5.0-experimental",
                    "revision": "e" * 40,
                    "catalogSha256": catalog_digest,
                    "manifestSha256": f"sha256:{'3' * 64}",
                    "scoringProfileSha256": f"sha256:{'4' * 64}",
                    "assetsSha256": f"sha256:{'5' * 64}",
                },
                "testRuntime": {
                    "cliVersion": "0.23.0-rc.4",
                    "cliTarget": "x86_64-unknown-linux-gnu",
                    "cliAsset": "iii-x86_64-unknown-linux-gnu.tar.gz",
                    "cliSha256": f"sha256:{'6' * 64}",
                    "workers": {"fp": "0.2.6"},
                },
            },
        },
        "runner": {
            "registry_worker": "harness-e2e",
            "registry_ref": "0.5.0-experimental",
            "revision": "e" * 40,
            "catalog_sha256": catalog_digest,
            "manifest_sha256": f"sha256:{'3' * 64}",
            "scoring_profile_sha256": f"sha256:{'4' * 64}",
            "assets_sha256": f"sha256:{'5' * 64}",
        },
        "workflow": {
            "repository": "iii-hq/harness-e2e",
            "file": "exact-stack-e2e.yml",
            "ref": "main",
        },
        "runtime": {
            "cli": {
                "version": "0.23.0-rc.4",
                "target": "x86_64-unknown-linux-gnu",
                "asset": "iii-x86_64-unknown-linux-gnu.tar.gz",
                "sha256": f"sha256:{'6' * 64}",
            },
        },
        "security": {"oidc_audience": "release-control-harness-e2e"},
        "orchestration": {**graph, "graph_sha256": MODULE.canonical_sha256(graph)},
    }


def catalog():
    return {
        "schema": "e2e-scenario-catalog/v2",
        "runner": {
            "name": "harness-e2e",
            "version": "0.5.0-experimental",
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
class ReleaseControlCampaignTest(unittest.TestCase):
    def test_common_runner_contains_only_the_compose_path(self):
        runner = RUNNER_SCRIPT.read_text()
        self.assertIn("compose::validate", runner)
        self.assertIn("compose::up", runner)
        self.assertIn("compose::status", runner)
        self.assertIn("compose::down", runner)
        self.assertNotIn("iii " + "worker", runner)
        self.assertNotIn("iii-" + "worker", runner)
        self.assertNotIn("iii." + "lock", runner)

    def test_common_runner_uses_isolated_namespaces_and_restricted_secrets(self):
        runner = RUNNER_SCRIPT.read_text()
        self.assertIn("daemon_namespace=", runner)
        self.assertIn("project_namespace=", runner)
        self.assertIn("chmod 600", runner)
        self.assertIn("--namespace \"$project_namespace\"", runner)

    def test_rejects_contract_discriminators(self):
        value = campaign_contract()
        value["schema" + "_version"] = 2
        with self.assertRaisesRegex(ValueError, "unknown fields"):
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
        self.assertEqual(
            request["run_contract"]["plan"]["catalog_sha256"],
            catalog()["catalog_sha256"],
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
        contract["plan"]["definition"]["runner"]["catalogSha256"] = digest
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
        contract["plan"]["definition"]["runner"]["catalogSha256"] = digest
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
        contract["plan"]["definition"]["runner"]["catalogSha256"] = digest
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

    def test_campaign_matrix_keeps_fixture_groups_ephemeral(self):
        value = campaign_contract()
        value["plan"]["definition"]["groups"][0]["scenarios"] = [
            "shell_coder_sandbox",
            "engineering_ticket_git_handoff",
        ]
        common = MODULE.campaign_matrix(value)["include"][0]
        self.assertEqual(common["runs_on"], ["ubuntu-latest"])
        self.assertEqual(
            set(common), {"group_id", "execution_kind", "runs_on"}
        )

    def test_campaign_matrix_keeps_browser_groups_ephemeral(self):
        value = campaign_contract()
        value["plan"]["definition"]["groups"][0]["scenarios"] = [
            "browser_cross_site",
        ]
        common = MODULE.campaign_matrix(value)["include"][0]
        self.assertEqual(common["runs_on"], ["ubuntu-latest"])

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

    def test_rejects_a_tampered_graph_digest(self):
        changed = campaign_contract()
        changed["orchestration"]["graph_sha256"] = f"sha256:{'0' * 64}"
        with self.assertRaisesRegex(ValueError, "graph_sha256 does not match"):
            MODULE.validate_contract(changed)

    def test_materializes_deterministic_compose(self):
        contract = campaign_contract()
        with tempfile.TemporaryDirectory() as directory:
            data_dir = Path(directory).resolve() / "runs"
            first = MODULE.materialize_compose(contract, "project-one", data_dir, {}, {})
            second = MODULE.materialize_compose(contract, "project-one", data_dir, {}, {})
        self.assertEqual(first, second)
        nodes = {
            node["worker"]: node
            for node in contract["orchestration"]["nodes"]
            if node["kind"] == "binary"
        }
        for worker, node in nodes.items():
            container = first["containers"][worker]
            self.assertEqual(container["worker"], f"package://{worker}")
            self.assertEqual(container["version"], node["version"])
            self.assertNotEqual(container["version"], "next")
            self.assertNotIn("scripts", container)
            self.assertNotIn("start_after", container)
        self.assertEqual(first["containers"]["harness-e2e"]["version"], "0.5.0-experimental")
        self.assertEqual(first["containers"]["harness-e2e"]["depends_on"], ["state"])

    def test_rejects_forbidden_artifacts_and_version_conflicts(self):
        forbidden = campaign_contract()
        forbidden["orchestration"]["nodes"][0]["kind"] = "bundle"
        with self.assertRaisesRegex(ValueError, "forbidden kind bundle"):
            MODULE.validate_contract(forbidden)

        conflict = campaign_contract()
        conflict["orchestration"]["nodes"].append(
            {
                **conflict["orchestration"]["nodes"][0],
                "version": "9.9.9",
            }
        )
        conflict["orchestration"]["nodes"].sort(key=lambda node: node["worker"])
        with self.assertRaisesRegex(ValueError, "more than one version"):
            MODULE.validate_contract(conflict)

    def test_compose_evidence_binds_graph_yaml_namespaces_and_lifecycle(self):
        contract = campaign_contract()
        expected = {
            node["worker"]: node["version"]
            for node in contract["orchestration"]["nodes"]
            if node["kind"] == "binary"
        }
        with tempfile.TemporaryDirectory() as directory:
            compose_path = Path(directory) / "worker-compose.yaml"
            compose_path.write_text("namespace: project-one\ncontainers: {}\n")
            evidence = MODULE.compose_evidence(
                contract,
                compose_path,
                "compose-one",
                "project-one",
                {name: {"status": "ok"} for name in ("validate", "up", "status", "down")},
                {
                    "workers": [
                        {"name": worker, "version": version, "namespace": "project-one"}
                        for worker, version in expected.items()
                    ]
                },
                {"before": [], "during": [], "after": []},
            )
        self.assertEqual(evidence["orchestration_graph_sha256"], contract["orchestration"]["graph_sha256"])
        self.assertEqual(evidence["namespaces"], {"daemon": "compose-one", "project": "project-one"})
        self.assertEqual(set(evidence["lifecycle"]), {"validate", "up", "status", "down"})
        self.assertNotIn("schema" + "_version", json.dumps(evidence))

    def test_compose_evidence_rejects_the_removed_lifecycle_executable(self):
        contract = campaign_contract()
        expected = {
            node["worker"]: node["version"]
            for node in contract["orchestration"]["nodes"]
            if node["kind"] == "binary"
        }
        with tempfile.TemporaryDirectory() as directory:
            compose_path = Path(directory) / "worker-compose.yaml"
            compose_path.write_text("namespace: project-one\ncontainers: {}\n")
            with self.assertRaisesRegex(ValueError, "forbidden lifecycle executable"):
                MODULE.compose_evidence(
                    contract,
                    compose_path,
                    "compose-one",
                    "project-one",
                    {name: {} for name in ("validate", "up", "status", "down")},
                    {"workers": [{"name": worker, "version": version} for worker, version in expected.items()]},
                    {
                        "before": [],
                        "during": [{"comm": "iii-" + "worker", "args": "iii-" + "worker"}],
                        "after": [],
                    },
                )

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
