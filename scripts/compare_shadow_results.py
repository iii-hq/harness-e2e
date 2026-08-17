#!/usr/bin/env python3
"""Compare the stable semantics of internal and extracted E2E reports."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from typing import Any


def canonical_hash(value: Any) -> str:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return f"sha256:{hashlib.sha256(encoded).hexdigest()}"


def load_report(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict) or "schema_version" in value:
        raise ValueError(f"{path} is not a current results report")
    if not isinstance(value.get("execution"), dict):
        raise ValueError(f"{path} has no execution identity")
    if not isinstance(value.get("system_under_test"), dict):
        raise ValueError(f"{path} has no system-under-test identity")
    return value


def dimension_projection(run: dict[str, Any]) -> dict[str, Any]:
    return {
        item.get("dimension"): item.get("passed")
        for item in run.get("dimensions", [])
        if isinstance(item, dict) and isinstance(item.get("dimension"), str)
    }


def deliverable_projection(run: dict[str, Any]) -> list[dict[str, Any]]:
    values = []
    for item in run.get("deliverables", []):
        if not isinstance(item, dict):
            continue
        values.append(
            {
                "id": item.get("id"),
                "kind": item.get("kind"),
                "content_sha256": item.get("content_sha256"),
                "schema_valid": item.get("schema_valid"),
                "provenance_valid": item.get("provenance_valid"),
                "invariants": sorted(
                    (
                        {
                            "id": invariant.get("id"),
                            "passed": invariant.get("passed"),
                        }
                        for invariant in item.get("invariants", [])
                        if isinstance(invariant, dict)
                    ),
                    key=lambda invariant: str(invariant["id"]),
                ),
            }
        )
    return sorted(values, key=lambda item: str(item["id"]))


def scenario_projection(scenario: dict[str, Any]) -> dict[str, Any]:
    contract = {
        "scenario_id": scenario.get("scenario_id"),
        "scenario_version": scenario.get("scenario_version"),
        "case_id": scenario.get("case_id"),
        "case": scenario.get("case"),
        "execution_policy": scenario.get("execution_policy"),
    }
    runs = []
    for run in scenario.get("runs", []):
        if not isinstance(run, dict):
            continue
        runs.append(
            {
                "status": run.get("status"),
                "dimensions": dimension_projection(run),
                "hard_gates": sorted(
                    (
                        {
                            "id": gate.get("id"),
                            "dimension": gate.get("dimension"),
                            "passed": gate.get("passed"),
                        }
                        for gate in run.get("hard_gates", [])
                        if isinstance(gate, dict)
                    ),
                    key=lambda gate: str(gate["id"]),
                ),
                "deliverables": deliverable_projection(run),
            }
        )
    runs.sort(key=canonical_hash)
    aggregate = scenario.get("aggregate") or {}
    return {
        "contract_sha256": canonical_hash(contract),
        "passed": scenario.get("passed"),
        "runs": runs,
        "aggregate": {
            "runs": aggregate.get("runs"),
            "scored_runs": aggregate.get("scored_runs"),
            "passed_runs": aggregate.get("passed_runs"),
            "required_passes": aggregate.get("required_passes"),
            "hard_gate_failures": aggregate.get("hard_gate_failures"),
            "technical_failures": aggregate.get("technical_failures"),
        },
    }


def report_cases(report: dict[str, Any]) -> dict[str, dict[str, Any]]:
    cases: dict[str, dict[str, Any]] = {}
    for scenario in report.get("scenarios", []):
        if not isinstance(scenario, dict):
            continue
        key = f"{scenario.get('scenario_id', '')}::{scenario.get('case_id', '')}"
        if key in cases:
            raise ValueError(f"duplicate scenario case: {key}")
        cases[key] = scenario_projection(scenario)
    return cases


def append_difference(
    mismatches: list[dict[str, Any]],
    field: str,
    primary: Any,
    shadow: Any,
) -> None:
    if primary != shadow:
        mismatches.append({"field": field, "primary": primary, "shadow": shadow})


def compare(primary: dict[str, Any], shadow: dict[str, Any]) -> dict[str, Any]:
    mismatches: list[dict[str, Any]] = []
    append_difference(
        mismatches,
        "system_under_test.stack",
        primary["system_under_test"].get("stack"),
        shadow["system_under_test"].get("stack"),
    )
    for field in ["subject", "judge", "judge_protocol"]:
        append_difference(mismatches, field, primary.get(field), shadow.get(field))
    primary_cases = report_cases(primary)
    shadow_cases = report_cases(shadow)
    append_difference(
        mismatches,
        "case_ids",
        sorted(primary_cases),
        sorted(shadow_cases),
    )
    for case_id in sorted(set(primary_cases) & set(shadow_cases)):
        left = primary_cases[case_id]
        right = shadow_cases[case_id]
        for field in ["contract_sha256", "passed", "aggregate", "runs"]:
            append_difference(
                mismatches,
                f"cases.{case_id}.{field}",
                left[field],
                right[field],
            )
    return {
        "equivalent": not mismatches,
        "primary": {
            "execution_id": primary["execution"].get("execution_id"),
            "e2e_revision": primary["system_under_test"].get("e2e_revision"),
            "subject_revision": primary["system_under_test"].get("stack"),
        },
        "shadow": {
            "execution_id": shadow["execution"].get("execution_id"),
            "e2e_revision": shadow["system_under_test"].get("e2e_revision"),
            "subject_revision": shadow["system_under_test"].get("stack"),
        },
        "case_count": len(set(primary_cases) & set(shadow_cases)),
        "mismatches": mismatches,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--primary", type=Path, required=True)
    parser.add_argument("--shadow", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--fail-on-mismatch", action="store_true")
    args = parser.parse_args()
    result = compare(load_report(args.primary), load_report(args.shadow))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=2, sort_keys=True))
    return 1 if args.fail_on_mismatch and not result["equivalent"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
