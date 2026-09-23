#!/usr/bin/env python3
"""Validate and materialize the exact-stack campaign contract.

Release Control owns every campaign decision — the suite, the models, the
policy — and states each one exactly once in the contract it dispatches. This
repository owns the runtime: which scenarios a pinned runner release can
execute, how the stack boots, and what evidence comes back. No campaign
configuration is read from this repository, and nothing is verified twice:
unknown fields are ignored so either side can add one and ship alone.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import re
from pathlib import Path
from typing import Any


CONTRACT_SCHEMA = "rc-e2e/v2"
SHA256 = re.compile(r"^sha256:[0-9a-f]{64}$")
GIT_SHA = re.compile(r"^[0-9a-f]{40}$")
VERSION = re.compile(
    r"^[0-9]+\.[0-9]+\.[0-9]+"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)
EXECUTION_KINDS = {
    "harness_turn",
    "scripted_dialogue",
    "composite_flow",
    "adaptive_flow",
}
#: The application under test. Its package graph is the stack being measured.
APPLICATION = "harness"
#: The runner that executes the scenarios inside that stack.
RUNNER = "harness-e2e"
#: What a declaration means when it does not name a version.
DEFAULT_SELECTOR = "latest"
#: The stack this repository declares. Every execution starts from it.
BASE_COMPOSE = Path(__file__).resolve().parents[1] / "worker-compose.base.yaml"


def declared_base() -> dict[str, Any]:
    import yaml

    project = yaml.safe_load(BASE_COMPOSE.read_text())
    if not isinstance(project, dict):
        raise ValueError("base compose must be an object")
    return project


def load_object(path: Path, label: str) -> dict[str, Any]:
    value = json.loads(path.read_text())
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be a JSON object")
    return value


def canonical(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True)


def canonical_sha256(value: Any) -> str:
    return f"sha256:{hashlib.sha256(canonical(value).encode()).hexdigest()}"


def require_keys(value: Any, required: set[str], label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be an object")
    missing = sorted(required - set(value))
    if missing:
        raise ValueError(f"{label} is missing fields: {', '.join(missing)}")
    return value


def require_text(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{label} must be a non-empty string")
    return value


def require_digest(value: Any, label: str) -> str:
    value = require_text(value, label)
    if not SHA256.fullmatch(value):
        raise ValueError(f"{label} must be sha256:<64 lowercase hex>")
    return value


def require_version(value: Any, label: str) -> str:
    value = require_text(value, label)
    if not VERSION.fullmatch(value):
        raise ValueError(f"{label} must be an exact version")
    return value


def require_positive_integer(value: Any, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 1:
        raise ValueError(f"{label} must be a positive integer")
    return value


def require_nonnegative_integer(value: Any, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise ValueError(f"{label} must be a non-negative integer")
    return value


def validate_identity(suite: dict[str, Any], role: str) -> None:
    identity = require_keys(suite.get(role), {"provider", "model"}, f"suite.{role}")
    require_text(identity.get("provider"), f"suite.{role}.provider")
    require_text(identity.get("model"), f"suite.{role}.model")


def validate_suite(suite: Any) -> dict[str, Any]:
    """Shape of what Release Control asked for.

    Execution policy — run bounds, retry rules per kind, canonical fault
    profiles — is enforced once, by the campaign aggregator that consumes the
    manifest this contract materializes.
    """
    require_keys(
        suite,
        {"id", "label", "lane", "seed", "subject", "groups"},
        "suite",
    )
    suite_id = require_text(suite.get("id"), "suite.id")
    if not re.fullmatch(r"[a-z][a-z0-9-]{0,63}", suite_id):
        raise ValueError("suite.id must be kebab-case")
    require_text(suite.get("label"), "suite.label")
    require_text(suite.get("lane"), "suite.lane")
    if suite.get("seed") is not None:
        require_positive_integer(suite.get("seed"), "suite.seed")
    validate_identity(suite, "subject")
    if suite.get("agent_profile") is not None:
        agent = require_text(suite["agent_profile"], "suite.agent_profile")
        if not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,63}", agent):
            raise ValueError("suite.agent_profile must be a Directory agent id")

    groups = suite.get("groups")
    if not isinstance(groups, list) or not groups:
        raise ValueError("suite.groups must be a non-empty array")
    seen: set[str] = set()
    for index, group in enumerate(groups):
        label = f"suite.groups[{index}]"
        group = require_keys(
            group,
            {"id", "execution_kind", "runs", "technical_retries"},
            label,
        )
        group_id = require_text(group.get("id"), f"{label}.id")
        if not re.fullmatch(r"[a-z][a-z0-9-]{0,63}", group_id) or group_id in seen:
            raise ValueError("suite group ids must be unique kebab-case values")
        seen.add(group_id)
        kind = require_text(group.get("execution_kind"), f"{label}.execution_kind")
        if kind not in EXECUTION_KINDS:
            raise ValueError(f"{label}.execution_kind is unsupported")
        require_positive_integer(group.get("runs"), f"{label}.runs")
        require_nonnegative_integer(group.get("technical_retries"), f"{label}.technical_retries")
        scenarios = group.get("scenarios")
        if not isinstance(scenarios, list) or not scenarios or len(set(scenarios)) != len(scenarios):
            raise ValueError(f"{label}.scenarios must be a non-empty unique array")
        for scenario in scenarios:
            require_text(scenario, f"{label}.scenarios[]")
    return suite


def validate_contract(contract: dict[str, Any]) -> dict[str, Any]:
    require_keys(
        contract,
        {
            "schema",
            "campaign_id",
            "execution_id",
            "attempt",
            "idempotency_key",
            "runtime",
            "security",
            "suite",
        },
        "contract",
    )
    # How a run is named and attributed is the dispatcher's business. These
    # fields are carried and displayed, so the executor asks that they be
    # present and legible, not that they match a particular spelling.
    require_text(contract.get("schema"), "schema")
    require_text(contract.get("campaign_id"), "campaign_id")
    require_text(contract.get("execution_id"), "execution_id")
    require_positive_integer(contract.get("attempt"), "attempt")
    require_text(contract.get("idempotency_key"), "idempotency_key")

    cli = require_keys(
        require_keys(contract.get("runtime"), {"cli"}, "runtime").get("cli"),
        {"version", "target", "asset", "sha256"},
        "runtime.cli",
    )
    require_version(cli.get("version"), "runtime.cli.version")
    require_text(cli.get("target"), "runtime.cli.target")
    asset = require_text(cli.get("asset"), "runtime.cli.asset")
    if not asset.startswith("iii-") or asset.startswith("iii-" + "worker"):
        raise ValueError("runtime.cli.asset must name the iii CLI archive")
    require_digest(cli.get("sha256"), "runtime.cli.sha256")
    template = contract["runtime"].get("template")
    if template is not None:
        require_keys(template, {"id", "repository", "ref", "revision"}, "runtime.template")
        if not isinstance(template["id"], str) or not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,63}", template["id"]):
            raise ValueError("runtime.template.id must be an iii template id")
        if template["repository"] != "iii-hq/templates" or template["ref"] != "main":
            raise ValueError("runtime.template must originate from iii-hq/templates main")
        if not isinstance(template["revision"], str) or not GIT_SHA.fullmatch(template["revision"]):
            raise ValueError("runtime.template.revision must be a full lowercase git SHA")

    security = require_keys(contract.get("security"), {"oidc_audience"}, "security")
    audience = require_text(security.get("oidc_audience"), "security.oidc_audience")
    if not re.fullmatch(r"[A-Za-z0-9._:/-]+", audience):
        raise ValueError("security.oidc_audience contains unsupported characters")

    validate_suite(contract.get("suite"))
    return contract


def campaign_manifest(contract: dict[str, Any]) -> dict[str, Any]:
    """The suite, in the shape the campaign aggregator consumes."""
    suite = contract["suite"]
    groups = []
    for group in suite["groups"]:
        materialized = {
            "id": group["id"],
            "execution_kind": group["execution_kind"],
            "runs": group["runs"],
            "technical_retries": group["technical_retries"],
        }
        materialized["scenarios"] = group["scenarios"]
        groups.append(materialized)
    return {
        "kind": "harness-e2e-campaign",
        "campaign_id": suite["id"],
        "lane": suite["lane"],
        "failure_policy": "advisory",
        "groups": groups,
    }


def observed_versions(workers_payload: dict[str, Any], namespace: str | None = None) -> dict[str, str]:
    """What the engine reports it installed, by short worker name."""
    rows = workers_payload.get("workers")
    versions: dict[str, str] = {}
    for row in rows if isinstance(rows, list) else []:
        if not isinstance(row, dict) or row.get("namespace") not in (None, namespace):
            continue
        name, version = row.get("name"), row.get("version")
        if isinstance(name, str) and isinstance(version, str):
            versions[name.rsplit("/", 1)[-1]] = version
    return dict(sorted(versions.items()))


def materialize_request(
    contract: dict[str, Any], catalog: dict[str, Any], group_id: str | None = None,
    installed: dict[str, str] | None = None,
) -> dict[str, Any]:
    runner = require_keys(catalog.get("runner"), {"name", "version", "revision"}, "catalog.runner")
    catalog_sha256 = require_digest(catalog.get("catalog_sha256"), "catalog.catalog_sha256")
    descriptors = catalog.get("scenarios")
    if not isinstance(descriptors, list):
        raise ValueError("scenario catalog scenarios must be a list")
    by_id = {
        item["scenario_id"]: item
        for item in descriptors
        if isinstance(item, dict) and isinstance(item.get("scenario_id"), str)
    }

    suite = contract["suite"]
    group = next((item for item in suite["groups"] if item["id"] == group_id), None)
    if group is None:
        raise ValueError("a valid campaign group id is required")
    selected_cases: list[dict[str, Any]] = []
    for scenario_id in group["scenarios"]:
        descriptor = by_id.get(scenario_id)
        if descriptor is None:
            raise ValueError(f"{runner['name']}@{runner['version']} has no scenario {scenario_id}")
        selected_cases.append(
            {
                "scenario_id": scenario_id,
                "behavior_sha256": require_digest(
                    descriptor.get("behavior_sha256"), f"{scenario_id}.behavior_sha256"
                ),
                "case_id": require_text(descriptor.get("case_id"), f"{scenario_id}.case_id"),
                "seed": require_nonnegative_integer(descriptor.get("seed"), f"{scenario_id}.seed"),
                "inputs_sha256": require_digest(descriptor.get("inputs_sha256"), f"{scenario_id}.inputs_sha256"),
                "contract_sha256": require_digest(descriptor.get("contract_sha256"), f"{scenario_id}.contract_sha256"),
            }
        )

    request = {
        "label": f"{suite['label']} · {group['id']}",
        "lane": suite["lane"],
        "model": suite["subject"]["model"],
        "provider": suite["subject"]["provider"],
        "scenarios": group["scenarios"],
        "runs": group["runs"],
        "seed": suite["seed"],
        "rotating_seeds": [],
        "technical_retries": group["technical_retries"],
        "progress_interval_seconds": suite.get("progress_interval_seconds", 15),
        "run_contract": {
            "mode": {"environment": "demonstration", "decision": "observe_only"},
            # The declaration carries a selector, so the versions here are the
            # ones the engine installed, read back before the run starts.
            "target": {
                "application": APPLICATION,
                "version": (installed or {}).get(APPLICATION, DEFAULT_SELECTOR),
                "stack": {
                    "mode": "registry",
                    "stack_versions": installed or {},
                    "stack_lock_digest": canonical_sha256(installed or {}),
                },
            },
            # The plan is this contract: what Release Control froze and sent.
            "plan": {
                "id": contract["execution_id"],
                "revision": str(contract["attempt"]),
                "sha256": canonical_sha256(contract),
                "catalog_sha256": catalog_sha256,
            },
            "runner": runner,
            "attempt": contract["attempt"],
            "selected_cases": selected_cases,
            "correlation": {
                "system": "release-control",
                "deployment_id": contract["campaign_id"],
                "operation_id": contract["campaign_id"],
            },
        },
    }
    if suite.get("agent_profile") is not None:
        request["agent"] = suite["agent_profile"]
    # The runner keys admission on the fully materialized request, including the
    # cases and their fingerprints — those only exist after scenarios-list, so
    # the dispatch key cannot be reused. Deterministic for transport retries.
    request["idempotency_key"] = observation_idempotency_key(request)
    return request


def observation_idempotency_key(request: dict[str, Any]) -> str:
    intent = {
        key: request[key]
        for key in (
            "run_contract",
            "lane",
            "model",
            "provider",
            "scenarios",
            "runs",
            "seed",
            "rotating_seeds",
            "technical_retries",
        )
    }
    if request.get("agent") is not None:
        intent["agent"] = request["agent"]
    return f"rc:e2e:{canonical_sha256(intent).removeprefix('sha256:')}"


def assignments(values: list[str], label: str) -> dict[str, str]:
    result: dict[str, str] = {}
    for value in values:
        key, separator, resolved = value.partition("=")
        if not separator or not key or key in result:
            raise ValueError(f"{label} values must be unique KEY=VALUE assignments")
        result[key] = resolved
    return result


def declared_workers(compose_path: Path) -> dict[str, str]:
    """The workers a compose project declares, with the selector each carries."""
    import yaml

    project = yaml.safe_load(compose_path.read_text())
    if not isinstance(project, dict):
        raise ValueError("compose project must be an object")
    declared: dict[str, str] = {}
    for container in (project.get("containers") or {}).values():
        source = str(container.get("worker", ""))
        if source.startswith("package://"):
            # Compose rewrites a declaration with the registry host it resolved
            # against, so workers are compared by the name they are known by.
            package = source.removeprefix("package://").rsplit("/", 1)[-1]
            declared[package] = str(container.get("version", DEFAULT_SELECTOR))
    return dict(sorted(declared.items()))


def project_roots(compose_path: Path) -> list[str]:
    return [f"{worker}@{selector}" for worker, selector in declared_workers(compose_path).items()]


def group_template(contract: dict[str, Any], group_id: str) -> str:
    group = next(group for group in contract["suite"]["groups"] if group["id"] == group_id)
    if "linkly_tutorial" not in group.get("scenarios", []):
        return contract["runtime"].get("template", {}).get("id", "")
    if (group["scenarios"] != ["linkly_tutorial"] or group["runs"] != 1
            or group["technical_retries"] != 0):
        raise ValueError("linkly_tutorial needs a fresh scaffold: one scenario, one run and no retries")
    return "linkly-agentic"


def project_engine_config(project: dict[str, Any], port: int) -> dict[str, Any]:
    workers = [{"name": "iii" + "-worker-manager", "config": {"host": "127.0.0.1", "port": port}}]
    workers.extend({"name": name, "config": config}
                   for name, config in project.get("engine", {}).get("workers", {}).items())
    return {"workers": workers}


def with_fixture(template: dict[str, Any], fixture: dict[str, Any]) -> dict[str, Any]:
    """Keep a scenario's required container names/configuration over the chosen base."""
    result = copy.deepcopy(template)
    containers = result.setdefault("containers", {})
    aliases = {"package://shell": "package://ide", "package://console": "package://ade"}
    for name, container in fixture.get("containers", {}).items():
        worker = aliases.get(container.get("worker"), container.get("worker"))
        for previous in list(containers):
            source = containers[previous].get("worker")
            if aliases.get(source, source) == worker:
                del containers[previous]
        containers[name] = copy.deepcopy(container)
    engine = result.setdefault("engine", {})
    engine_workers = engine.get("workers", {}) | fixture.get("engine", {}).get("workers", {})
    engine.update(fixture.get("engine", {}))
    if engine_workers:
        engine["workers"] = engine_workers
    return result


