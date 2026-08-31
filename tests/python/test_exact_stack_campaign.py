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
    target_workers = sorted(versions)
    roots = [
        {"worker": "harness-e2e", "version": "0.6.0-experimental", "role": "runner"},
        {"worker": "fp", "version": "0.2.6", "role": "runtime"},
        {"worker": "harness", "version": versions["harness"], "role": "target"},
    ]
    nodes = []
    all_versions = {**versions, "fp": "0.2.6", "harness-e2e": "0.6.0-experimental"}
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
        "schema": "rc-e2e/v2",
        "campaign_id": "11111111-1111-4111-8111-111111111111",
        "execution_id": "22222222-2222-4222-8222-222222222222",
        "attempt": 1,
        "idempotency_key": f"rc:e2e:{'a' * 64}",
        "stack_revision": "b" * 40,
        "orchestration": {**graph, "graph_sha256": MODULE.canonical_sha256(graph)},
        "runtime": {
            "cli": {
                "version": "0.23.0-rc.4",
                "target": "x86_64-unknown-linux-gnu",
                "asset": "iii-x86_64-unknown-linux-gnu.tar.gz",
                "sha256": f"sha256:{'6' * 64}",
            }
        },
        "security": {"oidc_audience": "release-control-harness-e2e"},
        "suite": {
            "id": "daily",
            "label": "Daily campaign",
            "lane": "daily",
            "seed": 4404,
            "progress_interval_seconds": 15,
            "subject": {"provider": "deepseek", "model": "deepseek-v4-flash"},
            "judge": {"provider": "zai", "model": "glm-5.3"},
            "groups": [
                {
                    "id": "daily-core",
                    "execution_kind": "harness_turn",
                    "scenarios": ["direct_answer"],
                    "runs": 1,
                    "technical_retries": 1,
                    "weight": 4,
                },
                {
                    "id": "weekly-fault-l2",
                    "execution_kind": "fault_injection",
                    "runs": 3,
                    "technical_retries": 0,
                    "weight": 2,
                    "fault_profile": "weekly-l2-recovery",
                    "fault_scenario": "stateful.2",
                    "soak_minutes": 60,
                },
            ],
        },
    }


