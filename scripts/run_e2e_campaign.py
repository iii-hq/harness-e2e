#!/usr/bin/env python3
"""Validate and run canonical Harness E2E campaign manifests.

The runner deliberately translates one campaign group into one ordinary
`harness-e2e run` invocation. Groups are the retry boundary: non-replay-safe
ScriptedDialogue, CompositeFlow, and AdaptiveFlow scenarios never share an
invocation with replay-safe HarnessTurn scenarios.
"""

from __future__ import annotations

import argparse
import dataclasses
import datetime as dt
import json
import os
import pathlib
import re
import shlex
import subprocess
import sys
import time
import uuid
from collections.abc import Callable, Mapping, Sequence
from typing import Any


CAMPAIGN_KIND = "harness-e2e-campaign"
ROOT_FIELDS = {"kind", "campaign_id", "lane", "failure_policy", "groups"}
GROUP_FIELDS = {
    "id",
    "execution_kind",
    "runs",
    "technical_retries",
    "scenarios",
}
FAILURE_POLICIES = {"advisory", "enforcing"}
EXECUTION_KINDS = {
    "harness_turn",
    "scripted_dialogue",
    "composite_flow",
    "adaptive_flow",
}
SAFE_ID = re.compile(r"^[a-z][a-z0-9_-]{0,63}$")
SAFE_LANE = re.compile(r"^[a-z][a-z0-9_-]{0,31}$")
SAFE_EXECUTION_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$")
FORBIDDEN_SEED_FIELDS = {"seed", "seeds", "rotating_seed", "rotating_seeds"}

# Canonical campaign scope. Adding a scenario to a reviewed manifest requires
# intentionally declaring its execution kind here, preventing typos and,
# critically, preventing a ScriptedDialogue from silently entering a retryable
# HarnessTurn group.
SCENARIO_EXECUTION_KIND = {
    "engineering_ticket": "harness_turn",
    "engineering_ticket_git_handoff": "harness_turn",
    "engineering_endurance_ladder": "harness_turn",
    "tool_contract_recovery": "harness_turn",
    "policy_bound_action": "scripted_dialogue",
    "cross_app_transaction": "harness_turn",
    "database_migration_recovery": "harness_turn",
    "research_pipeline": "harness_turn",
    "performance_regression": "harness_turn",
    "browser_cross_site": "harness_turn",
    "moving_target": "harness_turn",
    "incident_response": "adaptive_flow",
    "release_train_recovery": "adaptive_flow",
    "cross_repo_contract_migration": "adaptive_flow",
}


class CampaignError(ValueError):
    """A campaign is invalid or cannot be executed safely."""


@dataclasses.dataclass(frozen=True)
class CampaignGroup:
    id: str
    execution_kind: str
    runs: int
    technical_retries: int
    scenarios: tuple[str, ...]


@dataclasses.dataclass(frozen=True)
class Campaign:
    campaign_id: str
    lane: str
    failure_policy: str
    groups: tuple[CampaignGroup, ...]


