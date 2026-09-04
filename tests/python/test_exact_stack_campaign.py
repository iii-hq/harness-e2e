import hashlib
import importlib.util
import json
import os
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path


ROOT = Path(__file__).parents[2]
SCRIPT = ROOT / "scripts" / "exact_stack_campaign.py"
RUNNER_SCRIPT = ROOT / "scripts" / "run_exact_stack_group.sh"
WORKFLOW = ROOT / ".github" / "workflows" / "exact-stack-e2e.yml"
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
        self.assertIn("compose::add", runner)
        self.assertIn("compose::up", runner)
        self.assertIn("compose::status", runner)
        self.assertIn("compose::down", runner)
        self.assertNotIn("iii " + "worker", runner)
        self.assertNotIn("iii-" + "worker", runner)
        self.assertNotIn("iii." + "lock", runner)

    def test_common_runner_restricts_secret_files(self):
        runner = RUNNER_SCRIPT.read_text()
        self.assertIn("chmod 600", runner)
        self.assertLess(runner.index("validate-layout"), runner.index('secrets_dir="$run_root/secrets"'))

    def test_runtime_layout_requires_canonical_disjoint_roots(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            allowed = root / "target"
            artifacts = allowed / "evidence"
            runtime = root / "runtime"
            artifacts.mkdir(parents=True)
            runtime.mkdir()
            MODULE.validate_runtime_layout(artifacts, runtime, allowed)
            for overlapping in [artifacts, artifacts / "runtime", allowed]:
                overlapping.mkdir(exist_ok=True)
                with self.assertRaisesRegex(ValueError, "must not overlap"):
                    MODULE.validate_runtime_layout(artifacts, overlapping, allowed)
            link = root / "runtime-link"
            link.symlink_to(artifacts, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "must not overlap"):
                MODULE.validate_runtime_layout(artifacts, link, allowed)
            outside = root / "outside"
            outside.mkdir()
            with self.assertRaisesRegex(ValueError, "canonical target directory"):
                MODULE.validate_runtime_layout(allowed / ".." / "outside", runtime, allowed)

    def test_common_runner_rejects_uploaded_tmpdir_before_writing_credentials(self):
        (ROOT / "target").mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="artifact-layout-test-", dir=ROOT / "target") as directory:
            artifacts = Path(directory)
            result = subprocess.run(
                ["bash", str(RUNNER_SCRIPT)],
                env={
                    **os.environ,
                    "HARNESS_E2E_ARTIFACTS_DIR": str(artifacts),
                    "HARNESS_E2E_STACK_LOCK": json.dumps(campaign_contract()),
                    "HARNESS_E2E_CAMPAIGN_GROUP_ID": "daily-core",
                    "TMPDIR": str(artifacts),
                    "DEEPSEEK_API_KEY": "fake-secret-never-upload",
                    "ZAI_API_KEY": "fake-secret-never-upload",
                },
                capture_output=True,
                text=True,
                timeout=15,
                check=False,
            )
            self.assertEqual(result.returncode, 2, result.stderr)
            self.assertIn("must not overlap", result.stdout)
            self.assertFalse(list(artifacts.rglob("*.env")))
            self.assertFalse(list(artifacts.glob("harness-e2e-compose.*")))

    def test_uploads_preserve_hidden_files_only_from_validated_packages_or_safe_diagnostics(self):
        workflow = WORKFLOW.read_text()
        for kind in ["group", "root"]:
            step = workflow.split(f"- name: Upload {kind} observation bundle", 1)[1].split("\n      - name:", 1)[0]
            self.assertIn("include-hidden-files: true", step)
            self.assertIn(f"steps.{kind}_package.outcome == 'success'", step)
            self.assertIn(f"steps.{kind}_package_failure.outcome == 'success'", step)
            self.assertIn(f"steps.{kind}_package_failure.outputs.path", step)

    def test_packaging_failure_diagnostic_never_copies_the_unvalidated_tree(self):
        workflow = WORKFLOW.read_text()
        for kind in ["group", "root"]:
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                unvalidated = root / "unvalidated"
                unvalidated.mkdir()
                (unvalidated / ".env").write_text("API_KEY=fake-secret-never-upload\n")
                step = workflow.split(f"- name: Preserve safe {kind} packaging diagnostic", 1)[1].split("\n      - name:", 1)[0]
                self.assertIn(f"steps.{kind}_package.outcome == 'failure'", step)
                script = textwrap.dedent(step.split("run: |\n", 1)[1])
                github_output = root / "github-output"
                subprocess.run(
                    ["bash", "-c", script],
                    cwd=unvalidated,
                    env={**os.environ, "RUNNER_TEMP": str(root), "GITHUB_OUTPUT": str(github_output)},
                    check=True,
                    capture_output=True,
                    text=True,
                )
                diagnostic = Path(github_output.read_text().strip().removeprefix("path="))
                self.assertFalse(diagnostic.is_relative_to(unvalidated))
                self.assertEqual([path.name for path in diagnostic.iterdir()], ["failure.json"])
                failure = json.loads((diagnostic / "failure.json").read_text())
                self.assertEqual(failure["phase"], "artifact_packaging")
                self.assertEqual(failure["outcome"], "infra_failed")
                self.assertNotIn("fake-secret-never-upload", json.dumps(failure))

    def test_common_runner_preserves_native_data_and_awaits_compose_add(self):
        runner = RUNNER_SCRIPT.read_text()
        self.assertIn('e2e_data="$artifact_dir/native"', runner)
        self.assertIn("await_compose_add", runner)
        self.assertIn("compose::operation", runner)
        self.assertNotIn('e2e_data="$run_root/e2e-data"', runner)

    def test_finalizer_uploads_root_evidence_even_after_aggregate_failure(self):
        workflow = WORKFLOW.read_text()
        root_upload = workflow.split("- name: Upload root observation bundle", 1)[1]
        self.assertIn("if: always()", root_upload.split("- name:", 1)[0])
        self.assertIn("group observation artifact was not available", workflow)

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

    def test_scaffold_carries_project_roots_and_execution_config(self):
        contract = campaign_contract()
        with tempfile.TemporaryDirectory() as directory:
            data_dir = Path(directory).resolve() / "runs"
            scaffold = MODULE.project_scaffold(contract, "project-one", data_dir, {}, {})
        roots = {
            root["worker"]: root["version"]
            for root in contract["orchestration"]["roots"]
        }
        self.assertEqual(set(scaffold["containers"]), set(roots))
        for worker, version in roots.items():
            container = scaffold["containers"][worker]
            self.assertEqual(container["worker"], f"package://{worker}")
            self.assertEqual(container["version"], version)
        environment = scaffold["containers"]["harness-e2e"]["environment"]
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

    def test_compose_evidence_binds_graph_yaml_namespace_and_lifecycle(self):
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
                "project-one",
                {name: {"status": "ok"} for name in ("add", "up", "status", "down")},
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
        self.assertEqual(evidence["namespace"], "project-one")
        self.assertEqual(set(evidence["lifecycle"]), {"add", "up", "status", "down"})
        self.assertEqual(
            evidence["runtime"]["requested_roots"],
            {"fp": "0.2.6", "harness": "1.9.0", "harness-e2e": "0.6.0-experimental"},
        )
        self.assertEqual(evidence["runtime"]["observed_versions"]["state"], "0.22.1")

    def test_compose_evidence_reports_a_missing_container(self):
        contract = campaign_contract()
        with tempfile.TemporaryDirectory() as directory:
            compose_path = Path(directory) / "worker-compose.yaml"
            compose_path.write_text("namespace: project-one\ncontainers: {}\n")
            with self.assertRaisesRegex(ValueError, "missing requested roots: fp"):
                MODULE.compose_evidence(
                    contract,
                    compose_path,
                    "project-one",
                    {name: {} for name in ("add", "up", "status", "down")},
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
                    "project-one",
                    {name: {} for name in ("add", "up", "status", "down")},
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

    def test_package_preserves_native_journal_without_terminal_results(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            event = root / "native/executions/run-1/journal/events/00000001.json"
            event.parent.mkdir(parents=True)
            event.write_text('{"event":"RunCommitted"}\n')
            manifest = MODULE.package_bundle(
                root, campaign_contract(), {"run_id": 7, "run_attempt": 1}
            )
        self.assertIsNone(manifest["terminal_payload"])
        self.assertIn(
            "native/executions/run-1/journal/events/00000001.json",
            [entry["path"] for entry in manifest["files"]],
        )

    def test_group_and_root_packages_preserve_hidden_checkpoint_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            group = root / "groups" / "daily-core"
            checkpoint = group / "native/executions/.workflow-state/workflow-resume/state-v1.json"
            checkpoint.parent.mkdir(parents=True)
            payload = b'{"state_sha256":"sha256:checkpoint","state":{"sequence":3}}\n'
            checkpoint.write_bytes(payload)
            for package_root in [group, root]:
                manifest = MODULE.package_bundle(package_root, campaign_contract(), {})
                reference = next(entry for entry in manifest["files"] if entry["path"].endswith("state-v1.json"))
                self.assertEqual(reference["path"], checkpoint.relative_to(package_root).as_posix())
                self.assertEqual(reference["sha256"], f"sha256:{hashlib.sha256(payload).hexdigest()}")
                self.assertEqual(reference["size_bytes"], len(payload))
                self.assertEqual(checkpoint.read_bytes(), payload)

    def test_package_rejects_credential_paths_instead_of_silently_omitting_them(self):
        for relative in [".env", ".env.local", "provider-zai.env", "secrets/key", ".aws/credentials", ".ssh/id_ed25519", ".gnupg/key"]:
            with self.subTest(relative=relative), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                credential = root / relative
                credential.parent.mkdir(parents=True, exist_ok=True)
                credential.write_text("fake-secret-never-upload\n")
                with self.assertRaisesRegex(ValueError, "reserved credential path"):
                    MODULE.package_bundle(root, campaign_contract(), {})

    def test_separate_runtime_secrets_are_not_part_of_the_artifact_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifacts = root / "target" / "evidence"
            artifacts.mkdir(parents=True)
            secrets = root / "runtime" / "secrets"
            secrets.mkdir(parents=True)
            (secrets / "provider-zai.env").write_text("ZAI_API_KEY=fake-secret-never-upload\n")
            (artifacts / "failure.json").write_text('{"outcome":"infra_failed"}\n')
            MODULE.validate_runtime_layout(artifacts, secrets.parent, root / "target")
            manifest = MODULE.package_bundle(artifacts, campaign_contract(), {})
            self.assertEqual([entry["path"] for entry in manifest["files"]], ["failure.json"])

    def test_package_rejects_symlinks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            outside = root.parent / f"{root.name}-outside.json"
            outside.write_text("{}\n")
            try:
                (root / "linked.json").symlink_to(outside)
                with self.assertRaisesRegex(ValueError, "contains symlink"):
                    MODULE.package_bundle(root, campaign_contract(), {})
            finally:
                outside.unlink(missing_ok=True)


if __name__ == "__main__":
    unittest.main()
