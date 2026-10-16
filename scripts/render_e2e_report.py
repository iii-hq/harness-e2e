#!/usr/bin/env python3
"""Render canonical Harness E2E artifacts as a small Markdown report."""

from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import html
import json
import re
from pathlib import Path
from typing import Any

from report_execution import collect_runs, obj, read_json


SUMMARY_LIMIT = 60_000
REPEATED_DISCOVERY_RULE = "repeated_contract_discovery"
REPEATED_TOOL_ERROR_RULE = "repeated_tool_error"


def text(value: Any, limit: int | None = None) -> str:
    """Make untrusted result text inert in GitHub-flavored Markdown."""
    if value is None:
        rendered = "unknown"
    elif isinstance(value, (dict, list)):
        rendered = json.dumps(value, ensure_ascii=False, sort_keys=True)
    else:
        rendered = str(value)
    rendered = re.sub(r"\s+", " ", rendered).strip().replace("@", "@\u200b")
    if limit and len(rendered) > limit:
        rendered = rendered[: limit - 1] + "…"
    rendered = html.escape(rendered, quote=True)
    return re.sub(r"([\\`*{}\[\]()#+.!|>_~-])", r"\\\1", rendered)


def value(mapping: dict[str, Any], key: str) -> str:
    return text(mapping.get(key))


def cost(attempt: dict[str, Any]) -> str:
    report = obj(attempt.get("cost"))
    return text(report.get("total_usd"))


def incomplete_criteria(run: dict[str, Any]) -> list[dict[str, Any]]:
    criteria = []
    for criterion in run.get("criteria") or []:
        criterion = obj(criterion)
        awarded, possible = criterion.get("awarded"), criterion.get("possible")
        if awarded is None or not isinstance(awarded, (int, float)) or not isinstance(possible, (int, float)) or awarded < possible:
            criteria.append(criterion)
    return criteria


