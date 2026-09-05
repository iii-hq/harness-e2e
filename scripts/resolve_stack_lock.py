#!/usr/bin/env python3
"""Build the exact-stack contract this runner will execute.

Release Control names a profile of the master test plan and a stack policy; it
resolves nothing. The composition is the runner's, so this script turns

    profile snapshot (harness-e2e test-plan materialize) + stack policy + CLI version

into one `rc-e2e/v2` contract per materialized campaign — the same shape
`exact_stack_campaign.py` has always validated, now assembled here instead of
arriving over the wire. Every worker version is resolved to an exact,
immutable one before it reaches Compose; `latest` never survives into a
contract, because a campaign has to be able to say afterwards what it ran.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any


REGISTRY_API_URL = os.environ.get("HARNESS_E2E_REGISTRY_API_URL", "https://api.workers.iii.dev")
GITHUB_API_URL = os.environ.get("GITHUB_API_URL", "https://api.github.com")
III_REPOSITORY = "iii-hq/iii"
WORKERS_REPOSITORY = "iii-hq/workers"

CONTRACT_SCHEMA = "rc-e2e/v2"
CLI_TARGET = "x86_64-unknown-linux-gnu"
CLI_ASSET = f"iii-{CLI_TARGET}.tar.gz"

#: The application under test. Its Registry graph is the stack a campaign measures.
TARGET_ROOT = "harness"
#: Workers the measurement itself needs, and which are therefore not under test.
RUNTIME_ROOTS = ("browser", "fp", "provider-deepseek", "provider-zai")
#: The runner executing the scenarios inside that stack.
RUNNER_ROOT = "harness-e2e"

EXACT_VERSION = re.compile(
    r"^[0-9]+\.[0-9]+\.[0-9]+"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)
GIT_SHA = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^(?:sha256:)?[0-9a-f]{64}$")


class ResolutionError(RuntimeError):
    """A stack the campaign cannot be assembled from."""


def canonical(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True)


def canonical_sha256(value: Any) -> str:
    return f"sha256:{hashlib.sha256(canonical(value).encode()).hexdigest()}"


def get_json(url: str, payload: dict[str, Any] | None = None, token: str | None = None) -> Any:
    body = json.dumps(payload).encode() if payload is not None else None
    headers = {"Accept": "application/json"}
    if payload is not None:
        headers["Content-Type"] = "application/json"
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(url, data=body, headers=headers)
    # Only load shedding and gateway faults are worth another call; every other
    # answer is the service's considered one about this exact question.
    last: Exception | None = None
    for attempt in range(3):
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                return json.loads(response.read().decode())
        except urllib.error.HTTPError as error:
            if error.code != 429 and error.code < 500:
                raise ResolutionError(f"{url} answered HTTP {error.code}") from error
            last = error
        except (OSError, ValueError) as error:
            last = error
        time.sleep(0.5 * (attempt + 1))
    raise ResolutionError(f"{url} did not answer: {last}")


def normalize_sha256(value: Any, label: str) -> str:
    if not isinstance(value, str) or not SHA256.fullmatch(value):
        raise ResolutionError(f"{label} is not a SHA-256 checksum")
    return value if value.startswith("sha256:") else f"sha256:{value}"


def resolve_graph(worker: str, selector: str, target: str) -> dict[str, Any]:
    """Resolve one root to its exact version and the complete graph Compose needs."""
    payload = get_json(f"{REGISTRY_API_URL}/resolve", {"worker": worker, "version": selector, "target": target})
    root = payload.get("root") if isinstance(payload, dict) else None
    graph = payload.get("graph") if isinstance(payload, dict) else None
    raw_edges = payload.get("edges") if isinstance(payload, dict) else None
    if not isinstance(root, dict) or not isinstance(graph, list) or not isinstance(raw_edges, list):
        raise ResolutionError(f"Registry answer for {worker}@{selector} is incomplete")
    version = root.get("version")
    if root.get("name") != worker or not isinstance(version, str) or not EXACT_VERSION.fullmatch(version):
        raise ResolutionError(f"Registry did not resolve {worker}@{selector} to an exact version")

    nodes = []
    for entry in graph:
        if not isinstance(entry, dict):
            raise ResolutionError(f"Registry graph of {worker}@{version} has a malformed node")
        name, node_version, kind = entry.get("name"), entry.get("version"), entry.get("type")
        if not isinstance(name, str) or not isinstance(node_version, str) or not EXACT_VERSION.fullmatch(node_version):
            raise ResolutionError(f"Registry graph of {worker}@{version} has an unpinned node")
        if kind == "engine":
            nodes.append({"worker": name, "version": node_version, "kind": kind})
            continue
        if kind != "binary":
            raise ResolutionError(f"node {name}@{node_version} has unsupported artifact kind {kind}")
        artifact = (entry.get("binaries") or {}).get(target)
        if not isinstance(artifact, dict):
            raise ResolutionError(f"node {name}@{node_version} has no {target} artifact")
        url = artifact.get("url")
        if not isinstance(url, str) or not url.startswith("https://"):
            raise ResolutionError(f"node {name}@{node_version} artifact URL must use HTTPS")
        nodes.append(
            {
                "worker": name,
                "version": node_version,
                "kind": kind,
                "artifact": {
                    "target": target,
                    "url": url,
                    "sha256": normalize_sha256(artifact.get("sha256"), f"node {name}@{node_version} checksum"),
                },
            }
        )

    edges = []
    for entry in raw_edges:
        if not isinstance(entry, dict) or not isinstance(entry.get("from"), str) or not isinstance(entry.get("to"), str):
            raise ResolutionError(f"Registry graph of {worker}@{version} has a malformed edge")
        edges.append({"from": entry["from"], "to": entry["to"]})
    return {"root": {"worker": worker, "version": version}, "nodes": nodes, "edges": edges}


def merge_graphs(roles: dict[str, str], graphs: list[dict[str, Any]]) -> dict[str, Any]:
    """One graph for the whole project. Two versions of a worker cannot compose."""
    nodes: dict[str, dict[str, Any]] = {}
    edges: dict[tuple[str, str], dict[str, str]] = {}
    for graph in graphs:
        for node in graph["nodes"]:
            previous = nodes.get(node["worker"])
            if previous and previous["version"] != node["version"]:
                raise ResolutionError(
                    f"stack needs two versions of {node['worker']}: {previous['version']} and {node['version']}"
                )
            nodes.setdefault(node["worker"], node)
        for edge in graph["edges"]:
            if edge["from"] == edge["to"]:
                continue
            edges.setdefault((edge["from"], edge["to"]), edge)

    roots = sorted(
        (
            {"worker": graph["root"]["worker"], "version": graph["root"]["version"], "role": roles[graph["root"]["worker"]]}
            for graph in graphs
        ),
        key=lambda root: (root["role"], root["worker"], root["version"]),
    )
    graph = {
        "roots": roots,
        "nodes": [nodes[worker] for worker in sorted(nodes)],
        "edges": [edges[key] for key in sorted(edges)],
    }
    return {**graph, "graph_sha256": canonical_sha256(graph)}


def resolve_cli(version: str, token: str | None) -> dict[str, str]:
    """The digest of the exact `iii` archive the campaign installs."""
    if not EXACT_VERSION.fullmatch(version):
        raise ResolutionError(f"cli_version {version} is not an exact version")
    release = get_json(f"{GITHUB_API_URL}/repos/{III_REPOSITORY}/releases/tags/iii/v{version}", token=token)
    for asset in release.get("assets") or []:
        if asset.get("name") == CLI_ASSET:
            return {
                "version": version,
                "target": CLI_TARGET,
                "asset": CLI_ASSET,
                "sha256": normalize_sha256(asset.get("digest"), f"iii/v{version} {CLI_ASSET} digest"),
            }
    raise ResolutionError(f"release iii/v{version} publishes no {CLI_ASSET}")


def resolve_stack_revision(harness_version: str, token: str | None) -> str:
    """The `iii-hq/workers` commit the stack under test was released from."""
    ref = get_json(
        f"{GITHUB_API_URL}/repos/{WORKERS_REPOSITORY}/git/ref/tags/harness/v{harness_version}", token=token
    )
    obj = ref.get("object") or {}
    for _ in range(5):
        sha, kind = obj.get("sha"), obj.get("type")
        if kind == "commit" and isinstance(sha, str) and GIT_SHA.fullmatch(sha):
            return sha
        if kind != "tag":
            raise ResolutionError(f"harness/v{harness_version} points at unsupported object {kind}")
        obj = (get_json(f"{GITHUB_API_URL}/repos/{WORKERS_REPOSITORY}/git/tags/{sha}", token=token).get("object")) or {}
    raise ResolutionError(f"harness/v{harness_version} nests too many annotated tags")


def suite_groups(campaign: dict[str, Any]) -> list[dict[str, Any]]:
    """A materialized campaign's groups, in the shape the contract states them."""
    groups = []
    for group in campaign.get("groups") or []:
        materialized = {
            "id": group["id"],
            "execution_kind": group["execution_kind"],
            "runs": group["runs"],
            "technical_retries": group["technical_retries"],
            "weight": group["difficulty_weight"],
        }
        if group["execution_kind"] == "fault_injection":
            materialized |= {
                "fault_profile": group["fault_profile"],
                "fault_scenario": group["fault_scenario"],
                "soak_minutes": group["soak_minutes"],
            }
        else:
            materialized["scenarios"] = list(group["scenarios"])
        groups.append(materialized)
    return groups