def scoped_config_name(namespace: str, name: str) -> str:
    """A configuration id both Compose and the engine accept.

    `<namespace>-<name>` when it fits their 64-character limit; otherwise the
    readable prefix with a stable digest, because Compose refuses to generate
    a truncated name and the engine refuses a longer id.
    """
    candidate = f"{namespace}-{name}"
    if len(candidate) <= 64:
        return candidate
    digest = hashlib.sha256(candidate.encode()).hexdigest()[:8]
    return f"{candidate[:55].rstrip('-')}-{digest}"

def project_scaffold(
    contract: dict[str, Any],
    namespace: str,
    data_dir: Path,
    env_file: str | None,
    environment: dict[str, str],
    template: dict[str, Any] | None = None,
    template_packages: dict[str, str] | None = None,
    profile_root: Path | None = None,
    base: dict[str, Any] | None = None,
    group_id: str | None = None,
) -> dict[str, Any]:
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,62}[a-z0-9]", namespace):
        raise ValueError("project namespace must be lowercase kebab-case")
    if not data_dir.is_absolute():
        raise ValueError("data directory must be absolute")

    scenarios = {
        scenario for group in contract["suite"]["groups"] for scenario in group.get("scenarios", [])
    }
    harness_override = {}
    if "fanout_ladder" in scenarios:
        harness_override["max_children"] = 16
    if "depth_ladder" in scenarios:
        harness_override["max_depth"] = 6

    declared_environment: dict[str, dict[str, str]] = {}
    for key, value in environment.items():
        worker, separator, name = key.partition(".")
        if not separator or not re.fullmatch(r"[A-Z][A-Z0-9_]*", name):
            raise ValueError(f"invalid container environment assignment: {key}")
        declared_environment.setdefault(worker, {})[name] = value

    overrides = contract.get("runtime", {}).get("stack") or {}
    if not isinstance(overrides, dict):
        raise ValueError("runtime.stack must be an object of worker selectors")

    # The project is the template when there is one and the base otherwise.
    # Add the runner and the campaign's provider to either project. Templates
    # only enable their own default providers; credentials alone cannot start
    # the provider selected by the campaign.
    if env_file and not Path(env_file).is_absolute():
        raise ValueError("env file must be absolute")
    manifest = copy.deepcopy(template if template is not None else (declared_base() if base is None else base))
    containers = manifest.setdefault("containers", {})
    containers.setdefault(RUNNER, {"worker": f"package://{RUNNER}"})
    if group_id is not None:
        group = next((group for group in contract["suite"]["groups"] if group["id"] == group_id), None)
        if group is None:
            raise ValueError(f"unknown campaign group: {group_id}")
        if {"form_flow_build", "state_machine_canvas_build"} & set(group["scenarios"]):
            if not any(item.get("worker") == "package://canvas" for item in containers.values()):
                if "canvas" in containers:
                    raise ValueError("container canvas is already used by another worker")
                containers["canvas"] = {"worker": "package://canvas"}
    provider = contract["suite"]["subject"]["provider"]
    if not re.fullmatch(r"[a-z][a-z0-9-]*", provider):
        raise ValueError("suite.subject.provider must be a provider package name")
    provider_package = f"provider-{provider}"
    provider_source = f"package://{provider_package}"
    if not any(container.get("worker") == provider_source for container in containers.values()):
        if provider_package in containers:
            raise ValueError(f"container {provider_package} is already used by another worker")
        containers[provider_package] = {"worker": provider_source}
    package_names: dict[str, list[str]] = {}
    for name, container in containers.items():
        source = container.get("worker", "")
        # Only the executor's private env files may supply credentials.
        container.pop("env_file", None)
        if source.startswith("path://"):
            path = Path(source.removeprefix("path://"))
            if not path.is_absolute() and (not source.startswith("path://./") or ".." in path.parts):
                raise ValueError(f"declared worker {name} must stay inside its project")
            continue
        package = source.removeprefix("package://")
        package = (template_packages or {}).get(package, package)
        if not source.startswith("package://"):
            raise ValueError(f"declared worker {name} has an unsupported source: {source}")
        container["worker"] = f"package://{package}"
        # Release Control may hold a worker to a particular release; everything
        # else runs latest. A template's own pin is its author's, not ours: a
        # fixture checked out at an old commit would otherwise downgrade the
        # very application under test.
        container["version"] = overrides.get(package, DEFAULT_SELECTOR)
        package_names.setdefault(package, []).append(name)
    if template is not None:
        # Compose expands dependencies by container name, so a template that
        # renamed a role has to point at the name its own project uses.
        for container in containers.values():
            if "start_after" in container:
                container["start_after"] = [
                    dependency if dependency in containers else package_names.get(
                        (template_packages or {}).get(dependency, dependency), [dependency]
                    )[0]
                    for dependency in container["start_after"]
                ]
    for name, container in containers.items():
        worker = container["worker"].removeprefix("package://")
        if worker in declared_environment:
            container.setdefault("environment", {}).update(sorted(declared_environment[worker].items()))
        if worker == RUNNER:
            container["config_name"] = scoped_config_name(namespace, "harness-e2e")
            container["config_override"] = {
                "data_dir": str(data_dir),
                "control_database": "primary",
                "control_namespace": namespace,
            }
        elif worker == APPLICATION and harness_override:
            container["config_name"] = scoped_config_name(namespace, "harness")
            container.setdefault("config_override", {}).update(harness_override)
        # Compose 0.24.2 derives `<namespace>-<container>` for the rest and
        # refuses to start when that exceeds 64 characters, which a long group
        # id reaches: only then name the container ourselves.
        if "config_name" not in container and len(f"{namespace}-{name}") > 64:
            container["config_name"] = scoped_config_name(namespace, name)
    if profile_root is not None:
        if not profile_root.is_absolute():
            raise ValueError("profile root must be absolute")
        if "iii-directory" not in package_names:
            containers["iii-directory"] = {
                "worker": "package://iii-directory",
                "version": DEFAULT_SELECTOR,
            }
            package_names["iii-directory"] = ["iii-directory"]
        for name in package_names["iii-directory"]:
            containers[name]["config_name"] = scoped_config_name(namespace, "directory")
            containers[name].setdefault("config_override", {}).update({
                "auto_download": False,
                "skills_folder": str(profile_root / ".iii/registry-skills"),
                "local_skills_folder": str(profile_root / "skills"),
                "agents_folder": str(profile_root / "agents"),
                "global_agents_folder": str(profile_root / ".iii/empty/agents"),
                "agents_skills_folder": str(profile_root / ".agents/skills"),
                "global_agents_skills_folder": str(profile_root / ".iii/empty/skills"),
            })
    # One env file, every container. A worker reads the keys it knows and
    # ignores the rest; the template's own `./.env` reference is replaced by it.
    if env_file:
        for container in containers.values():
            container["env_file"] = [env_file]
    manifest.update({
        "namespace": namespace,
        "startup_timeout": "5m",
        "stop_timeout": "30s",
        "containers": containers,
    })
    return manifest


