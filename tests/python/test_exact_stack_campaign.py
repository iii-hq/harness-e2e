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
BASE_COMPOSE = ROOT / "worker-compose.base.yaml"
SPEC = importlib.util.spec_from_file_location("exact_stack_campaign", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)

import yaml

def declared_base():
    """The stack this repository declares, as every execution starts from it."""
    return yaml.safe_load(BASE_COMPOSE.read_text())


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
            "groups": [
                {
                    "id": "daily-core",
                    "execution_kind": "harness_turn",
                    "scenarios": ["direct_answer"],
                    "runs": 1,
                    "technical_retries": 1,
                },
                {
                    "id": "weekly-fault-l2",
                    "execution_kind": "fault_injection",
                    "runs": 3,
                    "technical_retries": 0,
                    "fault_profile": "weekly-l2-recovery",
                    "fault_scenario": "stateful.2",
                    "soak_minutes": 60,
                },
            ],
        },
    }


def catalog():
    return {
        "schema": "e2e-scenario-catalog",
        "runner": {
            "name": "harness-e2e",
            "version": "0.6.0-experimental",
            "revision": "e" * 40,
        },
        "catalog_sha256": f"sha256:{'f' * 64}",
        "scenarios": [
            {
                "scenario_id": "direct_answer",
                "behavior_sha256": "sha256:" + "c" * 64,
                "case_id": "direct_answer:4404",
                "seed": 4404,
                "inputs_sha256": f"sha256:{'1' * 64}",
                "contract_sha256": f"sha256:{'2' * 64}",
            }
        ],
    }