def repeated_contract_discovery(attempt: dict[str, Any]) -> list[dict[str, Any]]:
    findings = []
    origins = {}
    notices = []
    calls = []
    call_ids = []
    messages = obj(attempt.get("transcript")).get("messages") or []
    for index, raw_entry in enumerate(messages):
        entry = obj(raw_entry)
        custom = obj(entry.get("custom"))
        if custom.get("custom_type") == "model_notice" and obj(custom.get("data")).get("kind") == "registry-changed":
            notices.append({"transcript_index": index, "entry_id": entry.get("entry_id")})
            continue
        message = obj(entry.get("message"))
        if message.get("role") == "assistant":
            for block in message.get("content") or []:
                block = obj(block)
                if block.get("type") == "function_call" and isinstance(block.get("id"), str):
                    call_ids.append(block["id"])
                arguments = obj(block.get("arguments"))
                target = (
                    arguments.get("function")
                    if block.get("function_id") == "agent_trigger"
                    else block.get("function_id")
                )
                if (
                    block.get("type") == "function_call"
                    and isinstance(block.get("id"), str)
                    and target == "engine::functions::info"
                ):
                    calls.append({
                        "transcript_index": index,
                        "entry_id": entry.get("entry_id"),
                        "function_call_id": block["id"],
                    })
    duplicate_call_ids = {
        call_id
        for call_id, count in Counter(call_ids).items()
        if count > 1
    }
    calls = {
        call["function_call_id"]: call
        for call in calls
        if call["function_call_id"] not in duplicate_call_ids
    }
    info_result_counts = Counter(
        message.get("function_call_id")
        for entry in messages
        for message in [obj(obj(entry).get("message"))]
        if message.get("role") == "function_result"
        and message.get("function_id") == "engine::functions::info"
        and isinstance(message.get("function_call_id"), str)
    )

    for index, raw_entry in enumerate(messages):
        entry = obj(raw_entry)
        message = obj(entry.get("message"))
        if (
            message.get("role") != "function_result"
            or message.get("function_id") != "engine::functions::info"
            or message.get("is_error") is not False
        ):
            continue
        for block in message.get("content") or []:
            block = obj(block)
            if block.get("type") != "text" or not isinstance(block.get("text"), str):
                continue
            try:
                payload = obj(json.loads(block["text"]))
            except (TypeError, ValueError):
                continue
            functions = payload.get("functions")
            if not isinstance(functions, list) and isinstance(payload.get("function_id"), str):
                functions = [payload]
            if not isinstance(functions, list) or not functions:
                continue
            call_id = message.get("function_call_id")
            if info_result_counts[call_id] != 1:
                continue
            matched_call = calls.get(call_id)
            if not matched_call or matched_call["transcript_index"] >= index:
                continue
            if not all(obj(item).get("contract_status") == "unchanged_in_context" for item in functions):
                origins[call_id] = {
                    "transcript_index": index,
                    "entry_id": entry.get("entry_id"),
                    "function_ids": {
                        obj(item).get("function_id")
                        for item in functions
                        if obj(item).get("contract_status") != "unchanged_in_context"
                        and isinstance(obj(item).get("function_id"), str)
                    },
                }
                continue

            observed = []
            repeated_call = matched_call
            for item in functions:
                function = obj(item)
                source_call_id = function.get("source_function_call_id")
                origin = origins.get(source_call_id)
                if not origin or function.get("function_id") not in origin["function_ids"]:
                    observed = []
                    break
                notice = next(
                    (
                        candidate
                        for candidate in reversed(notices)
                        if repeated_call
                        and origin["transcript_index"]
                        < candidate["transcript_index"]
                        < repeated_call["transcript_index"]
                    ),
                    None,
                )
                observed.append({
                    "function_id": function.get("function_id"),
                    "source_function_call_id": source_call_id,
                    "source_result": {
                        "transcript_index": origin["transcript_index"],
                        "entry_id": origin.get("entry_id"),
                    },
                    "registry_changed_notice": notice,
                })
            if observed:
                findings.append({
                    "rule_id": REPEATED_DISCOVERY_RULE,
                    "cause": (
                        "harness_notice_correlated"
                        if all(item["registry_changed_notice"] for item in observed)
                        else "unknown"
                    ),
                    "repeated_result": {
                        "transcript_index": index,
                        "entry_id": entry.get("entry_id"),
                        "function_call_id": call_id,
                    },
                    "repeated_call": repeated_call,
                    "functions": observed,
                })
    return findings