def compose_evidence(
    contract: dict[str, Any],
    compose_path: Path,
    namespace: str,
    lifecycle: dict[str, Any],
    workers_payload: dict[str, Any],
    processes: dict[str, Any],
) -> dict[str, Any]:
    worker_rows = workers_payload.get("workers")
    if not isinstance(worker_rows, list):
        raise ValueError("engine worker evidence must contain a workers array")
    import yaml

    # What the project was asked to run is the compose it was assembled from.
    requested = declared_workers(compose_path)
    observed = observed_versions(workers_payload, namespace)
    # The engine lists a worker by its container name, which a template picks
    # freely (Linkly runs `ade` as `console`). Compose already gated the start;
    # a container the engine does not report is drift to show, never a reason
    # to discard a finished run.
    containers = (yaml.safe_load(compose_path.read_text()) or {}).get("containers") or {}
    missing = sorted(
        name for name, container in containers.items()
        if str(container.get("worker", "")).startswith("package://") and name not in observed
    )
    # The declaration carries a selector, so the versions that matter are the
    # ones the engine ended up installing. They are recorded, not compared.
    version_report_warnings: list[str] = []
    if missing:
        version_report_warnings.append(
            "containers the engine did not report: " + ", ".join(missing)
        )

    forbidden = "iii" + "-worker"
    for phase, rows in processes.items():
        if not isinstance(rows, list):
            raise ValueError(f"process inventory {phase} must be an array")
        for row in rows:
            if not isinstance(row, dict):
                raise ValueError(f"process inventory {phase} contains an invalid row")
            command = str(row.get("comm", ""))
            executable = str(row.get("args", "")).split(maxsplit=1)[0]
            if Path(command).name == forbidden or Path(executable).name == forbidden:
                raise ValueError(f"forbidden lifecycle executable observed during {phase}")

    return {
        "contract_sha256": canonical_sha256(contract),
        "compose_sha256": f"sha256:{hashlib.sha256(compose_path.read_bytes()).hexdigest()}",
        "namespace": namespace,
        "runtime": {
            "cli": contract["runtime"]["cli"],
            "requested_roots": dict(sorted(requested.items())),
            "observed_versions": dict(sorted(observed.items())),
            "version_report_warnings": version_report_warnings,
        },
        "lifecycle": lifecycle,
        "processes": processes,
        "forbidden_lifecycle_executable_absent": True,
    }


