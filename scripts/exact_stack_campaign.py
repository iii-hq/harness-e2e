#!/usr/bin/env python3
"""Validate and materialize Release Control campaigns owned by Harness E2E."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path
from typing import Any
from uuid import UUID


SHA256 = re.compile(r"^sha256:[0-9a-f]{64}$")
VERSION = re.compile(
    r"^[0-9]+\.[0-9]+\.[0-9]+"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)
CAMPAIGN_EXECUTION_KINDS = {
    "harness_turn",
    "scripted_dialogue",
    "composite_flow",
    "adaptive_flow",
    "fault_injection",
}
DIFFICULTY_WEIGHTS = {
    "L0": 1,
    "L1": 1,
    "L2": 2,
    "L3": 3,
    "L4": 4,
    "L5": 5,
}
CAMPAIGN_REPOSITORY = "iii-hq/harness-e2e"
CAMPAIGN_WORKFLOW = "exact-stack-e2e.yml"
SUPPORTED_CATALOG_SCHEMAS = {
    "e2e-scenario-catalog/v1",
    "e2e-scenario-catalog/v2",
    "e2e-scenario-catalog/v4",
}
ORCHESTRATION_ROLES = {"target", "runtime", "runner"}
ORCHESTRATION_KINDS = {"binary", "engine"}


def load_object(path: Path, label: str) -> dict[str, Any]:
    value = json.loads(path.read_text())
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be a JSON object")
    return value


def canonical(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True)


def canonical_sha256(value: Any) -> str:
    return f"sha256:{hashlib.sha256(canonical(value).encode()).hexdigest()}"


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


def require_version_map(value: Any, label: str) -> dict[str, str]:
    if not isinstance(value, dict) or not value:
        raise ValueError(f"{label} must be a non-empty object")
    result: dict[str, str] = {}
    for worker, version in value.items():
        if not isinstance(worker, str) or not re.fullmatch(r"[a-z0-9][a-z0-9_-]*", worker):
            raise ValueError(f"{label} contains an invalid worker name")
        if not isinstance(version, str) or not VERSION.fullmatch(version):
            raise ValueError(f"{label} contains an invalid version for {worker}")
        result[worker] = version
    return result


def require_positive_integer(value: Any, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 1:
        raise ValueError(f"{label} must be a positive integer")
    return value


def require_nonnegative_integer(value: Any, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise ValueError(f"{label} must be a non-negative integer")
    return value


def require_keys(
    value: Any,
    required: set[str],
    optional: set[str],
    label: str,
) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be an object")
    keys = set(value)
    missing = sorted(required - keys)
    unknown = sorted(keys - required - optional)
    if missing:
        raise ValueError(f"{label} is missing fields: {', '.join(missing)}")
    if unknown:
        raise ValueError(f"{label} contains unknown fields: {', '.join(unknown)}")
    return value


def validate_identity(definition: dict[str, Any], role: str) -> None:
    identity = require_keys(
        definition.get(role), {"provider", "model"}, set(), f"plan.definition.{role}"
    )
    require_text(identity.get("provider"), f"plan.definition.{role}.provider")
    require_text(identity.get("model"), f"plan.definition.{role}.model")


def validate_campaign_definition(definition: dict[str, Any]) -> None:
    require_keys(
        definition,
        {
            "mode",
            "entrypoint",
            "lane",
            "label",
            "failurePolicy",
            "subject",
            "judge",
            "manifest",
            "scoring",
            "catalog",
            "groups",
            "progressIntervalSeconds",
            "retentionClass",
            "executor",
            "runner",
            "testRuntime",
        },
        set(),
        "plan.definition",
    )
    if definition.get("entrypoint") != "e2e::run":
        raise ValueError("campaign plan must use e2e::run")
    require_text(definition.get("label"), "plan.definition.label")
    lane = require_text(definition.get("lane"), "plan.definition.lane")
    if lane not in {"manual", "daily", "weekly", "post-deploy"}:
        raise ValueError("campaign lane must be manual, daily, weekly, or post-deploy")
    if definition.get("failurePolicy") != "advisory":
        raise ValueError("campaign failurePolicy must be advisory")
    validate_identity(definition, "subject")
    validate_identity(definition, "judge")

    manifest = require_keys(
        definition.get("manifest"), {"id", "sha256"}, set(), "plan.definition.manifest"
    )
    require_text(manifest.get("id"), "plan.definition.manifest.id")
    require_digest(manifest.get("sha256"), "plan.definition.manifest.sha256")

    scoring = require_keys(
        definition.get("scoring"), {"profile", "sha256"}, set(), "plan.definition.scoring"
    )
    if scoring.get("profile") != "difficulty-weighted-v1":
        raise ValueError("campaign scoring profile must be difficulty-weighted-v1")
    require_digest(scoring.get("sha256"), "plan.definition.scoring.sha256")

    catalog = require_keys(
        definition.get("catalog"), {"revision", "sha256", "seed"}, set(), "plan.definition.catalog"
    )
    require_text(catalog.get("revision"), "plan.definition.catalog.revision")
    require_digest(catalog.get("sha256"), "plan.definition.catalog.sha256")
    require_positive_integer(catalog.get("seed"), "plan.definition.catalog.seed")
    require_positive_integer(
        definition.get("progressIntervalSeconds"), "plan.definition.progressIntervalSeconds"
    )
    if definition.get("retentionClass") != "longitudinal":
        raise ValueError("plan.definition.retentionClass must be longitudinal")

    executor = require_keys(
        definition.get("executor"),
        {"provider", "repository", "workflow", "ref", "oidcAudience"},
        set(),
        "plan.definition.executor",
    )
    if executor != {
        "provider": "github_actions",
        "repository": CAMPAIGN_REPOSITORY,
        "workflow": CAMPAIGN_WORKFLOW,
        "ref": "main",
        "oidcAudience": "release-control-harness-e2e",
    }:
        raise ValueError("plan.definition.executor must name the canonical advisory workflow")
    plan_runner = require_keys(
        definition.get("runner"),
        {
            "registryWorker",
            "registryRef",
            "revision",
            "catalogSha256",
            "manifestSha256",
            "scoringProfileSha256",
            "assetsSha256",
        },
        set(),
        "plan.definition.runner",
    )
    for field in ("catalogSha256", "manifestSha256", "scoringProfileSha256", "assetsSha256"):
        require_digest(plan_runner.get(field), f"plan.definition.runner.{field}")
    runtime = require_keys(
        definition.get("testRuntime"),
        {"cliVersion", "cliTarget", "cliAsset", "cliSha256", "workers"},
        set(),
        "plan.definition.testRuntime",
    )
    cli_version = require_text(runtime.get("cliVersion"), "plan.definition.testRuntime.cliVersion")
    if not VERSION.fullmatch(cli_version):
        raise ValueError("plan.definition.testRuntime.cliVersion must be exact")
    require_text(runtime.get("cliTarget"), "plan.definition.testRuntime.cliTarget")
    require_text(runtime.get("cliAsset"), "plan.definition.testRuntime.cliAsset")
    require_digest(runtime.get("cliSha256"), "plan.definition.testRuntime.cliSha256")
    require_version_map(runtime.get("workers"), "plan.definition.testRuntime.workers")

    groups = definition.get("groups")
    if not isinstance(groups, list) or not groups:
        raise ValueError("campaign groups must be a non-empty array")
    seen: set[str] = set()
    for index, group in enumerate(groups):
        label = f"plan.definition.groups[{index}]"
        if not isinstance(group, dict):
            raise ValueError(f"{label} must be an object")
        is_fault_shape = group.get("executionKind") == "fault_injection"
        require_keys(
            group,
            {"id", "executionKind", "scenarios", "runs", "technicalRetries", "difficultyTier", "difficultyWeight"}
            | ({"faultProfile", "faultScenario", "soakMinutes"} if is_fault_shape else set()),
            set(),
            label,
        )
        group_id = require_text(group.get("id"), f"{label}.id")
        if not re.fullmatch(r"[a-z][a-z0-9-]{0,63}", group_id) or group_id in seen:
            raise ValueError("campaign group ids must be unique kebab-case values")
        seen.add(group_id)
        execution_kind = require_text(group.get("executionKind"), f"{label}.executionKind")
        if execution_kind not in CAMPAIGN_EXECUTION_KINDS:
            raise ValueError(f"{label}.executionKind is unsupported")
        runs = require_positive_integer(group.get("runs"), f"{label}.runs")
        retries = require_nonnegative_integer(
            group.get("technicalRetries"), f"{label}.technicalRetries"
        )
        tier = require_text(group.get("difficultyTier"), f"{label}.difficultyTier")
        expected_weight = DIFFICULTY_WEIGHTS.get(tier)
        if expected_weight is None or group.get("difficultyWeight") != expected_weight:
            raise ValueError(f"{label}.difficultyWeight does not match {tier}")
        scenarios = group.get("scenarios")
        if execution_kind == "fault_injection":
            if scenarios not in (None, []):
                raise ValueError(f"{label}.scenarios must be empty for fault injection")
            if retries != 0 or runs < 3 or group.get("soakMinutes") != 60:
                raise ValueError(
                    f"{label} fault injection requires runs>=3, technicalRetries=0, and soakMinutes=60"
                )
            require_text(group.get("faultProfile"), f"{label}.faultProfile")
            require_text(group.get("faultScenario"), f"{label}.faultScenario")
        else:
            if not isinstance(scenarios, list) or not scenarios or len(set(scenarios)) != len(scenarios):
                raise ValueError(f"{label}.scenarios must be a non-empty unique array")
            if not all(isinstance(item, str) and item for item in scenarios):
                raise ValueError(f"{label}.scenarios must contain non-empty strings")


def campaign_matrix(contract: dict[str, Any]) -> dict[str, Any]:
    validate_contract(contract)
    definition = contract["plan"]["definition"]
    include = []
    for group in definition["groups"]:
        is_fault = group["executionKind"] == "fault_injection"
        include.append(
            {
                "group_id": group["id"],
                "execution_kind": group["executionKind"],
                "runs_on": (
                    ["self-hosted", "harness-e2e"]
                    if is_fault
                    else ["ubuntu-latest"]
                ),
            }
        )
    return {"include": include}


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


def validate_exact_contract(contract: dict[str, Any], target: dict[str, Any], runner: dict[str, Any]) -> None:
    origin = target.get("origin")
    if origin is not None:
        require_keys(
            origin,
            {"operation_id", "step_id", "worker", "version", "source_sha", "deployment_run_id", "deployment_run_attempt"},
            set(),
            "target.origin",
        )
        require_uuid(origin.get("operation_id"), "target.origin.operation_id")
        require_uuid(origin.get("step_id"), "target.origin.step_id")
        require_text(origin.get("worker"), "target.origin.worker")
        origin_version = require_text(origin.get("version"), "target.origin.version")
        if not VERSION.fullmatch(origin_version):
            raise ValueError("target.origin.version must be exact")
        origin_sha = require_text(origin.get("source_sha"), "target.origin.source_sha")
        if not re.fullmatch(r"[0-9a-f]{40}", origin_sha):
            raise ValueError("target.origin.source_sha must be a full lowercase git SHA")
        require_positive_integer(origin.get("deployment_run_id"), "target.origin.deployment_run_id")
        require_positive_integer(origin.get("deployment_run_attempt"), "target.origin.deployment_run_attempt")

    base = require_keys(target.get("base"), {"kind", "id"}, set(), "target.base")
    if base.get("kind") not in {"deployment", "snapshot"}:
        raise ValueError("target.base.kind must be deployment or snapshot")
    require_uuid(base.get("id"), "target.base.id")

    runtime = require_keys(contract.get("runtime"), {"cli"}, set(), "runtime")
    cli = require_keys(runtime.get("cli"), {"version", "target", "asset", "sha256"}, set(), "runtime.cli")
    cli_version = require_text(cli.get("version"), "runtime.cli.version")
    if not VERSION.fullmatch(cli_version):
        raise ValueError("runtime.cli.version must be an exact version")
    require_text(cli.get("target"), "runtime.cli.target")
    asset = require_text(cli.get("asset"), "runtime.cli.asset")
    if not asset.startswith("iii-") or asset.startswith("iii-" + "worker"):
        raise ValueError("runtime.cli.asset must name the iii CLI archive")
    require_digest(cli.get("sha256"), "runtime.cli.sha256")

    runner_ref = require_text(runner.get("registry_ref"), "runner.registry_ref")
    if not VERSION.fullmatch(runner_ref):
        raise ValueError("runner.registry_ref must be an exact version")
    runner_revision = runner.get("revision")
    if not isinstance(runner_revision, str) or not re.fullmatch(r"[0-9a-f]{40}", runner_revision):
        raise ValueError("runner.revision must be a full lowercase git SHA")

    security = require_keys(contract.get("security"), {"oidc_audience"}, set(), "security")
    audience = require_text(security.get("oidc_audience"), "security.oidc_audience")
    if not re.fullmatch(r"[A-Za-z0-9._:/-]+", audience):
        raise ValueError("security.oidc_audience contains unsupported characters")


def validate_orchestration(contract: dict[str, Any]) -> dict[str, Any]:
    orchestration = require_keys(
        contract.get("orchestration"),
        {"roots", "nodes", "edges", "graph_sha256"},
        set(),
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
        root = require_keys(root, {"worker", "version", "role"}, set(), f"orchestration.roots[{index}]")
        worker = require_text(root["worker"], f"orchestration.roots[{index}].worker")
        version = require_text(root["version"], f"orchestration.roots[{index}].version")
        role = require_text(root["role"], f"orchestration.roots[{index}].role")
        if not VERSION.fullmatch(version) or role not in ORCHESTRATION_ROLES:
            raise ValueError("orchestration roots require an exact version and a known role")
        root_keys.append((role, worker, version))
    if root_keys != sorted(root_keys) or len(set(root_keys)) != len(root_keys):
        raise ValueError("orchestration.roots must be unique and ordered by role, worker, version")

    node_versions: dict[str, str] = {}
    node_kinds: dict[str, str] = {}
    for index, node in enumerate(nodes):
        node = require_keys(node, {"worker", "version", "kind"}, {"artifact"}, f"orchestration.nodes[{index}]")
        worker = require_text(node["worker"], f"orchestration.nodes[{index}].worker")
        version = require_text(node["version"], f"orchestration.nodes[{index}].version")
        kind = require_text(node["kind"], f"orchestration.nodes[{index}].kind")
        if not VERSION.fullmatch(version):
            raise ValueError(f"orchestration node {worker} does not have an exact version")
        if kind not in ORCHESTRATION_KINDS:
            raise ValueError(f"orchestration node {worker} has forbidden kind {kind}")
        if worker in node_versions:
            raise ValueError(f"orchestration contains more than one version of {worker}")
        node_versions[worker] = version
        node_kinds[worker] = kind
        if kind == "binary":
            artifact = require_keys(
                node.get("artifact"), {"target", "url", "sha256"}, set(), f"orchestration.nodes[{index}].artifact"
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
        edge = require_keys(edge, {"from", "to"}, set(), f"orchestration.edges[{index}]")
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
    if node_versions.get("harness") != contract["target"]["version"]:
        raise ValueError("target.version must match the orchestrated harness node")
    runner = contract["runner"]
    if node_versions.get(runner["registry_worker"]) != runner["registry_ref"]:
        raise ValueError("runner pin must match its orchestrated node")

    graph = {"roots": roots, "nodes": nodes, "edges": edges}
    if require_digest(orchestration["graph_sha256"], "orchestration.graph_sha256") != canonical_sha256(graph):
        raise ValueError("orchestration.graph_sha256 does not match the canonical graph")
    return orchestration


def validate_contract(contract: dict[str, Any]) -> dict[str, Any]:
    require_keys(
        contract,
        {
            "campaign_id",
            "execution_id",
            "attempt",
            "idempotency_key",
            "target",
            "plan",
            "runner",
            "workflow",
            "runtime",
            "security",
            "orchestration",
        },
        set(),
        "stack lock",
    )
    require_uuid(contract.get("campaign_id"), "campaign_id")
    require_uuid(contract.get("execution_id"), "execution_id")
    attempt = contract.get("attempt")
    if not isinstance(attempt, int) or attempt < 1:
        raise ValueError("attempt must be a positive integer")
    key = require_text(contract.get("idempotency_key"), "idempotency_key")
    if not re.fullmatch(r"rc:e2e:[0-9a-f]{64}", key):
        raise ValueError("idempotency_key must be rc:e2e:<sha256>")

    target = require_keys(
        contract.get("target"),
        {"application", "version", "source_sha", "deployment_id", "origin", "base"},
        set(),
        "target",
    )
    if target.get("application") != "harness":
        raise ValueError("target application must be harness")
    target_version = require_text(target.get("version"), "target.version")
    if not VERSION.fullmatch(target_version):
        raise ValueError("target.version must be exact")
    if target.get("source_sha") is not None and not re.fullmatch(r"[0-9a-f]{40}", target["source_sha"]):
        raise ValueError("target.source_sha must be a full lowercase git SHA when present")
    if target.get("deployment_id") is not None:
        require_uuid(target["deployment_id"], "target.deployment_id")
    plan = require_keys(contract.get("plan"), {"id", "revision", "sha256", "definition"}, set(), "plan")
    require_uuid(plan.get("id"), "plan.id")
    revision = plan.get("revision")
    if not isinstance(revision, int) or revision < 1:
        raise ValueError("plan.revision must be a positive integer")
    require_digest(plan.get("sha256"), "plan.sha256")
    definition = plan.get("definition")
    if not isinstance(definition, dict):
        raise ValueError("plan.definition must be an object")
    if definition.get("mode") != "campaign":
        raise ValueError("plan mode must be campaign")
    validate_campaign_definition(definition)

    runner = require_keys(
        contract.get("runner"),
        {
            "registry_worker",
            "registry_ref",
            "revision",
            "catalog_sha256",
            "manifest_sha256",
            "scoring_profile_sha256",
            "assets_sha256",
        },
        set(),
        "runner",
    )
    if require_text(runner.get("registry_worker"), "runner.registry_worker") != "harness-e2e":
        raise ValueError("runner.registry_worker must be harness-e2e")
    runner_ref = require_text(runner.get("registry_ref"), "runner.registry_ref")
    if not re.fullmatch(r"[A-Za-z0-9._-]+", runner_ref):
        raise ValueError("runner.registry_ref is invalid")
    validate_exact_contract(contract, target, runner)
    require_digest(runner.get("catalog_sha256"), "runner.catalog_sha256")
    require_digest(runner.get("manifest_sha256"), "runner.manifest_sha256")
    require_digest(
        runner.get("scoring_profile_sha256"),
        "runner.scoring_profile_sha256",
    )
    require_digest(runner.get("assets_sha256"), "runner.assets_sha256")
    if runner.get("catalog_sha256") != definition["catalog"]["sha256"]:
        raise ValueError("runner catalog digest must match the campaign definition")
    if runner.get("manifest_sha256") != definition["manifest"]["sha256"]:
        raise ValueError("runner manifest digest must match the campaign definition")
    if runner.get("scoring_profile_sha256") != definition["scoring"]["sha256"]:
        raise ValueError("runner scoring digest must match the campaign definition")
    definition_runner = definition["runner"]
    if {
        "registry_worker": definition_runner["registryWorker"],
        "registry_ref": definition_runner["registryRef"],
        "revision": definition_runner["revision"],
        "catalog_sha256": definition_runner["catalogSha256"],
        "manifest_sha256": definition_runner["manifestSha256"],
        "scoring_profile_sha256": definition_runner["scoringProfileSha256"],
        "assets_sha256": definition_runner["assetsSha256"],
    } != runner:
        raise ValueError("runner identity must match the immutable campaign definition")
    workflow = require_keys(contract.get("workflow"), {"repository", "file", "ref"}, set(), "workflow")
    if workflow.get("repository") != CAMPAIGN_REPOSITORY:
        raise ValueError(f"workflow.repository must be {CAMPAIGN_REPOSITORY}")
    if workflow.get("file") != CAMPAIGN_WORKFLOW:
        raise ValueError(f"workflow.file must be {CAMPAIGN_WORKFLOW}")
    if workflow.get("ref") != "main":
        raise ValueError("workflow.ref must be main")
    if workflow != {
        "repository": definition["executor"]["repository"],
        "file": definition["executor"]["workflow"],
        "ref": definition["executor"]["ref"],
    }:
        raise ValueError("workflow must match the immutable campaign definition")
    if contract["security"]["oidc_audience"] != definition["executor"]["oidcAudience"]:
        raise ValueError("security audience must match the immutable campaign definition")
    orchestration = validate_orchestration(contract)
    runtime_definition = definition["testRuntime"]
    if contract["runtime"]["cli"] != {
        "version": runtime_definition["cliVersion"],
        "target": runtime_definition["cliTarget"],
        "asset": runtime_definition["cliAsset"],
        "sha256": runtime_definition["cliSha256"],
    }:
        raise ValueError("runtime.cli must match the immutable campaign definition")
    runtime_roots = {
        root["worker"]: root["version"]
        for root in orchestration["roots"]
        if root["role"] == "runtime"
    }
    if runtime_roots != runtime_definition["workers"]:
        raise ValueError("orchestration runtime roots must match the immutable campaign definition")
    return contract


def materialize_request(
    contract: dict[str, Any], catalog: dict[str, Any], group_id: str | None = None
) -> dict[str, Any]:
    validate_contract(contract)
    if catalog.get("schema") not in SUPPORTED_CATALOG_SCHEMAS:
        raise ValueError("unsupported scenario catalog schema")
    runner = catalog.get("runner")
    if not isinstance(runner, dict):
        raise ValueError("scenario catalog has no runner identity")
    for field in ("name", "version", "revision"):
        require_text(runner.get(field), f"catalog.runner.{field}")
    require_digest(catalog.get("catalog_sha256"), "catalog.catalog_sha256")
    catalog_asset_sha256 = canonical_sha256(catalog)
    expected_runner = contract["runner"]
    if runner.get("name") != expected_runner["registry_worker"] or runner.get("version") != expected_runner["registry_ref"]:
        raise ValueError("scenario catalog runner does not match the exact runner pin")
    if runner.get("revision") != expected_runner["revision"]:
        raise ValueError("scenario catalog runner revision does not match the contract")
    if catalog_asset_sha256 != expected_runner["catalog_sha256"]:
        raise ValueError("scenario catalog digest does not match the contract")
    descriptors = catalog.get("scenarios")
    if not isinstance(descriptors, list):
        raise ValueError("scenario catalog scenarios must be a list")
    by_id: dict[str, dict[str, Any]] = {}
    for item in descriptors:
        if isinstance(item, dict) and isinstance(item.get("scenario_id"), str):
            by_id[item["scenario_id"]] = item

    definition = contract["plan"]["definition"]
    group = next(
        (item for item in definition["groups"] if item["id"] == group_id), None
    )
    if group is None:
        raise ValueError("a valid campaign group id is required")
    if group["executionKind"] == "fault_injection":
        raise ValueError("fault injection groups are executed by the protected supervisor")
    scenarios = group["scenarios"]
    runs = group["runs"]
    technical_retries = group["technicalRetries"]
    plan_seed = definition["catalog"]["seed"]
    label = f"{definition['label']} · {group['id']}"
    selected_cases: list[dict[str, Any]] = []
    for scenario_id in scenarios:
        descriptor = by_id.get(scenario_id)
        if descriptor is None:
            raise ValueError(f"scenario catalog is missing {scenario_id}")
        scenario_version = descriptor.get("scenario_version")
        descriptor_seed = descriptor.get("seed")
        if not isinstance(scenario_version, int) or scenario_version < 1:
            raise ValueError(f"scenario {scenario_id} has an invalid version")
        if not isinstance(descriptor_seed, int) or descriptor_seed < 0:
            raise ValueError(f"scenario {scenario_id} has an invalid seed")
        selected_cases.append(
            {
                "scenario_id": scenario_id,
                "scenario_version": scenario_version,
                "case_id": require_text(descriptor.get("case_id"), f"{scenario_id}.case_id"),
                "seed": descriptor_seed,
                "inputs_sha256": require_digest(
                    descriptor.get("inputs_sha256"), f"{scenario_id}.inputs_sha256"
                ),
                "contract_sha256": require_digest(
                    descriptor.get("contract_sha256"), f"{scenario_id}.contract_sha256"
                ),
            }
        )

    target_stack = role_versions(contract["orchestration"], "target")
    target_stack_digest = canonical_sha256(target_stack)
    run_contract = {
        "mode": {"environment": "demonstration", "decision": "observe_only"},
        "target": {
            "application": "harness",
            "version": contract["target"]["version"],
            "stack": {
                "mode": "registry",
                "stack_versions": target_stack,
                "stack_lock_digest": target_stack_digest,
            },
        },
        "plan": {
            # Keep this shape aligned with the runner's native
            # ObservationPlanIdentity. Campaign-only group, manifest, and
            # scoring identity remain in the stack lock and root bundle.
            "id": contract["plan"]["id"],
            "revision": str(contract["plan"]["revision"]),
            "sha256": contract["plan"]["sha256"],
            "catalog_sha256": catalog["catalog_sha256"],
        },
        "runner": runner,
        "attempt": contract["attempt"],
        "selected_cases": selected_cases,
        "correlation": {
            "system": "release-control",
            "deployment_id": contract["target"].get("deployment_id") or contract["campaign_id"],
            "operation_id": contract["campaign_id"],
        },
    }
    request = {
        "label": f"{label} · Harness {contract['target']['version']}",
        "lane": definition["lane"],
        "model": definition["subject"]["model"],
        "provider": definition["subject"]["provider"],
        "judge_model": definition["judge"]["model"],
        "judge_provider": definition["judge"]["provider"],
        "scenarios": scenarios,
        "runs": runs,
        "seed": plan_seed,
        "rotating_seeds": [],
        "technical_retries": technical_retries,
        "progress_interval_seconds": definition.get("progressIntervalSeconds", 15),
        "run_contract": run_contract,
    }
    # The runner validates a Release Control key over the fully materialized request,
    # including cases and their contract fingerprints. Those fields only exist
    # after scenarios-list, so the GitHub dispatch key cannot be reused here.
    # This remains deterministic for transport retries of the same catalog.
    request["idempotency_key"] = observation_idempotency_key(request)
    return request


def observation_idempotency_key(request: dict[str, Any]) -> str:
    intent = {
        "run_contract": request["run_contract"],
        "lane": request["lane"],
        "model": request["model"],
        "provider": request["provider"],
        "judge_model": request["judge_model"],
        "judge_provider": request["judge_provider"],
        "scenarios": request["scenarios"],
        "runs": request["runs"],
        "seed": request["seed"],
        "rotating_seeds": request["rotating_seeds"],
        "technical_retries": request["technical_retries"],
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


def materialize_compose(
    contract: dict[str, Any],
    namespace: str,
    data_dir: Path,
    env_files: dict[str, str],
    environment: dict[str, str],
) -> dict[str, Any]:
    validate_contract(contract)
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,62}[a-z0-9]", namespace):
        raise ValueError("project namespace must be lowercase kebab-case")
    if not data_dir.is_absolute():
        raise ValueError("data directory must be absolute")

    orchestration = contract["orchestration"]
    nodes = {node["worker"]: node for node in orchestration["nodes"]}
    binary_workers = {worker for worker, node in nodes.items() if node["kind"] == "binary"}
    unknown_env_files = sorted(set(env_files) - binary_workers)
    if unknown_env_files:
        raise ValueError(f"env files name unknown containers: {', '.join(unknown_env_files)}")

    declared_environment: dict[str, dict[str, str]] = {}
    for key, value in environment.items():
        worker, separator, name = key.partition(".")
        if not separator or worker not in binary_workers or not re.fullmatch(r"[A-Z][A-Z0-9_]*", name):
            raise ValueError(f"invalid container environment assignment: {key}")
        declared_environment.setdefault(worker, {})[name] = value

    dependencies: dict[str, list[str]] = {worker: [] for worker in binary_workers}
    for edge in orchestration["edges"]:
        if edge["from"] in binary_workers and edge["to"] in binary_workers:
            dependencies[edge["from"]].append(edge["to"])

    runner_worker = contract["runner"]["registry_worker"]
    runner_environment = declared_environment.setdefault(runner_worker, {})
    runner_environment.update(
        {
            "HARNESS_E2E_WORKERS_REPOSITORY": "iii-hq/workers",
            "HARNESS_E2E_WORKERS_REVISION": contract["target"]["source_sha"],
            "HARNESS_E2E_STACK_MODE": "registry",
            "HARNESS_E2E_STACK_DIGEST": contract["orchestration"]["graph_sha256"],
        }
    )

    containers: dict[str, Any] = {}
    for worker in sorted(binary_workers):
        node = nodes[worker]
        container: dict[str, Any] = {
            "worker": f"package://{worker}",
            "version": node["version"],
        }
        needs = sorted(set(dependencies[worker]))
        if needs:
            container["depends_on"] = needs
        if worker in env_files:
            env_file = Path(env_files[worker])
            if not env_file.is_absolute():
                raise ValueError(f"env file for {worker} must be absolute")
            container["env_file"] = [str(env_file)]
        if worker in declared_environment:
            container["environment"] = dict(sorted(declared_environment[worker].items()))
        if worker == runner_worker:
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
    daemon_namespace: str,
    project_namespace: str,
    lifecycle: dict[str, Any],
    workers_payload: dict[str, Any],
    processes: dict[str, Any],
) -> dict[str, Any]:
    validate_contract(contract)
    worker_rows = workers_payload.get("workers")
    if not isinstance(worker_rows, list):
        raise ValueError("engine worker evidence must contain a workers array")
    expected = {
        node["worker"]: node["version"]
        for node in contract["orchestration"]["nodes"]
        if node["kind"] == "binary"
    }
    observed: dict[str, str] = {}
    for row in worker_rows:
        if not isinstance(row, dict) or row.get("namespace") not in (None, project_namespace):
            continue
        name = row.get("name")
        version = row.get("version")
        if isinstance(name, str) and isinstance(version, str) and name in expected:
            observed[name] = version
    mismatches = [
        f"{worker}: expected {version}, observed {observed.get(worker, 'missing')}"
        for worker, version in sorted(expected.items())
        if observed.get(worker) != version
    ]
    if mismatches:
        raise ValueError("compose runtime version mismatch: " + "; ".join(mismatches))

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

    compose_bytes = compose_path.read_bytes()
    return {
        "stack_lock_sha256": canonical_sha256(contract),
        "orchestration_graph_sha256": contract["orchestration"]["graph_sha256"],
        "compose_sha256": f"sha256:{hashlib.sha256(compose_bytes).hexdigest()}",
        "namespaces": {"daemon": daemon_namespace, "project": project_namespace},
        "runtime": {
            "cli": contract["runtime"]["cli"],
            "expected_versions": dict(sorted(expected.items())),
            "observed_versions": dict(sorted(observed.items())),
        },
        "lifecycle": lifecycle,
        "processes": processes,
        "forbidden_lifecycle_executable_absent": True,
    }


def package_bundle(root: Path, contract: dict[str, Any], workflow: dict[str, Any]) -> dict[str, Any]:
    validate_contract(contract)
    files = []
    for path in sorted(root.rglob("*")):
        if not path.is_file() or path.name == "bundle-manifest.json":
            continue
        relative = path.relative_to(root).as_posix()
        payload = path.read_bytes()
        files.append(
            {
                "path": relative,
                "sha256": f"sha256:{hashlib.sha256(payload).hexdigest()}",
                "size_bytes": len(payload),
            }
        )
    terminal = root / "results.json"
    failure = root / "failure.json"
    return {
        "schema": "e2e-observation-bundle/v1",
        "campaign_id": contract["campaign_id"],
        "execution_id": contract["execution_id"],
        "attempt": contract["attempt"],
        "stack_lock_sha256": canonical_sha256(contract),
        "workflow": workflow,
        "terminal_payload": "results.json" if terminal.is_file() else None,
        "failure_payload": "failure.json" if failure.is_file() else None,
        "files": files,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    validate = commands.add_parser("validate")
    validate.add_argument("--contract", type=Path, required=True)
    digest = commands.add_parser("digest")
    digest.add_argument("--contract", type=Path, required=True)
    materialize = commands.add_parser("materialize")
    materialize.add_argument("--contract", type=Path, required=True)
    materialize.add_argument("--catalog", type=Path, required=True)
    materialize.add_argument("--output", type=Path, required=True)
    materialize.add_argument("--group-id")
    matrix = commands.add_parser("matrix")
    matrix.add_argument("--contract", type=Path, required=True)
    compose = commands.add_parser("compose")
    compose.add_argument("--contract", type=Path, required=True)
    compose.add_argument("--namespace", required=True)
    compose.add_argument("--data-dir", type=Path, required=True)
    compose.add_argument("--env-file", action="append", default=[])
    compose.add_argument("--environment", action="append", default=[])
    compose.add_argument("--output", type=Path, required=True)
    evidence = commands.add_parser("compose-evidence")
    evidence.add_argument("--contract", type=Path, required=True)
    evidence.add_argument("--compose", type=Path, required=True)
    evidence.add_argument("--daemon-namespace", required=True)
    evidence.add_argument("--project-namespace", required=True)
    for name in ("validate", "up", "status", "down", "workers", "process-before", "process-during", "process-after"):
        evidence.add_argument(f"--{name}", type=Path, required=True)
    evidence.add_argument("--output", type=Path, required=True)
    package = commands.add_parser("package")
    package.add_argument("--root", type=Path, required=True)
    package.add_argument("--contract", type=Path, required=True)
    package.add_argument("--workflow", required=True)
    package.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        contract = validate_contract(load_object(args.contract, "stack lock"))
        if args.command == "validate":
            print(canonical(contract))
        elif args.command == "digest":
            print(canonical_sha256(contract))
        elif args.command == "materialize":
            request = materialize_request(
                contract,
                load_object(args.catalog, "scenario catalog"),
                group_id=args.group_id,
            )
            args.output.write_text(json.dumps(request, indent=2, sort_keys=True) + "\n")
        elif args.command == "matrix":
            print(canonical(campaign_matrix(contract)))
        elif args.command == "compose":
            try:
                import yaml
            except ImportError as error:  # pragma: no cover - CI installs PyYAML explicitly.
                raise ValueError("PyYAML is required to materialize worker-compose.yaml") from error
            manifest = materialize_compose(
                contract,
                args.namespace,
                args.data_dir,
                assignments(args.env_file, "env-file"),
                assignments(args.environment, "environment"),
            )
            args.output.write_text(yaml.safe_dump(manifest, sort_keys=False))
        elif args.command == "compose-evidence":
            lifecycle = {
                name: load_object(getattr(args, name), f"compose {name}")
                for name in ("validate", "up", "status", "down")
            }
            processes = {
                "before": json.loads(args.process_before.read_text()),
                "during": json.loads(args.process_during.read_text()),
                "after": json.loads(args.process_after.read_text()),
            }
            manifest = compose_evidence(
                contract,
                args.compose,
                args.daemon_namespace,
                args.project_namespace,
                lifecycle,
                load_object(args.workers, "engine workers"),
                processes,
            )
            args.output.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        else:
            workflow = json.loads(args.workflow)
            if not isinstance(workflow, dict):
                raise ValueError("workflow must be a JSON object")
            manifest = package_bundle(args.root, contract, workflow)
            args.output.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        return 0
    except (ValueError, json.JSONDecodeError) as error:
        print(f"error: {error}")
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
