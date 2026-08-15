"""Compatibility helpers for the versioned Harness E2E assessment contract."""

from __future__ import annotations

from copy import deepcopy
from typing import Any

ASSESSMENT_CONTRACT_VERSION = 1
LATEST_RESULTS_SCHEMA_VERSION = 3


class AssessmentContractError(ValueError):
    """Raised when a result declares an invalid assessment contract."""


def normalize_assessment_contract(result: dict[str, Any]) -> dict[str, Any]:
    """Return a v1 assessment contract, with explicit legacy unavailable states."""

    if not isinstance(result, dict):
        raise AssessmentContractError("E2E result must be an object")
    schema_version = result.get("schema_version", 1)
    contract = result.get("assessment_contract")
    if schema_version == LATEST_RESULTS_SCHEMA_VERSION:
        if not isinstance(contract, dict):
            raise AssessmentContractError("results v3 require assessment_contract")
        if contract.get("contract_version") != ASSESSMENT_CONTRACT_VERSION:
            raise AssessmentContractError("unsupported assessment contract version")
        runs = contract.get("runs")
        if not isinstance(runs, list):
            raise AssessmentContractError("assessment_contract.runs must be an array")
        _validate_run_identities(runs)
        return deepcopy(contract)
    if schema_version not in (1, 2):
        raise AssessmentContractError(f"unsupported results schema version {schema_version}")

    normalized_runs = []
    scenarios = result.get("scenarios", [])
    if not isinstance(scenarios, list):
        raise AssessmentContractError("legacy results scenarios must be an array")
    for scenario_index, scenario in enumerate(scenarios):
        if not isinstance(scenario, dict):
            continue
        runs = scenario.get("runs", [])
        if not isinstance(runs, list):
            continue
        for run_index, run in enumerate(runs):
            if not isinstance(run, dict):
                continue
            attempt_number = run.get("attempt_number", 1)
            normalized_runs.append(
                {
                    "run_id": str(run.get("run_id") or f"legacy-run-{scenario_index}-{run_index}"),
                    "attempt_id": str(
                        run.get("attempt_id") or f"legacy-attempt-{attempt_number}"
                    ),
                    "system_status": "unavailable",
                    "assessments": [],
                    "assets": [],
                    "ai_final_assessment": {
                        "availability": "not_evaluated",
                        "reason": "legacy result does not contain the assessment contract",
                    },
                    "effective_status": "unavailable",
                }
            )
    return {
        "contract_version": ASSESSMENT_CONTRACT_VERSION,
        "runs": normalized_runs,
    }


def _validate_run_identities(runs: list[Any]) -> None:
    seen: set[tuple[str, str]] = set()
    for run in runs:
        if not isinstance(run, dict):
            raise AssessmentContractError("assessment contract run must be an object")
        run_id = run.get("run_id")
        attempt_id = run.get("attempt_id")
        if not isinstance(run_id, str) or not run_id.strip():
            raise AssessmentContractError("assessment contract run_id is required")
        if not isinstance(attempt_id, str) or not attempt_id.strip():
            raise AssessmentContractError("assessment contract attempt_id is required")
        identity = (run_id, attempt_id)
        if identity in seen:
            raise AssessmentContractError("assessment contract repeats a run identity")
        seen.add(identity)