def repeated_tool_error(attempt: dict[str, Any]) -> list[dict[str, Any]]:
    calls = []
    results = {}
    duplicate_results = set()
    result_indices = []
    messages = obj(attempt.get("transcript")).get("messages") or []
    for index, raw_entry in enumerate(messages):
        entry = obj(raw_entry)
        message = obj(entry.get("message"))
        if message.get("role") == "assistant":
            for block in message.get("content") or []:
                block = obj(block)
                if block.get("type") != "function_call":
                    continue
                function_id = block.get("function_id")
                arguments = block.get("arguments") if "arguments" in block else {}
                if function_id == "agent_trigger":
                    trigger = obj(arguments)
                    function_id = trigger.get("function")
                    arguments = trigger.get("payload") if "payload" in trigger else {}
                if not isinstance(function_id, str):
                    continue
                calls.append({
                    "call_id": block.get("id") if isinstance(block.get("id"), str) else None,
                    "function_id": function_id,
                    "arguments": arguments,
                    "arguments_key": json.dumps(
                        arguments, ensure_ascii=False, sort_keys=True, separators=(",", ":")
                    ),
                    "transcript_index": index,
                    "entry_id": entry.get("entry_id"),
                })
        if message.get("role") != "function_result":
            continue
        result_indices.append(index)
        key = (message.get("function_call_id"), message.get("function_id"))
        if not all(isinstance(item, str) for item in key):
            continue
        if key in results:
            duplicate_results.add(key)
            continue
        details = obj(message.get("details"))
        error = obj(details.get("error"))
        error_code = message.get("error_code")
        if error_code is None:
            error_code = details.get("code")
        if error_code is None:
            error_code = error.get("code")
        results[key] = {
            "is_error": message.get("is_error"),
            "error_code": error_code if isinstance(error_code, str) else None,
            "transcript_index": index,
            "entry_id": entry.get("entry_id"),
        }

    duplicate_call_ids = {
        call_id
        for call_id, count in Counter(call["call_id"] for call in calls if call["call_id"] is not None).items()
        if count > 1
    }
    findings = []
    for first, second in zip(calls, calls[1:]):
        if (
            first["call_id"] is None
            or second["call_id"] is None
            or first["call_id"] in duplicate_call_ids
            or second["call_id"] in duplicate_call_ids
            or first["function_id"] != second["function_id"]
            or first["arguments_key"] != second["arguments_key"]
        ):
            continue
        function_id = first["function_id"]
        if function_id.startswith("engine::functions::") or function_id == "directory::engine::functions::info":
            continue
        outcomes = [results.get((call["call_id"], function_id)) for call in (first, second)]
        if any(
            outcome is None or (call["call_id"], function_id) in duplicate_results
            for call, outcome in zip((first, second), outcomes)
        ):
            continue
        error_code = outcomes[0]["error_code"]
        if (
            not (
                first["transcript_index"]
                < outcomes[0]["transcript_index"]
                < second["transcript_index"]
                < outcomes[1]["transcript_index"]
            )
            or outcomes[0]["is_error"] is not True
            or outcomes[1]["is_error"] is not True
            or not error_code
            or outcomes[1]["error_code"] != error_code
            or any(
                first["transcript_index"] < index < outcomes[1]["transcript_index"]
                and index not in (outcomes[0]["transcript_index"], outcomes[1]["transcript_index"])
                for index in result_indices
            )
        ):
            continue
        namespace = obj(first["arguments"]).get("namespace")
        if function_id == "engine::triggers::info" and error_code == "NOT_FOUND" and namespace in (None, "default"):
            continue
        findings.append({
            "rule_id": REPEATED_TOOL_ERROR_RULE,
            "cause": "unknown",
            "function_id": function_id,
            "error_code": error_code,
            "calls": [
                {
                    "function_call_id": call["call_id"],
                    "call": {
                        "transcript_index": call["transcript_index"],
                        "entry_id": call["entry_id"],
                    },
                    "result": {
                        "transcript_index": outcome["transcript_index"],
                        "entry_id": outcome["entry_id"],
                    },
                }
                for call, outcome in zip((first, second), outcomes)
            ],
        })
    return findings


def needs_page(run: dict[str, Any]) -> bool:
    return bool(
        run.get("failures")
        or run.get("retry_attempts")
        or incomplete_criteria(run)
        or repeated_contract_discovery(run)
        or repeated_tool_error(run)
        or run.get("status") != "passed"
    )


