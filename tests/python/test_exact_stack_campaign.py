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

import yaml

def declared_base():
    """The Compose project of the default stack, as an execution starts from it."""
    return MODULE.declared_base()


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


def lock_of(versions: dict[str, str]):
    """A worker-compose.lock resolving each package to one version, with the
    registry host Compose records."""
    return {"version": 1, "containers": {
        worker: {"worker": f"package://api.workers.iii.dev/{worker}", "requested": "latest", "resolved": {
            "name": worker, "version": version, "type": "binary",
            "artifacts": {"x86_64-unknown-linux-gnu": {"sha256": "0" * 64, "url": f"https://registry.example/{worker}"}},
        }}
        for worker, version in versions.items()
    }}


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
        # Where a template comes from is the stack's choice, not a rule here.
        for field, value in [("revision", "0123abc"), ("repository", "other/repo"), ("ref", "feature")]:
            chosen = json.loads(json.dumps(contract))
            chosen["runtime"]["template"][field] = value
            with self.subTest(field=field):
                MODULE.validate_contract(chosen)
        # The groups check the template out at its revision: it has to be there.
        del contract["runtime"]["template"]["revision"]
        with self.assertRaisesRegex(ValueError, "revision"):
            MODULE.validate_contract(contract)

    def test_local_workers_and_named_package_instances_survive_scaffolding(self):
        contract = campaign_contract()
        template = {"containers": {
            "subject": {"worker": "package://harness", "version": "latest"},
            "link": {"worker": "path://./link", "scripts": {"run": "pnpm dev"}},
        }}
        project = MODULE.project_scaffold(contract, "project-one", Path("/data"), {}, {}, template)
        self.assertNotIn("harness", project["containers"])
        self.assertEqual(project["containers"]["subject"]["version"], "latest")
        self.assertEqual(project["containers"]["link"], {**template["containers"]["link"], "environment": {"III_TELEMETRY_ENABLED": "false"}})
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

    def test_one_env_file_reaches_every_container_including_those_born_later(self):
        contract = campaign_contract({
            "harness": "1.9.0", "state": "0.22.1", "llm-router": "1.4.0", "provider-deepseek": "0.1.0",
        })
        # The template ships its own `./.env` reference; ours replaces it. It does
        # not name iii-directory, so the agent profile has to create it.
        template = {"containers": {
            "harness": {"worker": "package://harness"},
            "state": {"worker": "package://state"},
            "llm-router": {"worker": "package://llm-router", "env_file": ["./.env"]},
            "provider-deepseek": {"worker": "package://provider-deepseek"},
        }}
        project = MODULE.project_scaffold(contract, "project-one", Path("/data"),
            "/private/.env", {}, template, profile_root=Path("/isolated/project"))
        containers = project["containers"]
        self.assertIn("harness-e2e", containers)  # the runner is added, never assumed
        self.assertIn("iii-directory", containers)
        for name, container in containers.items():
            with self.subTest(container=name):
                self.assertEqual(container["env_file"], ["/private/.env"])
        directory = containers["iii-directory"]["config_override"]
        self.assertFalse(directory["auto_download"])
        self.assertEqual(directory["agents_folder"], "/isolated/project/agents")
        with self.assertRaisesRegex(ValueError, "must be absolute"):
            MODULE.project_scaffold(contract, "project-one", Path("/data"), "relative/.env", {}, template)

    def test_a_template_pin_never_holds_the_application_back(self):
        """The execution's lock wins, everything else runs latest. The Linkly
        fixture is checked out at one commit whose compose pinned harness to
        1.8.17; honouring it downgraded the application under test below the
        runner's control-plane contract."""
        contract = campaign_contract()
        contract["runtime"]["lock"] = lock_of({"state": "0.22.1"})
        template = {"containers": {
            "harness": {"worker": "package://harness", "version": "1.8.17"},
            "state": {"worker": "package://state", "version": "0.22.8"},
            "http": {"worker": "package://http", "version": "0.21.9"},
        }}
        project = MODULE.project_scaffold(contract, "project-one", Path("/data"), None, {}, template)
        versions = {name: container["version"] for name, container in project["containers"].items()}
        self.assertEqual(versions, {
            "harness": "latest", "state": "0.22.1", "http": "latest", "harness-e2e": "latest",
            "provider-deepseek": "latest",
        })

    def test_campaign_provider_is_added_to_templates_and_receives_its_pin_and_credentials(self):
        template = {"containers": {
            "subject": {"worker": "package://harness"},
            "provider-openai": {"worker": "package://provider-openai"},
        }}
        for provider in ("deepseek", "zai", "anthropic"):
            with self.subTest(provider=provider):
                contract = campaign_contract()
                contract["suite"]["subject"]["provider"] = provider
                contract["runtime"]["lock"] = lock_of({f"provider-{provider}": "1.2.3"})
                project = MODULE.project_scaffold(
                    contract, "project-one", Path("/data"), "/private/.env", {}, template,
                )
                self.assertEqual(project["containers"].get(f"provider-{provider}"), {
                    "worker": f"package://provider-{provider}",
                    "version": "1.2.3",
                    "env_file": ["/private/.env"],
                    "environment": {"III_TELEMETRY_ENABLED": "false"},
                })
                self.assertNotIn("fp", project["containers"])
                self.assertNotIn("harness", project["containers"])
                self.assertEqual(set(template["containers"]), {"subject", "provider-openai"})

    def test_existing_provider_instances_are_preserved_without_duplicates(self):
        template = {"containers": {
            "harness": {"worker": "package://harness"},
            "selected-model": {
                "worker": "package://provider-deepseek", "config_name": "custom",
                "config_override": {"setting": "keep"},
            },
        }}
        project = MODULE.project_scaffold(
            campaign_contract(), "project-one", Path("/data"), "/private/.env", {}, template,
        )
        providers = {name: container for name, container in project["containers"].items()
                     if container["worker"] == "package://provider-deepseek"}
        self.assertEqual(list(providers), ["selected-model"])
        self.assertEqual(providers["selected-model"]["config_override"], {"setting": "keep"})
        self.assertEqual(providers["selected-model"]["env_file"], ["/private/.env"])

    def test_campaign_provider_name_cannot_replace_an_unrelated_template_container(self):
        template = {"containers": {
            "harness": {"worker": "package://harness"},
            "provider-deepseek": {"worker": "path://./link"},
        }}
        with self.assertRaisesRegex(ValueError, "provider-deepseek"):
            MODULE.project_scaffold(campaign_contract(), "project-one", Path("/data"), None, {}, template)

    def test_every_container_keeps_iii_telemetry_off_whatever_the_daemon_inherited(self):
        template = {"containers": {
            "harness": {"worker": "package://harness"},
            "link": {"worker": "path://./link", "environment": {"III_TELEMETRY_ENABLED": "true"}},
        }}
        for scaffold in (
            MODULE.project_scaffold(campaign_contract(), "project-one", Path("/data"), None,
                                    {"harness.III_TELEMETRY_ENABLED": "true"}),
            MODULE.project_scaffold(campaign_contract(), "project-one", Path("/data"), None, {}, template,
                                    profile_root=Path("/isolated/project")),
        ):
            for name, container in scaffold["containers"].items():
                self.assertEqual(container["environment"]["III_TELEMETRY_ENABLED"], "false", name)

    def test_base_projects_also_add_the_campaign_provider_when_it_is_not_declared(self):
        contract = campaign_contract()
        contract["suite"]["subject"]["provider"] = "anthropic"
        project = MODULE.project_scaffold(contract, "project-one", Path("/data"), "/private/.env", {})
        self.assertEqual(project["containers"].get("provider-anthropic"), {
            "worker": "package://provider-anthropic", "version": "latest", "env_file": ["/private/.env"],
            "environment": {"III_TELEMETRY_ENABLED": "false"},
        })

    def test_visual_worker_groups_include_canvas_only_for_their_shards(self):
        contract = campaign_contract()
        visual = {"execution_kind": "harness_turn", "runs": 1, "technical_retries": 0}
        contract["suite"]["groups"].extend([
            {"id": "case-form-flow-build", "scenarios": ["form_flow_build"], **visual},
            {"id": "case-state-machine-canvas-build", "scenarios": ["state_machine_canvas_build"], **visual},
        ])
        # The stack the execution assembles once carries Canvas for the suite.
        shared = MODULE.project_scaffold(contract, "project-one", Path("/data"), None, {})
        self.assertEqual(shared["containers"]["canvas"], {"worker": "package://canvas", "version": "latest",
                                                          "environment": {"III_TELEMETRY_ENABLED": "false"}})
        ordinary = MODULE.project_scaffold(
            contract, "project-one", Path("/data"), None, {}, group_id="daily-core",
        )
        self.assertNotIn("canvas", ordinary["containers"])
        for group_id in ("case-form-flow-build", "case-state-machine-canvas-build"):
            visual = MODULE.project_scaffold(
                contract, "project-one", Path("/data"), None, {}, group_id=group_id,
            )
            self.assertEqual(visual["containers"]["canvas"]["worker"], "package://canvas")
        with self.assertRaisesRegex(ValueError, "unknown campaign group"):
            MODULE.project_scaffold(
                contract, "project-one", Path("/data"), None, {}, group_id="unknown",
            )

        # Assembled and locked: a group that builds no visual worker leaves
        # Canvas out again, with what only Canvas brought in, and its lock
        # names exactly what it declares.
        lock = lock_of({"harness": "1.9.3", "state": "0.22.8", "canvas": "0.4.0", "canvas-store": "0.1.0"})
        lock["graphs"] = {"harness": ["harness", "state"], "canvas": ["canvas", "canvas-store", "state"]}
        contract["runtime"].update(lock=lock, compose={"containers": {
            name: {"worker": f"package://{name}", "version": "latest"} for name in lock["containers"]
        }})
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "contract.json").write_text(json.dumps(contract))
            for group_id, expected in (("daily-core", ["harness", "state"]),
                                       ("case-form-flow-build", ["harness", "state", "canvas", "canvas-store"])):
                output = root / group_id / "worker-compose.yaml"
                output.parent.mkdir()
                result = subprocess.run(
                    ["python3", str(SCRIPT), "project", "--contract", str(root / "contract.json"),
                     "--group-id", group_id, "--namespace", "project-one", "--data-dir", "/data",
                     "--output", str(output)],
                    capture_output=True, text=True,
                )
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                declared = yaml.safe_load(output.read_text())["containers"]
                written = yaml.safe_load((output.parent / "worker-compose.lock").read_text())
                with self.subTest(group=group_id):
                    self.assertEqual(sorted(set(declared) - {"harness-e2e", "provider-deepseek"}), sorted(expected))
                    self.assertEqual(sorted(written["containers"]), sorted(expected))
                    self.assertEqual(sorted(written["graphs"]), sorted({"harness", "canvas"} & set(expected)))

        template = {"containers": {"harness": {"worker": "package://harness"}}}
        pinned = MODULE.project_scaffold(
            contract, "project-one", Path("/data"), None, {}, template, group_id="case-form-flow-build",
        )
        self.assertEqual(pinned["containers"]["canvas"]["version"], "0.4.0")

    def test_an_agent_profile_on_the_default_stack_leaves_provider_and_directory_to_the_graph(self):
        """Harness depends on the Directory and on several providers, pinned in
        its graph. Declaring either again while assembling is a second spec
        Compose refuses ("conflicting sources, versions, or settings"); the
        groups then configure the Directory the graph brought."""
        contract = campaign_contract()
        contract["suite"]["agent_profile"] = "ade-worker-builder"
        contract["suite"]["subject"]["provider"] = "anthropic"
        assembled = MODULE.project_scaffold(
            contract, "e2e-prepare", Path("/data"), "/private/.env", {},
            profile_root=Path("/project"), assembling=True,
        )
        self.assertEqual(sorted(assembled["containers"]),
                         sorted(declared_base()["containers"]))
        # The same contract in a group, without the lock yet: both are ensured.
        group = MODULE.project_scaffold(
            contract, "e2e-group", Path("/data"), "/private/.env", {}, profile_root=Path("/project"),
        )
        self.assertIn("iii-directory", group["containers"])
        self.assertIn("provider-anthropic", group["containers"])
        # Frozen, the group configures the Directory the graph brought (see the
        # assembled-stack test); a lock with none cannot gain one.
        contract["runtime"]["lock"] = lock_of({"harness": "1.8.31"})
        contract["runtime"]["compose"] = {"containers": {"harness": {"worker": "package://harness"}}}
        with self.assertRaisesRegex(ValueError, "brings no iii-directory"):
            MODULE.project_scaffold(
                contract, "e2e-group", Path("/data"), None, {}, profile_root=Path("/project"), group_id="daily-core",
            )

    def test_a_group_starts_the_assembled_stack_exactly_and_keeps_its_lock_beside_it(self):
        """Compose checks the lock against every package container's worker and
        selector, so the group stamps what is per group and nothing else."""
        assembled = {"namespace": "e2e-prepare", "containers": {
            "harness": {"worker": "package://harness", "version": "latest"},
            # Named with the registry host: still the runner, the provider and
            # the Directory, never a second declaration of them.
            "e2e": {"worker": "package://api.workers.iii.dev/harness-e2e", "version": "0.12.3",
                    "config_name": "e2e-prepare-harness-e2e", "config_override": {"data_dir": "/prepare"}},
            "provider-deepseek": {"worker": "package://api.workers.iii.dev/provider-deepseek", "version": "latest"},
            "directory": {"worker": "package://api.workers.iii.dev/iii-directory", "version": "0.3.1"},
            "state": {"worker": "package://state", "version": "0.22.8", "env_file": ["/prepare/.env"]},
            "llm-router": {"worker": "package://llm-router"},
        }}
        contract = campaign_contract()
        contract["runtime"].update(compose=assembled, lock=lock_of({"harness": "1.9.3", "state": "0.22.8"}))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "contract.json").write_text(json.dumps(contract))
            output = root / "stack/worker-compose.yaml"
            output.parent.mkdir()
            result = subprocess.run(
                ["python3", str(SCRIPT), "project", "--contract", str(root / "contract.json"),
                 "--namespace", "e2e-group", "--data-dir", str(root / "native"),
                 "--env-file", str(root / ".env"), "--output", str(output),
                 "--profile-root", str(root / "project"),
                 "--environment", "harness-e2e.HARNESS_E2E_LANE=local-pr"],
                capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            project = yaml.safe_load(output.read_text())
            self.assertEqual(yaml.safe_load((output.parent / "worker-compose.lock").read_text()),
                             contract["runtime"]["lock"])
        self.assertEqual(project["namespace"], "e2e-group")
        self.assertEqual(
            {name: (c["worker"], c.get("version")) for name, c in project["containers"].items()},
            {name: (c["worker"], c.get("version")) for name, c in assembled["containers"].items()},
        )
        runner = project["containers"]["e2e"]
        self.assertEqual(runner["config_name"], "e2e-group-harness-e2e")
        self.assertEqual(runner["config_override"]["data_dir"], str(root / "native"))
        self.assertEqual(runner["environment"], {"HARNESS_E2E_LANE": "local-pr", "III_TELEMETRY_ENABLED": "false"})
        self.assertEqual(project["containers"]["directory"]["config_override"]["agents_folder"],
                         str(root / "project/agents"))
        self.assertEqual({tuple(c["env_file"]) for c in project["containers"].values()}, {(str(root / ".env"),)})

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

    def test_agent_profile_is_validated_and_reaches_native_admission(self):
        contract = campaign_contract()
        original = MODULE.materialize_request(contract, catalog(), group_id="daily-core")
        contract["suite"]["agent_profile"] = "console-ui"
        MODULE.validate_suite(contract["suite"])
        request = MODULE.materialize_request(contract, catalog(), group_id="daily-core")
        self.assertEqual(request["agent"], "console-ui")
        self.assertNotEqual(request["idempotency_key"], original["idempotency_key"])
        self.assertNotIn("agent_profile", request)
        for invalid in [" ", {"id": "reviewer", "content": "body"}]:
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

    def test_common_runner_keeps_the_one_env_file_private_and_out_of_the_evidence(self):
        runner = RUNNER_SCRIPT.read_text()
        self.assertIn('env_file="$project_dir/.env"', runner)
        self.assertIn('chmod 600 "$env_file"', runner)
        # The runtime tree is checked before any credential is written into it.
        self.assertLess(runner.index("validate-layout"), runner.index(': >"$env_file"'))
        # And written after a template scaffold, so it replaces the placeholder.
        self.assertLess(runner.index("project init"), runner.index(': >"$env_file"'))
        self.assertNotIn("$artifact_dir/.env", runner)

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
            contract_file = artifacts.parent / f"{artifacts.name}-contract.json"
            contract_file.write_text(json.dumps(campaign_contract()))
            self.addCleanup(contract_file.unlink)
            result = subprocess.run(
                ["bash", str(RUNNER_SCRIPT)],
                env={
                    **os.environ,
                    "HARNESS_E2E_ARTIFACTS_DIR": str(artifacts),
                    "HARNESS_E2E_CONTRACT": str(contract_file),
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
            (fake_repo / "extract_kanban_reports.py").write_text(
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
        self.assertIn("group observation artifact was not available",
                      (WORKFLOW.parents[2] / "scripts/executor.sh").read_text())

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
        self.assertEqual([group["id"] for group in manifest["groups"]], ["daily-core"])
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

    def test_compose_evidence_warns_about_a_container_the_engine_did_not_report(self):
        """The engine lists workers by container name; a template may run a
        package under another name (Linkly: `console` runs `ade`). Compose has
        already gated the start, so what it does not report is shown, and a
        finished run is never discarded over it."""
        contract = campaign_contract()
        workers = {"workers": [
            {"name": "console", "version": "1.9.35", "namespace": "project-one"},
            {"name": "harness", "version": "1.9.0", "namespace": "project-one"},
        ]}
        cases = (
            ({"console": {"worker": "package://ade"}, "harness": {"worker": "package://harness"}}, []),
            ({"fp": {"worker": "package://fp"}, "harness": {"worker": "package://harness"}},
             ["containers the engine did not report: fp"]),
        )
        for containers, warnings in cases:
            with self.subTest(containers=sorted(containers)), tempfile.TemporaryDirectory() as directory:
                compose_path = Path(directory) / "worker-compose.yaml"
                compose_path.write_text(yaml.safe_dump({"namespace": "project-one", "containers": containers}))
                evidence = MODULE.compose_evidence(
                    contract, compose_path, "project-one",
                    {name: {} for name in ("add", "up", "status", "down")},
                    workers, {"before": [], "during": [], "after": []},
                )
                self.assertEqual(evidence["runtime"]["version_report_warnings"], warnings)
                self.assertEqual(evidence["runtime"]["observed_versions"]["harness"], "1.9.0")

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
        start = block.index('if [[ -n "$project_template" ]]')
        template_branch = block[start:block.index("else", start)]
        # Only the runner, at the version the scaffold gave it; no template role.
        self.assertIn('add_args+=("worker=$(python3 "$contract_tool" roots --compose "$compose_file" | grep \'^harness-e2e@\')")', template_branch)
        # One add assembles the project; preparation's second asks only for
        # what no graph brought.
        self.assertEqual(block.count("compose_trigger compose::add"), 2)
        self.assertNotIn("exact-stack-scaffold", block)
        self.assertIn("await_compose_add", block)
        self.assertNotIn("runner_dependencies", MODULE.__dict__)

    def test_a_group_starts_the_locked_stack_frozen_and_preparation_only_assembles_it(self):
        """What the launcher does with the lock, without an engine: the frozen
        decision, compose::add only where a project is still to be assembled,
        and preparation stopping once the stack and its lock exist."""
        source = RUNNER_SCRIPT.read_text()
        decide = source[source.index("if [[ -n \"$assemble_only\" ]]; then"):source.index("profile_assets=")]
        start = source.index("failure_phase=project_assembly")
        assembly = source[start:source.index("jq -e '.status == \"ok\"' \"$artifact_dir/stack/up.json\"")]
        stubs = """set -Eeuo pipefail
artifact_dir=$1
contract_path=$1/contract.json
compose_file=$1/$3/worker-compose.yaml
contract_tool=$2
log() { :; }
await_compose_add() { :; }
compose_trigger() {
  printf '%s\\n' "$*" >>"$artifact_dir/calls"
  # Harness's graph brings the Directory when the case says so.
  if [[ -n "${BRINGS:-}" && "$*" == *"worker=harness@"* ]]; then cat "$BRINGS" >>"$compose_file"; fi
  echo '{"status":"ok"}'
}
"""
        compose = {"containers": {
            "harness": {"worker": "package://harness", "version": "latest"},
            "harness-e2e": {"worker": "package://harness-e2e", "version": "0.12.3"},
            "provider-deepseek": {"worker": "package://provider-deepseek", "version": "latest"},
        }}
        directory_from_graph = {"worker": "package://api.workers.iii.dev/iii-directory", "version": "1.2.29"}
        cases = (
            # (lock in contract, assemble_only, project template, agent profile,
            #  Directory already in the file) -> (frozen, calls)
            (True, "", "", False, False, "true", ["compose::up --json"]),
            (False, "1", "", False, False, "false", ["compose::add file="]),
            # With an agent profile the Directory is asked for on its own only
            # when no graph brought it, never alongside Harness.
            (False, "1", "", True, False, "false", ["compose::add file=", "compose::add file="]),
            (False, "1", "", True, True, "false", ["compose::add file="]),
            # Preparation assembles the stack itself, never a template project.
            (False, "1", "linkly-agentic", False, False, "false", ["compose::add file="]),
            (True, "", "linkly-agentic", False, False, "false", ["compose::add file=", "compose::up --json"]),
        )
        for locked, assemble_only, template, profile, brought, frozen, calls in cases:
            with self.subTest(locked=locked, assemble_only=assemble_only, template=template,
                              profile=profile, brought=brought), \
                    tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                # A template project lives in the run tree, a stack in the evidence.
                project = "project" if template and not assemble_only else "stack"
                (root / "stack").mkdir()
                (root / project).mkdir(exist_ok=True)
                (root / project / "worker-compose.yaml").write_text(yaml.safe_dump(compose))
                (root / "brings.yaml").write_text(
                    yaml.safe_dump({"containers": {"iii-directory": directory_from_graph}}).split("\n", 1)[1]
                )
                (root / "contract.json").write_text(json.dumps({
                    "runtime": {"lock": lock_of({}) if locked else None},
                    "suite": {"subject": {"provider": "deepseek"}},
                }))
                variables = (f"assemble_only={assemble_only!r}\nproject_template={template!r}\n"
                             f"execution_template=''\nlinkly_fixture=false\nprofile_assets={str(profile).lower()}\n")
                result = subprocess.run(
                    ["bash", "-c", stubs + variables + decide + 'printf "%s\\n" "$frozen" >"$artifact_dir/frozen"\n' + assembly,
                     "runner", str(root), str(SCRIPT), project],
                    capture_output=True, text=True,
                    env={**os.environ, "BRINGS": str(root / "brings.yaml") if brought else ""},
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual((root / "frozen").read_text().strip(), frozen)
                made = (root / "calls").read_text().splitlines()
                self.assertEqual(len(made), len(calls), made)
                for call, expected in zip(made, calls):
                    self.assertTrue(call.startswith(expected), (call, expected))
                if frozen == "true":
                    self.assertEqual(json.loads((root / "stack/add.json").read_text())["status"], "skipped")
                    self.assertIn('"frozen":true', made[-1])
                if assemble_only:
                    self.assertFalse(any("compose::up" in call for call in made), "preparation stops at the lock")
                if project == "project":
                    self.assertIn("worker=harness-e2e@0.12.3", made[0])
                    self.assertNotIn("worker=harness@", made[0])
                elif not locked:
                    self.assertIn("worker=harness@latest", made[0])
                    # What the executor only needs to exist is never asked
                    # for next to Harness: its graph pins them.
                    self.assertNotIn("worker=iii-directory", made[0])
                    self.assertNotIn("worker=provider-deepseek", made[0])
                if len(made) == 2 and assemble_only:
                    self.assertTrue(made[1].endswith("worker=iii-directory"), made[1])

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

    def test_only_identity_and_integrity_are_still_checked(self):
        # The audience reaches the token exchange; the digest is what the CLI
        # download is checked against. Everything else is the dispatcher's.
        cases = (
            (("security", "oidc_audience"), "not an audience!", "unsupported characters"),
            (("runtime", "cli", "sha256"), "latest", "sha256:<64 lowercase hex>"),
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