class ReleaseControlCampaignTest(unittest.TestCase):
    def test_execution_template_is_global_without_changing_scenarios_or_agent_admission(self):
        contract = campaign_contract()
        baseline = MODULE.materialize_request(contract, catalog(), group_id="daily-core")
        contract["runtime"]["template"] = {
            "id": "harness", "repository": "iii-hq/templates", "ref": "main", "revision": "c" * 40,
        }
        MODULE.validate_contract(contract)
        self.assertEqual(MODULE.group_template(contract, "daily-core"), "harness")
        selected = MODULE.materialize_request(contract, catalog(), group_id="daily-core")
        self.assertEqual(selected["scenarios"], baseline["scenarios"])
        self.assertNotIn("agent", selected)
        contract["suite"]["agent_profile"] = "tech-lead"
        self.assertEqual(MODULE.materialize_request(contract, catalog(), group_id="daily-core")["agent"], "tech-lead")
        group = contract["suite"]["groups"][0]
        group.update(scenarios=["linkly_tutorial"], execution_kind="scripted_dialogue", technical_retries=0)
        self.assertEqual(MODULE.group_template(contract, group["id"]), "linkly-agentic")
        for field, value in [("id", "../harness"), ("revision", "main"), ("repository", "other/repo"), ("ref", "feature")]:
            bad = json.loads(json.dumps(contract))
            bad["runtime"]["template"][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                MODULE.validate_contract(bad)

    def test_local_workers_and_named_package_instances_survive_scaffolding(self):
        contract = campaign_contract()
        template = {"containers": {
            "subject": {"worker": "package://harness", "version": "latest"},
            "link": {"worker": "path://./link", "scripts": {"run": "pnpm dev"}},
        }}
        project = MODULE.project_scaffold(contract, "project-one", Path("/data"), {}, {}, template)
        self.assertNotIn("harness", project["containers"])
        self.assertEqual(project["containers"]["subject"]["version"], "latest")
        self.assertEqual(project["containers"]["link"], template["containers"]["link"])
        template["containers"]["link"]["worker"] = "path://../elsewhere"
        with self.assertRaisesRegex(ValueError, "inside its project"):
            MODULE.project_scaffold(contract, "project-one", Path("/data"), {}, {}, template)

    def test_fixture_retains_its_roles_while_execution_template_supplies_the_base(self):
        template = {"engine": {"workers": {"base": {}}}, "containers": {
            "ide": {"worker": "package://ide"}, "kanban": {"worker": "package://kanban"},
        }}
        fixture = {"engine": {"workers": {"iii-stream": {}}}, "containers": {
            "shell": {"worker": "package://shell", "working_dir": "."},
        }}
        merged = MODULE.with_fixture(template, fixture)
        self.assertEqual(set(merged["containers"]), {"shell", "kanban"})
        self.assertEqual(merged["containers"]["shell"], fixture["containers"]["shell"])
        self.assertEqual(set(merged["engine"]["workers"]), {"base", "iii-stream"})
        self.assertIn("ide", template["containers"])

    def test_selected_assets_are_isolated_and_router_gets_the_provider_secret_files(self):
        contract = campaign_contract({
            "harness": "1.9.0", "state": "0.22.1", "iii-directory": "1.2.0",
            "llm-router": "1.4.0", "provider-deepseek": "0.1.0",
        })
        template = {"containers": {worker: {"worker": f"package://{worker}"}
                    for worker in ("harness", "state", "iii-directory", "llm-router", "provider-deepseek")}}
        project = MODULE.project_scaffold(contract, "project-one", Path("/data"),
            {"provider-deepseek": "/private/provider.env"}, {}, template, profile_root=Path("/isolated/project"))
        directory = project["containers"]["iii-directory"]["config_override"]
        self.assertFalse(directory["auto_download"])
        self.assertEqual(directory["agents_folder"], "/isolated/project/agents")
        self.assertEqual(directory["skills_folder"], "/isolated/project/.iii/registry-skills")
        self.assertEqual(directory["local_skills_folder"], "/isolated/project/skills")
        self.assertTrue(directory["global_agents_folder"].startswith("/isolated/project/"))
        self.assertEqual(project["containers"]["llm-router"]["env_file"], ["/private/provider.env"])

    def test_pinned_downloads_preserve_template_profiles_and_fail_on_real_errors(self):
        source = RUNNER_SCRIPT.read_text()
        start = source.index('if [[ "$profile_assets" == true ]]; then', source.index('failure_phase=project_start'))
        block = source[start:source.index("if jq -e '.suite.agent_profile != null'", start)]
        for mode in ("ok", "missing", "broken"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / "project/agents").mkdir(parents=True)
                (root / "template-assets/agents").mkdir(parents=True)
                (root / "template-assets/agents/tech-lead.md").write_text("selected template")
                (root / "contract.json").write_text("{}")
                # The declaration names the workers whose skills are pinned.
                (root / "artifacts/stack").mkdir(parents=True)
                (root / "artifacts/stack/declared-workers.json").write_text(
                    json.dumps([{"worker": "harness", "version": "1.2.3"}])
                )
                shell = '''set -Eeuo pipefail
run_root=$1
artifact_dir=$1/artifacts
project_dir=$1/project
contract_path=$1/contract.json
compose_file=$1/worker-compose.yaml
contract_tool="$2"
profile_assets=true
fail() { printf '%s\\n' "$1" >&2; return 1; }
project_trigger() {
  test "$1" = directory::skills::download_from_registry
  test "$(jq -r '.version' <<<"$2")" = 1.2.3
  if [[ "$MODE" == missing ]]; then echo 'D310 not_found: registry worker "harness" has no published skills bundle.' >&2; return 1; fi
  if [[ "$MODE" == broken ]]; then echo 'registry unavailable' >&2; return 1; fi
  printf 'downloaded profile' >"$project_dir/agents/tech-lead.md"
  printf '{"source":{"version":"1.2.3"}}\\n'
}
'''
                result = subprocess.run(["bash", "-c", shell + block, "runner", str(root), str(SCRIPT)],
                    env={**os.environ, "MODE": mode}, capture_output=True, text=True)
                if mode == "broken":
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("Could not load pinned skills", result.stderr)
                else:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual((root / "project/agents/tech-lead.md").read_text(), "selected template")

    def test_protected_faults_reject_overrides_instead_of_ignoring_them(self):
        source = (ROOT / "scripts/run_exact_stack_fault.sh").read_text()
        guard = source.index(".runtime.template != null or .suite.agent_profile != null")
        self.assertLess(guard, source.index('test -x "$supervisor"'))
        self.assertIn("does not support execution template or agent profile overrides", source)

    def test_agent_profile_is_validated_and_reaches_native_admission(self):
        contract = campaign_contract()
        original = MODULE.materialize_request(contract, catalog(), group_id="daily-core")
        contract["suite"]["agent_profile"] = "console-ui"
        MODULE.validate_suite(contract["suite"])
        request = MODULE.materialize_request(contract, catalog(), group_id="daily-core")
        self.assertEqual(request["agent"], "console-ui")
        self.assertNotEqual(request["idempotency_key"], original["idempotency_key"])
        self.assertNotIn("agent_profile", request)
        for invalid in ["../reviewer", " ", {"id": "reviewer", "content": "body"}]:
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                MODULE.validate_suite({**contract["suite"], "agent_profile": invalid})

    def test_runner_waits_for_an_existing_profile_and_fails_if_it_stays_missing(self):
        source = RUNNER_SCRIPT.read_text()
        block = source[source.index("if jq -e '.suite.agent_profile != null'"):source.index("failure_phase=runner_readiness")]
        for available_after in [0, 1, 99]:
            with self.subTest(available_after=available_after), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / "stack").mkdir()
                (root / "contract.json").write_text(json.dumps({"suite": {"agent_profile": "console-ui"}}))
                shell = '''set -Eeuo pipefail
artifact_dir=$1
contract_path=$1/contract.json
attempts=0
sleep() { SECONDS=$((SECONDS + 60)); }
fail() { printf '%s\\n' "$1" >&2; return 1; }
project_trigger() {
  jq -cn --arg function "$1" --argjson payload "$2" '{function:$function,payload:$payload}' >>"$artifact_dir/calls.jsonl"
  attempts=$((attempts + 1))
  if ((attempts <= AVAILABLE_AFTER)); then return 1; fi
  printf '{"id":"console-ui"}\\n'
}
'''
                result = subprocess.run(["bash", "-c", shell + block, "runner", str(root), str(SCRIPT)],
                                        env={**os.environ, "AVAILABLE_AFTER": str(available_after)},
                                        capture_output=True, text=True)
                calls = [json.loads(line) for line in (root / "calls.jsonl").read_text().splitlines()]
                self.assertTrue(all(call == {"function": "directory::agents::get", "payload": {"id": "console-ui"}} for call in calls))
                self.assertEqual(len(calls), min(available_after + 1, 3))
                if available_after == 99:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("Directory profile console-ui is unavailable after 120s", result.stderr)
                else:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(json.loads((root / "stack/agent-profile.json").read_text()), {"id": "console-ui"})

    def test_linkly_requires_a_fresh_group_for_its_whole_dialogue(self):
        contract = campaign_contract()
        group = contract['suite']['groups'][0]
        group.update(scenarios=['linkly_tutorial'], execution_kind='scripted_dialogue', technical_retries=0)
        self.assertEqual(MODULE.group_template(contract, group['id']), 'linkly-agentic')
        for overrides in ({'runs': 2}, {'technical_retries': 1}, {'scenarios': ['linkly_tutorial', 'direct_answer']}):
            with self.subTest(overrides=overrides), self.assertRaisesRegex(ValueError, 'fresh'):
                changed = json.loads(json.dumps(contract))
                changed['suite']['groups'][0].update(overrides)
                MODULE.group_template(changed, group['id'])
        self.assertEqual(MODULE.group_template(campaign_contract(), 'daily-core'), '')

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

    def test_common_runner_keeps_grading_files_outside_the_subject_project(self):
        runner = RUNNER_SCRIPT.read_text()
        self.assertIn('evaluation_dir="$run_root/evaluation"', runner)
        self.assertIn('harness-e2e.HARNESS_E2E_RUN_DIR=$evaluation_dir', runner)
        self.assertNotIn('harness-e2e.HARNESS_E2E_RUN_DIR=$project_dir', runner)

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

    def test_common_runner_waits_for_the_runner_function_after_compose_starts(self):
        runner = RUNNER_SCRIPT.read_text()
        self.assertIn("failure_phase=runner_readiness", runner)
        self.assertIn("E2E runner did not register e2e::scenarios-list", runner)
        self.assertLess(
            runner.index("failure_phase=runner_readiness"),
            runner.index("failure_phase=materialization"),
        )

    def test_common_runner_reports_terminal_failure_and_keeps_partial_results(self):
        runner = RUNNER_SCRIPT.read_text()
        results_block = runner.split("terminal_phase=$(", 1)[1].split(
            "\nfailure_phase=compose_down", 1
        )[0]
        results_block = "terminal_phase=$(" + results_block
        shell = """set -Eeuo pipefail
artifact_dir=$1
e2e_data="$artifact_dir/native"
repo_root=$2
remote_execution_id=execution-1
project_trigger() {
  if [[ "$1" == "e2e::results-get" ]]; then
    printf '%s\n' "$RESULTS_RESPONSE"
  else
    return 1
  fi
}
fail() {
  printf '[FAIL] %s\n' "$1" >&2
  return 1
}
""" + results_block

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifacts = root / "artifacts"
            native = artifacts / "native/execution-1"
            native.mkdir(parents=True)
            (artifacts / "logs").mkdir()
            (artifacts / "status.json").write_text(
                json.dumps(
                    {
                        "execution_id": "execution-1",
                        "phase": "failed",
                        "terminal": True,
                        "error": "shell preflight failed",
                    }
                )
            )
            journal = native / "journal/events/00000001.json"
            journal.parent.mkdir(parents=True)
            journal.write_text('{"event":"RunFailed"}\n')

            fake_repo = root / "repo/scripts"
            fake_repo.mkdir(parents=True)
            (fake_repo / "extract_swe_reports.py").write_text(
                "import pathlib, sys\n"
                "pathlib.Path(sys.argv[sys.argv.index('--output-dir') + 1]).mkdir(parents=True, exist_ok=True)\n"
                "print('{}')\n"
            )

            for invalid_path in (None, 7, {}):
                with self.subTest(result_path=invalid_path):
                    missing = subprocess.run(
                        ["bash", "-c", shell, "runner", str(artifacts), str(fake_repo.parent)],
                        env={
                            **os.environ,
                            "RESULTS_RESPONSE": json.dumps(
                                {
                                    "execution_id": "execution-1",
                                    "phase": "failed",
                                    "result_path": invalid_path,
                                    "observation": {"outcome": {"error": "shell preflight failed"}},
                                }
                            ),
                        },
                        capture_output=True,
                        text=True,
                        check=False,
                    )
                    self.assertNotEqual(missing.returncode, 0)
                    self.assertIn("[FAIL] shell preflight failed", missing.stderr)
            self.assertTrue(journal.is_file())

            payloads = {
                "results.json": b'{"result":"partial"}\n',
                "manifest.json": b'{"manifest":"partial"}\n',
                "observation.json": b'{"outcome":{"error":"shell preflight failed"}}\n',
            }
            for name, payload in payloads.items():
                (native / name).write_bytes(payload)
            partial_response = {
                "execution_id": "execution-1",
                "phase": "failed",
                "result_path": "execution-1/results.json",
                "observation": {
                    "evidence": {
                        "results_sha256": f"sha256:{hashlib.sha256(payloads['results.json']).hexdigest()}",
                        "manifest_sha256": f"sha256:{hashlib.sha256(payloads['manifest.json']).hexdigest()}",
                    }
                },
            }
            partial = subprocess.run(
                ["bash", "-c", shell, "runner", str(artifacts), str(fake_repo.parent)],
                env={**os.environ, "RESULTS_RESPONSE": json.dumps(partial_response)},
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertNotEqual(partial.returncode, 0)
            self.assertIn("[FAIL] shell preflight failed", partial.stderr)
            for name, payload in payloads.items():
                self.assertEqual((artifacts / name).read_bytes(), payload)

            (artifacts / "status.json").write_text(json.dumps({"phase": "completed"}))
            completed = subprocess.run(
                ["bash", "-c", shell, "runner", str(artifacts), str(fake_repo.parent)],
                env={**os.environ, "RESULTS_RESPONSE": json.dumps(partial_response)},
                capture_output=True,
                text=True,
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)

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

    def test_suite_materializes_the_manifest_the_aggregator_consumes(self):
        manifest = MODULE.campaign_manifest(campaign_contract())
        self.assertEqual(manifest["kind"], "harness-e2e-campaign")
        self.assertEqual(manifest["campaign_id"], "daily")
        self.assertEqual(manifest["lane"], "daily")
        self.assertEqual([group["id"] for group in manifest["groups"]], ["daily-core", "weekly-fault-l2"])
        # Every case counts the same: no weight and no profile travel.
        self.assertEqual(
            sorted(manifest),
            ["campaign_id", "failure_policy", "groups", "kind", "lane"],
        )
        self.assertEqual(
            sorted(manifest["groups"][0]),
            ["execution_kind", "id", "runs", "scenarios", "technical_retries"],
        )
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

    def test_scaffold_carries_project_roots_and_execution_config(self):
        contract = campaign_contract()
        with tempfile.TemporaryDirectory() as directory:
            data_dir = Path(directory).resolve() / "runs"
            scaffold = MODULE.project_scaffold(contract, "project-one", data_dir, {}, {})
        declared = declared_base()["containers"]
        self.assertEqual(set(scaffold["containers"]), set(declared))
        for worker, container in declared.items():
            scaffolded = scaffold["containers"][worker]
            self.assertEqual(scaffolded["worker"], f"package://{worker}")
            self.assertEqual(scaffolded["version"], container.get("version", "latest"))
        runner = scaffold["containers"]["harness-e2e"]
        self.assertEqual(runner["config_name"], "project-one-harness-e2e")
        self.assertEqual(
            runner["config_override"],
            {
                "data_dir": str(data_dir),
                "control_database": "primary",
                "control_namespace": "project-one",
            },
        )
        self.assertNotIn("config_override", scaffold["containers"]["harness"])
        self.assertNotIn("config_name", scaffold["containers"]["harness"])

    def test_scaffold_provisions_only_requested_harness_limits(self):
        for scenarios, expected in (
            (["fanout_ladder"], {"max_children": 16}),
            (["depth_ladder"], {"max_depth": 6}),
            (["fanout_ladder", "depth_ladder"], {"max_children": 16, "max_depth": 6}),
        ):
            with self.subTest(scenarios=scenarios), tempfile.TemporaryDirectory() as directory:
                contract = campaign_contract()
                contract["suite"]["groups"][0]["scenarios"] = scenarios
                scaffold = MODULE.project_scaffold(
                    contract, "project-one", Path(directory).resolve() / "runs", {}, {}
                )
                harness = scaffold["containers"]["harness"]
                self.assertEqual(harness["config_override"], expected)
                self.assertEqual(harness["config_name"], "project-one-harness")

    def test_compose_evidence_binds_the_declaration_yaml_namespace_and_lifecycle(self):
        contract = campaign_contract()
        expected = {"harness": "1.9.0", "harness-e2e": "0.6.0-experimental"}
        with tempfile.TemporaryDirectory() as directory:
            compose_path = Path(directory) / "worker-compose.yaml"
            compose_path.write_text(yaml.safe_dump({
                "namespace": "project-one",
                "containers": {
                    worker: {"worker": f"package://{worker}", "version": "latest"}
                    for worker in expected
                },
            }))
            declared = MODULE.declared_workers(compose_path)
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
        self.assertEqual(evidence["contract_sha256"], MODULE.canonical_sha256(contract))
        self.assertEqual(evidence["namespace"], "project-one")
        self.assertEqual(set(evidence["lifecycle"]), {"add", "up", "status", "down"})
        self.assertEqual(
            evidence["runtime"]["requested_roots"],
            declared,
        )
        self.assertEqual(evidence["runtime"]["observed_versions"]["harness"], "1.9.0")

    def test_compose_evidence_reports_a_missing_container(self):
        contract = campaign_contract()
        with tempfile.TemporaryDirectory() as directory:
            compose_path = Path(directory) / "worker-compose.yaml"
            compose_path.write_text(yaml.safe_dump({
                "namespace": "project-one",
                "containers": {"fp": {"worker": "package://fp", "version": "latest"}},
            }))
            with self.assertRaisesRegex(ValueError, "missing declared workers: fp"):
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
            checkpoint = group / "native/executions/.workflow-state/workflow-resume/state.json"
            checkpoint.parent.mkdir(parents=True)
            payload = b'{"state_sha256":"sha256:checkpoint","state":{"sequence":3}}\n'
            checkpoint.write_bytes(payload)
            for package_root in [group, root]:
                manifest = MODULE.package_bundle(package_root, campaign_contract(), {})
                reference = next(entry for entry in manifest["files"] if entry["path"].endswith("state.json"))
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

    def test_a_template_project_gets_the_runner_through_compose_add(self):
        """A template is brought up as it stands and only the runner is asked
        for, so the engine installs what the runner needs — nothing is declared
        on its behalf, and no template role is passed to be expanded."""
        source = RUNNER_SCRIPT.read_text()
        start = source.index("failure_phase=project_assembly")
        block = source[start:source.index("failure_phase=project_start", start)]
        template_branch = block[:block.index("else")]
        self.assertIn('compose_trigger compose::add "file=$compose_file" "worker=harness-e2e@', template_branch)
        self.assertNotIn("exact-stack-scaffold", template_branch)
        self.assertIn("await_compose_add", template_branch)
        self.assertNotIn("runner_dependencies", MODULE.__dict__)

    def test_identity_travels_verbatim_while_requested_values_keep_their_shape(self):
        contract = campaign_contract()
        contract.update({
            "schema": "rc-e2e/v3-preview",
            "campaign_id": "nightly-2026-09",
            "execution_id": "run 41",
            "idempotency_key": "whatever the dispatcher wants",
            "stack_revision": "main",
        })
        validated = MODULE.validate_contract(contract)
        self.assertEqual(validated["campaign_id"], "nightly-2026-09")
        self.assertEqual(validated["stack_revision"], "main")

    def test_values_the_executor_turns_into_requests_are_still_checked(self):
        # A bad value in these reaches a URL or a token exchange rather than a
        # column, so their shape is not the dispatcher's to choose.
        cases = (
            (("security", "oidc_audience"), "not an audience!", "unsupported characters"),
            (("runtime", "cli", "version"), "latest", "exact version"),
            (("runtime", "template", "id"), "../../etc/passwd", "iii template id"),
        )
        for path, value, expected in cases:
            with self.subTest(field=".".join(path)):
                contract = campaign_contract()
                contract["runtime"]["template"] = {
                    "id": "linkly-agentic", "repository": "iii-hq/templates",
                    "ref": "main", "revision": "c" * 40,
                }
                target = contract
                for key in path[:-1]:
                    target = target[key]
                target[path[-1]] = value
                with self.assertRaisesRegex(ValueError, expected):
                    MODULE.validate_contract(contract)


if __name__ == "__main__":
    unittest.main()