def attempt_section(attempt: dict[str, Any], label: str) -> list[str]:
    lines = [
        f"## {label}",
        "",
        f"- Session: {value(attempt, 'session_id')}",
        f"- Wall time ms: {value(attempt, 'wall_time_ms')}",
        f"- Function calls: {value(obj(attempt.get('efficiency')), 'function_calls')}",
        f"- Total tokens: {value(obj(attempt.get('efficiency')), 'total_tokens')}",
        "",
        "| Attempt | Technical | Completion | Score | Cost USD | Status |",
        "| --- | --- | --- | ---: | ---: | --- |",
        f"| {value(attempt, 'attempt_number')} / {value(attempt, 'attempt_id')} | {value(attempt, 'technical')} | {value(attempt, 'completion')} | {value(attempt, 'score')} | {cost(attempt)} | {value(attempt, 'status')} |",
        "",
    ]
    failures = [obj(item) for item in attempt.get("failures") or [] if isinstance(item, dict)]
    if failures:
        lines.extend(["### Failures", ""])
        for failure in failures:
            lines.append(
                "- "
                f"phase={value(failure, 'phase')}; domain={value(failure, 'domain')}; "
                f"code={value(failure, 'code')}; message={value(failure, 'message')}"
            )
        lines.append("")

    criteria = incomplete_criteria(attempt)
    if criteria:
        lines.extend(["### Criteria with missing points", ""])
        for criterion in criteria:
            awarded = "not evaluated" if criterion.get("awarded") is None else text(criterion.get("awarded"))
            lines.append(
                f"- {value(criterion, 'id')}: {awarded}/{value(criterion, 'possible')} — "
                f"{value(criterion, 'reason')}"
            )
        lines.append("")

    repeated = repeated_contract_discovery(attempt)
    if repeated:
        lines.extend([
            "### Repeated contract discovery",
            "",
            "The function result reported contracts already present in context.",
            "",
        ])
        for finding in repeated:
            for function in finding["functions"]:
                source = obj(function.get("source_result"))
                notice = obj(function.get("registry_changed_notice"))
                lines.append(
                    f"- result_index={text(obj(finding.get('repeated_result')).get('transcript_index'))}; "
                    f"result_entry_id={text(obj(finding.get('repeated_result')).get('entry_id'))}; "
                    f"function_call_id={text(obj(finding.get('repeated_result')).get('function_call_id'))}; "
                    f"function_id={value(function, 'function_id')}; "
                    f"source_function_call_id={value(function, 'source_function_call_id')}; "
                    f"source_index={text(source.get('transcript_index'))}; "
                    f"source_entry_id={text(source.get('entry_id'))}; "
                    f"registry_changed_notice="
                    f"{text(notice.get('entry_id') if notice else None)}"
                )
        lines.extend([
            "",
            "A registry-changed notice is reported only when it was observed between the source result and the repeated function call; unknown means no such evidence was present in that interval.",
        ])
        lines.append("")

    repeated_errors = repeated_tool_error(attempt)
    if repeated_errors:
        lines.extend([
            "### Repeated tool error",
            "",
            "Adjacent equivalent calls returned the same explicit error code. This is advisory evidence; cause is unknown.",
            "",
        ])
        for finding in repeated_errors:
            calls = finding["calls"]
            lines.append(
                f"- function_id={value(finding, 'function_id')}; error_code={value(finding, 'error_code')}; "
                f"first_call_id={value(calls[0], 'function_call_id')}; "
                f"first_call_index={text(obj(calls[0].get('call')).get('transcript_index'))}; "
                f"first_result_index={text(obj(calls[0].get('result')).get('transcript_index'))}; "
                f"second_call_id={value(calls[1], 'function_call_id')}; "
                f"second_call_index={text(obj(calls[1].get('call')).get('transcript_index'))}; "
                f"second_result_index={text(obj(calls[1].get('result')).get('transcript_index'))}"
            )
        lines.append("")

    evidence = [obj(item) for item in attempt.get("evidence") or [] if isinstance(item, dict)]
    for deliverable in attempt.get("deliverables") or []:
        artifact = obj(obj(deliverable).get("artifact"))
        if artifact:
            evidence.append(artifact)
    if evidence:
        lines.extend(["### Evidence references", ""])
        for reference in evidence:
            lines.append(
                f"- id={value(reference, 'id')}; kind={value(reference, 'kind')}; "
                f"path={value(reference, 'path')}; sha256={value(reference, 'sha256')}"
            )
        lines.append("")
    return lines


def render_run(entry: dict[str, Any], source: str) -> str:
    run = obj(entry.get("run"))
    lines = [
        f"# E2E run: {text(entry.get('scenario_id'))}",
        "",
        f"- Scenario: {text(entry.get('scenario_id'))}",
        f"- Run: {value(run, 'run_id')}",
        f"- Repetition: {text(entry.get('repetition'))}",
        f"- Canonical source: {text('results.json' if source == 'results' else 'journal run checkpoint')}",
        "",
    ]
    for attempt in run.get("retry_attempts") or []:
        if isinstance(attempt, dict):
            lines.extend(attempt_section(attempt, "Failed retry attempt"))
    lines.extend(attempt_section(run, "Run outcome (including retries)"))
    return "\n".join(lines).rstrip() + "\n"