def validate_runtime_layout(artifact_root: Path, runtime_root: Path, allowed_root: Path) -> None:
    """Runtime state and provider secrets must never enter the uploaded tree."""
    artifact = artifact_root.resolve(strict=True)
    runtime = runtime_root.resolve(strict=True)
    allowed = allowed_root.resolve(strict=True)
    if artifact == allowed or not artifact.is_relative_to(allowed):
        raise ValueError("artifact root must remain below the canonical target directory")
    if artifact.is_relative_to(runtime) or runtime.is_relative_to(artifact):
        raise ValueError("runtime and artifact roots must not overlap")


def _package_files(root: Path) -> list[dict[str, Any]]:
    """Hash regular files without ever traversing or dereferencing symlinks."""
    if root.is_symlink():
        raise ValueError(f"artifact root must not be a symlink: {root}")
    resolved_root = root.resolve(strict=True)
    files: list[dict[str, Any]] = []
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root).as_posix()
        if path.is_symlink():
            raise ValueError(f"artifact tree contains symlink: {relative}")
        if (
            path.name == ".env"
            or path.name.startswith(".env.")
            or path.name.endswith(".env")
            or path.name in {"secrets", ".aws", ".ssh", ".gnupg"}
        ):
            raise ValueError(f"artifact tree contains a reserved credential path: {relative}")
        if not path.is_file() or path.name == "bundle-manifest.json":
            continue
        resolved = path.resolve(strict=True)
        try:
            resolved.relative_to(resolved_root)
        except ValueError as error:
            raise ValueError(f"artifact escapes root: {relative}") from error
        payload = path.read_bytes()
        files.append(
            {
                "path": relative,
                "sha256": f"sha256:{hashlib.sha256(payload).hexdigest()}",
                "size_bytes": len(payload),
            }
        )
    return files


