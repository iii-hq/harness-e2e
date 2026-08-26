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
import hashlib
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
SCORING_PROFILE = "difficulty-weighted-v1"
ROOT_FIELDS = {
    "kind",
    "campaign_id",
    "lane",
    "failure_policy",
    "scoring_profile",
    "groups",
}
COMMON_GROUP_FIELDS = {
    "id",
    "execution_kind",
    "runs",
    "technical_retries",
    "difficulty_weight",
}
SCENARIO_GROUP_FIELDS = COMMON_GROUP_FIELDS | {
    "scenarios",
}
FAULT_GROUP_FIELDS = COMMON_GROUP_FIELDS | {
    "fault_profile",
    "fault_scenario",
    "soak_minutes",
}
FAILURE_POLICIES = {"advisory", "enforcing"}
EXECUTION_KINDS = {
    "harness_turn",
    "scripted_dialogue",
    "composite_flow",
    "adaptive_flow",
    "fault_injection",
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
    "timer_wake": "harness_turn",
    "shell_coder_sandbox": "harness_turn",
    "chess_engine_build": "harness_turn",
    "git_regression_forensics": "harness_turn",
    "contention_ledger": "harness_turn",
    "security_review": "composite_flow",
    "engineering_ticket": "harness_turn",
    "engineering_ticket_git_handoff": "harness_turn",
    "engineering_endurance_ladder": "harness_turn",
    "tool_contract_recovery": "harness_turn",
    "policy_bound_action": "scripted_dialogue",
    "cross_app_transaction": "harness_turn",
    "research_pipeline": "harness_turn",
    "performance_regression": "harness_turn",
    "browser_cross_site": "harness_turn",
    "moving_target": "harness_turn",
    "incident_response": "adaptive_flow",
    "release_train_recovery": "adaptive_flow",
    "cross_repo_contract_migration": "adaptive_flow",
}

# Generated from `harness-e2e catalog`. The parser checks the reviewed weight
# against the same canonical capability tier later embedded in results.json.
# L0/L1 map to 1; L2..L5 map directly to 2..5.
SCENARIO_DIFFICULTY_WEIGHT = {
    "timer_wake": 2,
    "shell_coder_sandbox": 4,
    "chess_engine_build": 2,
    "git_regression_forensics": 4,
    "contention_ledger": 3,
    "security_review": 4,
    "engineering_ticket": 4,
    "engineering_ticket_git_handoff": 4,
    "engineering_endurance_ladder": 4,
    "tool_contract_recovery": 4,
    "policy_bound_action": 4,
    "cross_app_transaction": 4,
    "research_pipeline": 4,
    "performance_regression": 2,
    "browser_cross_site": 4,
    "moving_target": 2,
    "incident_response": 5,
    "release_train_recovery": 5,
    "cross_repo_contract_migration": 5,
}

FAULT_PROFILE_WEIGHT = {
    "weekly-l2-recovery": 2,
    "weekly-l3-recovery": 3,
    "weekly-l4-recovery": 4,
}

MARKDOWN_SCENARIO_SECTIONS = {
    "Plans",
    "Version",
    "Before Test",
    "Prompt",
    "Validations",
}
MARKDOWN_PLAN_EXECUTION = {
    "daily": (1, 1),
    "weekly": (3, 1),
    "post-release": (1, 0),
    "endurance": (1, 0),
}


class CampaignError(ValueError):
    """A campaign is invalid or cannot be executed safely."""


@dataclasses.dataclass(frozen=True)
class CampaignGroup:
    id: str
    execution_kind: str
    runs: int
    technical_retries: int
    difficulty_weight: int
    scenarios: tuple[str, ...]
    fault_profile: str | None = None
    fault_scenario: str | None = None
    soak_minutes: int = 0


@dataclasses.dataclass(frozen=True)
class Campaign:
    campaign_id: str
    lane: str
    failure_policy: str
    scoring_profile: str
    groups: tuple[CampaignGroup, ...]