def _expect_object(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise CampaignError(f"{label} must be a JSON object")
    return value


def _reject_unknown_fields(
    value: Mapping[str, Any], allowed: set[str], label: str
) -> None:
    unknown = sorted(set(value) - allowed)
    if unknown:
        raise CampaignError(f"{label} contains unsupported field(s): {', '.join(unknown)}")
    missing = sorted(allowed - set(value))
    if missing:
        raise CampaignError(f"{label} is missing required field(s): {', '.join(missing)}")


def _reject_seed_fields(value: Any, path: str = "campaign") -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            if key in FORBIDDEN_SEED_FIELDS:
                raise CampaignError(
                    f"{path}.{key} is forbidden; canonical campaigns never select seeds"
                )
            _reject_seed_fields(child, f"{path}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            _reject_seed_fields(child, f"{path}[{index}]")


def _expect_bounded_int(value: Any, label: str, minimum: int, maximum: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise CampaignError(f"{label} must be an integer")
    if not minimum <= value <= maximum:
        raise CampaignError(f"{label} must be in {minimum}..={maximum}")
    return value


def parse_campaign(value: Any, source: str = "campaign") -> Campaign:
    root = _expect_object(value, source)
    _reject_seed_fields(root)
    _reject_unknown_fields(root, ROOT_FIELDS, source)
    if root["kind"] != CAMPAIGN_KIND:
        raise CampaignError(f"{source}.kind must be {CAMPAIGN_KIND!r}")

    campaign_id = root["campaign_id"]
    if not isinstance(campaign_id, str) or not SAFE_ID.fullmatch(campaign_id):
        raise CampaignError(f"{source}.campaign_id is not a safe campaign id")
    lane = root["lane"]
    if not isinstance(lane, str) or not SAFE_LANE.fullmatch(lane):
        raise CampaignError(f"{source}.lane is not a safe lane id")
    failure_policy = root["failure_policy"]
    if failure_policy not in FAILURE_POLICIES:
        raise CampaignError(
            f"{source}.failure_policy must be one of {sorted(FAILURE_POLICIES)}"
        )
    raw_groups = root["groups"]
    if not isinstance(raw_groups, list) or not raw_groups:
        raise CampaignError(f"{source}.groups must be a non-empty array")

    groups: list[CampaignGroup] = []
    group_ids: set[str] = set()
    selected_scenarios: set[str] = set()
    for index, raw_group in enumerate(raw_groups):
        label = f"{source}.groups[{index}]"
        group = _expect_object(raw_group, label)
        _reject_unknown_fields(group, GROUP_FIELDS, label)
        group_id = group["id"]
        if not isinstance(group_id, str) or not SAFE_ID.fullmatch(group_id):
            raise CampaignError(f"{label}.id is not a safe group id")
        if group_id in group_ids:
            raise CampaignError(f"{source} repeats group id {group_id!r}")
        group_ids.add(group_id)

        execution_kind = group["execution_kind"]
        if execution_kind not in EXECUTION_KINDS:
            raise CampaignError(
                f"{label}.execution_kind must be one of {sorted(EXECUTION_KINDS)}"
            )
        runs = _expect_bounded_int(group["runs"], f"{label}.runs", 1, 20)
        retries = _expect_bounded_int(
            group["technical_retries"], f"{label}.technical_retries", 0, 3
        )
        if execution_kind in {
            "scripted_dialogue",
            "composite_flow",
            "adaptive_flow",
        } and retries != 0:
            raise CampaignError(
                f"{label} is {execution_kind} and must set technical_retries=0"
            )

        raw_scenarios = group["scenarios"]
        if not isinstance(raw_scenarios, list) or not raw_scenarios:
            raise CampaignError(f"{label}.scenarios must be a non-empty array")
        scenarios: list[str] = []
        for scenario_index, scenario_id in enumerate(raw_scenarios):
            scenario_label = f"{label}.scenarios[{scenario_index}]"
            if not isinstance(scenario_id, str):
                raise CampaignError(f"{scenario_label} must be a string")
            expected_kind = SCENARIO_EXECUTION_KIND.get(scenario_id)
            if expected_kind is None:
                raise CampaignError(f"{scenario_label} has unknown scenario id {scenario_id!r}")
            if expected_kind != execution_kind:
                raise CampaignError(
                    f"scenario {scenario_id!r} is {expected_kind}, not {execution_kind}"
                )
            if scenario_id in selected_scenarios:
                raise CampaignError(
                    f"{source} selects scenario {scenario_id!r} more than once"
                )
            selected_scenarios.add(scenario_id)
            scenarios.append(scenario_id)
        if execution_kind == "adaptive_flow" and (runs != 1 or len(scenarios) != 1):
            raise CampaignError(
                f"{label} is adaptive_flow and must select exactly one scenario with runs=1"
            )
        groups.append(
            CampaignGroup(
                id=group_id,
                execution_kind=execution_kind,
                runs=runs,
                technical_retries=retries,
                scenarios=tuple(scenarios),
            )
        )

    return Campaign(
        campaign_id=campaign_id,
        lane=lane,
        failure_policy=failure_policy,
        groups=tuple(groups),
    )


def load_campaign(path: pathlib.Path) -> Campaign:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as error:
        raise CampaignError(f"campaign manifest does not exist: {path}") from error
    except json.JSONDecodeError as error:
        raise CampaignError(
            f"{path}:{error.lineno}:{error.colno}: invalid JSON: {error.msg}"
        ) from error
    return parse_campaign(value, str(path))


def build_group_command(
    campaign: Campaign,
    group: CampaignGroup,
    *,
    e2e_bin: pathlib.Path,
    output: pathlib.Path,
    model: str | None = None,
    provider: str | None = None,
    url: str | None = None,
    progress_interval_seconds: int | None = None,
) -> list[str]:
    command = [
        str(e2e_bin),
        "run",
        "--output",
        str(output),
        "--runs",
        str(group.runs),
        "--technical-retries",
        str(group.technical_retries),
    ]
    if model:
        command.extend(["--model", model])
    if provider:
        command.extend(["--provider", provider])
    if url:
        command.extend(["--url", url])
    if progress_interval_seconds is not None:
        command.extend(
            ["--progress-interval-seconds", str(progress_interval_seconds)]
        )
    for scenario_id in group.scenarios:
        command.extend(["--scenario", scenario_id])
    return command


def _utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")


def _write_json_atomic(path: pathlib.Path, value: Mapping[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temporary.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    temporary.replace(path)


def execute_campaign(
    campaign: Campaign,
    *,
    e2e_bin: pathlib.Path,
    output_root: pathlib.Path,
    execution_id: str,
    dry_run: bool,
    advisory: bool,
    model: str | None = None,
    provider: str | None = None,
    url: str | None = None,
    progress_interval_seconds: int | None = None,
    environ: Mapping[str, str] | None = None,
    run_process: Callable[..., Any] = subprocess.run,
    monotonic: Callable[[], float] = time.monotonic,
) -> dict[str, Any]:
    if not SAFE_EXECUTION_ID.fullmatch(execution_id):
        raise CampaignError("execution_id contains unsafe characters")
    base_environment = dict(os.environ if environ is None else environ)
    if not dry_run:
        if not model and not base_environment.get("HARNESS_E2E_MODEL"):
            raise CampaignError(
                "model is required via --model or HARNESS_E2E_MODEL"
            )
        if not provider and not base_environment.get("HARNESS_E2E_PROVIDER"):
            raise CampaignError(
                "provider is required via --provider or HARNESS_E2E_PROVIDER"
            )

    execution_root = output_root / campaign.campaign_id / execution_id
    started_at = _utc_now()
    group_results: list[dict[str, Any]] = []
    for group in campaign.groups:
        group_output = execution_root / group.id
        command = build_group_command(
            campaign,
            group,
            e2e_bin=e2e_bin,
            output=group_output,
            model=model,
            provider=provider,
            url=url,
            progress_interval_seconds=progress_interval_seconds,
        )
        print(f"[{campaign.campaign_id}/{group.id}] {shlex.join(command)}", flush=True)
        result: dict[str, Any] = {
            "group_id": group.id,
            "execution_kind": group.execution_kind,
            "scenarios": list(group.scenarios),
            "runs": group.runs,
            "technical_retries": group.technical_retries,
            "output": str(group_output),
            "command": command,
        }
        if dry_run:
            result.update(
                {
                    "status": "dry_run",
                    "exit_code": None,
                    "duration_ms": 0,
                }
            )
            group_results.append(result)
            continue

        group_output.mkdir(parents=True, exist_ok=True)
        child_environment = dict(base_environment)
        child_environment["HARNESS_E2E_LANE"] = campaign.lane
        child_environment["HARNESS_E2E_CAMPAIGN_ID"] = campaign.campaign_id
        child_environment["HARNESS_E2E_CAMPAIGN_GROUP"] = group.id
        started = monotonic()
        error_message: str | None = None
        try:
            completed = run_process(command, env=child_environment, check=False)
            return_code = int(completed.returncode)
        except OSError as error:
            return_code = 127
            error_message = str(error)
        duration_ms = max(0, round((monotonic() - started) * 1000))
        result.update(
            {
                "status": "passed" if return_code == 0 else "failed",
                "exit_code": return_code,
                "duration_ms": duration_ms,
            }
        )
        if error_message is not None:
            result["error"] = error_message
        group_results.append(result)

    objective_passed = all(result["status"] in {"passed", "dry_run"} for result in group_results)
    process_exit_code = 0 if dry_run or advisory or objective_passed else 1
    return {
        "kind": "harness-e2e-campaign-summary",
        "campaign_id": campaign.campaign_id,
        "lane": campaign.lane,
        "execution_id": execution_id,
        "advisory": advisory,
        "dry_run": dry_run,
        "started_at": started_at,
        "completed_at": _utc_now(),
        "objective_passed": objective_passed,
        "process_exit_code": process_exit_code,
        "groups": group_results,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Validate and run a canonical Harness E2E campaign"
    )
    parser.add_argument("manifest", type=pathlib.Path)
    parser.add_argument("--validate-only", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--advisory",
        action="store_true",
        help="run every group and return zero even if objective groups fail",
    )
    mode.add_argument(
        "--enforcing",
        action="store_true",
        help="return non-zero when one or more objective groups fail",
    )
    parser.add_argument(
        "--e2e-bin",
        type=pathlib.Path,
        default=pathlib.Path(
            os.environ.get("HARNESS_E2E_BIN", "target/release/harness-e2e")
        ),
    )
    parser.add_argument(
        "--output-root",
        type=pathlib.Path,
        default=pathlib.Path(
            os.environ.get("HARNESS_E2E_CAMPAIGN_OUTPUT", "target/e2e-campaigns")
        ),
    )
    parser.add_argument("--execution-id")
    parser.add_argument("--summary", type=pathlib.Path)
    parser.add_argument("--model")
    parser.add_argument("--provider")
    parser.add_argument("--url")
    parser.add_argument("--progress-interval-seconds", type=int)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        campaign = load_campaign(args.manifest)
        if args.validate_only:
            print(f"valid campaign: {campaign.campaign_id}")
            return 0
        if args.progress_interval_seconds is not None and args.progress_interval_seconds < 0:
            raise CampaignError("progress_interval_seconds must be non-negative")
        execution_id = args.execution_id or uuid.uuid4().hex
        advisory = (
            True
            if args.advisory
            else False
            if args.enforcing
            else campaign.failure_policy == "advisory"
        )
        summary = execute_campaign(
            campaign,
            e2e_bin=args.e2e_bin,
            output_root=args.output_root,
            execution_id=execution_id,
            dry_run=args.dry_run,
            advisory=advisory,
            model=args.model,
            provider=args.provider,
            url=args.url,
            progress_interval_seconds=args.progress_interval_seconds,
        )
        summary_path = args.summary
        if summary_path is None and not args.dry_run:
            summary_path = (
                args.output_root
                / campaign.campaign_id
                / execution_id
                / "campaign-summary.json"
            )
        if summary_path is not None:
            _write_json_atomic(summary_path, summary)
            print(f"summary: {summary_path}")
        print(json.dumps(summary, sort_keys=True))
        return int(summary["process_exit_code"])
    except CampaignError as error:
        print(f"campaign error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
