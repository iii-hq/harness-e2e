#!/usr/bin/env python3
"""Report one execution to Release Control's run ledger.

Three messages, one endpoint, and none of them may be lost:

* ``materialized`` — what this commit expanded the requested profile into,
  posted before anything runs, so Release Control knows which shards to expect
  and how many runs each scenario was planned.
* ``shard`` — the runs of one campaign group, posted whatever the group did.
  Runs come from ``results.json``; when the group died before writing one they
  come from the journal checkpoints, which are written as each run commits.
  A group that produced nothing still reports, saying so.
* ``summary`` — the aggregate the finalizer computed, posted whatever the
  finalizer did.

Release Control never rejects a report for its shape, so this script never
withholds one: an incomplete report is evidence, an absent report is a gap.
"""

from __future__ import annotations

import argparse
import json
import os
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any


class ReportError(RuntimeError):
    """The report could not be delivered."""


def read_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return None


def obj(value: Any) -> dict[str, Any]:
    return value if isinstance(value, dict) else {}


def prune(value: dict[str, Any]) -> dict[str, Any]:
    return {key: item for key, item in value.items() if item is not None}


# ---------------------------------------------------------------------------
# Runs
# ---------------------------------------------------------------------------


def runs_from_results(results: dict[str, Any]) -> list[dict[str, Any]]:
    """One reported run per ``scenarios[].runs[]`` entry of a runner report."""
    runs = []
    for scenario in results.get("scenarios") or []:
        scenario = obj(scenario)
        scenario_id = scenario.get("scenario_id")
        if not isinstance(scenario_id, str) or not scenario_id:
            continue
        case = obj(scenario.get("case"))
        seed = case.get("seed")
        for repetition, run in enumerate(scenario.get("runs") or []):
            if not isinstance(run, dict):
                continue
            runs.append(
                prune(
                    {
                        "scenario_id": scenario_id,
                        "scenario_version": scenario.get("scenario_version"),
                        "case_id": scenario.get("case_id"),
                        "seed": None if seed is None else str(seed),
                        "repetition": repetition,
                        "tier": obj(case.get("complexity")).get("tier"),
                        "definition_sha256": case.get("inputs_sha256"),
                        "run": run,
                    }
                )
            )
    return runs


def journal_slots(artifacts: Path) -> dict[str, dict[str, Any]]:
    """Slot identity as the runner committed it, keyed by slot id."""
    slots: dict[str, dict[str, Any]] = {}
    for event_path in sorted((artifacts / "journal" / "events").glob("*-slot-inventory-committed.json")):
        event = obj(read_json(event_path))
        for slot in event.get("slots") or []:
            slot = obj(slot)
            if isinstance(slot.get("slot_id"), str):
                slots[slot["slot_id"]] = slot
    return slots


def runs_from_journal(artifacts: Path) -> list[dict[str, Any]]:
    """Every run the journal committed, for a group that never wrote results."""
    slots = journal_slots(artifacts)
    runs = []
    for checkpoint_path in sorted((artifacts / "journal" / "runs").glob("*/*.json")):
        checkpoint = obj(read_json(checkpoint_path))
        run = obj(checkpoint.get("run")) or checkpoint
        slot_id = checkpoint.get("slot_id") or checkpoint_path.parent.name
        slot = obj(slots.get(slot_id))
        scenario_id = slot.get("scenario_id") or run.get("scenario_id")
        if not isinstance(scenario_id, str) or not scenario_id:
            # Release Control cannot attribute a run with no scenario; the
            # checkpoint stays in the uploaded evidence either way.
            continue
        runs.append(
            prune(
                {
                    "slot_id": slot_id,
                    "scenario_id": scenario_id,
                    "case_id": slot.get("case_id"),
                    "seed": slot.get("seed"),
                    "repetition": slot.get("repetition"),
                    "run": run,
                }
            )
        )
    return runs


def collect_runs(artifacts: Path) -> tuple[list[dict[str, Any]], str]:
    results = obj(read_json(artifacts / "results.json"))
    if results.get("scenarios"):
        return runs_from_results(results), "results"
    runs = runs_from_journal(artifacts)
    return runs, "journal" if runs else "none"


# ---------------------------------------------------------------------------
# Identity
# ---------------------------------------------------------------------------


def identity_of(args: argparse.Namespace, artifacts: Path | None) -> dict[str, Any]:
    plan = obj(read_json(args.plan)) if args.plan else {}
    resolution = obj(read_json(args.resolution)) if args.resolution else {}
    results = obj(read_json(artifacts / "results.json")) if artifacts else {}
    snapshot = obj(read_json(args.profile_snapshot)) if args.profile_snapshot else {}
    return prune(
        {
            "plan_sha256": plan.get("sha256"),
            "profile_sha256": snapshot.get("profile_sha256"),
            "definition_sha256": snapshot.get("definition_sha256"),
            "stack_versions": resolution.get("stack_versions"),
            "stack_lock_sha256": args.contract_sha256,
            "runner_revision": args.runner_sha,
            "cli_version": resolution.get("cli_version") or args.cli_version,
            "subject": obj(plan.get("subject")) or None,
            "judge": obj(plan.get("judge")) or None,
            "result_contract_sha256": results.get("result_contract_sha256"),
            "scoring_profile_sha256": results.get("scoring_profile_sha256"),
        }
    )


