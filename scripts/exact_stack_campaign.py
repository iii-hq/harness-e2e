#!/usr/bin/env python3
"""Validate and materialize the exact-stack campaign contract.

Release Control owns every campaign decision — the suite, the models, the
weights, the policy — and states each one exactly once in the contract it
dispatches. This repository owns the runtime: which scenarios a pinned runner
release can execute, how the stack boots, and what evidence comes back. No
campaign configuration is read from this repository, and nothing is verified
twice: unknown fields are ignored so either side can add one and ship alone.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path
from typing import Any
from uuid import UUID


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
    "fault_injection",
}
ORCHESTRATION_ROLES = {"target", "runtime", "runner"}
ORCHESTRATION_KINDS = {"binary", "engine"}
APPLICATION = "harness"


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


def require_uuid(value: Any, label: str) -> str:
    if not isinstance(value, str):
        raise ValueError(f"{label} must be a UUID")
    parsed = UUID(value)
    if str(parsed) != value.lower():
        raise ValueError(f"{label} must use canonical UUID form")
    return str(parsed)


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
        {"id", "label", "lane", "seed", "subject", "judge", "groups"},
        "suite",
    )
    suite_id = require_text(suite.get("id"), "suite.id")
    if not re.fullmatch(r"[a-z][a-z0-9-]{0,63}", suite_id):
        raise ValueError("suite.id must be kebab-case")
    require_text(suite.get("label"), "suite.label")
    require_text(suite.get("lane"), "suite.lane")
    require_positive_integer(suite.get("seed"), "suite.seed")
    validate_identity(suite, "subject")
    validate_identity(suite, "judge")

    groups = suite.get("groups")
    if not isinstance(groups, list) or not groups:
        raise ValueError("suite.groups must be a non-empty array")
    seen: set[str] = set()
    for index, group in enumerate(groups):
        label = f"suite.groups[{index}]"
        group = require_keys(
            group,
            {"id", "execution_kind", "runs", "technical_retries", "weight"},
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
        weight = group.get("weight")
        if weight not in {1, 2, 3, 4, 5}:
            raise ValueError(f"{label}.weight must be 1-5")
        if kind == "fault_injection":
            require_text(group.get("fault_profile"), f"{label}.fault_profile")
            require_text(group.get("fault_scenario"), f"{label}.fault_scenario")
            require_nonnegative_integer(group.get("soak_minutes"), f"{label}.soak_minutes")
            continue
        scenarios = group.get("scenarios")
        if not isinstance(scenarios, list) or not scenarios or len(set(scenarios)) != len(scenarios):
            raise ValueError(f"{label}.scenarios must be a non-empty unique array")
        for scenario in scenarios:
            require_text(scenario, f"{label}.scenarios[]")
    return suite


def validate_orchestration(contract: dict[str, Any]) -> dict[str, Any]:
    orchestration = require_keys(
        contract.get("orchestration"),
        {"roots", "nodes", "edges", "graph_sha256"},
        "orchestration",
    )
    roots = orchestration["roots"]
    nodes = orchestration["nodes"]
    edges = orchestration["edges"]
    if not isinstance(roots, list) or not roots:
        raise ValueError("orchestration.roots must be a non-empty array")
    if not isinstance(nodes, list) or not nodes:
        raise ValueError("orchestration.nodes must be a non-empty array")
    if not isinstance(edges, list):
        raise ValueError("orchestration.edges must be an array")

    root_keys: list[tuple[str, str, str]] = []
    for index, root in enumerate(roots):
        root = require_keys(root, {"worker", "version", "role"}, f"orchestration.roots[{index}]")
        worker = require_text(root["worker"], f"orchestration.roots[{index}].worker")
        version = require_version(root["version"], f"orchestration.roots[{index}].version")
        role = require_text(root["role"], f"orchestration.roots[{index}].role")
        if role not in ORCHESTRATION_ROLES:
            raise ValueError(f"orchestration.roots[{index}].role is unknown")
        root_keys.append((role, worker, version))
    if root_keys != sorted(root_keys) or len(set(root_keys)) != len(root_keys):
        raise ValueError("orchestration.roots must be unique and ordered by role, worker, version")

    node_versions: dict[str, str] = {}
    for index, node in enumerate(nodes):
        node = require_keys(node, {"worker", "version", "kind"}, f"orchestration.nodes[{index}]")
        worker = require_text(node["worker"], f"orchestration.nodes[{index}].worker")
        version = require_version(node["version"], f"orchestration.nodes[{index}].version")
        kind = require_text(node["kind"], f"orchestration.nodes[{index}].kind")
        if kind not in ORCHESTRATION_KINDS:
            raise ValueError(f"orchestration node {worker} has forbidden kind {kind}")
        if worker in node_versions:
            raise ValueError(f"orchestration contains more than one version of {worker}")
        node_versions[worker] = version
        if kind == "binary":
            artifact = require_keys(
                node.get("artifact"), {"target", "url", "sha256"}, f"orchestration.nodes[{index}].artifact"
            )
            require_text(artifact["target"], f"orchestration.nodes[{index}].artifact.target")
            url = require_text(artifact["url"], f"orchestration.nodes[{index}].artifact.url")
            if not url.startswith("https://"):
                raise ValueError(f"orchestration node {worker} artifact URL must use https")
            require_digest(artifact["sha256"], f"orchestration.nodes[{index}].artifact.sha256")
        elif "artifact" in node:
            raise ValueError(f"engine node {worker} must not carry an artifact")
    if [node["worker"] for node in nodes] != sorted(node_versions):
        raise ValueError("orchestration.nodes must be ordered by worker")

    normalized_edges: list[tuple[str, str]] = []
    for index, edge in enumerate(edges):
        edge = require_keys(edge, {"from", "to"}, f"orchestration.edges[{index}]")
        source = require_text(edge["from"], f"orchestration.edges[{index}].from")
        destination = require_text(edge["to"], f"orchestration.edges[{index}].to")
        if source not in node_versions or destination not in node_versions or source == destination:
            raise ValueError("orchestration edge must connect two distinct declared nodes")
        normalized_edges.append((source, destination))
    if normalized_edges != sorted(normalized_edges) or len(set(normalized_edges)) != len(normalized_edges):
        raise ValueError("orchestration.edges must be unique and ordered by from, to")

    for role, worker, version in root_keys:
        if node_versions.get(worker) != version:
            raise ValueError(f"orchestration root {role}:{worker}@{version} is absent from nodes")
    if len([root for role, root, _ in root_keys if role == "runner"]) != 1:
        raise ValueError("orchestration must declare exactly one runner root")
    if APPLICATION not in node_versions:
        raise ValueError(f"orchestration must include the {APPLICATION} node under test")

    graph = {"roots": roots, "nodes": nodes, "edges": edges}
    if require_digest(orchestration["graph_sha256"], "orchestration.graph_sha256") != canonical_sha256(graph):
        raise ValueError("orchestration.graph_sha256 does not match the canonical graph")
    return orchestration


def validate_contract(contract: dict[str, Any]) -> dict[str, Any]:
    require_keys(
        contract,
        {
            "schema",
            "campaign_id",
            "execution_id",
            "attempt",
            "idempotency_key",
            "stack_revision",
            "orchestration",
            "runtime",
            "security",
            "suite",
        },
        "contract",
    )
    if contract.get("schema") != CONTRACT_SCHEMA:
        raise ValueError(f"contract.schema must be {CONTRACT_SCHEMA}")
    require_uuid(contract.get("campaign_id"), "campaign_id")
    require_uuid(contract.get("execution_id"), "execution_id")
    require_positive_integer(contract.get("attempt"), "attempt")
    key = require_text(contract.get("idempotency_key"), "idempotency_key")
    if not re.fullmatch(r"rc:e2e:[0-9a-f]{64}", key):
        raise ValueError("idempotency_key must be rc:e2e:<sha256>")
    revision = require_text(contract.get("stack_revision"), "stack_revision")
    if not GIT_SHA.fullmatch(revision):
        raise ValueError("stack_revision must be a full lowercase git SHA")

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

    security = require_keys(contract.get("security"), {"oidc_audience"}, "security")
    audience = require_text(security.get("oidc_audience"), "security.oidc_audience")
    if not re.fullmatch(r"[A-Za-z0-9._:/-]+", audience):
        raise ValueError("security.oidc_audience contains unsupported characters")

    validate_suite(contract.get("suite"))
    validate_orchestration(contract)
    return contract


def runner_worker(contract: dict[str, Any]) -> str:
    return next(root["worker"] for root in contract["orchestration"]["roots"] if root["role"] == "runner")


def role_versions(orchestration: dict[str, Any], role: str) -> dict[str, str]:
    adjacency: dict[str, set[str]] = {}
    for edge in orchestration["edges"]:
        adjacency.setdefault(edge["from"], set()).add(edge["to"])
    pending = [root["worker"] for root in orchestration["roots"] if root["role"] == role]
    reachable: set[str] = set()
    while pending:
        worker = pending.pop()
        if worker in reachable:
            continue
        reachable.add(worker)
        pending.extend(sorted(adjacency.get(worker, set()), reverse=True))
    versions = {node["worker"]: node["version"] for node in orchestration["nodes"] if node["worker"] in reachable}
    return dict(sorted(versions.items()))


def campaign_matrix(contract: dict[str, Any]) -> dict[str, Any]:
    return {
        "include": [
            {
                "group_id": group["id"],
                "execution_kind": group["execution_kind"],
                "runs_on": (
                    ["self-hosted", "harness-e2e"]
                    if group["execution_kind"] == "fault_injection"
                    else ["ubuntu-latest"]
                ),
            }
            for group in contract["suite"]["groups"]
        ]
    }


def admission_outputs(
    contract: dict[str, Any],
    campaign_id: str,
    execution_id: str,
    attempt: int,
    test_plan_id: str,
) -> list[str]:
    """Everything the workflow needs, so the workflow needs to know nothing.

    The dispatch inputs are checked against the contract here rather than in
    YAML: this file is pinned per campaign by runner_sha, so the contract can
    change shape without the workflow — which is read from the default branch —
    ever having to change with it.
    """
    if (
        contract["campaign_id"] != campaign_id
        or contract["execution_id"] != execution_id
        or contract["attempt"] != attempt
        or contract["suite"]["id"] != test_plan_id
    ):
        raise ValueError("dispatch inputs do not describe this contract")
    return [
        f"contract_sha256={canonical_sha256(contract)}",
        f"matrix={canonical(campaign_matrix(contract))}",
        f"oidc_audience={contract['security']['oidc_audience']}",
    ]


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
            "difficulty_weight": group["weight"],
        }
        if group["execution_kind"] == "fault_injection":
            materialized |= {
                "fault_profile": group["fault_profile"],
                "fault_scenario": group["fault_scenario"],
                "soak_minutes": group["soak_minutes"],
            }
        else:
            materialized["scenarios"] = group["scenarios"]
        groups.append(materialized)
    return {
        "kind": "harness-e2e-campaign",
        "campaign_id": suite["id"],
        "lane": suite["lane"],
        "failure_policy": "advisory",
        "scoring_profile": "difficulty-weighted-v1",
        "groups": groups,
    }


def materialize_request(
    contract: dict[str, Any], catalog: dict[str, Any], group_id: str | None = None
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
    if group["execution_kind"] == "fault_injection":
        raise ValueError("fault injection groups are executed by the protected supervisor")

    selected_cases: list[dict[str, Any]] = []
    for scenario_id in group["scenarios"]:
        descriptor = by_id.get(scenario_id)
        if descriptor is None:
            raise ValueError(f"{runner['name']}@{runner['version']} has no scenario {scenario_id}")
        selected_cases.append(
            {
                "scenario_id": scenario_id,
                "scenario_version": require_positive_integer(
                    descriptor.get("scenario_version"), f"{scenario_id}.scenario_version"
                ),
                "case_id": require_text(descriptor.get("case_id"), f"{scenario_id}.case_id"),
                "seed": require_nonnegative_integer(descriptor.get("seed"), f"{scenario_id}.seed"),
                "inputs_sha256": require_digest(descriptor.get("inputs_sha256"), f"{scenario_id}.inputs_sha256"),
                "contract_sha256": require_digest(descriptor.get("contract_sha256"), f"{scenario_id}.contract_sha256"),
            }
        )

    target_stack = role_versions(contract["orchestration"], "target")
    request = {
        "label": f"{suite['label']} · {group['id']} · Harness {target_stack[APPLICATION]}",
        "lane": suite["lane"],
        "model": suite["subject"]["model"],
        "provider": suite["subject"]["provider"],
        "judge_model": suite["judge"]["model"],
        "judge_provider": suite["judge"]["provider"],
        "scenarios": group["scenarios"],
        "runs": group["runs"],
        "seed": suite["seed"],
        "rotating_seeds": [],
        "technical_retries": group["technical_retries"],
        "progress_interval_seconds": suite.get("progress_interval_seconds", 15),
        "run_contract": {
            "mode": {"environment": "demonstration", "decision": "observe_only"},
            "target": {
                "application": APPLICATION,
                "version": target_stack[APPLICATION],
                "stack": {
                    "mode": "registry",
                    "stack_versions": target_stack,
                    "stack_lock_digest": canonical_sha256(target_stack),
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
            "judge_model",
            "judge_provider",
            "scenarios",
            "runs",
            "seed",
            "rotating_seeds",
            "technical_retries",
        )
    }
    return f"rc:e2e:{canonical_sha256(intent).removeprefix('sha256:')}"


def assignments(values: list[str], label: str) -> dict[str, str]:
    result: dict[str, str] = {}
    for value in values:
        key, separator, resolved = value.partition("=")
        if not separator or not key or key in result:
            raise ValueError(f"{label} values must be unique KEY=VALUE assignments")
        result[key] = resolved
    return result


def project_roots(contract: dict[str, Any]) -> list[str]:
    return [f"{root['worker']}@{root['version']}" for root in contract["orchestration"]["roots"]]


def project_scaffold(
    contract: dict[str, Any],
    namespace: str,
    data_dir: Path,
    env_files: dict[str, str],
    environment: dict[str, str],
) -> dict[str, Any]:
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,62}[a-z0-9]", namespace):
        raise ValueError("project namespace must be lowercase kebab-case")
    if not data_dir.is_absolute():
        raise ValueError("data directory must be absolute")

    orchestration = contract["orchestration"]
    roots = {root["worker"]: root["version"] for root in orchestration["roots"]}
    unknown_env_files = sorted(set(env_files) - set(roots))
    if unknown_env_files:
        raise ValueError(f"env files name unknown project roots: {', '.join(unknown_env_files)}")

    declared_environment: dict[str, dict[str, str]] = {}
    for key, value in environment.items():
        worker, separator, name = key.partition(".")
        if not separator or worker not in roots or not re.fullmatch(r"[A-Z][A-Z0-9_]*", name):
            raise ValueError(f"invalid container environment assignment: {key}")
        declared_environment.setdefault(worker, {})[name] = value

    # The runner reads its own registry stack identity from the environment.
    target_stack = role_versions(orchestration, "target")
    declared_environment.setdefault(runner_worker(contract), {}).update(
        {
            "HARNESS_E2E_WORKERS_REPOSITORY": "iii-hq/workers",
            "HARNESS_E2E_WORKERS_REVISION": contract["stack_revision"],
            "HARNESS_E2E_STACK_MODE": "registry",
            "HARNESS_E2E_STACK_VERSIONS": canonical(target_stack),
            "HARNESS_E2E_STACK_DIGEST": canonical_sha256(target_stack),
        }
    )

    containers: dict[str, Any] = {}
    for worker in sorted(roots):
        container: dict[str, Any] = {
            "worker": f"package://{worker}",
            "version": roots[worker],
        }
        if worker in env_files:
            env_file = Path(env_files[worker])
            if not env_file.is_absolute():
                raise ValueError(f"env file for {worker} must be absolute")
            container["env_file"] = [str(env_file)]
        if worker in declared_environment:
            container["environment"] = dict(sorted(declared_environment[worker].items()))
        if worker == runner_worker(contract):
            container["config_name"] = f"{namespace}-harness-e2e"
            container["config_override"] = {"data_dir": str(data_dir)}
        containers[worker] = container

    return {
        "namespace": namespace,
        "startup_timeout": "5m",
        "stop_timeout": "30s",
        "containers": containers,
    }


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
    requested = {
        root["worker"]: root["version"]
        for root in contract["orchestration"]["roots"]
    }
    observed: dict[str, str] = {}
    for row in worker_rows:
        if not isinstance(row, dict) or row.get("namespace") not in (None, namespace):
            continue
        name = row.get("name")
        version = row.get("version")
        if isinstance(name, str) and isinstance(version, str):
            observed[name] = version
    missing = [worker for worker in sorted(requested) if worker not in observed]
    if missing:
        raise ValueError("iii project is missing requested roots: " + ", ".join(missing))
    # Artifact identity is enforced by the sha256-pinned lock at download time.
    # A registered worker whose self-reported metadata version differs from the
    # registry version is a fleet metadata defect, recorded as a warning rather
    # than disproof of the stack.
    version_report_warnings = [
        f"{worker}: registry {version}, self-reported {observed[worker]}"
        for worker, version in sorted(requested.items())
        if observed[worker] != version
    ]

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
        "orchestration_graph_sha256": contract["orchestration"]["graph_sha256"],
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
        "schema": "e2e-observation-bundle/v1",
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
    for name in ("validate", "digest", "matrix", "manifest", "groups"):
        command = commands.add_parser(name)
        command.add_argument("--contract", type=Path, required=True)
        if name == "manifest":
            command.add_argument("--output", type=Path, required=True)
    admit = commands.add_parser("admit")
    admit.add_argument("--contract", type=Path, required=True)
    admit.add_argument("--campaign-id", required=True)
    admit.add_argument("--execution-id", required=True)
    admit.add_argument("--attempt", type=int, required=True)
    admit.add_argument("--test-plan-id", required=True)
    materialize = commands.add_parser("materialize")
    materialize.add_argument("--contract", type=Path, required=True)
    materialize.add_argument("--catalog", type=Path, required=True)
    materialize.add_argument("--output", type=Path, required=True)
    materialize.add_argument("--group-id")
    roots = commands.add_parser("roots")
    roots.add_argument("--contract", type=Path, required=True)
    project = commands.add_parser("project")
    project.add_argument("--contract", type=Path, required=True)
    project.add_argument("--namespace", required=True)
    project.add_argument("--data-dir", type=Path, required=True)
    project.add_argument("--env-file", action="append", default=[])
    project.add_argument("--environment", action="append", default=[])
    project.add_argument("--output", type=Path, required=True)
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
        contract = validate_contract(load_object(args.contract, "contract"))
        if args.command == "validate":
            print(canonical(contract))
        elif args.command == "digest":
            print(canonical_sha256(contract))
        elif args.command == "matrix":
            print(canonical(campaign_matrix(contract)))
        elif args.command == "groups":
            for group in contract["suite"]["groups"]:
                print(group["id"])
        elif args.command == "admit":
            for line in admission_outputs(
                contract, args.campaign_id, args.execution_id, args.attempt, args.test_plan_id
            ):
                print(line)
        elif args.command == "manifest":
            args.output.write_text(json.dumps(campaign_manifest(contract), indent=2) + "\n")
        elif args.command == "materialize":
            request = materialize_request(
                contract, load_object(args.catalog, "scenario catalog"), group_id=args.group_id
            )
            args.output.write_text(json.dumps(request, indent=2, sort_keys=True) + "\n")
        elif args.command == "roots":
            for root in project_roots(contract):
                print(root)
        elif args.command == "project":
            try:
                import yaml
            except ImportError as error:  # pragma: no cover - CI installs PyYAML explicitly.
                raise ValueError("PyYAML is required to create the iii project scaffold") from error
            manifest = project_scaffold(
                contract,
                args.namespace,
                args.data_dir,
                assignments(args.env_file, "env-file"),
                assignments(args.environment, "environment"),
            )
            args.output.write_text(yaml.safe_dump(manifest, sort_keys=False))
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
