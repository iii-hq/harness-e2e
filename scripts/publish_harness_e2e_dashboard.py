#!/usr/bin/env python3
"""Update the static Harness E2E execution manifest and retained reports."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import tempfile
from datetime import datetime
from pathlib import Path
from typing import Any

MANIFEST_FILENAME = "executions.json"
FAILURE_CONCLUSIONS = {
    "action_required",
    "failure",
    "startup_failure",
    "stale",
    "timed_out",
}
MAX_PUBLIC_LIST_ITEMS = 100
MAX_PUBLIC_TEXT_CHARS = 2_000


class PublishError(ValueError):
    """Raised when dashboard publication inputs are unsafe or malformed."""


def write_text_atomic(path: Path, text: str) -> None:
    """Replace a published artifact atomically within its destination directory."""
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", suffix=".tmp", dir=path.parent
    )
    temporary_path = Path(temporary_name)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as temporary:
            temporary.write(text)
            temporary.flush()
            os.fsync(temporary.fileno())
        os.replace(temporary_path, path)
    finally:
        temporary_path.unlink(missing_ok=True)


def write_json_atomic(path: Path, value: dict[str, Any]) -> None:
    write_text_atomic(
        path, json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n"
    )


def load_json(path: Path | None) -> dict[str, Any] | None:
    if path is None or not path.is_file():
        return None
    try:
        value = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as exc:
        raise PublishError(f"cannot decode {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise PublishError(f"{path} must contain an object")
    return value


def load_manifest(path: Path) -> dict[str, Any]:
    if not path.is_file():
        return {"executions": []}
    text = path.read_text().strip()
    try:
        value = json.loads(text)
    except json.JSONDecodeError as exc:
        raise PublishError(f"cannot decode {path}: {exc}") from exc
    if not isinstance(value, dict) or not isinstance(value.get("executions"), list):
        raise PublishError(f"{path} has an invalid execution manifest")
    return value


def validate_artifact_identity(
    value: dict[str, Any] | None,
    metadata: dict[str, Any],
    *,
    label: str,
) -> tuple[dict[str, Any] | None, dict[str, Any] | None]:
    if value is None:
        return None, None
    execution = value.get("execution")
    if not isinstance(execution, dict):
        return None, {
            "kind": "artifact_identity",
            "message": f"Ignored {label}: execution identity is missing",
        }
    actual_run_id = str(execution.get("run_id") or "")
    actual_attempt = int(
        optional_number(execution.get("attempt") or execution.get("run_attempt"))
        or 0
    )
    expected_run_id = str(metadata["run_id"])
    expected_attempt = int(metadata["attempt"])
    if actual_run_id == expected_run_id and actual_attempt == expected_attempt:
        return value, None
    actual = f"{actual_run_id or 'unknown'} attempt {actual_attempt or 'unknown'}"
    expected = f"{expected_run_id} attempt {expected_attempt}"
    return None, {
        "kind": "artifact_identity",
        "message": f"Ignored {label} from {actual}; expected {expected}",
    }


def optional_number(value: Any) -> float | None:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    return float(value)


def sum_complete(values: list[float | None]) -> float | None:
    return sum(value for value in values if value is not None) if values and all(
        value is not None for value in values
    ) else None


def sum_subject_counts(
    subjects: list[dict[str, Any]], field: str
) -> int | None:
    values = [optional_number(subject.get(field)) for subject in subjects]
    total = sum_complete(values)
    return int(total) if total is not None else None


def mean_available(values: list[float | None]) -> tuple[float | None, int]:
    available = [value for value in values if value is not None]
    if not available:
        return None, 0
    return sum(available) / len(available), len(available)


def run_metric(run: dict[str, Any], metric_id: str) -> float | None:
    metrics = run.get("metrics", {})
    totals = metrics.get("totals", {}) if isinstance(metrics, dict) else {}
    if not isinstance(totals, dict):
        totals = {}
    if metric_id == "tokens":
        input_tokens = optional_number(totals.get("input_tokens"))
        output_tokens = optional_number(totals.get("output_tokens"))
        if input_tokens is None or output_tokens is None:
            return None
        return input_tokens + output_tokens
    if metric_id == "duration_seconds":
        wall_time_ms = optional_number(run.get("wall_time_ms"))
        return wall_time_ms / 1000 if wall_time_ms is not None else None
    if metric_id == "cost_usd":
        cost = run.get("cost", {})
        return (
            optional_number(cost.get("total_usd"))
            if isinstance(cost, dict)
            else None
        )
    if metric_id == "context_compactions":
        efficiency = run.get("efficiency", {})
        return (
            optional_number(efficiency.get("context_compactions"))
            if isinstance(efficiency, dict)
            else None
        )
    return optional_number(totals.get(metric_id))


def scenario_contract(
    scenario: dict[str, Any],
    scenario_id: str,
    runs: list[dict[str, Any]],
) -> dict[str, Any]:
    execution_policy = scenario.get("execution_policy", {})
    if not isinstance(execution_policy, dict):
        execution_policy = {}
    contract = {
        "case_id": scenario.get("case_id") or None,
        "execution_policy": execution_policy,
        "scenario_id": scenario_id,
        "scenario_version": int(
            optional_number(scenario.get("scenario_version")) or 1
        ),
    }
    if isinstance(scenario.get("case"), dict):
        contract["case"] = scenario["case"]
    return contract


def contract_fingerprint(contract: dict[str, Any]) -> str:
    canonical = json.dumps(
        contract,
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    ).encode("utf-8")
    value = 2_166_136_261
    for byte in canonical:
        value ^= byte
        value = (value * 16_777_619) & 0xFFFF_FFFF
    return f"fnv1a32:{value:08x}"


def build_scenario_metrics(detail: dict[str, Any]) -> list[dict[str, Any]]:
    metric_ids = (
        "tokens",
        "duration_seconds",
        "cost_usd",
        "function_calls",
        "function_call_errors",
        "context_compactions",
        "sessions",
        "turns",
    )
    grouped: dict[tuple[str, str], dict[str, Any]] = {}
    reports = detail.get("reports", [])
    if not isinstance(reports, list):
        return []
    for report_entry in reports:
        if not isinstance(report_entry, dict) or not report_entry.get("available"):
            continue
        report = report_entry.get("report", {})
        report_scenarios = report.get("scenarios", []) if isinstance(report, dict) else []
        if not isinstance(report_scenarios, list):
            continue
        for scenario in report_scenarios:
            if not isinstance(scenario, dict):
                continue
            scenario_id = str(
                scenario.get("scenario_id")
                or report_entry.get("scenario_id")
                or ""
            )
            if not scenario_id:
                continue
            subject_id = str(report_entry.get("subject_id") or "")
            runs = scenario.get("runs", [])
            if not isinstance(runs, list):
                continue
            key = (subject_id, scenario_id)
            entry = grouped.setdefault(
                key,
                {
                    "runs": [],
                    "scenario": scenario,
                },
            )
            entry["runs"].extend(run for run in runs if isinstance(run, dict))

    result = []
    for (subject_id, scenario_id), entry in sorted(grouped.items()):
        runs = entry["runs"]
        contract = scenario_contract(entry["scenario"], scenario_id, runs)
        averages: dict[str, float | None] = {}
        samples: dict[str, int] = {}
        for metric_id in metric_ids:
            average, sample_count = mean_available(
                [run_metric(run, metric_id) for run in runs]
            )
            averages[metric_id] = average
            samples[metric_id] = sample_count
        result.append(
            {
                "subject_id": subject_id,
                "scenario_id": scenario_id,
                "scenario_version": contract["scenario_version"],
                "contract_fingerprint": contract_fingerprint(contract),
                "run_count": len(runs),
                "averages": averages,
                "samples": samples,
            }
        )
    return result


def build_execution_efficiency_totals(detail: dict[str, Any]) -> dict[str, float | None]:
    tokens: list[float | None] = []
    function_calls: list[float | None] = []
    context_compactions: list[float | None] = []
    reports = detail.get("reports", [])
    if not isinstance(reports, list):
        reports = []
    for report_entry in reports:
        if not isinstance(report_entry, dict) or not report_entry.get("available"):
            continue
        report = report_entry.get("report", {})
        scenarios = report.get("scenarios", []) if isinstance(report, dict) else []
        if not isinstance(scenarios, list):
            continue
        for scenario in scenarios:
            runs = scenario.get("runs", []) if isinstance(scenario, dict) else []
            if not isinstance(runs, list):
                continue
            for run in runs:
                if not isinstance(run, dict):
                    continue
                tokens.append(run_metric(run, "tokens"))
                function_calls.append(run_metric(run, "function_calls"))
                context_compactions.append(run_metric(run, "context_compactions"))
    return {
        "total_tokens": sum_complete(tokens),
        "function_calls": sum_complete(function_calls),
        "context_compactions": sum_complete(context_compactions),
    }


def _pick(value: Any, keys: tuple[str, ...]) -> dict[str, Any]:
    source = value if isinstance(value, dict) else {}
    return {key: source[key] for key in keys if key in source}


def _public_failures(value: Any) -> list[dict[str, Any]]:
    if not isinstance(value, list):
        return []
    return [
        _bounded_json(_pick(failure, ("domain", "phase", "message")))
        for failure in value[:MAX_PUBLIC_LIST_ITEMS]
        if isinstance(failure, dict)
    ]


def _public_gates(value: Any) -> list[dict[str, Any]]:
    if not isinstance(value, list):
        return []
    return [
        _pick(gate, ("id", "dimension", "passed", "reason"))
        for gate in value[:MAX_PUBLIC_LIST_ITEMS]
        if isinstance(gate, dict)
    ]


def _public_deliverables(value: Any) -> list[dict[str, Any]]:
    if not isinstance(value, list):
        return []
    deliverables = []
    for item in value:
        if not isinstance(item, dict):
            continue
        deliverable = _pick(
            item,
            (
                "id",
                "kind",
                "media_type",
                "content_sha256",
                "content_size_bytes",
                "schema_valid",
                "provenance_valid",
            ),
        )
        deliverable["invariants"] = _bounded_json(item.get("invariants", []))
        if isinstance(item.get("artifact"), dict):
            deliverable["artifact"] = _pick(
                item["artifact"],
                ("id", "kind", "sha256", "size_bytes", "media_type"),
            )
        deliverables.append(deliverable)
        if len(deliverables) >= MAX_PUBLIC_LIST_ITEMS:
            break
    return deliverables


def _public_dimensions(value: Any) -> list[dict[str, Any]]:
    if not isinstance(value, list):
        return []
    return [
        {
            **_pick(item, ("dimension", "passed")),
            "signals": _bounded_json(item.get("signals")),
        }
        for item in value[:MAX_PUBLIC_LIST_ITEMS]
        if isinstance(item, dict)
    ]


def _public_metrics(value: Any) -> dict[str, Any] | None:
    if not isinstance(value, dict):
        return None
    totals = _pick(
        value.get("totals"),
        (
            "input_tokens",
            "output_tokens",
            "cache_read_tokens",
            "cache_creation_tokens",
            "reasoning_tokens",
            "function_calls",
            "function_call_errors",
            "context_compactions",
            "sessions",
            "turns",
            "cost_usd",
        ),
    )
    return {"totals": totals} if totals else None


def _public_efficiency(value: Any) -> dict[str, Any] | None:
    if not isinstance(value, dict):
        return None
    efficiency = _pick(value, ("context_compactions",))
    return efficiency or None


def _bounded_json(value: Any, *, depth: int = 0) -> Any:
    if isinstance(value, str):
        return value[:MAX_PUBLIC_TEXT_CHARS]
    if value is None or isinstance(value, (int, float, bool)):
        return value
    if depth >= 8:
        return None
    if isinstance(value, list):
        return [
            _bounded_json(item, depth=depth + 1)
            for item in value[:MAX_PUBLIC_LIST_ITEMS]
        ]
    if isinstance(value, dict):
        return {
            str(key)[:MAX_PUBLIC_TEXT_CHARS]: _bounded_json(item, depth=depth + 1)
            for key, item in list(value.items())[:MAX_PUBLIC_LIST_ITEMS]
        }
    return str(value)[:MAX_PUBLIC_TEXT_CHARS]


def _public_evidence(value: Any) -> list[dict[str, Any]]:
    if not isinstance(value, list):
        return []
    return [
        _bounded_json(_pick(item, ("artifact_id", "artifact_sha256", "locator")))
        for item in value[:MAX_PUBLIC_LIST_ITEMS]
        if isinstance(item, dict)
    ]


def _public_analyzer(value: Any) -> dict[str, Any] | None:
    if not isinstance(value, dict):
        return None
    return _bounded_json(
        _pick(value, ("analyzer", "provider", "model", "input_sha256"))
    )


def _public_assessment(value: Any) -> dict[str, Any] | None:
    if not isinstance(value, dict):
        return None
    assessment = _bounded_json(
        _pick(
            value,
            (
                "criterion_id",
                "target",
                "kind",
                "policy",
                "dimension",
                "source",
                "outcome",
                "score",
                "confidence",
                "summary",
            ),
        )
    )
    assessment["evidence"] = _public_evidence(value.get("evidence"))
    analyzer = _public_analyzer(value.get("analyzer"))
    if analyzer is not None:
        assessment["analyzer"] = analyzer
    if isinstance(value.get("analyzer_usage"), dict):
        assessment["analyzer_usage"] = _pick(
            value["analyzer_usage"],
            ("latency_ms", "input_tokens", "output_tokens", "cost_usd"),
        )
    return assessment


def _public_asset_assessment(value: Any) -> dict[str, Any] | None:
    if not isinstance(value, dict) or not isinstance(value.get("validation"), dict):
        return None
    validation = _bounded_json(
        _pick(value["validation"], ("asset_id", "outcome", "summary"))
    )
    validation["evidence"] = _public_evidence(value["validation"].get("evidence"))
    qualitative = _public_assessment(value.get("qualitative_assessment"))
    if qualitative is None:
        return None
    return {"validation": validation, "qualitative_assessment": qualitative}


def _public_final_assessment(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        return {
            "availability": "not_evaluated",
            "reason": "assessment contract unavailable",
        }
    assessment = _bounded_json(_pick(value, ("availability", "reason")))
    result = value.get("result")
    if isinstance(result, dict):
        public_result = _bounded_json(
            _pick(
                result,
                (
                    "verdict",
                    "quality_score",
                    "confidence",
                    "summary",
                    "facts",
                    "strengths",
                    "concerns",
                    "recommendation",
                    "limitations",
                ),
            )
        )
        public_result["evidence"] = _public_evidence(result.get("evidence"))
        assessment["result"] = public_result
    analyzer = _public_analyzer(value.get("analyzer"))
    if analyzer is not None:
        assessment["analyzer"] = analyzer
    if isinstance(value.get("analyzer_usage"), dict):
        assessment["analyzer_usage"] = _pick(
            value["analyzer_usage"],
            ("latency_ms", "input_tokens", "output_tokens", "cost_usd"),
        )
    return assessment


def _public_run_assessment(value: Any) -> dict[str, Any] | None:
    if not isinstance(value, dict):
        return None
    run = _bounded_json(
        _pick(value, ("run_id", "attempt_id", "system_status", "effective_status"))
    )
    run["assessments"] = [
        assessment
        for item in value.get("assessments", [])[:MAX_PUBLIC_LIST_ITEMS]
        if (assessment := _public_assessment(item)) is not None
    ] if isinstance(value.get("assessments"), list) else []
    run["assets"] = [
        assessment
        for item in value.get("assets", [])[:MAX_PUBLIC_LIST_ITEMS]
        if (assessment := _public_asset_assessment(item)) is not None
    ] if isinstance(value.get("assets"), list) else []
    run["ai_final_assessment"] = _public_final_assessment(
        value.get("ai_final_assessment")
    )
    return run


def _public_assessment_contract(value: Any) -> tuple[str, dict[str, Any]]:
    if not isinstance(value, dict) or not isinstance(value.get("runs"), list):
        return "unavailable", {"runs": []}
    runs = [
        run
        for item in value["runs"][:MAX_PUBLIC_LIST_ITEMS]
        if (run := _public_run_assessment(item)) is not None
    ]
    return "available", {"runs": runs}


def _unavailable_run_assessment(value: dict[str, Any]) -> dict[str, Any]:
    return {
        "run_id": str(value.get("run_id") or ""),
        "attempt_id": str(value.get("attempt_id") or ""),
        "system_status": "unavailable",
        "assessments": [],
        "assets": [],
        "ai_final_assessment": {
            "availability": "not_evaluated",
            "reason": "assessment contract unavailable",
        },
        "effective_status": "unavailable",
    }


def _public_retry(value: Any) -> dict[str, Any] | None:
    if not isinstance(value, dict):
        return None
    retry = _pick(
        value,
        (
            "run_id",
            "attempt_id",
            "attempt_number",
            "session_id",
            "wall_time_ms",
            "status",
        ),
    )
    retry["cost"] = _pick(value.get("cost"), ("subject_usd", "judge_usd", "total_usd"))
    retry["failures"] = _public_failures(value.get("failures"))
    retry["deliverables"] = _public_deliverables(value.get("deliverables"))
    retry["dimensions"] = _public_dimensions(value.get("dimensions"))
    efficiency = _public_efficiency(value.get("efficiency"))
    if efficiency is not None:
        retry["efficiency"] = efficiency
    return retry


def _public_run(
    value: Any,
    assessments: dict[tuple[str, str], dict[str, Any]],
) -> dict[str, Any] | None:
    if not isinstance(value, dict):
        return None
    run = _pick(
        value,
        (
            "run_id",
            "attempt_id",
            "attempt_number",
            "session_id",
            "wall_time_ms",
            "score",
            "status",
            "judge_attempts",
        ),
    )
    metrics = _public_metrics(value.get("metrics"))
    if metrics is not None:
        run["metrics"] = metrics
    efficiency = _public_efficiency(value.get("efficiency"))
    if efficiency is not None:
        run["efficiency"] = efficiency
    run["cost"] = _pick(value.get("cost"), ("subject_usd", "judge_usd", "total_usd"))
    run["criteria"] = _bounded_json(value.get("criteria", []))
    run["hard_gates"] = _public_gates(value.get("hard_gates"))
    run["failures"] = _public_failures(value.get("failures"))
    run["deliverables"] = _public_deliverables(value.get("deliverables"))
    run["dimensions"] = _public_dimensions(value.get("dimensions"))
    retries = [
        retry
        for item in value.get("retry_attempts", [])
        if (retry := _public_retry(item)) is not None
    ] if isinstance(value.get("retry_attempts"), list) else []
    run["retry_attempts"] = retries
    identity = (str(value.get("run_id") or ""), str(value.get("attempt_id") or ""))
    run["assessment"] = assessments.get(identity) or _unavailable_run_assessment(value)
    return run


def _public_scenario(
    value: Any,
    assessments: dict[tuple[str, str], dict[str, Any]],
) -> dict[str, Any] | None:
    if not isinstance(value, dict):
        return None
    scenario = _pick(
        value,
        ("scenario_id", "scenario_version", "case_id", "passed"),
    )
    if isinstance(value.get("case"), dict):
        scenario["case"] = _bounded_json(value["case"])
    if isinstance(value.get("execution_policy"), dict):
        scenario["execution_policy"] = _bounded_json(value["execution_policy"])
    aggregate = _pick(
        value.get("aggregate"),
        (
            "runs",
            "scored_runs",
            "passed_runs",
            "required_passes",
            "pass_rate",
            "median_score",
            "hard_gate_failures",
            "technical_failures",
        ),
    )
    raw_aggregate = value.get("aggregate")
    if isinstance(raw_aggregate, dict):
        aggregate["cost"] = _pick(
            raw_aggregate.get("cost"),
            ("subject_usd", "judge_usd", "total_usd"),
        )
    scenario["aggregate"] = aggregate
    scenario["runs"] = [
        run
        for item in value.get("runs", [])
        if (run := _public_run(item, assessments)) is not None
    ] if isinstance(value.get("runs"), list) else []
    scenario["assessment_summary"] = _assessment_summary(
        [run["assessment"] for run in scenario["runs"]]
    )
    return scenario


def _public_subject_summary(value: Any) -> dict[str, Any] | None:
    if not isinstance(value, dict):
        return None
    subject = _pick(
        value,
        (
            "id",
            "model",
            "provider",
            "engine_revision",
            "passed",
            "expected_reports",
            "received_reports",
            "scenario_pass_rate",
            "report_coverage",
            "hard_gate_failures",
            "technical_failures",
            "retry_attempts",
            "total_cost_usd",
            "wall_time_seconds",
        ),
    )
    subject["judge"] = _pick(value.get("judge"), ("model", "provider", "protocol"))
    subject["scenarios"] = [
        _pick(
            scenario,
            (
                "id",
                "status",
                "passed",
                "runs",
                "median_score",
                "pass_rate",
                "hard_gate_failures",
                "technical_failures",
                "retries",
                "total_cost_usd",
                "wall_time_seconds",
                "assessment_summary",
            ),
        )
        for scenario in value.get("scenarios", [])
        if isinstance(scenario, dict)
    ] if isinstance(value.get("scenarios"), list) else []
    return subject


def _contract_runs_for_report(
    value: dict[str, Any], contract: dict[str, Any]
) -> list[dict[str, Any]]:
    identities: set[tuple[str, str]] = set()
    scenarios = value.get("scenarios", [])
    if isinstance(scenarios, list):
        for scenario in scenarios:
            runs = scenario.get("runs", []) if isinstance(scenario, dict) else []
            if not isinstance(runs, list):
                continue
            identities.update(
                (
                    str(run.get("run_id") or ""),
                    str(run.get("attempt_id") or ""),
                )
                for run in runs
                if isinstance(run, dict)
            )
    if not identities:
        return contract["runs"]
    return [
        run
        for run in contract["runs"]
        if (str(run.get("run_id") or ""), str(run.get("attempt_id") or ""))
        in identities
    ]


def _public_report(value: Any) -> dict[str, Any] | None:
    if not isinstance(value, dict):
        return None
    availability, contract = _public_assessment_contract(value.get("assessment_contract"))
    contract["runs"] = _contract_runs_for_report(value, contract)
    assessments = {
        (str(run.get("run_id") or ""), str(run.get("attempt_id") or "")): run
        for run in contract["runs"]
    }
    report = _bounded_json(
        _pick(
            value,
            (
                "execution",
                "system_under_test",
                "manifest",
                "subject",
                "judge",
                "judge_protocol",
                "engine_revision",
                "passed",
                "redaction",
            ),
        )
    )
    report["assessment_availability"] = availability
    report["assessment_contract"] = contract
    report["scenarios"] = [
        scenario
        for item in value.get("scenarios", [])[:MAX_PUBLIC_LIST_ITEMS]
        if (scenario := _public_scenario(item, assessments)) is not None
    ] if isinstance(value.get("scenarios"), list) else []
    report["assessment_summary"] = _assessment_summary(
        [
            run["assessment"]
            for scenario in report["scenarios"]
            for run in scenario.get("runs", [])
            if isinstance(run, dict) and isinstance(run.get("assessment"), dict)
        ]
    )
    return report


def complete_public_detail(
    detail: dict[str, Any],
    metadata: dict[str, Any],
) -> dict[str, Any]:
    """Build a bounded public projection without prompt, transcript, or asset content."""
    public_detail = _bounded_json(
        _pick(
            detail,
            (
                "id",
                "label",
                "run_id",
                "attempt",
                "workflow_name",
                "workflow_url",
                "event",
                "actor",
                "started_at",
                "completed_at",
                "conclusion",
                "status",
                "availability",
                "detail_path",
                "generated_at",
                "lane",
                "release",
                "source",
                "stack",
                "requested_runs",
                "scenario_metrics",
                "capability",
                "totals",
                "workflow_duration_seconds",
                "first_failure",
            ),
        )
    )
    execution_metadata = detail.get("execution", {})
    if not isinstance(execution_metadata, dict):
        execution_metadata = {}
    public_detail["execution"] = _bounded_json({**execution_metadata, **metadata})
    public_detail["subjects"] = [
        subject
        for item in detail.get("subjects", [])[:MAX_PUBLIC_LIST_ITEMS]
        if (subject := _public_subject_summary(item)) is not None
    ] if isinstance(detail.get("subjects"), list) else []
    reports = []
    assessment_runs = []
    scenario_summaries: dict[tuple[str, str], dict[str, Any]] = {}
    raw_reports = detail.get("reports", [])
    if isinstance(raw_reports, list):
        for item in raw_reports[:MAX_PUBLIC_LIST_ITEMS]:
            if not isinstance(item, dict):
                continue
            public_report = _public_report(item.get("report"))
            record = _bounded_json(
                _pick(item, ("subject_id", "scenario_id", "available", "reason"))
            )
            if public_report is not None:
                record["report"] = public_report
                assessment_runs.extend(
                    run["assessment"]
                    for scenario in public_report["scenarios"]
                    for run in scenario.get("runs", [])
                    if isinstance(run, dict)
                    and isinstance(run.get("assessment"), dict)
                )
                for scenario in public_report["scenarios"]:
                    scenario_summaries[
                        (
                            str(record.get("subject_id") or ""),
                            str(scenario.get("scenario_id") or ""),
                        )
                    ] = scenario["assessment_summary"]
            reports.append(record)
    public_detail["reports"] = reports
    public_detail["assessment_summary"] = _assessment_summary(assessment_runs)
    for subject in public_detail["subjects"]:
        for scenario in subject.get("scenarios", []):
            summary = scenario_summaries.get(
                (str(subject.get("id") or ""), str(scenario.get("id") or ""))
            )
            if summary is not None:
                scenario["assessment_summary"] = summary
        subject["assessment_summary"] = _assessment_summary(
            [
                run
                for report in reports
                if report.get("subject_id") == subject.get("id")
                and isinstance(report.get("report"), dict)
                for scenario in report["report"]["scenarios"]
                for run_value in scenario.get("runs", [])
                if isinstance(run_value, dict)
                and isinstance(run_value.get("assessment"), dict)
                for run in [run_value["assessment"]]
            ]
        )
    return public_detail

def elapsed_seconds(started_at: str, completed_at: str) -> float | None:
    if not started_at or not completed_at:
        return None
    try:
        start = datetime.fromisoformat(started_at.replace("Z", "+00:00"))
        end = datetime.fromisoformat(completed_at.replace("Z", "+00:00"))
    except ValueError:
        return None
    return max(0.0, (end - start).total_seconds())


def execution_status(
    conclusion: str,
    subjects: list[dict[str, Any]],
    expected_reports: int,
    received_reports: int,
    hard_gate_failures: int,
    technical_failures: int,
    *,
    has_failed_job: bool = False,
) -> str:
    if conclusion == "cancelled":
        return "cancelled"
    if not conclusion:
        return "running"
    if not subjects or expected_reports == 0:
        if conclusion in FAILURE_CONCLUSIONS or has_failed_job:
            return "infra_failed"
        return "incomplete"
    if received_reports < expected_reports:
        return "incomplete"
    if technical_failures:
        return "technical_failed"
    if hard_gate_failures:
        return "hard_gate_failed"
    if conclusion != "success" or has_failed_job:
        return "infra_failed"
    if not all(bool(subject.get("passed")) for subject in subjects):
        return "infra_failed"
    return "passed"


def _compact_message(value: Any, fallback: str) -> str:
    message = " ".join(str(value or "").split()) or fallback
    return message[:500]


def failed_job_diagnostic(jobs_document: dict[str, Any] | None) -> dict[str, Any] | None:
    jobs = jobs_document.get("jobs", []) if jobs_document else []
    if not isinstance(jobs, list):
        return None
    failed_jobs = [
        job
        for job in jobs
        if isinstance(job, dict) and job.get("conclusion") in FAILURE_CONCLUSIONS
    ]
    failed_jobs.sort(key=lambda job: (str(job.get("started_at") or ""), str(job.get("name") or "")))
    if not failed_jobs:
        return None
    job = failed_jobs[0]
    steps = job.get("steps", [])
    failed_steps = [
        step
        for step in steps
        if isinstance(step, dict) and step.get("conclusion") in FAILURE_CONCLUSIONS
    ] if isinstance(steps, list) else []
    failed_steps.sort(
        key=lambda step: (
            int(optional_number(step.get("number")) or 0),
            str(step.get("name") or ""),
        )
    )
    step = failed_steps[0] if failed_steps else None
    job_name = str(job.get("name") or "workflow job")
    step_name = str(step.get("name") or "") if step else ""
    return {
        "kind": "job",
        "job_name": job_name,
        "step_name": step_name,
        "message": _compact_message(
            f"{job_name}: {step_name}" if step_name else f"{job_name} failed",
            "Workflow job failed",
        ),
        "url": str(job.get("html_url") or ""),
    }


def _report_scenarios(detail: dict[str, Any] | None) -> list[tuple[str, str, dict[str, Any]]]:
    reports = detail.get("reports", []) if detail else []
    if not isinstance(reports, list):
        return []
    result = []
    for report_entry in reports:
        if not isinstance(report_entry, dict) or not report_entry.get("available"):
            continue
        report = report_entry.get("report", {})
        scenarios = report.get("scenarios", []) if isinstance(report, dict) else []
        if not isinstance(scenarios, list):
            continue
        for scenario in scenarios:
            if isinstance(scenario, dict):
                result.append(
                    (
                        str(report_entry.get("subject_id") or ""),
                        str(scenario.get("scenario_id") or report_entry.get("scenario_id") or ""),
                        scenario,
                    )
                )
    return result


def report_diagnostic(
    status: str,
    snapshot: dict[str, Any] | None,
    detail: dict[str, Any] | None,
) -> dict[str, Any] | None:
    if status == "incomplete" and snapshot:
        for subject in snapshot.get("subjects", []):
            if not isinstance(subject, dict):
                continue
            for scenario in subject.get("scenarios", []):
                if isinstance(scenario, dict) and scenario.get("status") == "missing_report":
                    subject_id = str(subject.get("id") or "")
                    scenario_id = str(scenario.get("id") or "")
                    return {
                        "kind": "missing_report",
                        "subject_id": subject_id,
                        "scenario_id": scenario_id,
                        "message": f"Missing report for {subject_id}/{scenario_id}",
                    }

    if status == "technical_failed":
        for subject_id, scenario_id, scenario in _report_scenarios(detail):
            runs = scenario.get("runs", [])
            if not isinstance(runs, list):
                continue
            for run in runs:
                failures = run.get("failures", []) if isinstance(run, dict) else []
                if not isinstance(failures, list) or not failures:
                    continue
                failure = next((item for item in failures if isinstance(item, dict)), None)
                if failure:
                    return {
                        "kind": "technical",
                        "subject_id": subject_id,
                        "scenario_id": scenario_id,
                        "phase": str(failure.get("phase") or "execute"),
                        "message": _compact_message(
                            failure.get("message"), "Technical execution failure"
                        ),
                    }

    if status == "hard_gate_failed":
        for subject_id, scenario_id, scenario in _report_scenarios(detail):
            runs = scenario.get("runs", [])
            if not isinstance(runs, list):
                continue
            for run in runs:
                gates = run.get("hard_gates", []) if isinstance(run, dict) else []
                if not isinstance(gates, list):
                    continue
                gate = next(
                    (
                        item
                        for item in gates
                        if isinstance(item, dict) and not item.get("passed", False)
                    ),
                    None,
                )
                if gate:
                    return {
                        "kind": "hard_gate",
                        "subject_id": subject_id,
                        "scenario_id": scenario_id,
                        "id": str(gate.get("id") or ""),
                        "message": _compact_message(gate.get("reason"), "Hard gate failed"),
                    }

    return None


def _assessment_summaries_from_detail(
    detail: dict[str, Any] | None,
) -> tuple[
    dict[str, Any],
    dict[str, dict[str, Any]],
    dict[tuple[str, str], dict[str, Any]],
]:
    all_runs: list[dict[str, Any]] = []
    subject_runs: dict[str, list[dict[str, Any]]] = {}
    scenario_runs: dict[tuple[str, str], list[dict[str, Any]]] = {}
    reports = detail.get("reports", []) if detail else []
    if not isinstance(reports, list):
        reports = []
    for record in reports:
        if not isinstance(record, dict) or not record.get("available"):
            continue
        report = record.get("report")
        if not isinstance(report, dict):
            continue
        public_report = _public_report(report)
        if public_report is None:
            continue
        subject_id = str(record.get("subject_id") or "")
        for scenario in public_report["scenarios"]:
            scenario_id = str(
                scenario.get("scenario_id") or record.get("scenario_id") or ""
            )
            runs = [
                run["assessment"]
                for run in scenario.get("runs", [])
                if isinstance(run, dict) and isinstance(run.get("assessment"), dict)
            ]
            all_runs.extend(runs)
            subject_runs.setdefault(subject_id, []).extend(runs)
            scenario_runs.setdefault((subject_id, scenario_id), []).extend(runs)
    return (
        _assessment_summary(all_runs),
        {key: _assessment_summary(runs) for key, runs in subject_runs.items()},
        {key: _assessment_summary(runs) for key, runs in scenario_runs.items()},
    )


def build_summary(
    snapshot: dict[str, Any] | None,
    metadata: dict[str, Any],
    *,
    detail: dict[str, Any] | None = None,
    jobs_document: dict[str, Any] | None = None,
    artifact_failure: dict[str, Any] | None = None,
) -> dict[str, Any]:
    subjects = snapshot.get("subjects", []) if snapshot else []
    if not isinstance(subjects, list):
        subjects = []
    valid_subjects = [
        {
            **subject,
            "scenarios": [
                dict(scenario)
                for scenario in subject.get("scenarios", [])
                if isinstance(scenario, dict)
            ] if isinstance(subject.get("scenarios"), list) else [],
        }
        for subject in subjects
        if isinstance(subject, dict)
    ]
    assessment_summary, subject_assessments, scenario_assessments = (
        _assessment_summaries_from_detail(detail)
    )
    for subject in valid_subjects:
        subject_id = str(subject.get("id") or "")
        subject["assessment_summary"] = subject_assessments.get(
            subject_id, _assessment_summary([])
        )
        for scenario in subject["scenarios"]:
            scenario["assessment_summary"] = scenario_assessments.get(
                (subject_id, str(scenario.get("id") or "")),
                _assessment_summary([]),
            )
    scenarios = [
        scenario
        for subject in valid_subjects
        for scenario in subject.get("scenarios", [])
        if isinstance(scenario, dict)
    ]
    expected_reports = sum_subject_counts(valid_subjects, "expected_reports")
    received_reports = sum_subject_counts(valid_subjects, "received_reports")
    passed_scenarios = (
        sum(bool(scenario.get("passed")) for scenario in scenarios)
        if valid_subjects
        else None
    )
    subject_costs = [
        optional_number(subject.get("total_cost_usd")) for subject in valid_subjects
    ]
    subject_wall_times = [
        optional_number(subject.get("wall_time_seconds")) for subject in valid_subjects
    ]
    hard_gate_failures = sum_subject_counts(valid_subjects, "hard_gate_failures")
    technical_failures = sum_subject_counts(valid_subjects, "technical_failures")
    retries = sum_subject_counts(valid_subjects, "retry_attempts")
    missing_reports = (
        max(0, expected_reports - received_reports)
        if expected_reports is not None and received_reports is not None
        else None
    )
    conclusion = metadata["conclusion"]
    job_failure = failed_job_diagnostic(jobs_document)
    status = execution_status(
        conclusion,
        valid_subjects,
        expected_reports or 0,
        received_reports or 0,
        hard_gate_failures or 0,
        technical_failures or 0,
        has_failed_job=job_failure is not None,
    )
    first_failure = (
        job_failure or artifact_failure
        if status == "infra_failed"
        else report_diagnostic(status, snapshot, detail)
        or artifact_failure
        or job_failure
    )
    snapshot_execution = snapshot.get("execution", {}) if snapshot else {}
    if not isinstance(snapshot_execution, dict):
        snapshot_execution = {}

    return {
        "id": metadata["id"],
        "run_id": metadata["run_id"],
        "attempt": metadata["attempt"],
        "workflow_name": metadata["workflow_name"],
        "workflow_url": metadata["workflow_url"],
        "event": metadata["event"],
        "actor": metadata["actor"],
        "started_at": metadata["started_at"],
        "completed_at": metadata["completed_at"],
        "conclusion": conclusion,
        "status": status,
        "first_failure": first_failure,
        "workflow_duration_seconds": elapsed_seconds(
            metadata["started_at"], metadata["completed_at"]
        ),
        "availability": "aggregate" if snapshot else "unavailable",
        "detail_path": None,
        "generated_at": snapshot.get("generated_at", "") if snapshot else "",
        "lane": snapshot.get("lane", "daily") if snapshot else "daily",
        "source": (
            snapshot.get("source", {})
            if snapshot
            else {
                "sha": metadata["head_sha"],
                "ref": metadata["head_branch"],
                "repository": metadata["repository"],
            }
        ),
        "release": snapshot.get("release", {}) if snapshot else {},
        "stack": snapshot.get("stack") if snapshot else None,
        "system_under_test_observations": (
            snapshot.get("system_under_test_observations") if snapshot else None
        ),
        "requested_runs": snapshot.get("requested_runs") if snapshot else None,
        "subjects": valid_subjects,
        "assessment_summary": assessment_summary,
        "totals": {
            "expected_reports": expected_reports,
            "received_reports": received_reports,
            "report_coverage": (
                received_reports / expected_reports * 100
                if expected_reports and received_reports is not None
                else None
            ),
            "passed_scenarios": passed_scenarios,
            "scenario_pass_rate": (
                passed_scenarios / expected_reports * 100
                if expected_reports and passed_scenarios is not None
                else None
            ),
            "total_cost_usd": sum_complete(subject_costs),
            "wall_time_seconds": sum_complete(subject_wall_times),
            "hard_gate_failures": hard_gate_failures,
            "technical_failures": technical_failures,
            "missing_reports": missing_reports,
            "retries": retries,
        },
        "execution": {**snapshot_execution, **metadata},
    }


def sort_key(execution: dict[str, Any]) -> tuple[str, int]:
    timestamp = (
        execution.get("completed_at")
        or execution.get("started_at")
        or execution.get("generated_at")
        or ""
    )
    return str(timestamp), int(execution.get("attempt") or 0)


def _sha256_json(value: Any) -> str:
    encoded = json.dumps(
        value,
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    ).encode("utf-8")
    return f"sha256:{hashlib.sha256(encoded).hexdigest()}"


def _assessment_summary(runs: list[dict[str, Any]]) -> dict[str, Any]:
    status_keys = (
        "unavailable",
        "passed",
        "passed_with_concerns",
        "hard_gate_failed",
        "subject_error",
        "judge_error",
        "resource_limit",
        "infrastructure_error",
    )
    outcome_keys = (
        "passed",
        "failed",
        "partial",
        "not_evaluated",
        "unavailable",
        "error",
    )
    validation_keys = (
        "valid",
        "invalid",
        "malformed",
        "oversized",
        "not_produced",
        "unreadable",
        "unsafe_path",
        "removed_during_cleanup",
        "unexpected",
        "not_evaluated",
    )
    availability_keys = (
        "not_requested",
        "not_evaluated",
        "available",
        "unavailable",
        "malformed",
        "failed",
    )
    verdict_keys = ("pass", "pass_with_concerns", "fail", "inconclusive")
    summary = {
        "run_count": 0,
        "assessment_count": 0,
        "asset_count": 0,
        "evidence_reference_count": 0,
        "system_statuses": dict.fromkeys(status_keys, 0),
        "effective_statuses": dict.fromkeys(status_keys, 0),
        "assessment_outcomes": dict.fromkeys(outcome_keys, 0),
        "asset_qualitative_outcomes": dict.fromkeys(outcome_keys, 0),
        "asset_validation_outcomes": dict.fromkeys(validation_keys, 0),
        "ai_availability": dict.fromkeys(availability_keys, 0),
        "ai_verdicts": dict.fromkeys(verdict_keys, 0),
        "median_quality_score": None,
        "median_confidence": None,
    }
    evidence: set[tuple[str, str, str]] = set()
    quality_scores: list[float] = []
    confidence: list[float] = []

    def count(bucket: str, key: Any) -> None:
        name = str(key or "")
        counts = summary[bucket]
        if name in counts:
            counts[name] += 1

    def remember(items: Any) -> None:
        if not isinstance(items, list):
            return
        for item in items:
            if not isinstance(item, dict):
                continue
            evidence.add(
                (
                    str(item.get("artifact_id") or ""),
                    str(item.get("artifact_sha256") or ""),
                    str(item.get("locator") or ""),
                )
            )

    for run in runs:
        if not isinstance(run, dict):
            continue
        summary["run_count"] += 1
        count("system_statuses", run.get("system_status"))
        count("effective_statuses", run.get("effective_status"))
        assessments = run.get("assessments", [])
        if isinstance(assessments, list):
            for assessment in assessments:
                if not isinstance(assessment, dict):
                    continue
                summary["assessment_count"] += 1
                count("assessment_outcomes", assessment.get("outcome"))
                remember(assessment.get("evidence"))
        assets = run.get("assets", [])
        if isinstance(assets, list):
            for asset in assets:
                if not isinstance(asset, dict):
                    continue
                summary["asset_count"] += 1
                validation = asset.get("validation", {})
                qualitative = asset.get("qualitative_assessment", {})
                if isinstance(validation, dict):
                    count("asset_validation_outcomes", validation.get("outcome"))
                    remember(validation.get("evidence"))
                if isinstance(qualitative, dict):
                    count("asset_qualitative_outcomes", qualitative.get("outcome"))
                    remember(qualitative.get("evidence"))
        final = run.get("ai_final_assessment", {})
        if not isinstance(final, dict):
            continue
        count("ai_availability", final.get("availability"))
        result = final.get("result")
        if not isinstance(result, dict):
            continue
        count("ai_verdicts", result.get("verdict"))
        score = optional_number(result.get("quality_score"))
        final_confidence = optional_number(result.get("confidence"))
        if score is not None:
            quality_scores.append(score)
        if final_confidence is not None:
            confidence.append(final_confidence)
        remember(result.get("evidence"))

    summary["evidence_reference_count"] = len(evidence)
    summary["median_quality_score"] = _median(quality_scores)
    summary["median_confidence"] = _median(confidence)
    return summary


def _assessment_profile_sha256(
    scenario_version: int,
    runs: list[dict[str, Any]],
) -> str:
    definitions: set[str] = set()
    for run in runs:
        assessments = run.get("assessments", [])
        assets = run.get("assets", [])
        if isinstance(assessments, list):
            for assessment in assessments:
                if not isinstance(assessment, dict):
                    continue
                definitions.add(
                    json.dumps(
                        _pick(
                            assessment,
                            (
                                "criterion_id",
                                "target",
                                "kind",
                                "policy",
                                "dimension",
                                "source",
                            ),
                        ),
                        ensure_ascii=False,
                        separators=(",", ":"),
                        sort_keys=True,
                    )
                )
        if isinstance(assets, list):
            for asset in assets:
                if not isinstance(asset, dict):
                    continue
                validation = asset.get("validation", {})
                qualitative = asset.get("qualitative_assessment", {})
                if not isinstance(validation, dict) or not isinstance(qualitative, dict):
                    continue
                definition = {
                    "asset_id": validation.get("asset_id"),
                    **_pick(
                        qualitative,
                        (
                            "criterion_id",
                            "target",
                            "kind",
                            "policy",
                            "dimension",
                            "source",
                        ),
                    ),
                }
                definitions.add(
                    json.dumps(
                        definition,
                        ensure_ascii=False,
                        separators=(",", ":"),
                        sort_keys=True,
                    )
                )
    return _sha256_json(
        {"scenario_version": scenario_version, "assessments": sorted(definitions)}
    )


def _analyzer_profile_sha256(runs: list[dict[str, Any]]) -> str:
    analyzers: set[str] = set()

    def remember(value: Any) -> None:
        if not isinstance(value, dict):
            return
        analyzers.add(
            json.dumps(
                _pick(value, ("analyzer", "provider", "model")),
                ensure_ascii=False,
                separators=(",", ":"),
                sort_keys=True,
            )
        )

    for run in runs:
        assessments = run.get("assessments", [])
        assets = run.get("assets", [])
        if isinstance(assessments, list):
            for assessment in assessments:
                if isinstance(assessment, dict):
                    remember(assessment.get("analyzer"))
        if isinstance(assets, list):
            for asset in assets:
                if not isinstance(asset, dict):
                    continue
                qualitative = asset.get("qualitative_assessment", {})
                if isinstance(qualitative, dict):
                    remember(qualitative.get("analyzer"))
        final = run.get("ai_final_assessment", {})
        if isinstance(final, dict):
            remember(final.get("analyzer"))
    return _sha256_json({"analyzers": sorted(analyzers)})


def _median(values: list[float]) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    midpoint = len(ordered) // 2
    if len(ordered) % 2:
        return ordered[midpoint]
    return (ordered[midpoint - 1] + ordered[midpoint]) / 2


def _evaluated_version(system: Any) -> tuple[str, str, str] | None:
    if not isinstance(system, dict) or not isinstance(system.get("stack"), dict):
        return None
    stack = system["stack"]
    mode = str(stack.get("mode") or "")
    identity = {
        "stack": stack,
        "engine_version": system.get("engine_version"),
        "engine_revision": system.get("engine_revision"),
        "harness_version": system.get("harness_version"),
        "contract_hashes": system.get("contract_hashes", {}),
    }
    if mode == "source":
        revision = str(stack.get("workers_revision") or "")
        label = f"Source {revision[:12]}" if revision else "Source revision"
    elif mode == "registry":
        versions = stack.get("stack_versions", {})
        labels = (
            [f"{worker}@{version}" for worker, version in sorted(versions.items())[:2]]
            if isinstance(versions, dict)
            else []
        )
        digest = str(stack.get("stack_lock_digest") or "")
        label = " · ".join(labels) or f"Registry {digest[:12]}"
    else:
        return None
    return _sha256_json(identity), mode, label


def _scenario_status(scenario: dict[str, Any]) -> str:
    aggregate = scenario.get("aggregate", {})
    if not isinstance(aggregate, dict):
        aggregate = {}
    if int(optional_number(aggregate.get("technical_failures")) or 0) > 0:
        return "technical_failed"
    if int(optional_number(aggregate.get("hard_gate_failures")) or 0) > 0:
        return "hard_gate_failed"
    return "passed" if scenario.get("passed") else "infra_failed"


def _static_run_metrics(run: dict[str, Any]) -> dict[str, Any]:
    status = str(run.get("status") or "infrastructure_error")
    assessment = run.get("assessment")
    if not isinstance(assessment, dict):
        assessment = _unavailable_run_assessment(run)
    return {
        "score": optional_number(run.get("score")),
        "cost_usd": run_metric(run, "cost_usd"),
        "tokens": run_metric(run, "tokens"),
        "duration_seconds": run_metric(run, "duration_seconds"),
        "context_compactions": run_metric(run, "context_compactions"),
        "status": status,
        "assessment": assessment,
    }


def _side_summary(
    evaluated_version_id: str,
    observations: list[dict[str, Any]],
) -> dict[str, Any]:
    runs = [run for observation in observations for run in observation["runs"]]
    scores = [value for run in runs if (value := run["score"]) is not None]
    costs = [value for run in runs if (value := run["cost_usd"]) is not None]
    tokens = [value for run in runs if (value := run["tokens"]) is not None]
    durations = [
        value for run in runs if (value := run["duration_seconds"]) is not None
    ]
    context_compactions = [
        value for run in runs if (value := run["context_compactions"]) is not None
    ]
    outcomes = {
        "passed": sum(run["status"] == "passed" for run in runs),
        "hard_gate_failed": sum(
            run["status"] == "hard_gate_failed" for run in runs
        ),
        "technical_failed": sum(
            run["status"] in {"subject_error", "judge_error", "resource_limit"}
            for run in runs
        ),
        "infra_failed": sum(
            run["status"] == "infrastructure_error" for run in runs
        ),
    }
    return {
        "evaluated_version_id": evaluated_version_id,
        "execution_count": len({item["execution_id"] for item in observations}),
        "total_runs": len(runs),
        "scored_runs": len(scores),
        "case_count": len({item["case_id"] for item in observations}),
        "median_score": _median(scores),
        "pass_rate": outcomes["passed"] / len(runs) if runs else None,
        "median_cost_usd": _median(costs),
        "median_tokens": _median(tokens),
        "median_duration_seconds": _median(durations),
        "median_context_compactions": _median(context_compactions),
        "outcomes": outcomes,
        "samples": {
            "score": len(scores),
            "cost_usd": len(costs),
            "tokens": len(tokens),
            "duration_seconds": len(durations),
            "context_compactions": len(context_compactions),
        },
        "assessment_summary": _assessment_summary(
            [run["assessment"] for run in runs]
        ),
    }


def build_static_test_catalog(
    site_dir: Path,
    executions: list[dict[str, Any]],
) -> dict[str, Any]:
    """Build a compact catalog plus lazy per-test evidence shards for Pages."""
    cohorts: dict[str, dict[str, Any]] = {}
    versions: dict[tuple[str, str], dict[str, Any]] = {}
    version_execution_ids: dict[tuple[str, str], set[str]] = {}
    observations: dict[tuple[str, int], list[dict[str, Any]]] = {}

    for execution in executions:
        detail_path = execution.get("detail_path")
        if not isinstance(detail_path, str) or not detail_path.startswith("runs/"):
            continue
        detail = load_json(site_dir / detail_path)
        if detail is None:
            continue
        reports = detail.get("reports", [])
        if not isinstance(reports, list):
            continue
        for report_entry in reports:
            if not isinstance(report_entry, dict) or not report_entry.get("available"):
                continue
            report = report_entry.get("report")
            if not isinstance(report, dict):
                continue
            subject = report.get("subject", {})
            judge = report.get("judge", {})
            if not isinstance(subject, dict):
                continue
            if not isinstance(judge, dict):
                judge = {}
            report_execution = report.get("execution")
            lane = str(
                (
                    report_execution.get("lane")
                    if isinstance(report_execution, dict)
                    else None
                )
                or detail.get("lane")
                or execution.get("lane")
                or "daily"
            )
            cohort_value = {
                "lane": lane,
                "subject_provider": str(subject.get("provider") or ""),
                "subject_model": str(subject.get("model") or ""),
                "judge_provider": str(judge.get("provider") or "") or None,
                "judge_model": str(judge.get("model") or "") or None,
                "judge_protocol": str(report.get("judge_protocol") or "") or None,
            }
            cohort_id = _sha256_json(cohort_value)
            cohorts.setdefault(cohort_id, {"id": cohort_id, **cohort_value})
            evaluated = _evaluated_version(report.get("system_under_test"))
            if evaluated is None:
                continue
            evaluated_id, stack_mode, label = evaluated
            version_key = (cohort_id, evaluated_id)
            completed_at = str(execution.get("completed_at") or "")
            descriptor = versions.setdefault(
                version_key,
                {
                    "id": evaluated_id,
                    "cohort_id": cohort_id,
                    "label": label,
                    "stack_mode": stack_mode,
                    "completed_at": completed_at,
                    "execution_count": 0,
                },
            )
            descriptor["completed_at"] = max(
                str(descriptor["completed_at"]), completed_at
            )
            version_execution_ids.setdefault(version_key, set()).add(
                str(execution.get("id") or "")
            )
            scenarios = report.get("scenarios", [])
            if not isinstance(scenarios, list):
                continue
            for scenario in scenarios:
                if not isinstance(scenario, dict):
                    continue
                test_id = str(
                    scenario.get("scenario_id")
                    or report_entry.get("scenario_id")
                    or ""
                )
                if not test_id:
                    continue
                test_version = int(optional_number(scenario.get("scenario_version")) or 1)
                case_id = str(scenario.get("case_id") or f"{test_id}:v{test_version}")
                contract_sha256 = _sha256_json(
                    {
                        "scenario_id": test_id,
                        "scenario_version": test_version,
                        "case": scenario.get("case"),
                        "execution_policy": scenario.get("execution_policy", {}),
                    }
                )
                raw_runs = scenario.get("runs", [])
                runs = [
                    _static_run_metrics(run)
                    for run in raw_runs
                    if isinstance(run, dict)
                ] if isinstance(raw_runs, list) else []
                assessment_runs = [run["assessment"] for run in runs]
                observations.setdefault((test_id, test_version), []).append(
                    {
                        "execution_id": str(execution.get("id") or ""),
                        "evaluated_version_id": evaluated_id,
                        "cohort_id": cohort_id,
                        "completed_at": completed_at,
                        "case_id": case_id,
                        "contract_sha256": contract_sha256,
                        "assessment_profile_sha256": _assessment_profile_sha256(
                            test_version, assessment_runs
                        ),
                        "analyzer_profile_sha256": _analyzer_profile_sha256(
                            assessment_runs
                        ),
                        "status": _scenario_status(scenario),
                        "runs": runs,
                    }
                )

    for key, descriptor in versions.items():
        descriptor["execution_count"] = len(version_execution_ids.get(key, set()))
    revision = _sha256_json(
        [
            {
                "id": execution.get("id"),
                "completed_at": execution.get("completed_at"),
                "status": execution.get("status"),
            }
            for execution in executions
        ]
    )
    tests_dir = site_dir / "tests"
    data_dir = tests_dir / "data"
    data_dir.mkdir(parents=True, exist_ok=True)
    rows_by_test: dict[str, dict[str, Any]] = {}
    retained_shards: set[str] = set()
    for (test_id, test_version), test_observations in sorted(observations.items()):
        shard_digest = _sha256_json([test_id, test_version]).removeprefix("sha256:")
        shard_name = f"{shard_digest}.json"
        shard_path = f"./tests/data/{shard_name}"
        retained_shards.add(shard_name)
        public_observations = []
        for observation in test_observations:
            scores = [
                run["score"]
                for run in observation["runs"]
                if run["score"] is not None
            ]
            context_compactions = [
                run["context_compactions"]
                for run in observation["runs"]
                if run["context_compactions"] is not None
            ]
            public_observations.append(
                {
                    key: observation[key]
                    for key in (
                        "execution_id",
                        "evaluated_version_id",
                        "cohort_id",
                        "completed_at",
                        "case_id",
                        "contract_sha256",
                        "assessment_profile_sha256",
                        "analyzer_profile_sha256",
                        "status",
                    )
                }
                | {
                    "median_score": _median(scores),
                    "median_context_compactions": _median(context_compactions),
                    "run_count": len(observation["runs"]),
                    "scored_runs": len(scores),
                    "assessment_summary": _assessment_summary(
                        [run["assessment"] for run in observation["runs"]]
                    ),
                }
            )
        (data_dir / shard_name).write_text(
            json.dumps(
                {
                    "test_id": test_id,
                    "test_version": test_version,
                    "observations": public_observations,
                },
                indent=2,
                sort_keys=True,
            )
            + "\n"
        )
        row = rows_by_test.setdefault(
            test_id,
            {
                "test_id": test_id,
                "lifecycle": "active",
                "current_version": test_version,
                "available_versions": [],
                "selected_version": test_version,
                "result": None,
                "version_results": {},
                "shards": {},
            },
        )
        row["current_version"] = max(row["current_version"], test_version)
        row["selected_version"] = row["current_version"]
        row["shards"][str(test_version)] = shard_path
        row["available_versions"].append(
            {
                "version": test_version,
                "execution_count": len(
                    {item["execution_id"] for item in test_observations}
                ),
                "run_count": sum(len(item["runs"]) for item in test_observations),
                "last_seen": max(
                    (item["completed_at"] for item in test_observations),
                    default=None,
                ),
            }
        )
        sides: dict[str, Any] = {}
        grouped_sides: dict[tuple[str, str], list[dict[str, Any]]] = {}
        for observation in test_observations:
            grouped_sides.setdefault(
                (observation["cohort_id"], observation["evaluated_version_id"]),
                [],
            ).append(observation)
        for (cohort_id, evaluated_id), side_observations in grouped_sides.items():
            contracts: dict[str, str | None] = {}
            assessment_profiles: dict[str, str | None] = {}
            analyzer_profiles: dict[str, str | None] = {}
            for observation in side_observations:
                case_id = observation["case_id"]
                contract = observation["contract_sha256"]
                contracts[case_id] = (
                    None
                    if case_id in contracts and contracts[case_id] != contract
                    else contract
                )
                assessment_profile = observation["assessment_profile_sha256"]
                assessment_profiles[case_id] = (
                    None
                    if case_id in assessment_profiles
                    and assessment_profiles[case_id] != assessment_profile
                    else assessment_profile
                )
                analyzer_profile = observation["analyzer_profile_sha256"]
                analyzer_profiles[case_id] = (
                    None
                    if case_id in analyzer_profiles
                    and analyzer_profiles[case_id] != analyzer_profile
                    else analyzer_profile
                )
            sides[f"{cohort_id}::{evaluated_id}"] = {
                "summary": _side_summary(evaluated_id, side_observations),
                "contracts": dict(sorted(contracts.items())),
                "assessment_profiles": dict(sorted(assessment_profiles.items())),
                "analyzer_profiles": dict(sorted(analyzer_profiles.items())),
            }
        row["version_results"][str(test_version)] = {"sides": sides}

    for candidate in data_dir.glob("*.json"):
        if candidate.name not in retained_shards:
            candidate.unlink()
    rows = sorted(rows_by_test.values(), key=lambda row: row["test_id"])
    for row in rows:
        row["available_versions"].sort(
            key=lambda descriptor: descriptor["version"], reverse=True
        )
    evaluated_versions = sorted(
        versions.values(),
        key=lambda descriptor: (descriptor["completed_at"], descriptor["id"]),
        reverse=True,
    )
    catalog = {
        "evaluated_versions": {
            "revision": revision,
            "cohorts": sorted(cohorts.values(), key=lambda cohort: cohort["id"]),
            "versions": evaluated_versions,
        },
        "tests": {
            "revision": revision,
            "rows": rows,
            "total": len(rows),
            "next_cursor": None,
        },
    }
    (tests_dir / "index.json").write_text(
        json.dumps(catalog, indent=2, sort_keys=True, ensure_ascii=False) + "\n"
    )
    return catalog


def _metadata_from_summary(execution: dict[str, Any]) -> dict[str, Any]:
    source = execution.get("source", {})
    if not isinstance(source, dict):
        source = {}
    return {
        "id": str(execution.get("id") or ""),
        "run_id": str(execution.get("run_id") or ""),
        "attempt": int(optional_number(execution.get("attempt")) or 1),
        "workflow_name": str(execution.get("workflow_name") or ""),
        "workflow_url": str(execution.get("workflow_url") or ""),
        "event": str(execution.get("event") or ""),
        "actor": str(execution.get("actor") or ""),
        "started_at": str(execution.get("started_at") or ""),
        "completed_at": str(execution.get("completed_at") or ""),
        "conclusion": str(execution.get("conclusion") or ""),
        "head_sha": str(source.get("sha") or ""),
        "head_branch": str(source.get("ref") or ""),
        "repository": str(source.get("repository") or ""),
    }


def migrate_retained_details(
    site_dir: Path,
    manifest: dict[str, Any],
) -> None:
    """Migrate retained reports while preserving their complete detail fields."""
    runs_dir = site_dir / "runs"
    for execution in manifest.get("executions", []):
        if not isinstance(execution, dict):
            continue
        relative_path = execution.get("detail_path")
        if not isinstance(relative_path, str) or not relative_path.startswith("runs/"):
            continue
        candidate = site_dir / relative_path
        if candidate.parent != runs_dir or not candidate.is_file():
            continue
        retained = load_json(candidate)
        if retained is None:
            continue
        public_detail = complete_public_detail(
            retained,
            _metadata_from_summary(execution),
        )
        write_json_atomic(candidate, public_detail)


def publish(
    site_dir: Path,
    *,
    snapshot_path: Path | None,
    detail_path: Path | None,
    jobs_path: Path | None = None,
    metadata: dict[str, Any],
    repo_url: str,
    max_summaries: int,
    max_details: int,
) -> dict[str, Any]:
    if max_summaries < 1 or max_details < 0 or max_details > max_summaries:
        raise PublishError("retention must satisfy 0 <= details <= summaries")
    site_dir.mkdir(parents=True, exist_ok=True)
    runs_dir = site_dir / "runs"
    runs_dir.mkdir(parents=True, exist_ok=True)
    manifest_path = site_dir / MANIFEST_FILENAME
    legacy_manifest_path = site_dir / "executions.js"
    manifest = load_manifest(manifest_path)
    migrate_retained_details(site_dir, manifest)
    raw_snapshot = load_json(snapshot_path)
    raw_detail = load_json(detail_path)
    snapshot, snapshot_identity_failure = validate_artifact_identity(
        raw_snapshot,
        metadata,
        label="benchmark snapshot",
    )
    detail, detail_identity_failure = validate_artifact_identity(
        raw_detail,
        metadata,
        label="execution detail",
    )
    summary_snapshot = snapshot or detail
    artifact_failure = (
        snapshot_identity_failure or detail_identity_failure
        if summary_snapshot is None
        else None
    )
    jobs_document = load_json(jobs_path)
    summary = build_summary(
        summary_snapshot,
        metadata,
        detail=detail,
        jobs_document=jobs_document,
        artifact_failure=artifact_failure,
    )

    existing_by_id = {
        str(entry.get("id")): entry
        for entry in manifest.get("executions", [])
        if isinstance(entry, dict) and entry.get("id")
    }
    previous = existing_by_id.get(metadata["id"])
    if (
        snapshot is None
        and previous
        and previous.get("availability") in {"aggregate", "full"}
    ):
        preserved = dict(previous)
        preserved.update(
            {
                key: summary[key]
                for key in (
                    "workflow_name",
                    "workflow_url",
                    "event",
                    "actor",
                    "started_at",
                    "completed_at",
                    "conclusion",
                    "workflow_duration_seconds",
                )
            }
        )
        summary = preserved

    if detail is not None:
        scenario_metrics = build_scenario_metrics(detail)
        efficiency_totals = build_execution_efficiency_totals(detail)
        public_detail = complete_public_detail(detail, metadata)
        relative_detail_path = f"runs/{metadata['id']}.json"
        write_json_atomic(site_dir / relative_detail_path, public_detail)
        summary["availability"] = "full"
        summary["detail_path"] = relative_detail_path
        summary["scenario_metrics"] = scenario_metrics
        summary["totals"].update(efficiency_totals)

    existing_by_id[metadata["id"]] = summary
    executions = sorted(existing_by_id.values(), key=sort_key, reverse=True)
    dropped = executions[max_summaries:]
    executions = executions[:max_summaries]

    for entry in dropped:
        stored_path = entry.get("detail_path")
        if isinstance(stored_path, str) and stored_path.startswith("runs/"):
            candidate = site_dir / stored_path
            if candidate.parent == runs_dir and candidate.is_file():
                candidate.unlink()

    for index, entry in enumerate(executions):
        if index < max_details:
            continue
        stored_path = entry.get("detail_path")
        if isinstance(stored_path, str) and stored_path.startswith("runs/"):
            candidate = site_dir / stored_path
            if candidate.parent == runs_dir and candidate.is_file():
                candidate.unlink()
        entry["detail_path"] = None
        entry["availability"] = (
            "aggregate" if entry.get("subjects") else "unavailable"
        )

    retained_paths = {
        str(entry.get("detail_path"))
        for entry in executions
        if isinstance(entry.get("detail_path"), str)
        and str(entry["detail_path"]).startswith("runs/")
    }
    for candidate in runs_dir.glob("*.json"):
        if f"runs/{candidate.name}" not in retained_paths:
            candidate.unlink()

    build_static_test_catalog(site_dir, executions)

    updated = {
        "mode": "published",
        "last_update": metadata["completed_at"] or metadata["started_at"],
        "repo_url": repo_url,
        "retention": {
            "summaries": max_summaries,
            "details": max_details,
        },
        "executions": executions,
    }
    write_json_atomic(manifest_path, updated)
    legacy_manifest_path.unlink(missing_ok=True)
    return updated


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    parser.add_argument("--site-dir", type=Path, required=True)
    parser.add_argument("--snapshot", type=Path)
    parser.add_argument("--detail", type=Path)
    parser.add_argument("--jobs", type=Path)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--attempt", type=int, required=True)
    parser.add_argument("--workflow-name", required=True)
    parser.add_argument("--workflow-url", required=True)
    parser.add_argument("--event", required=True)
    parser.add_argument("--actor", default="")
    parser.add_argument("--started-at", default="")
    parser.add_argument("--completed-at", default="")
    parser.add_argument("--conclusion", required=True)
    parser.add_argument("--head-sha", default="")
    parser.add_argument("--head-branch", default="")
    parser.add_argument("--repository", required=True)
    parser.add_argument("--repo-url", required=True)
    parser.add_argument("--max-summaries", type=int, default=100)
    parser.add_argument("--max-details", type=int, default=30)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.attempt < 1:
        raise PublishError("attempt must be positive")
    metadata = {
        "id": f"{args.run_id}-{args.attempt}",
        "run_id": args.run_id,
        "attempt": args.attempt,
        "workflow_name": args.workflow_name,
        "workflow_url": args.workflow_url,
        "event": args.event,
        "actor": args.actor,
        "started_at": args.started_at,
        "completed_at": args.completed_at,
        "conclusion": args.conclusion,
        "head_sha": args.head_sha,
        "head_branch": args.head_branch,
        "repository": args.repository,
    }
    updated = publish(
        args.site_dir,
        snapshot_path=args.snapshot,
        detail_path=args.detail,
        jobs_path=args.jobs,
        metadata=metadata,
        repo_url=args.repo_url,
        max_summaries=args.max_summaries,
        max_details=args.max_details,
    )
    print(
        json.dumps(
            {
                "execution_id": metadata["id"],
                "summaries": len(updated["executions"]),
                "full_details": sum(
                    entry.get("availability") == "full"
                    for entry in updated["executions"]
                ),
            }
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
