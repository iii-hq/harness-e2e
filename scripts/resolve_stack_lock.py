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
TEMPLATES_REPOSITORY = "iii-hq/templates"

CONTRACT_SCHEMA = "rc-e2e/v2"
CLI_TARGET = "x86_64-unknown-linux-gnu"
CLI_ASSET = f"iii-{CLI_TARGET}.tar.gz"



EXACT_VERSION = re.compile(
    r"^[0-9]+\.[0-9]+\.[0-9]+"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)
GIT_SHA = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^(?:sha256:)?[0-9a-f]{64}$")
TEMPLATE_ID = re.compile(r"^[a-z0-9][a-z0-9_-]{0,63}$")


class ResolutionError(RuntimeError):
    """A stack the campaign cannot be assembled from."""


#: The runner executing the scenarios inside the declared stack.
RUNNER_ROOT = "harness-e2e"


def runner_selector(plan: dict[str, Any], pinned: dict[str, Any]) -> str:
    """An explicit stack pin wins; otherwise use Release Control's release."""
    if RUNNER_ROOT in pinned:
        return str(pinned[RUNNER_ROOT])
    runner = plan.get("runner")
    version = runner.get("version") if isinstance(runner, dict) else None
    if version is None:
        return "latest"
    if not isinstance(version, str) or not EXACT_VERSION.fullmatch(version):
        raise ResolutionError("plan runner version is not an exact version")
    return version


def resolve_canvas_version(selector: str) -> str:
    response = get_json(f"{REGISTRY_API_URL}/resolve", {"worker": "canvas", "version": selector})
    root = response.get("root") if isinstance(response, dict) else None
    version = root.get("version") if isinstance(root, dict) else None
    if not isinstance(version, str) or not EXACT_VERSION.fullmatch(version):
        raise ResolutionError("Registry did not resolve canvas to an exact version")
    return version


def canonical(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True)


def resolve_template(template_id: str | None, token: str | None) -> dict[str, str] | None:
    """Pin the template to the commit every shard checks out.

    Whether the template exists and what it declares, the executor learns from
    `iii project init` on that commit, which fails loudly when it does not.
    """
    if template_id is None:
        return None
    if not isinstance(template_id, str) or not TEMPLATE_ID.fullmatch(template_id):
        raise ResolutionError("template must be an iii template id")
    revision = get_json(f"{GITHUB_API_URL}/repos/{TEMPLATES_REPOSITORY}/commits/main", token=token).get("sha")
    if not isinstance(revision, str) or not GIT_SHA.fullmatch(revision):
        raise ResolutionError("templates main did not resolve to a commit")
    return {"id": template_id, "repository": TEMPLATES_REPOSITORY, "ref": "main", "revision": revision}


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


def suite_groups(campaign: dict[str, Any]) -> list[dict[str, Any]]:
    """A materialized campaign's groups, in the shape the contract states them."""
    groups = []
    for group in campaign.get("groups") or []:
        materialized = {
            "id": group["id"],
            "execution_kind": group["execution_kind"],
            "runs": group["runs"],
            "technical_retries": group["technical_retries"],
        }
        materialized["scenarios"] = list(group["scenarios"])
        groups.append(materialized)
    return groups


def build_contract(
    campaign: dict[str, Any],
    *,
    execution_id: str,
    snapshot: dict[str, Any],
    plan: dict[str, Any],
    cli: dict[str, str],
    stack: dict[str, str],
    oidc_audience: str,
    template: dict[str, str] | None = None,
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
        "groups": suite_groups(campaign),
    }
    if plan.get("agent_profile") is not None:
        suite["agent_profile"] = plan["agent_profile"]
    body = {
        "schema": CONTRACT_SCHEMA,
        # There is no campaign row to name any more; the execution is the unit.
        "campaign_id": execution_id,
        "execution_id": execution_id,
        "attempt": 1,
        # What the declared stack should resolve to, when Release Control wants
        # an execution held to particular releases. Empty means the selectors in
        # the declaration stand.
        "runtime": {"cli": cli, "stack": stack, **({"template": template} if template else {})},
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
    pinned = {str(worker): str(selector) for worker, selector in sorted(pinned.items())}
    # Release Control names the runner release in the plan, and an explicit
    # stack pin still wins. Either way it is a selector, so it travels with the
    # others and the scaffold lays it over the declaration.
    pinned[RUNNER_ROOT] = runner_selector(plan, pinned)
    if any(
        {"form_flow_build", "state_machine_canvas_build"} & set(group.get("scenarios") or [])
        for campaign in snapshot["campaigns"]
        for group in campaign["groups"]
    ):
        pinned["canvas"] = resolve_canvas_version(pinned.get("canvas", "latest"))
    template = resolve_template(plan.get("template"), token)
    cli = resolve_cli(args.cli_version, token)

    args.output_dir.mkdir(parents=True, exist_ok=True)
    include = []
    for campaign in snapshot["campaigns"]:
        contract = build_contract(
            campaign,
            execution_id=args.execution_id,
            snapshot=snapshot,
            plan=plan,
            cli=cli,
            stack=pinned,
            oidc_audience=args.oidc_audience,
            template=template,
        )
        (args.output_dir / f"{campaign['campaign_id']}.json").write_text(json.dumps(contract, indent=2) + "\n")
        for group in contract["suite"]["groups"]:
            include.append(
                {
                    "campaign_id": campaign["campaign_id"],
                    "group_id": group["id"],
                    "execution_kind": group["execution_kind"],
                    "runs_on": ["ubuntu-latest"],
                    **({"template_revision": template["revision"]} if template else {}),
                }
            )

    # The workflow reads this by name, and it is read from the default branch
    # while the executor is pinned per campaign. Renaming it, or dropping a
    # field a pinned executor still reads, breaks every revision but the newest.
    summary = {
        "matrix": {"include": include},
        "stack_overrides": pinned,
        "cli_version": cli["version"],
        "campaign_ids": [campaign["campaign_id"] for campaign in snapshot["campaigns"]],
        **({"template": template} if template else {}),
    }
    (args.output_dir / "resolution.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(canonical(summary))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