def _expect_object(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise CampaignError(f"{label} must be a JSON object")
    return value


def _reject_unknown_fields(
    value: Mapping[str, Any],
    allowed: set[str],
    label: str,
    required: set[str] | None = None,
) -> None:
    unknown = sorted(set(value) - allowed)
    if unknown:
        raise CampaignError(f"{label} contains unsupported field(s): {', '.join(unknown)}")
    missing = sorted((allowed if required is None else required) - set(value))
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
    scoring_profile = root["scoring_profile"]
    if scoring_profile != SCORING_PROFILE:
        raise CampaignError(
            f"{source}.scoring_profile must be {SCORING_PROFILE!r}"
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
        execution_kind = group.get("execution_kind")
        fields = (
            FAULT_GROUP_FIELDS
            if execution_kind == "fault_injection"
            else SCENARIO_GROUP_FIELDS
        )
        _reject_unknown_fields(group, fields, label)
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
        difficulty_weight = _expect_bounded_int(
            group["difficulty_weight"], f"{label}.difficulty_weight", 1, 5
        )
        if execution_kind in {
            "scripted_dialogue",
            "composite_flow",
            "adaptive_flow",
        } and retries != 0:
            raise CampaignError(
                f"{label} is {execution_kind} and must set technical_retries=0"
            )

        if execution_kind == "fault_injection":
            if retries != 0:
                raise CampaignError(
                    f"{label} is fault_injection and must set technical_retries=0"
                )
            profile = group["fault_profile"]
            scenario = group["fault_scenario"]
            if profile not in FAULT_PROFILE_WEIGHT:
                raise CampaignError(f"{label}.fault_profile is not canonical")
            if not isinstance(scenario, str) or not scenario:
                raise CampaignError(f"{label}.fault_scenario must be a non-empty string")
            soak_minutes = _expect_bounded_int(
                group["soak_minutes"], f"{label}.soak_minutes", 0, 180
            )
            if runs < 3:
                raise CampaignError(f"{label}.runs must be at least 3")
            if difficulty_weight != FAULT_PROFILE_WEIGHT[profile]:
                raise CampaignError(
                    f"{label}.difficulty_weight does not match {profile}"
                )
            groups.append(
                CampaignGroup(
                    id=group_id,
                    execution_kind=execution_kind,
                    runs=runs,
                    technical_retries=retries,
                    difficulty_weight=difficulty_weight,
                    scenarios=(),
                    fault_profile=profile,
                    fault_scenario=scenario,
                    soak_minutes=soak_minutes,
                )
            )
            continue

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
        expected_weight = max(SCENARIO_DIFFICULTY_WEIGHT[item] for item in scenarios)
        if difficulty_weight != expected_weight:
            raise CampaignError(
                f"{label}.difficulty_weight must be {expected_weight} for its canonical cases"
            )
        groups.append(
            CampaignGroup(
                id=group_id,
                execution_kind=execution_kind,
                runs=runs,
                technical_retries=retries,
                difficulty_weight=difficulty_weight,
                scenarios=tuple(scenarios),
            )
        )

    return Campaign(
        campaign_id=campaign_id,
        lane=lane,
        failure_policy=failure_policy,
        scoring_profile=scoring_profile,
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


def discover_markdown_scenarios(
    directory: pathlib.Path, campaign_id: str
) -> tuple[str, ...]:
    """Read plan participation only; the Rust compiler remains authoritative."""
    if not directory.exists():
        return ()
    selected: list[str] = []
    for path in sorted(directory.glob("*.md")):
        source = path.read_text(encoding="utf-8")
        structural_source = _without_fenced_markdown(source, path)
        headings = re.findall(
            r"^## ([^\r\n]+)\r?$", structural_source, flags=re.MULTILINE
        )
        if set(headings) != MARKDOWN_SCENARIO_SECTIONS or len(headings) != len(
            MARKDOWN_SCENARIO_SECTIONS
        ):
            raise CampaignError(
                f"{path} must contain each canonical Markdown section exactly once"
            )
        plans_match = re.search(
            r"^## Plans\r?\n(?P<body>.*?)(?=^## )",
            structural_source,
            flags=re.MULTILINE | re.DOTALL,
        )
        if plans_match is None:
            raise CampaignError(f"{path} has no parseable Plans section")
        plans = [
            line.removeprefix("- ").strip()
            for line in plans_match.group("body").splitlines()
            if line.strip()
        ]
        if any(
            not line.startswith("- ")
            for line in plans_match.group("body").splitlines()
            if line.strip()
        ):
            raise CampaignError(f"{path} Plans section must be a Markdown bullet list")
        if campaign_id not in plans:
            continue
        stem = path.stem.lower().replace("-", "_").replace(" ", "_")
        if not re.fullmatch(r"[a-z][a-z0-9_]*", stem) or "__" in stem:
            raise CampaignError(f"{path} cannot produce a safe Markdown scenario id")
        selected.append(stem)
    if len(selected) != len(set(selected)):
        raise CampaignError("Markdown scenarios produce duplicate ids")
    return tuple(selected)


def _without_fenced_markdown(source: str, path: pathlib.Path) -> str:
    lines: list[str] = []
    fence: str | None = None
    for line in source.splitlines(keepends=True):
        stripped = line.lstrip()
        marker = stripped[:1] if stripped[:1] in {"`", "~"} else None
        is_fence = marker is not None and len(stripped) - len(stripped.lstrip(marker)) >= 3
        if is_fence:
            if fence is None:
                fence = marker
            elif fence == marker:
                fence = None
            lines.append("\n" if line.endswith("\n") else "")
        elif fence is None:
            lines.append(line)
        else:
            lines.append("\n" if line.endswith("\n") else "")
    if fence is not None:
        raise CampaignError(f"{path} contains an unclosed fenced code block")
    return "".join(lines)


def attach_markdown_group(
    campaign: Campaign, directory: pathlib.Path
) -> Campaign:
    scenarios = discover_markdown_scenarios(directory, campaign.campaign_id)
    if not scenarios:
        return campaign
    selected = {scenario for group in campaign.groups for scenario in group.scenarios}
    duplicate = selected.intersection(scenarios)
    if duplicate:
        raise CampaignError(
            f"campaign already selects Markdown scenario(s): {', '.join(sorted(duplicate))}"
        )
    group_id = f"{campaign.campaign_id}-markdown"
    if any(group.id == group_id for group in campaign.groups):
        raise CampaignError(f"campaign already contains reserved group id {group_id!r}")
    try:
        runs, technical_retries = MARKDOWN_PLAN_EXECUTION[campaign.campaign_id]
    except KeyError as error:
        raise CampaignError(
            f"campaign {campaign.campaign_id!r} has no canonical Markdown execution policy"
        ) from error
    group = CampaignGroup(
        id=group_id,
        execution_kind="harness_turn",
        runs=runs,
        technical_retries=technical_retries,
        difficulty_weight=2,
        scenarios=scenarios,
    )
    return dataclasses.replace(campaign, groups=(*campaign.groups, group))


def build_group_command(
    campaign: Campaign,
    group: CampaignGroup,
    *,
    e2e_bin: pathlib.Path,
    output: pathlib.Path,
    model: str | None = None,
    provider: str | None = None,
    judge_model: str | None = None,
    judge_provider: str | None = None,
    url: str | None = None,
    progress_interval_seconds: int | None = None,
) -> list[str]:
    if group.execution_kind == "fault_injection":
        raise CampaignError("fault_injection groups require the protected supervisor")
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
    if judge_model:
        command.extend(["--judge-model", judge_model])
    if judge_provider:
        command.extend(["--judge-provider", judge_provider])
    if url:
        command.extend(["--url", url])
    if progress_interval_seconds is not None:
        command.extend(
            ["--progress-interval-seconds", str(progress_interval_seconds)]
        )
    for scenario_id in group.scenarios:
        command.extend(["--scenario", scenario_id])
    return command


def _load_json(path: pathlib.Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise CampaignError(f"cannot read JSON artifact {path}: {error}") from error
    return _expect_object(value, str(path))


def _canonical_sha256(value: Any) -> str:
    encoded = json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    return f"sha256:{hashlib.sha256(encoded).hexdigest()}"


def _file_reference(path: pathlib.Path, root: pathlib.Path) -> dict[str, Any]:
    payload = path.read_bytes()
    return {
        "path": str(path.relative_to(root)),
        "sha256": f"sha256:{hashlib.sha256(payload).hexdigest()}",
        "size_bytes": len(payload),
        "media_type": "application/json",
    }


def _tier_weight(value: Any) -> int | None:
    if not isinstance(value, str):
        return None
    return {
        "l0_atomic": 1,
        "l1_sequential": 1,
        "l2_stateful": 2,
        "l3_concurrent": 3,
        "l4_coordinated": 4,
        "l5_adaptive": 5,
    }.get(value)


def _regular_group_measurement(
    group: CampaignGroup, output: pathlib.Path
) -> dict[str, Any]:
    results_path = output / "results.json"
    if not results_path.is_file():
        return {
            "objective_score": None,
            "score_availability": "unavailable",
            "coverage": 0.0,
            "product_passed": None,
            "infrastructure_valid": False,
        }
    report = _load_json(results_path)
    scenarios = report.get("scenarios")
    if not isinstance(scenarios, list):
        raise CampaignError(f"{results_path}: scenarios must be an array")
    medians: list[float] = []
    expected_runs = group.runs * len(group.scenarios)
    scored_runs = 0
    technical_failures = 0
    observed_weights: list[int] = []
    for scenario in scenarios:
        if not isinstance(scenario, dict):
            continue
        aggregate = scenario.get("aggregate")
        if isinstance(aggregate, dict):
            median = aggregate.get("median_score")
            if isinstance(median, (int, float)) and not isinstance(median, bool):
                medians.append(float(median))
            scored = aggregate.get("scored_runs")
            if isinstance(scored, int) and not isinstance(scored, bool):
                scored_runs += scored
            failures = aggregate.get("technical_failures")
            if isinstance(failures, int) and not isinstance(failures, bool):
                technical_failures += failures
        case = scenario.get("case")
        complexity = case.get("complexity") if isinstance(case, dict) else None
        tier = complexity.get("tier") if isinstance(complexity, dict) else None
        weight = _tier_weight(tier)
        if weight is not None:
            observed_weights.append(weight)
    if observed_weights and max(observed_weights) != group.difficulty_weight:
        raise CampaignError(
            f"{results_path}: observed difficulty weight does not match campaign"
        )
    coverage = min(1.0, scored_runs / expected_runs) if expected_runs else 0.0
    objective_score = sum(medians) / len(medians) if medians else None
    availability = (
        "complete"
        if objective_score is not None
        and coverage >= 1.0
        and technical_failures == 0
        else "partial"
        if objective_score is not None
        else "unavailable"
    )
    return {
        "objective_score": objective_score,
        "score_availability": availability,
        "coverage": coverage,
        "product_passed": report.get("passed")
        if isinstance(report.get("passed"), bool)
        else None,
        "infrastructure_valid": technical_failures == 0,
    }


def _fault_group_measurement(
    group: CampaignGroup, output: pathlib.Path
) -> dict[str, Any]:
    evaluations = sorted(output.glob("run-*/fault-evaluation.json"))
    if not evaluations:
        return {
            "objective_score": None,
            "score_availability": "unavailable",
            "coverage": 0.0,
            "product_passed": None,
            "infrastructure_valid": False,
        }
    scores: list[float] = []
    infrastructure_valid = True
    product_passed = True
    for path in evaluations:
        evaluation = _load_json(path)
        classification = evaluation.get("classification")
        if classification == "infrastructure_failure":
            infrastructure_valid = False
            continue
        score = 100.0 if classification == "correct_recovery" else 0.0
        scores.append(score)
        product_passed = product_passed and score == 100.0
    coverage = min(1.0, len(scores) / group.runs)
    objective_score = sum(scores) / len(scores) if scores else None
    availability = (
        "complete"
        if objective_score is not None
        and coverage >= 1.0
        and infrastructure_valid
        else "partial"
        if objective_score is not None
        else "unavailable"
    )
    return {
        "objective_score": objective_score,
        "score_availability": availability,
        "coverage": coverage,
        "product_passed": product_passed if scores else None,
        "infrastructure_valid": infrastructure_valid,
    }


def score_campaign(
    campaign: Campaign, group_results: Sequence[dict[str, Any]]
) -> dict[str, Any]:
    by_id = {result["group_id"]: result for result in group_results}
    scored_weight = 0
    expected_weight = sum(group.difficulty_weight for group in campaign.groups)
    weighted_score = 0.0
    coverage = 0.0
    product_values: list[bool] = []
    infrastructure_valid = True
    for group in campaign.groups:
        result = by_id[group.id]
        output = pathlib.Path(result["output"])
        measurement = (
            _fault_group_measurement(group, output)
            if group.execution_kind == "fault_injection"
            else _regular_group_measurement(group, output)
        )
        result.update(
            {
                **measurement,
                "difficulty_weight": group.difficulty_weight,
            }
        )
        score = measurement["objective_score"]
        if isinstance(score, (int, float)):
            scored_weight += group.difficulty_weight
            weighted_score += float(score) * group.difficulty_weight
        coverage += float(measurement["coverage"])
        if isinstance(measurement["product_passed"], bool):
            product_values.append(measurement["product_passed"])
        infrastructure_valid = (
            infrastructure_valid and measurement["infrastructure_valid"]
        )
    harness_score = weighted_score / scored_weight if scored_weight else None
    availability = (
        "complete"
        if scored_weight == expected_weight and infrastructure_valid
        else "partial"
        if harness_score is not None
        else "unavailable"
    )
    return {
        "profile": campaign.scoring_profile,
        "harness_score": harness_score,
        "score_availability": availability,
        "scored_weight": scored_weight,
        "expected_weight": expected_weight,
        "coverage": coverage / len(campaign.groups),
        "product_passed": all(product_values)
        if len(product_values) == len(campaign.groups)
        else None,
        "infrastructure_valid": infrastructure_valid,
    }


def _utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")


def _write_json_atomic(path: pathlib.Path, value: Mapping[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temporary.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    temporary.replace(path)


def _run_process(
    command: Sequence[str],
    *,
    environment: Mapping[str, str],
    run_process: Callable[..., Any],
) -> tuple[int, str | None]:
    try:
        completed = run_process(list(command), env=dict(environment), check=False)
        return int(completed.returncode), None
    except OSError as error:
        return 127, str(error)


def _execute_fault_group(
    group: CampaignGroup,
    *,
    e2e_bin: pathlib.Path,
    fault_runner: pathlib.Path,
    profile_root: pathlib.Path,
    output: pathlib.Path,
    environment: Mapping[str, str],
    run_process: Callable[..., Any],
    monotonic: Callable[[], float],
) -> tuple[int, str | None, list[list[str]]]:
    if not group.fault_profile or not group.fault_scenario:
        raise CampaignError(f"fault group {group.id} is incomplete")
    output.mkdir(parents=True, exist_ok=True)
    profile = profile_root / f"{group.fault_profile}.json"
    plan = output / "fault-plan.json"
    commands: list[list[str]] = []
    plan_command = [
        str(e2e_bin),
        "fault-plan",
        "--profile",
        str(profile),
        "--output",
        str(plan),
    ]
    commands.append(plan_command)
    return_code, error = _run_process(
        plan_command, environment=environment, run_process=run_process
    )
    if return_code != 0:
        return return_code, error, commands

    deadline = monotonic() + group.soak_minutes * 60
    iteration = 0
    while iteration < group.runs or monotonic() < deadline:
        iteration += 1
        iteration_output = output / f"run-{iteration}"
        iteration_output.mkdir(parents=True, exist_ok=True)
        supervisor_command = [
            str(fault_runner),
            "--e2e-bin",
            str(e2e_bin),
            "--profile",
            str(profile),
            "--plan",
            str(plan),
            "--scenario",
            group.fault_scenario,
            "--iteration",
            str(iteration),
            "--output",
            str(iteration_output),
        ]
        commands.append(supervisor_command)
        return_code, error = _run_process(
            supervisor_command, environment=environment, run_process=run_process
        )
        if return_code != 0:
            return return_code, error, commands
        journal = iteration_output / "fault-journal.json"
        evaluation = iteration_output / "fault-evaluation.json"
        evaluation_command = [
            str(e2e_bin),
            "fault-evaluate",
            "--profile",
            str(profile),
            "--plan",
            str(plan),
            "--journal",
            str(journal),
            "--output",
            str(evaluation),
        ]
        results = iteration_output / "results"
        if results.exists():
            evaluation_command.extend(["--results", str(results)])
        commands.append(evaluation_command)
        return_code, error = _run_process(
            evaluation_command, environment=environment, run_process=run_process
        )
        if return_code != 0:
            return return_code, error, commands
    return 0, None, commands


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
    judge_model: str | None = None,
    judge_provider: str | None = None,
    url: str | None = None,
    progress_interval_seconds: int | None = None,
    fault_runner: pathlib.Path = pathlib.Path(
        "/opt/iii-harness-e2e/run-weekly-stress"
    ),
    profile_root: pathlib.Path = pathlib.Path("config/profiles"),
    environ: Mapping[str, str] | None = None,
    run_process: Callable[..., Any] = subprocess.run,
    monotonic: Callable[[], float] = time.monotonic,
) -> dict[str, Any]:
    if not SAFE_EXECUTION_ID.fullmatch(execution_id):
        raise CampaignError("execution_id contains unsafe characters")
    base_environment = dict(os.environ if environ is None else environ)
    if bool(judge_model) != bool(judge_provider):
        raise CampaignError("judge_model and judge_provider must be supplied together")
    if not dry_run:
        if not model and not base_environment.get("HARNESS_E2E_MODEL"):
            raise CampaignError(
                "model is required via --model or HARNESS_E2E_MODEL"
            )
        if not provider and not base_environment.get("HARNESS_E2E_PROVIDER"):
            raise CampaignError(
                "provider is required via --provider or HARNESS_E2E_PROVIDER"
            )
        has_markdown = any(
            group.id == f"{campaign.campaign_id}-markdown"
            for group in campaign.groups
        )
        resolved_judge_model = judge_model or base_environment.get(
            "HARNESS_E2E_JUDGE_MODEL"
        )
        resolved_judge_provider = judge_provider or base_environment.get(
            "HARNESS_E2E_JUDGE_PROVIDER"
        )
        if has_markdown and (not resolved_judge_model or not resolved_judge_provider):
            raise CampaignError(
                "Markdown campaign groups require an explicit judge model and provider"
            )

    execution_root = output_root / campaign.campaign_id / execution_id
    started_at = _utc_now()
    group_results: list[dict[str, Any]] = []
    for group in campaign.groups:
        group_output = execution_root / group.id
        command = (
            [str(fault_runner), "--profile", str(group.fault_profile)]
            if group.execution_kind == "fault_injection"
            else build_group_command(
                campaign,
                group,
                e2e_bin=e2e_bin,
                output=group_output,
                model=model,
                provider=provider,
                judge_model=judge_model,
                judge_provider=judge_provider,
                url=url,
                progress_interval_seconds=progress_interval_seconds,
            )
        )
        print(f"[{campaign.campaign_id}/{group.id}] {shlex.join(command)}", flush=True)
        result: dict[str, Any] = {
            "group_id": group.id,
            "execution_kind": group.execution_kind,
            "scenarios": list(group.scenarios),
            "runs": group.runs,
            "technical_retries": group.technical_retries,
            "difficulty_weight": group.difficulty_weight,
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
        materialized_group = {
            "schema": "harness-e2e-materialized-campaign-group/v1",
            "campaign_id": campaign.campaign_id,
            "lane": campaign.lane,
            "failure_policy": campaign.failure_policy,
            "group": dataclasses.asdict(group),
            "runner": str(e2e_bin),
            "model": model or child_environment.get("HARNESS_E2E_MODEL"),
            "provider": provider or child_environment.get("HARNESS_E2E_PROVIDER"),
            "judge_model": judge_model
            or child_environment.get("HARNESS_E2E_JUDGE_MODEL"),
            "judge_provider": judge_provider
            or child_environment.get("HARNESS_E2E_JUDGE_PROVIDER"),
            "url": url or child_environment.get("III_URL", "ws://127.0.0.1:49134"),
            "progress_interval_seconds": progress_interval_seconds,
        }
        canonical = json.dumps(
            materialized_group, sort_keys=True, separators=(",", ":")
        ).encode("utf-8")
        materialized_group["sha256"] = f"sha256:{hashlib.sha256(canonical).hexdigest()}"
        materialized_path = group_output / "materialized-campaign-group.json"
        _write_json_atomic(materialized_path, materialized_group)
        result["materialized_group"] = str(materialized_path)
        result["materialized_group_sha256"] = materialized_group["sha256"]
        started = monotonic()
        if group.execution_kind == "fault_injection":
            return_code, error_message, commands = _execute_fault_group(
                group,
                e2e_bin=e2e_bin,
                fault_runner=fault_runner,
                profile_root=profile_root,
                output=group_output,
                environment=child_environment,
                run_process=run_process,
                monotonic=monotonic,
            )
            result["commands"] = commands
        else:
            return_code, error_message = _run_process(
                command, environment=child_environment, run_process=run_process
            )
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

    scoring = score_campaign(campaign, group_results)
    objective_passed = (
        scoring["product_passed"]
        if isinstance(scoring["product_passed"], bool)
        else all(result["status"] in {"passed", "dry_run"} for result in group_results)
    )
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
        "scoring": scoring,
        "process_exit_code": process_exit_code,
        "groups": group_results,
    }


def aggregate_existing_campaign(
    campaign: Campaign,
    *,
    group_root: pathlib.Path,
    execution_id: str,
) -> dict[str, Any]:
    """Aggregate group artifacts produced by isolated workflow jobs."""
    if not SAFE_EXECUTION_ID.fullmatch(execution_id):
        raise CampaignError("execution_id contains unsafe characters")
    group_results: list[dict[str, Any]] = []
    for group in campaign.groups:
        output = group_root / group.id
        failure = output / "failure.json"
        has_native_result = (
            any(output.glob("run-*/fault-evaluation.json"))
            if group.execution_kind == "fault_injection"
            else (output / "results.json").is_file()
        )
        group_results.append(
            {
                "group_id": group.id,
                "execution_kind": group.execution_kind,
                "scenarios": list(group.scenarios),
                "runs": group.runs,
                "technical_retries": group.technical_retries,
                "difficulty_weight": group.difficulty_weight,
                "output": str(output),
                "status": "passed"
                if has_native_result and not failure.is_file()
                else "failed",
                "exit_code": 0
                if has_native_result and not failure.is_file()
                else 1,
                "duration_ms": None,
            }
        )
    scoring = score_campaign(campaign, group_results)
    return {
        "kind": "harness-e2e-campaign-summary",
        "campaign_id": campaign.campaign_id,
        "lane": campaign.lane,
        "execution_id": execution_id,
        "advisory": True,
        "dry_run": False,
        "started_at": None,
        "completed_at": _utc_now(),
        "objective_passed": scoring["product_passed"],
        "scoring": scoring,
        "process_exit_code": 0,
        "groups": group_results,
    }


def build_campaign_bundle(
    summary: Mapping[str, Any],
    *,
    summary_path: pathlib.Path,
    manifest_path: pathlib.Path,
    scoring_profile_path: pathlib.Path,
) -> dict[str, Any]:
    root = summary_path.parent
    groups: list[dict[str, Any]] = []
    for raw_group in summary.get("groups", []):
        if not isinstance(raw_group, dict):
            continue
        output = pathlib.Path(str(raw_group["output"]))
        artifacts: list[dict[str, Any]] = []
        candidates = [
            output / "results.json",
            output / "manifest.json",
            output / "observation.json",
            output / "failure.json",
            output / "fault-plan.json",
        ]
        candidates.extend(sorted(output.glob("run-*/*.json")))
        for path in candidates:
            if path.is_file():
                artifacts.append(_file_reference(path, root))
        groups.append(
            {
                "group_id": raw_group.get("group_id"),
                "execution_kind": raw_group.get("execution_kind"),
                "status": raw_group.get("status"),
                "difficulty_weight": raw_group.get("difficulty_weight"),
                "objective_score": raw_group.get("objective_score"),
                "score_availability": raw_group.get("score_availability"),
                "artifacts": artifacts,
            }
        )
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    scoring_profile = json.loads(scoring_profile_path.read_text(encoding="utf-8"))
    return {
        "schema": "e2e-campaign-observation-bundle/v1",
        "campaign_id": summary["campaign_id"],
        "execution_id": summary["execution_id"],
        "lane": summary["lane"],
        "manifest_sha256": _canonical_sha256(manifest),
        "scoring_profile": SCORING_PROFILE,
        "scoring_profile_sha256": _canonical_sha256(scoring_profile),
        "summary": _file_reference(summary_path, root),
        "groups": groups,
    }


def validate_campaign_bundle(
    bundle: Mapping[str, Any], *, root: pathlib.Path
) -> None:
    """Verify every native artifact referenced by a campaign bundle.

    The bundle deliberately hashes file bytes instead of reparsing JSON. This
    makes the exact Harness artifacts recoverable and prevents an ingesting
    service from silently normalizing or reconstructing them.
    """
    if bundle.get("schema") != "e2e-campaign-observation-bundle/v1":
        raise CampaignError("unsupported campaign bundle schema")
    references: list[Any] = [bundle.get("summary")]
    groups = bundle.get("groups")
    if not isinstance(groups, list):
        raise CampaignError("campaign bundle groups must be an array")
    for group in groups:
        if not isinstance(group, dict) or not isinstance(group.get("artifacts"), list):
            raise CampaignError("campaign bundle group artifacts must be an array")
        references.extend(group["artifacts"])

    resolved_root = root.resolve()
    for reference in references:
        if not isinstance(reference, dict):
            raise CampaignError("campaign bundle artifact reference must be an object")
        relative = reference.get("path")
        expected_sha256 = reference.get("sha256")
        expected_size = reference.get("size_bytes")
        if not isinstance(relative, str) or not relative:
            raise CampaignError("campaign bundle artifact path must be non-empty")
        candidate = (resolved_root / relative).resolve()
        try:
            candidate.relative_to(resolved_root)
        except ValueError as error:
            raise CampaignError(
                f"campaign bundle artifact escapes root: {relative}"
            ) from error
        try:
            payload = candidate.read_bytes()
        except OSError as error:
            raise CampaignError(
                f"cannot read campaign bundle artifact {relative}: {error}"
            ) from error
        actual_sha256 = f"sha256:{hashlib.sha256(payload).hexdigest()}"
        if actual_sha256 != expected_sha256:
            raise CampaignError(f"campaign bundle digest mismatch: {relative}")
        if len(payload) != expected_size:
            raise CampaignError(f"campaign bundle size mismatch: {relative}")


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
    parser.add_argument("--aggregate-existing-root", type=pathlib.Path)
    parser.add_argument("--summary", type=pathlib.Path)
    parser.add_argument("--bundle", type=pathlib.Path)
    parser.add_argument(
        "--scoring-profile",
        type=pathlib.Path,
        default=pathlib.Path(__file__).resolve().parents[1]
        / "config"
        / "scoring"
        / "difficulty-weighted-v1.json",
    )
    parser.add_argument(
        "--fault-runner",
        type=pathlib.Path,
        default=pathlib.Path(
            os.environ.get(
                "HARNESS_E2E_FAULT_RUNNER",
                "/opt/iii-harness-e2e/run-weekly-stress",
            )
        ),
    )
    parser.add_argument(
        "--profile-root",
        type=pathlib.Path,
        default=pathlib.Path(__file__).resolve().parents[1] / "config" / "profiles",
    )
    parser.add_argument("--model")
    parser.add_argument("--provider")
    parser.add_argument("--judge-model")
    parser.add_argument("--judge-provider")
    parser.add_argument("--url")
    parser.add_argument(
        "--scenarios-directory", type=pathlib.Path, default=pathlib.Path("scenarios")
    )
    parser.add_argument("--progress-interval-seconds", type=int)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        campaign = attach_markdown_group(
            load_campaign(args.manifest), args.scenarios_directory
        )
        scoring_profile = _load_json(args.scoring_profile)
        if scoring_profile.get("profile") != SCORING_PROFILE:
            raise CampaignError("scoring profile identity does not match the campaign")
        if args.validate_only:
            print(
                json.dumps(
                    {
                        "campaign_id": campaign.campaign_id,
                        "manifest_sha256": _canonical_sha256(
                            json.loads(args.manifest.read_text(encoding="utf-8"))
                        ),
                        "scoring_profile": SCORING_PROFILE,
                        "scoring_profile_sha256": _canonical_sha256(scoring_profile),
                    },
                    sort_keys=True,
                )
            )
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
        if args.aggregate_existing_root is not None:
            if args.dry_run:
                raise CampaignError("--aggregate-existing-root cannot be used with --dry-run")
            summary = aggregate_existing_campaign(
                campaign,
                group_root=args.aggregate_existing_root,
                execution_id=execution_id,
            )
        else:
            summary = execute_campaign(
                campaign,
                e2e_bin=args.e2e_bin,
                output_root=args.output_root,
                execution_id=execution_id,
                dry_run=args.dry_run,
                advisory=advisory,
                model=args.model,
                provider=args.provider,
                judge_model=args.judge_model,
                judge_provider=args.judge_provider,
                url=args.url,
                progress_interval_seconds=args.progress_interval_seconds,
                fault_runner=args.fault_runner,
                profile_root=args.profile_root,
            )
        summary["manifest_sha256"] = _canonical_sha256(
            json.loads(args.manifest.read_text(encoding="utf-8"))
        )
        summary["scoring"]["profile_sha256"] = _canonical_sha256(
            scoring_profile
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
            if not args.dry_run:
                bundle_path = args.bundle or summary_path.parent / "campaign-bundle.json"
                bundle = build_campaign_bundle(
                    summary,
                    summary_path=summary_path,
                    manifest_path=args.manifest,
                    scoring_profile_path=args.scoring_profile,
                )
                _write_json_atomic(bundle_path, bundle)
                validate_campaign_bundle(bundle, root=summary_path.parent)
                print(f"bundle: {bundle_path}")
        print(json.dumps(summary, sort_keys=True))
        return int(summary["process_exit_code"])
    except CampaignError as error:
        print(f"campaign error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