# ---------------------------------------------------------------------------
# Payloads
# ---------------------------------------------------------------------------


def materialized_payload(args: argparse.Namespace) -> dict[str, Any]:
    snapshot = obj(read_json(args.profile_snapshot))
    profile = obj(snapshot.get("profile"))
    return {
        "kind": "materialized",
        "execution_id": args.execution_id,
        "schema": snapshot.get("schema") or "harness-e2e-profile-snapshot/v1",
        "profile": prune(
            {
                "id": profile.get("id"),
                "label": profile.get("label"),
                "repetitions": profile.get("repetitions"),
                "technical_retries": profile.get("technical_retries"),
                "profile_sha256": snapshot.get("profile_sha256"),
                "definition_sha256": snapshot.get("definition_sha256"),
            }
        ),
        "campaigns": snapshot.get("campaigns") or [],
        "budget": snapshot.get("budget"),
        "identity": identity_of(args, None),
    }


def shard_payload(args: argparse.Namespace) -> dict[str, Any]:
    artifacts = args.artifacts
    runs, source = collect_runs(artifacts) if artifacts and artifacts.is_dir() else ([], "none")
    failure = obj(read_json(artifacts / "failure.json")) if artifacts else {}
    return {
        "kind": "shard",
        "execution_id": args.execution_id,
        "shard": f"{args.campaign_id}/{args.group_id}",
        "schema": "harness-e2e-run-checkpoint/v1" if source == "journal" else "harness-e2e-results/v1",
        "group": prune(
            {
                "campaign_id": args.campaign_id,
                "group_id": args.group_id,
                "outcome": args.outcome,
                "evidence": source,
                "failure": failure or None,
            }
        ),
        "identity": identity_of(args, artifacts),
        "runs": runs,
    }


def summary_payload(args: argparse.Namespace) -> dict[str, Any]:
    summary = obj(read_json(args.summary)) if args.summary else {}
    return {
        "kind": "summary",
        "execution_id": args.execution_id,
        "schema": "harness-e2e-campaign-summary/v1",
        "summary": summary,
        "identity": identity_of(args, None),
        "runs": [],
    }


# ---------------------------------------------------------------------------
# Delivery
# ---------------------------------------------------------------------------


def oidc_token(audience: str) -> str:
    url = os.environ.get("ACTIONS_ID_TOKEN_REQUEST_URL")
    token = os.environ.get("ACTIONS_ID_TOKEN_REQUEST_TOKEN")
    if not url or not token:
        raise ReportError("no GitHub Actions OIDC token is available to this job")
    request = urllib.request.Request(
        f"{url}&audience={urllib.parse.quote(audience)}", headers={"Authorization": f"Bearer {token}"}
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        value = json.loads(response.read().decode()).get("value")
    if not isinstance(value, str) or not value:
        raise ReportError("the OIDC token endpoint returned no token")
    return value


def post(api_url: str, execution_id: str, payload: dict[str, Any], token: str) -> dict[str, Any]:
    url = f"{api_url.rstrip('/')}/internal/test-executions/{execution_id}/reports"
    body = json.dumps(payload).encode()
    request = urllib.request.Request(
        url,
        data=body,
        headers={"Content-Type": "application/json", "Authorization": f"Bearer {token}"},
        method="POST",
    )
    last: Exception | None = None
    for attempt in range(4):
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                return json.loads(response.read().decode())
        except urllib.error.HTTPError as error:
            detail = error.read().decode(errors="replace")[:500]
            # Release Control answers a well-formed report with 200. A 4xx is a
            # considered refusal — retrying it only hides the reason.
            if error.code < 500 and error.code != 429:
                raise ReportError(f"Release Control refused the report: HTTP {error.code} {detail}") from error
            last = error
        except (OSError, ValueError) as error:
            last = error
        time.sleep(2 * (attempt + 1))
    raise ReportError(f"Release Control did not accept the report: {last}")


BUILDERS = {"materialized": materialized_payload, "shard": shard_payload, "summary": summary_payload}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("kind", choices=sorted(BUILDERS))
    parser.add_argument("--execution-id", required=True)
    parser.add_argument("--api-url", default=os.environ.get("RELEASE_CONTROL_API_URL", ""))
    parser.add_argument("--oidc-audience", required=True)
    parser.add_argument("--artifacts", type=Path, help="a group's uploaded evidence root")
    parser.add_argument("--campaign-id")
    parser.add_argument("--group-id")
    parser.add_argument("--outcome", help="what the group step concluded")
    parser.add_argument("--profile-snapshot", type=Path)
    parser.add_argument("--plan", type=Path)
    parser.add_argument("--resolution", type=Path)
    parser.add_argument("--summary", type=Path)
    parser.add_argument("--contract-sha256")
    parser.add_argument("--runner-sha")
    parser.add_argument("--cli-version")
    parser.add_argument("--output", type=Path, help="write the payload here instead of posting it")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    payload = BUILDERS[args.kind](args)
    if args.output:
        args.output.write_text(json.dumps(payload, indent=2) + "\n")
        return 0
    if not args.api_url:
        raise ReportError("RELEASE_CONTROL_API_URL is required")
    accepted = post(args.api_url, args.execution_id, payload, oidc_token(args.oidc_audience))
    print(json.dumps(accepted))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