def infrastructure_section(failure: dict[str, Any]) -> list[str]:
    if not failure:
        return []
    lines = ["## Infrastructure failure", "", "Canonical source: `failure.json`", ""]
    for key in ("phase", "outcome", "domain", "code", "message", "error", "exit_code"):
        if key in failure:
            lines.append(f"- {text(key)}: {text(failure.get(key), 1_000)}")
    lines.append("")
    return lines


def diagnostics_document(results: dict[str, Any], runs: list[dict[str, Any]], source: str) -> dict[str, Any]:
    affected = []
    cohorts = {}
    attempts_analyzed = 0
    attempts_missing_transcript = 0
    for entry in runs:
        run = obj(entry.get("run"))
        attempts = []
        for kind, attempt in [
            *(("retry", obj(item)) for item in run.get("retry_attempts") or [] if isinstance(item, dict)),
            ("terminal", run),
        ]:
            if not isinstance(obj(attempt.get("transcript")).get("messages"), list):
                attempts_missing_transcript += 1
                continue
            attempts_analyzed += 1
            occurrences = [*repeated_contract_discovery(attempt), *repeated_tool_error(attempt)]
            if occurrences:
                attempts.append({
                    "kind": kind,
                    "attempt_id": attempt.get("attempt_id"),
                    "attempt_number": attempt.get("attempt_number"),
                    "occurrences": occurrences,
                })
        if not attempts:
            continue
        identity = {
            key: entry.get(key)
            for key in ("scenario_id", "case_id", "seed", "behavior_sha256", "definition_sha256")
            if entry.get(key) is not None
        }
        affected.append({
            **identity,
            "run_id": run.get("run_id"),
            "repetition": entry.get("repetition"),
            "attempts": attempts,
        })
        rules = {
            occurrence["rule_id"]
            for attempt in attempts
            for occurrence in attempt["occurrences"]
        }
        for rule_id in sorted(rules):
            cohort_key = json.dumps({"rule_id": rule_id, **identity}, sort_keys=True, ensure_ascii=False)
            cohort = cohorts.setdefault(cohort_key, {
                "rule_id": rule_id,
                **identity,
                "affected_runs": 0,
                "occurrences": 0,
            })
            cohort["affected_runs"] += 1
            cohort["occurrences"] += sum(
                occurrence["rule_id"] == rule_id
                for attempt in attempts
                for occurrence in attempt["occurrences"]
            )

    identity = {
        key: results.get(key)
        for key in ("execution", "system_under_test", "subject", "result_contract_sha256")
        if results.get(key) is not None
    }
    evidence_sufficiency = (
        "insufficient"
        if attempts_analyzed == 0
        else "partial"
        if attempts_missing_transcript
        else "observed_runs"
    )
    return {
        "schema": "harness-e2e-diagnostics-v1",
        "identity": identity,
        "source": source,
        "evidence_sufficiency": evidence_sufficiency,
        "runs_collected": len(runs),
        "attempts_analyzed": attempts_analyzed,
        "attempts_missing_transcript": attempts_missing_transcript,
        "affected_runs": len(affected) if attempts_analyzed else None,
        "occurrences": (
            sum(cohort["occurrences"] for cohort in cohorts.values()) if attempts_analyzed else None
        ),
        "cohorts": list(cohorts.values()),
        "runs": affected,
    }