def build_contract(
    campaign: dict[str, Any],
    *,
    execution_id: str,
    snapshot: dict[str, Any],
    plan: dict[str, Any],
    orchestration: dict[str, Any],
    cli: dict[str, str],
    stack_revision: str,
    oidc_audience: str,
) -> dict[str, Any]:
    profile = snapshot["profile"]
    suite = {
        "id": campaign["campaign_id"],
        "label": f"{profile['label']} · {campaign['campaign_id']}",
        "lane": campaign["lane"],
        # Absent: each scenario keeps the canonical seed the profile materialized
        # it with, so the same slot is the same slot across executions.
        "seed": None,
        "subject": plan["subject"],
        "judge": plan["judge"],
        "groups": suite_groups(campaign),
    }
    body = {
        "schema": CONTRACT_SCHEMA,
        # There is no campaign row to name any more; the execution is the unit.
        "campaign_id": execution_id,
        "execution_id": execution_id,
        "attempt": 1,
        "stack_revision": stack_revision,
        "orchestration": orchestration,
        "runtime": {"cli": cli},
        "security": {"oidc_audience": oidc_audience},
        "suite": suite,
    }
    digest = hashlib.sha256(canonical({**body, "idempotency_key": ""}).encode()).hexdigest()
    return {**body, "idempotency_key": f"rc:e2e:{digest}"}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--execution-id", required=True)
    parser.add_argument("--profile-snapshot", type=Path, required=True, help="harness-e2e test-plan materialize output")
    parser.add_argument("--plan", type=Path, required=True, help="the plan JSON Release Control dispatched")
    parser.add_argument("--stack", required=True, help="the stack policy Release Control dispatched")
    parser.add_argument("--cli-version", required=True)
    parser.add_argument("--oidc-audience", required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    token = os.environ.get("GITHUB_TOKEN") or None
    snapshot = json.loads(args.profile_snapshot.read_text())
    plan = json.loads(args.plan.read_text())
    stack = json.loads(args.stack)
    if not isinstance(stack, dict):
        raise ResolutionError("stack must be a JSON object")
    pinned = stack.get("versions") if isinstance(stack.get("versions"), dict) else {}

    roles = {TARGET_ROOT: "target", RUNNER_ROOT: "runner"}
    roles |= {worker: "runtime" for worker in RUNTIME_ROOTS}
    graphs = [
        resolve_graph(worker, str(pinned.get(worker, "latest")), CLI_TARGET)
        for worker in (TARGET_ROOT, *RUNTIME_ROOTS, RUNNER_ROOT)
    ]
    orchestration = merge_graphs(roles, graphs)
    harness_version = next(root["version"] for root in orchestration["roots"] if root["worker"] == TARGET_ROOT)
    cli = resolve_cli(args.cli_version, token)
    stack_revision = resolve_stack_revision(harness_version, token)

    args.output_dir.mkdir(parents=True, exist_ok=True)
    include = []
    for campaign in snapshot["campaigns"]:
        contract = build_contract(
            campaign,
            execution_id=args.execution_id,
            snapshot=snapshot,
            plan=plan,
            orchestration=orchestration,
            cli=cli,
            stack_revision=stack_revision,
            oidc_audience=args.oidc_audience,
        )
        (args.output_dir / f"{campaign['campaign_id']}.json").write_text(json.dumps(contract, indent=2) + "\n")
        for group in contract["suite"]["groups"]:
            fault = group["execution_kind"] == "fault_injection"
            include.append(
                {
                    "campaign_id": campaign["campaign_id"],
                    "group_id": group["id"],
                    "execution_kind": group["execution_kind"],
                    "runs_on": ["self-hosted", "harness-e2e"] if fault else ["ubuntu-latest"],
                }
            )

    summary = {
        "matrix": {"include": include},
        "harness_version": harness_version,
        "stack_versions": {node["worker"]: node["version"] for node in orchestration["nodes"]},
        "stack_revision": stack_revision,
        "cli_version": cli["version"],
        "campaign_ids": [campaign["campaign_id"] for campaign in snapshot["campaigns"]],
    }
    (args.output_dir / "resolution.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(canonical(summary))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