def catalog():
    return {
        "schema": "e2e-scenario-catalog/v4",
        "runner": {
            "name": "harness-e2e",
            "version": "0.6.0-experimental",
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

    def test_a_field_this_version_does_not_know_is_carried_not_rejected(self):
        value = campaign_contract()
        value["suite"]["groups"][0]["timeout_minutes"] = 45
        value["reporting"] = {"channel": "#releases"}
        MODULE.validate_contract(value)

    def test_rejects_a_suite_the_pinned_runner_cannot_execute(self):
        contract = campaign_contract()
        contract["suite"]["groups"][0]["scenarios"] = ["a_scenario_that_never_shipped"]
        with self.assertRaisesRegex(ValueError, "has no scenario a_scenario_that_never_shipped"):
            MODULE.materialize_request(contract, catalog(), group_id="daily-core")

    def test_materializes_one_observe_only_group(self):
        contract = campaign_contract()
        request = MODULE.materialize_request(contract, catalog(), group_id="daily-core")
        self.assertEqual(request["scenarios"], ["direct_answer"])
        self.assertEqual(request["model"], "deepseek-v4-flash")
        self.assertEqual(request["judge_model"], "glm-5.3")
        self.assertEqual(request["run_contract"]["mode"]["decision"], "observe_only")
        self.assertEqual(
            set(request["run_contract"]["plan"]),
            {"id", "revision", "sha256", "catalog_sha256"},
        )
        # The plan the runner records is this contract, and the catalog digest
        # it verifies is the one its own scenarios-list reported.
        self.assertEqual(
            request["run_contract"]["plan"]["sha256"], MODULE.canonical_sha256(contract)
        )
        self.assertEqual(
            request["run_contract"]["plan"]["catalog_sha256"], catalog()["catalog_sha256"]
        )
        self.assertEqual(request["idempotency_key"], MODULE.observation_idempotency_key(request))

    def test_admission_emits_the_workflow_outputs_and_binds_the_dispatch(self):
        contract = campaign_contract()
        outputs = dict(
            line.split("=", 1)
            for line in MODULE.admission_outputs(
                contract,
                "11111111-1111-4111-8111-111111111111",
                "22222222-2222-4222-8222-222222222222",
                1,
                "daily",
            )
        )
        self.assertEqual(set(outputs), {"contract_sha256", "matrix", "oidc_audience"})
        self.assertEqual(outputs["contract_sha256"], MODULE.canonical_sha256(contract))
        self.assertEqual(outputs["oidc_audience"], "release-control-harness-e2e")
        self.assertEqual(json.loads(outputs["matrix"]), MODULE.campaign_matrix(contract))

    def test_admission_rejects_a_dispatch_that_describes_another_campaign(self):
        with self.assertRaisesRegex(ValueError, "dispatch inputs do not describe this contract"):
            MODULE.admission_outputs(
                campaign_contract(),
                "11111111-1111-4111-8111-111111111111",
                "22222222-2222-4222-8222-222222222222",
                1,
                "weekly",
            )

    def test_suite_materializes_the_manifest_the_aggregator_consumes(self):
        manifest = MODULE.campaign_manifest(campaign_contract())
        self.assertEqual(manifest["kind"], "harness-e2e-campaign")
        self.assertEqual(manifest["campaign_id"], "daily")
        self.assertEqual(manifest["lane"], "daily")
        self.assertEqual([group["id"] for group in manifest["groups"]], ["daily-core", "weekly-fault-l2"])
        self.assertEqual(manifest["groups"][0]["difficulty_weight"], 4)
        self.assertEqual(manifest["groups"][0]["scenarios"], ["direct_answer"])
        self.assertEqual(manifest["groups"][1]["fault_profile"], "weekly-l2-recovery")
        self.assertNotIn("scenarios", manifest["groups"][1])

    def test_preserves_catalog_owned_canonical_seed(self):
        changed = catalog()
        canonical_seed = 0x746F6F6C00000001
        changed["scenarios"][0]["seed"] = canonical_seed
        changed["scenarios"][0]["case_id"] = "direct_answer:v2:seed-746f6f6c00000001"
        request = MODULE.materialize_request(campaign_contract(), changed, group_id="daily-core")
        self.assertEqual(request["seed"], 4404)
        self.assertEqual(request["run_contract"]["selected_cases"][0]["seed"], canonical_seed)

    def test_rejects_invalid_catalog_seed(self):
        changed = catalog()
        changed["scenarios"][0]["seed"] = -1
        with self.assertRaisesRegex(ValueError, "non-negative integer"):
            MODULE.materialize_request(campaign_contract(), changed, group_id="daily-core")

    def test_accepts_exact_registry_prerelease_versions(self):
        MODULE.validate_contract(campaign_contract({"harness": "1.9.0-next.5", "state": "0.22.1"}))

    def test_campaign_matrix_isolates_faults_on_the_trusted_runner(self):
        matrix = MODULE.campaign_matrix(campaign_contract())
        self.assertEqual(len(matrix["include"]), 2)
        self.assertEqual(matrix["include"][0]["runs_on"], ["ubuntu-latest"])
        self.assertEqual(matrix["include"][1]["runs_on"], ["self-hosted", "harness-e2e"])

    def test_campaign_matrix_keeps_fixture_groups_ephemeral(self):
        value = campaign_contract()
        value["suite"]["groups"][0]["scenarios"] = [
            "shell_coder_sandbox",
            "engineering_ticket_git_handoff",
        ]
        common = MODULE.campaign_matrix(value)["include"][0]
        self.assertEqual(common["runs_on"], ["ubuntu-latest"])
        self.assertEqual(set(common), {"group_id", "execution_kind", "runs_on"})

    def test_campaign_matrix_keeps_browser_groups_ephemeral(self):
        value = campaign_contract()
        value["suite"]["groups"][0]["scenarios"] = ["browser_cross_site"]
        self.assertEqual(MODULE.campaign_matrix(value)["include"][0]["runs_on"], ["ubuntu-latest"])

    def test_requires_exactly_one_runner_and_the_application_under_test(self):
        missing_runner = campaign_contract()
        for root in missing_runner["orchestration"]["roots"]:
            if root["role"] == "runner":
                root["role"] = "runtime"
        missing_runner["orchestration"]["roots"].sort(
            key=lambda root: (root["role"], root["worker"], root["version"])
        )
        graph = {key: missing_runner["orchestration"][key] for key in ("roots", "nodes", "edges")}
        missing_runner["orchestration"]["graph_sha256"] = MODULE.canonical_sha256(graph)
        with self.assertRaisesRegex(ValueError, "exactly one runner root"):
            MODULE.validate_contract(missing_runner)

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
        self.assertEqual(first["containers"]["harness-e2e"]["version"], "0.6.0-experimental")
        self.assertEqual(first["containers"]["harness-e2e"]["depends_on"], ["state"])
        environment = first["containers"]["harness-e2e"]["environment"]
        self.assertEqual(environment["HARNESS_E2E_STACK_MODE"], "registry")
        self.assertEqual(environment["HARNESS_E2E_WORKERS_REVISION"], "b" * 40)
        self.assertIn("harness", json.loads(environment["HARNESS_E2E_STACK_VERSIONS"]))

    def test_rejects_forbidden_artifacts_and_version_conflicts(self):
        forbidden = campaign_contract()
        forbidden["orchestration"]["nodes"][0]["kind"] = "bundle"
        with self.assertRaisesRegex(ValueError, "forbidden kind bundle"):
            MODULE.validate_contract(forbidden)

        conflict = campaign_contract()
        conflict["orchestration"]["nodes"].append(
            {**conflict["orchestration"]["nodes"][0], "version": "9.9.9"}
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
        self.assertEqual(
            evidence["orchestration_graph_sha256"], contract["orchestration"]["graph_sha256"]
        )
        self.assertEqual(evidence["contract_sha256"], MODULE.canonical_sha256(contract))
        self.assertEqual(evidence["namespaces"], {"daemon": "compose-one", "project": "project-one"})
        self.assertEqual(set(evidence["lifecycle"]), {"validate", "up", "status", "down"})

    def test_compose_evidence_reports_a_missing_container(self):
        contract = campaign_contract()
        with tempfile.TemporaryDirectory() as directory:
            compose_path = Path(directory) / "worker-compose.yaml"
            compose_path.write_text("namespace: project-one\ncontainers: {}\n")
            with self.assertRaisesRegex(ValueError, "missing containers: fp"):
                MODULE.compose_evidence(
                    contract,
                    compose_path,
                    "compose-one",
                    "project-one",
                    {name: {} for name in ("validate", "up", "status", "down")},
                    {
                        "workers": [
                            {"name": node["worker"], "version": node["version"], "namespace": "project-one"}
                            for node in contract["orchestration"]["nodes"]
                            if node["worker"] != "fp"
                        ]
                    },
                    {"before": [], "during": [], "after": []},
                )

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
            manifest = MODULE.package_bundle(root, campaign_contract(), {"run_id": 7, "run_attempt": 1})
        self.assertEqual(manifest["terminal_payload"], "results.json")
        self.assertRegex(manifest["files"][0]["sha256"], r"^sha256:[0-9a-f]{64}$")


if __name__ == "__main__":
    unittest.main()