def render(artifacts: Path, output: Path) -> None:
    runs, source = collect_runs(artifacts)
    results = obj(read_json(artifacts / "results.json"))
    failure = obj(read_json(artifacts / "failure.json"))
    diagnostics = diagnostics_document(results, runs, source)
    selected = [entry for entry in runs if needs_page(obj(entry.get("run")))]
    failures = output / "failures"
    failures.mkdir(parents=True, exist_ok=True)

    rows = []
    for index, entry in enumerate(selected, 1):
        run = obj(entry.get("run"))
        identity = "\0".join(str(part) for part in (entry.get("scenario_id"), run.get("run_id"), run.get("attempt_id")))
        filename = f"{index:04d}-{hashlib.sha256(identity.encode()).hexdigest()[:12]}.md"
        (failures / filename).write_text(render_run(entry, source), encoding="utf-8")
        rows.append(
            f"| {text(entry.get('scenario_id'), 120)} | {text(run.get('technical'), 120)} | "
            f"{text(run.get('completion'), 120)} | {text(run.get('score'), 120)} | `{filename}` |"
        )

    sufficiency = {
        "insufficient": "insufficient (zero analyzable attempts)",
        "partial": (
            f"partial ({diagnostics['attempts_analyzed']} analyzed, "
            f"{diagnostics['attempts_missing_transcript']} missing transcript)"
        ),
        "observed_runs": "observed runs (all attempts analyzed)",
    }[diagnostics["evidence_sufficiency"]]
    lines = [
        "# Harness E2E report",
        "",
        f"- Canonical run source: {text(source)}",
        f"- Runs collected: {len(runs)}",
        f"- Runs requiring attention: {len(selected)}",
        f"- Evidence sufficiency: {sufficiency}",
        f"- Diagnostic affected runs: {text(diagnostics['affected_runs'])}",
        f"- Diagnostic occurrences: {text(diagnostics['occurrences'])}",
        "",
    ]
    if diagnostics["evidence_sufficiency"] == "partial":
        lines.insert(-1, "- Diagnostic counts include only analyzed attempts.")
    if failure:
        lines.extend(infrastructure_section(failure))
    if rows:
        lines.extend(
            [
                "## Runs requiring attention",
                "",
                "| Scenario | Technical | Completion | Score | Detail report path |",
                "| --- | --- | --- | ---: | --- |",
            ]
        )
        omitted = 0
        for index, row in enumerate(rows):
            candidate = "\n".join([*lines, row, ""]) + "\n"
            if len(candidate.encode()) > SUMMARY_LIMIT - 200:
                omitted = len(rows) - index
                break
            lines.append(row)
        if omitted:
            lines.extend(["", f"{omitted} additional run report(s) omitted from this summary; see `failures/`."])
        lines.append("")

    if diagnostics["cohorts"]:
        heading = [
            "## Diagnostic recurrence",
            "",
            "Counts report observed evidence only; they do not assign cause.",
            "",
            "| Rule | Scenario | Case | Seed | Behavior | Definition | Affected runs | Occurrences |",
            "| --- | --- | --- | --- | --- | --- | ---: | ---: |",
        ]
        if len(("\n".join([*lines, *heading, ""]) + "\n").encode()) <= SUMMARY_LIMIT - 200:
            lines.extend(heading)
            for cohort in diagnostics["cohorts"]:
                row = (
                    f"| {text(cohort.get('rule_id'), 120)} | {text(cohort.get('scenario_id'), 120)} | "
                    f"{text(cohort.get('case_id'), 120)} | {text(cohort.get('seed'), 120)} | "
                    f"{text(cohort.get('behavior_sha256'), 120)} | {text(cohort.get('definition_sha256'), 120)} | "
                    f"{cohort['affected_runs']} | {cohort['occurrences']} |"
                )
                candidate = "\n".join([*lines, row, ""]) + "\n"
                if len(candidate.encode()) > SUMMARY_LIMIT - 200:
                    lines.extend(["", "Additional diagnostic cohorts omitted from this summary; see `diagnostics.json`."])
                    break
                lines.append(row)
            lines.append("")
        else:
            lines.append("Diagnostic recurrence omitted from this summary; see `diagnostics.json`.")

    summary = "\n".join(lines).rstrip() + "\n"
    if len(summary.encode()) > SUMMARY_LIMIT:
        raise ValueError("summary header exceeds the GitHub Markdown size limit")
    (output / "summary.md").write_text(summary, encoding="utf-8")
    (output / "diagnostics.json").write_text(
        json.dumps(diagnostics, indent=2, ensure_ascii=False, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    render(args.artifacts, args.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