def package_bundle(root: Path, contract: dict[str, Any], workflow: dict[str, Any]) -> dict[str, Any]:
    files = _package_files(root)
    return {
        "schema": "e2e-observation-bundle",
        "campaign_id": contract["campaign_id"],
        "execution_id": contract["execution_id"],
        "attempt": contract["attempt"],
        "contract_sha256": canonical_sha256(contract),
        "workflow": workflow,
        "terminal_payload": "results.json" if (root / "results.json").is_file() else None,
        "failure_payload": "failure.json" if (root / "failure.json").is_file() else None,
        "files": files,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("validate", "digest", "manifest", "groups"):
        command = commands.add_parser(name)
        command.add_argument("--contract", type=Path, required=True)
        if name == "manifest":
            command.add_argument("--output", type=Path, required=True)
    materialize = commands.add_parser("materialize")
    materialize.add_argument("--contract", type=Path, required=True)
    materialize.add_argument("--catalog", type=Path, required=True)
    materialize.add_argument("--workers", type=Path)
    materialize.add_argument("--namespace")
    materialize.add_argument("--output", type=Path, required=True)
    materialize.add_argument("--group-id")
    roots = commands.add_parser("roots")
    roots.add_argument("--compose", type=Path, required=True)
    template = commands.add_parser("group-template")
    template.add_argument("--contract", type=Path, required=True)
    template.add_argument("--group-id", required=True)
    project = commands.add_parser("project")
    project.add_argument("--contract", type=Path, required=True)
    project.add_argument("--group-id")
    project.add_argument("--namespace", required=True)
    project.add_argument("--data-dir", type=Path, required=True)
    project.add_argument("--env-file", type=Path)
    project.add_argument("--environment", action="append", default=[])
    project.add_argument("--output", type=Path, required=True)
    project.add_argument("--template-compose", type=Path)
    project.add_argument("--base-compose", type=Path)
    project.add_argument("--fixture-compose", type=Path)
    project.add_argument("--profile-root", type=Path)
    project.add_argument("--template-package", action="append", default=[])
    project.add_argument("--engine-config", type=Path)
    project.add_argument("--engine-port", type=int, default=49134)
    evidence = commands.add_parser("compose-evidence")
    evidence.add_argument("--contract", type=Path, required=True)
    evidence.add_argument("--compose", type=Path, required=True)
    evidence.add_argument("--namespace", required=True)
    for name in ("add", "up", "status", "down", "workers", "process-before", "process-during", "process-after"):
        evidence.add_argument(f"--{name}", type=Path, required=True)
    evidence.add_argument("--output", type=Path, required=True)
    package = commands.add_parser("package")
    package.add_argument("--root", type=Path, required=True)
    package.add_argument("--contract", type=Path, required=True)
    package.add_argument("--workflow", required=True)
    package.add_argument("--output", type=Path, required=True)
    layout = commands.add_parser("validate-layout")
    layout.add_argument("--artifact-root", type=Path, required=True)
    layout.add_argument("--runtime-root", type=Path, required=True)
    layout.add_argument("--allowed-root", type=Path, required=True)
    args = parser.parse_args()

    try:
        if args.command == "validate-layout":
            validate_runtime_layout(args.artifact_root, args.runtime_root, args.allowed_root)
            return 0
        if args.command == "roots":
            # The declaration answers this one; there is no contract to read.
            for root in project_roots(args.compose):
                print(root)
            return 0
        contract = validate_contract(load_object(args.contract, "contract"))
        if args.command == "validate":
            print(canonical(contract))
        elif args.command == "digest":
            print(canonical_sha256(contract))
        elif args.command == "groups":
            for group in contract["suite"]["groups"]:
                print(group["id"])
        elif args.command == "manifest":
            args.output.write_text(json.dumps(campaign_manifest(contract), indent=2) + "\n")
        elif args.command == "materialize":
            installed = observed_versions(
                load_object(args.workers, "engine workers"), args.namespace
            ) if args.workers else {}
            request = materialize_request(
                contract, load_object(args.catalog, "scenario catalog"),
                group_id=args.group_id, installed=installed,
            )
            args.output.write_text(json.dumps(request, indent=2, sort_keys=True) + "\n")
        elif args.command == "project":
            try:
                import yaml
            except ImportError as error:  # pragma: no cover - CI installs PyYAML explicitly.
                raise ValueError("PyYAML is required to create the iii project scaffold") from error
            template = yaml.safe_load(args.template_compose.read_text()) if args.template_compose else None
            base = None
            if args.base_compose:
                base = yaml.safe_load(args.base_compose.read_text())
                if not isinstance(base, dict):
                    raise ValueError("base compose must be an object")
            if args.fixture_compose:
                if template is None:
                    raise ValueError("fixture-compose requires template-compose")
                # Selected local workers still belong to their own scaffold,
                # while the scenario keeps its canonical task directory.
                for container in template.get("containers", {}).values():
                    source = container.get("worker", "")
                    if source.startswith("path://./"):
                        container["worker"] = f"path://{(args.template_compose.parent / source.removeprefix('path://')).resolve()}"
                template = with_fixture(template, yaml.safe_load(args.fixture_compose.read_text()))
            manifest = project_scaffold(
                contract,
                args.namespace,
                args.data_dir,
                str(args.env_file) if args.env_file else None,
                assignments(args.environment, "environment"),
                template,
                assignments(args.template_package, "template-package"),
                args.profile_root,
                base,
                args.group_id,
            )
            if "engine" in manifest:
                manifest["engine"]["url"] = f"ws://127.0.0.1:{args.engine_port}"
            args.output.write_text(yaml.safe_dump(manifest, sort_keys=False))
            if args.engine_config:
                args.engine_config.write_text(yaml.safe_dump(project_engine_config(manifest, args.engine_port), sort_keys=False))
        elif args.command == "group-template":
            print(group_template(contract, args.group_id))
        elif args.command == "compose-evidence":
            manifest = compose_evidence(
                contract,
                args.compose,
                args.namespace,
                {name: load_object(getattr(args, name), f"compose {name}") for name in ("add", "up", "status", "down")},
                load_object(args.workers, "engine workers"),
                {
                    "before": json.loads(args.process_before.read_text()),
                    "during": json.loads(args.process_during.read_text()),
                    "after": json.loads(args.process_after.read_text()),
                },
            )
            args.output.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        else:
            workflow = json.loads(args.workflow)
            if not isinstance(workflow, dict):
                raise ValueError("workflow must be a JSON object")
            args.output.write_text(json.dumps(package_bundle(args.root, contract, workflow), indent=2, sort_keys=True) + "\n")
        return 0
    except (ValueError, json.JSONDecodeError) as error:
        print(f"error: {error}")
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
