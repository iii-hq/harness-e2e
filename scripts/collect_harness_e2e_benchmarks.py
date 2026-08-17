#!/usr/bin/env python3
"""Convert Harness E2E reports into compact benchmark and dashboard data."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

class CollectionError(ValueError):
    """Raised when E2E benchmark inputs are malformed or contradictory."""


@dataclass(frozen=True)
class CollectionConfig:
    reports_root: Path
    output_dir: Path
    subjects: list[dict[str, str]]
    scenarios: list[str]
    lane: str
    requested_runs: int
    source_sha: str
    source_ref: str
    repository: str
    workflow_url: str
    release_tag: str
    release_worker: str
    release_version: str
    release_url: str
    registry_tag: str
    judge_model: str
    judge_provider: str
    execution_run_id: str
    execution_attempt: int
    execution_event: str
    execution_actor: str
    generated_at: str
    stack_mode: str = "source"
    stack_versions: dict[str, str] = field(default_factory=dict)
    stack_digest: str = ""


def load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as exc:
        raise CollectionError(f"cannot decode {path}: {exc}") from exc


def require_string(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise CollectionError(f"{label} must be a non-empty string")
    return value


def require_number(value: Any, label: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise CollectionError(f"{label} must be a number")
    number = float(value)
    if not math.isfinite(number):
        raise CollectionError(f"{label} must be finite")
    return number


def optional_number(value: Any, label: str) -> float | None:
    if value is None:
        return None
    return require_number(value, label)


def semantic_result_status(
    *,
    passed: bool,
    hard_gate_failures: int,
    technical_failures: int,
    infra_failures: int = 0,
    complete: bool = True,
) -> str:
    if infra_failures:
        return "infra_failed"
    if not complete:
        return "incomplete"
    if technical_failures:
        return "technical_failed"
    if hard_gate_failures:
        return "hard_gate_failed"
    if not passed:
        return "infra_failed"
    return "passed"


def parse_subjects(raw: str) -> list[dict[str, str]]:
    try:
        subjects = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise CollectionError(f"subjects JSON is invalid: {exc}") from exc
    if not isinstance(subjects, list) or not subjects:
        raise CollectionError("subjects must be a non-empty JSON array")

    parsed: list[dict[str, str]] = []
    seen: set[str] = set()
    for index, subject in enumerate(subjects):
        if not isinstance(subject, dict):
            raise CollectionError(f"subjects[{index}] must be an object")
        entry = {
            "id": require_string(subject.get("id"), f"subjects[{index}].id"),
            "model": require_string(
                subject.get("model"), f"subjects[{index}].model"
            ),
            "provider": require_string(
                subject.get("provider"), f"subjects[{index}].provider"
            ),
        }
        if entry["id"] in seen:
            raise CollectionError(f"duplicate subject id: {entry['id']}")
        seen.add(entry["id"])
        parsed.append(entry)
    return parsed


def parse_scenarios(raw: str) -> list[str]:
    try:
        scenarios = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise CollectionError(f"scenarios JSON is invalid: {exc}") from exc
    if not isinstance(scenarios, list) or not scenarios:
        raise CollectionError("scenarios must be a non-empty JSON array")
    parsed = [require_string(value, "scenario") for value in scenarios]
    if len(set(parsed)) != len(parsed):
        raise CollectionError("scenarios must be unique")
    return parsed


def parse_stack_versions(raw: str) -> dict[str, str]:
    try:
        versions = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise CollectionError(f"stack_versions JSON is invalid: {exc}") from exc
    if not isinstance(versions, dict):
        raise CollectionError("stack_versions must be a JSON object")
    parsed: dict[str, str] = {}
    for worker, version in versions.items():
        if not isinstance(worker, str) or not worker:
            raise CollectionError(
                "stack_versions worker names must be non-empty strings"
            )
        parsed[worker] = require_string(version, f"stack_versions[{worker}]")
    return parsed


def discover_contexts(root: Path) -> dict[tuple[str, str], Path]:
    contexts: dict[tuple[str, str], Path] = {}
    if not root.exists():
        return contexts
    for path in sorted(root.rglob("benchmark-context.json")):
        context = load_json(path)
        if not isinstance(context, dict):
            raise CollectionError(f"{path} must contain an object")
        subject_id = require_string(context.get("subject_id"), f"{path}: subject_id")
        scenario_id = require_string(
            context.get("scenario_id"), f"{path}: scenario_id"
        )
        key = (subject_id, scenario_id)
        if key in contexts:
            raise CollectionError(
                f"duplicate benchmark context for {subject_id}/{scenario_id}"
            )
        contexts[key] = path
    return contexts


def load_deployment(context_path: Path | None) -> dict[str, Any] | None:
    if context_path is None:
        return None
    candidates = (
        context_path.parent / "deployment.json",
        context_path.parent.parent / "deployment.json",
        context_path.parent.parent.parent / "deployment.json",
    )
    seen: set[Path] = set()
    for candidate in candidates:
        if candidate in seen or not candidate.is_file():
            continue
        seen.add(candidate)
        value = load_json(candidate)
        if not isinstance(value, dict):
            raise CollectionError(f"{candidate} must contain an object")
        return value
    return None


def stack_metadata(
    config: CollectionConfig,
    deployment: dict[str, Any] | None,
) -> tuple[dict[str, Any], dict[str, Any]]:
    versions = dict(config.stack_versions)
    digest = config.stack_digest
    release_version = config.release_version
    if deployment is not None:
        deployment_versions = deployment.get("stack_versions")
        if isinstance(deployment_versions, dict):
            versions = {
                str(worker): str(version)
                for worker, version in deployment_versions.items()
                if isinstance(worker, str) and isinstance(version, str)
            }
        deployment_digest = deployment.get("stack_lock_digest")
        if isinstance(deployment_digest, str):
            digest = deployment_digest
        actual_version = deployment.get("actual_release_version")
        if isinstance(actual_version, str) and actual_version:
            release_version = actual_version

    return (
        {
            "tag": config.release_tag,
            "worker": config.release_worker,
            "version": release_version,
            "url": config.release_url,
            "registry_tag": config.registry_tag,
        },
        {
            "mode": config.stack_mode,
            "versions": versions,
            "lock_digest": digest,
        },
    )


def compact_extra(value: dict[str, Any]) -> str:
    return json.dumps(value, separators=(",", ":"), sort_keys=True)


def metric(
    category: str,
    subject_id: str,
    scenario_id: str,
    metric_id: str,
    unit: str,
    value: float,
    extra: dict[str, Any],
    *,
    value_range: str | None = None,
) -> dict[str, Any]:
    result: dict[str, Any] = {
        "name": f"{category}::{subject_id}::{scenario_id}::{metric_id}",
        "unit": unit,
        "value": value,
        "extra": compact_extra(extra),
    }
    if value_range:
        result["range"] = value_range
    return result


def sum_known(values: list[float | None], *, require_all: bool = True) -> float | None:
    if require_all and any(value is None for value in values):
        return None
    known = [value for value in values if value is not None]
    return sum(known) if known else None


def validate_report(
    report: Any,
    *,
    config: CollectionConfig,
    subject: dict[str, str],
    scenario_id: str,
    path: Path,
    deployment: dict[str, Any] | None,
) -> tuple[dict[str, Any], dict[str, Any]]:
    if not isinstance(report, dict):
        raise CollectionError(f"{path} must contain an object")
    if "schema_version" in report:
        raise CollectionError(f"{path}: versioned results payloads are not supported")
    report_subject = report.get("subject")
    if not isinstance(report_subject, dict):
        raise CollectionError(f"{path}: subject must be an object")
    if (
        report_subject.get("model") != subject["model"]
        or report_subject.get("provider") != subject["provider"]
    ):
        raise CollectionError(
            f"{path}: report subject does not match {subject['id']}"
        )
    scenarios = report.get("scenarios")
    if not isinstance(scenarios, list) or len(scenarios) != 1:
        raise CollectionError(f"{path}: expected exactly one scenario")
    scenario = scenarios[0]
    if not isinstance(scenario, dict) or scenario.get("scenario_id") != scenario_id:
        raise CollectionError(f"{path}: scenario id does not match {scenario_id}")
    validate_identity(
        report,
        config=config,
        scenario=scenario,
        path=path,
        deployment=deployment,
    )
    return report, scenario


def validate_identity(
    report: dict[str, Any],
    *,
    config: CollectionConfig,
    scenario: dict[str, Any],
    path: Path,
    deployment: dict[str, Any] | None,
) -> None:
    execution = report.get("execution")
    if not isinstance(execution, dict):
        raise CollectionError(f"{path}: results require execution identity")
    for field in ("execution_id", "lane", "started_at", "completed_at"):
        require_string(execution.get(field), f"{path}: execution.{field}")
    if execution["lane"] != config.lane:
        raise CollectionError(
            f"{path}: execution lane {execution['lane']} does not match {config.lane}"
        )

    system = report.get("system_under_test")
    if not isinstance(system, dict):
        raise CollectionError(f"{path}: results require system_under_test")
    stack = system.get("stack")
    if not isinstance(stack, dict) or stack.get("mode") != config.stack_mode:
        observed = stack.get("mode") if isinstance(stack, dict) else None
        raise CollectionError(
            f"{path}: system stack mode {observed!r} does not match "
            f"{config.stack_mode!r}"
        )
    for field in (
        "engine_version",
        "harness_version",
        "e2e_repository",
        "e2e_revision",
    ):
        require_string(system.get(field), f"{path}: system_under_test.{field}")
    hashes = system.get("contract_hashes")
    if not isinstance(hashes, dict) or not hashes:
        raise CollectionError(f"{path}: system identity requires contract hashes")

    if config.stack_mode == "source":
        repository = require_string(
            stack.get("workers_repository"),
            f"{path}: stack.workers_repository",
        )
        revision = require_string(
            stack.get("workers_revision"), f"{path}: stack.workers_revision"
        )
        if repository != config.repository:
            raise CollectionError(
                f"{path}: source repository {repository} does not match "
                f"{config.repository}"
            )
        if len(config.source_sha) == 40 and revision != config.source_sha:
            raise CollectionError(
                f"{path}: source revision {revision} does not match {config.source_sha}"
            )
    else:
        expected_release, expected_stack = stack_metadata(config, deployment)
        del expected_release
        if stack.get("stack_versions") != expected_stack["versions"]:
            raise CollectionError(
                f"{path}: registry stack versions do not match deployment"
            )
        if stack.get("stack_lock_digest") != expected_stack["lock_digest"]:
            raise CollectionError(
                f"{path}: registry stack digest does not match deployment"
            )

    manifest_reference = report.get("manifest")
    if not isinstance(manifest_reference, dict):
        raise CollectionError(f"{path}: results require a manifest reference")
    manifest_path = verify_artifact_reference(
        manifest_reference, root=path.parent, label=f"{path}: manifest"
    )
    manifest = load_json(manifest_path)
    if not isinstance(manifest, dict) or "schema_version" in manifest:
        raise CollectionError(f"{manifest_path}: expected the unversioned manifest payload")
    for field in ("execution", "system_under_test", "subject", "judge"):
        if manifest.get(field) != report.get(field):
            raise CollectionError(f"{manifest_path}: {field} differs from results")
    control_plane = manifest.get("control_plane")
    if not isinstance(control_plane, dict) or not isinstance(
        control_plane.get("functions"), list
    ) or not control_plane["functions"]:
        raise CollectionError(f"{manifest_path}: control plane evidence is empty")

    case_id = require_string(scenario.get("case_id"), f"{path}: scenario.case_id")
    require_number(
        scenario.get("scenario_version"), f"{path}: scenario.scenario_version"
    )
    case = validate_scenario_case(scenario, path=path)
    if case.get("case_id") != case_id:
        raise CollectionError(f"{path}: scenario.case_id differs from scenario.case.case_id")
    if not isinstance(scenario.get("execution_policy"), dict):
        raise CollectionError(f"{path}: scenario.execution_policy must be an object")
    runs = scenario.get("runs")
    if not isinstance(runs, list):
        raise CollectionError(f"{path}: scenario.runs must be an array")
    for run in runs:
        if not isinstance(run, dict):
            raise CollectionError(f"{path}: run must be an object")
        verify_attempt_evidence(
            run, root=path.parent, label=f"{path}: run", case=case
        )
        run_id = run["run_id"]
        final_attempt_id = run["attempt_id"]
        final_attempt_number = int(run["attempt_number"])
        attempt_ids = {final_attempt_id}
        retries = run.get("retry_attempts", [])
        if not isinstance(retries, list):
            raise CollectionError(f"{path}: retry_attempts must be an array")
        for retry in retries:
            if not isinstance(retry, dict):
                raise CollectionError(f"{path}: retry attempt must be an object")
            verify_attempt_evidence(
                retry,
                root=path.parent,
                label=f"{path}: retry attempt",
                case=case,
            )
            if retry["run_id"] != run_id:
                raise CollectionError(f"{path}: retry belongs to a different run")
            if retry["attempt_id"] in attempt_ids:
                raise CollectionError(f"{path}: attempt ids must be distinct")
            attempt_ids.add(retry["attempt_id"])
            if int(retry["attempt_number"]) >= final_attempt_number:
                raise CollectionError(
                    f"{path}: retry attempt numbers must precede the final attempt"
                )


def validate_scenario_case(scenario: dict[str, Any], *, path: Path) -> dict[str, Any]:
    case = scenario.get("case")
    if not isinstance(case, dict):
        raise CollectionError(f"{path}: results v2 requires scenario.case")
    if case.get("scenario_id") != scenario.get("scenario_id"):
        raise CollectionError(f"{path}: scenario.case has a different scenario_id")
    if case.get("scenario_version") != scenario.get("scenario_version"):
        raise CollectionError(f"{path}: scenario.case has a different scenario_version")
    seed = require_number(case.get("seed"), f"{path}: scenario.case.seed")
    if not seed.is_integer() or not 0 <= seed <= 2**64 - 1:
        raise CollectionError(f"{path}: scenario.case.seed must be a uint64")
    expected_case_id = (
        f"{case['scenario_id']}:v{int(case['scenario_version'])}:seed-{int(seed):016x}"
    )
    if case.get("case_id") != expected_case_id:
        raise CollectionError(f"{path}: scenario.case case_id is inconsistent")
    inputs_hash = require_string(
        case.get("inputs_sha256"), f"{path}: scenario.case.inputs_sha256"
    )
    canonical_inputs = json.dumps(
        case.get("inputs"),
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    ).encode("utf-8")
    actual_hash = f"sha256:{hashlib.sha256(canonical_inputs).hexdigest()}"
    if inputs_hash != actual_hash:
        raise CollectionError(f"{path}: scenario.case inputs hash does not match")
    complexity = case.get("complexity")
    if not isinstance(complexity, dict) or not isinstance(
        complexity.get("profile"), dict
    ):
        raise CollectionError(f"{path}: scenario.case complexity is invalid")
    require_string(complexity.get("tier"), f"{path}: scenario.case.complexity.tier")
    profile = complexity["profile"]
    profile_fields = (
        "planning_depth",
        "dependency_depth",
        "parallel_branches",
        "external_systems",
        "state_transitions",
        "wake_cycles",
        "validation_loops",
        "artifact_count",
        "coordination_edges",
        "ambiguity_level",
    )
    for field in profile_fields:
        value = require_number(
            profile.get(field), f"{path}: scenario.case.complexity.profile.{field}"
        )
        if not value.is_integer() or value < 0:
            raise CollectionError(f"{path}: complexity field {field} is invalid")
    if complexity.get("tier") != complexity_tier(profile):
        raise CollectionError(f"{path}: scenario.case complexity classification differs")
    capabilities = case.get("required_capabilities")
    if (
        not isinstance(capabilities, list)
        or any(not isinstance(item, str) or not item for item in capabilities)
        or len(capabilities) != len(set(capabilities))
    ):
        raise CollectionError(f"{path}: scenario.case required capabilities are invalid")
    contract = case.get("deliverable_contract")
    if not isinstance(contract, dict):
        raise CollectionError(f"{path}: scenario.case deliverable contract is invalid")
    if not isinstance(contract.get("artifacts"), list) or not isinstance(
        contract.get("invariants"), list
    ):
        raise CollectionError(f"{path}: scenario.case deliverable contract is incomplete")
    if contract["artifacts"] and not contract.get("capture_before_cleanup"):
        raise CollectionError(f"{path}: deliverables must be captured before cleanup")
    return case


def complexity_tier(profile: dict[str, Any]) -> str:
    if profile["ambiguity_level"] >= 7 and (
        profile["validation_loops"] >= 2 or profile["wake_cycles"] >= 2
    ):
        return "l5_adaptive"
    if profile["coordination_edges"] >= 3 or (
        profile["dependency_depth"] >= 3
        and (profile["parallel_branches"] >= 2 or profile["validation_loops"] > 0)
    ):
        return "l4_coordinated"
    if profile["parallel_branches"] >= 2:
        return "l3_concurrent"
    if (
        profile["external_systems"] > 0
        or profile["state_transitions"] > 0
        or profile["wake_cycles"] > 0
        or profile["validation_loops"] > 0
    ):
        return "l2_stateful"
    if (
        profile["planning_depth"] > 1
        or profile["dependency_depth"] > 0
        or profile["artifact_count"] > 0
    ):
        return "l1_sequential"
    return "l0_atomic"


def verify_attempt_evidence(
    attempt: dict[str, Any], *, root: Path, label: str, case: dict[str, Any]
) -> None:
    for field in ("run_id", "attempt_id"):
        require_string(attempt.get(field), f"{label}.{field}")
    attempt_number = require_number(
        attempt.get("attempt_number"), f"{label}.attempt_number"
    )
    if attempt_number < 1 or not attempt_number.is_integer():
        raise CollectionError(f"{label}.attempt_number must be a positive integer")
    evidence = attempt.get("evidence", [])
    if not isinstance(evidence, list):
        raise CollectionError(f"{label}.evidence must be an array")
    for index, reference in enumerate(evidence):
        if not isinstance(reference, dict):
            raise CollectionError(f"{label}.evidence[{index}] must be an object")
        verify_artifact_reference(
            reference, root=root, label=f"{label}.evidence[{index}]"
        )
    verify_deliverables(attempt, root=root, label=label, case=case)
    dimensions = attempt.get("dimensions", [])
    if not isinstance(dimensions, list):
        raise CollectionError(f"{label}.dimensions must be an array")
    dimension_ids = [
        item.get("dimension") for item in dimensions if isinstance(item, dict)
    ]
    if len(dimension_ids) != len(set(dimension_ids)) or any(
        item not in {
            "deliverable",
            "structural_integrity",
            "efficiency",
            "e2e_infrastructure",
        }
        for item in dimension_ids
    ):
        raise CollectionError(f"{label}.dimensions are invalid")


def verify_deliverables(
    attempt: dict[str, Any], *, root: Path, label: str, case: dict[str, Any]
) -> None:
    contract = case["deliverable_contract"]
    expectations = {}
    for item in contract["artifacts"]:
        if not isinstance(item, dict):
            raise CollectionError(f"{label}: deliverable artifact contract is invalid")
        artifact_id = require_string(
            item.get("id"), f"{label}.deliverable contract id"
        )
        if artifact_id in expectations:
            raise CollectionError(f"{label}: duplicate deliverable artifact contract")
        expectations[artifact_id] = item
    deliverables = attempt.get("deliverables", [])
    if not isinstance(deliverables, list):
        raise CollectionError(f"{label}.deliverables must be an array")
    observed: dict[str, dict[str, Any]] = {}
    for index, deliverable in enumerate(deliverables):
        if not isinstance(deliverable, dict):
            raise CollectionError(f"{label}.deliverables[{index}] must be an object")
        deliverable_id = require_string(
            deliverable.get("id"), f"{label}.deliverables[{index}].id"
        )
        if deliverable_id in observed:
            raise CollectionError(f"{label}: duplicate deliverable {deliverable_id}")
        observed[deliverable_id] = deliverable
    if set(observed) != set(expectations):
        raise CollectionError(f"{label}: captured deliverables differ from their contract")
    expected_invariants = set()
    for item in contract["invariants"]:
        if not isinstance(item, dict):
            raise CollectionError(f"{label}: deliverable contract invariant is invalid")
        expected_invariants.add(
            require_string(item.get("id"), f"{label}: deliverable invariant id")
        )
    observed_invariants = set()
    for deliverable_id, deliverable in observed.items():
        expectation = expectations[deliverable_id]
        if deliverable.get("kind") != expectation.get("kind"):
            raise CollectionError(f"{label}: deliverable {deliverable_id} kind differs")
        artifact_reference = deliverable.get("artifact")
        if not isinstance(artifact_reference, dict):
            raise CollectionError(f"{label}: deliverable {deliverable_id} has no artifact")
        artifact_path = verify_artifact_reference(
            artifact_reference,
            root=root,
            label=f"{label}.deliverables[{deliverable_id}].artifact",
        )
        content = load_json(artifact_path)
        canonical = json.dumps(
            content, ensure_ascii=False, separators=(",", ":"), sort_keys=True
        ).encode("utf-8")
        content_hash = f"sha256:{hashlib.sha256(canonical).hexdigest()}"
        if content_hash != deliverable.get("content_sha256"):
            raise CollectionError(
                f"{label}: deliverable {deliverable_id} content hash differs"
            )
        invariants = deliverable.get("invariants")
        if not isinstance(invariants, list) or any(
            not isinstance(item, dict) for item in invariants
        ):
            raise CollectionError(
                f"{label}: deliverable {deliverable_id} invariants are invalid"
            )
        for item in invariants:
            invariant_id = require_string(
                item.get("id"), f"{label}: captured invariant id"
            )
            if invariant_id not in expected_invariants or invariant_id in observed_invariants:
                raise CollectionError(
                    f"{label}: deliverable {deliverable_id} invariants differ"
                )
            observed_invariants.add(invariant_id)
        if not isinstance(deliverable.get("schema_valid"), bool) or not isinstance(
            deliverable.get("provenance_valid"), bool
        ):
            raise CollectionError(
                f"{label}: deliverable {deliverable_id} validation flags are invalid"
            )
        provenance = deliverable.get("provenance")
        if contract.get("provenance_required") and (
            not isinstance(provenance, list) or not provenance
        ):
            raise CollectionError(
                f"{label}: deliverable {deliverable_id} provenance is missing"
            )
        if isinstance(provenance, list) and any(
            not isinstance(item, dict)
            or any(
                not isinstance(item.get(field), str) or not item[field]
                for field in ("kind", "source_id", "relation")
            )
            for item in provenance
        ):
            raise CollectionError(
                f"{label}: deliverable {deliverable_id} provenance is invalid"
            )
    if observed_invariants != expected_invariants:
        raise CollectionError(f"{label}: captured invariants differ from their contract")


def verify_artifact_reference(
    reference: dict[str, Any], *, root: Path, label: str
) -> Path:
    relative = require_string(reference.get("path"), f"{label}.path")
    relative_path = Path(relative)
    if relative_path.is_absolute() or any(
        part in ("", ".", "..") for part in relative_path.parts
    ):
        raise CollectionError(f"{label}.path must be a safe relative path")
    path = root.joinpath(*relative_path.parts)
    try:
        data = path.read_bytes()
    except OSError as exc:
        raise CollectionError(f"cannot read {label} artifact {path}: {exc}") from exc
    expected_size = require_number(reference.get("size_bytes"), f"{label}.size_bytes")
    if expected_size != len(data):
        raise CollectionError(f"{label} size does not match its reference")
    expected_hash = require_string(reference.get("sha256"), f"{label}.sha256")
    actual_hash = f"sha256:{hashlib.sha256(data).hexdigest()}"
    if expected_hash != actual_hash:
        raise CollectionError(f"{label} hash does not match its reference")
    return path


def result_path(context_path: Path | None) -> Path | None:
    if context_path is None:
        return None
    return context_path.parent / "results.json"


def collect(
    config: CollectionConfig,
) -> tuple[
    list[dict[str, Any]],
    list[dict[str, Any]],
    dict[str, Any],
    dict[str, Any],
]:
    contexts = discover_contexts(config.reports_root)
    quality: list[dict[str, Any]] = []
    efficiency: list[dict[str, Any]] = []
    snapshot_subjects: list[dict[str, Any]] = []
    execution_reports: list[dict[str, Any]] = []
    stack_observations: list[dict[str, Any]] = []
    release_observations: list[dict[str, Any]] = []
    scenario_contracts: dict[str, tuple[str, Any, str]] = {}
    system_observations: list[dict[str, Any]] = []
    result_execution_ids: set[str] = set()
    execution_id = f"{config.execution_run_id}-{config.execution_attempt}"
    execution = {
        "id": execution_id,
        "run_id": config.execution_run_id,
        "attempt": config.execution_attempt,
        "event": config.execution_event,
        "actor": config.execution_actor,
        "workflow_url": config.workflow_url,
    }

    for subject in config.subjects:
        scenario_snapshots: list[dict[str, Any]] = []
        subject_costs: list[float | None] = []
        subject_wall_times: list[float | None] = []
        subject_context_compactions: list[float | None] = []
        subject_passed = 0
        report_count = 0
        hard_gate_failures = 0
        technical_failures = 0
        infra_failures = 0
        retries = 0
        engine_revisions: set[str] = set()
        resolved_judge: dict[str, Any] | None = None

        for scenario_id in config.scenarios:
            context_path = contexts.get((subject["id"], scenario_id))
            deployment = load_deployment(context_path)
            report_path = result_path(context_path)
            release_metadata, stack = stack_metadata(config, deployment)
            if stack["versions"] or stack["lock_digest"]:
                if stack not in stack_observations:
                    stack_observations.append(stack)
            if deployment is not None and release_metadata not in release_observations:
                release_observations.append(release_metadata)
            base_extra: dict[str, Any] = {
                "execution": execution,
                "lane": config.lane,
                "generated_at": config.generated_at,
                "source": {
                    "sha": config.source_sha,
                    "ref": config.source_ref,
                    "repository": config.repository,
                },
                "workflow_url": config.workflow_url,
                "release": release_metadata,
                "stack": stack,
                "subject": subject,
                "judge": {
                    "model": config.judge_model,
                    "provider": config.judge_provider,
                },
                "scenario": scenario_id,
                "requested_runs": config.requested_runs,
            }

            if report_path is None or not report_path.is_file():
                execution_report = {
                    "subject_id": subject["id"],
                    "scenario_id": scenario_id,
                    "available": False,
                    "report": None,
                }
                if deployment is not None:
                    execution_report["deployment"] = deployment
                execution_reports.append(execution_report)
                status = (
                    "infra_failed"
                    if deployment is not None
                    and deployment.get("status") == "infra_failed"
                    else "missing_report"
                )
                infra_failures += int(status == "infra_failed")
                scenario_snapshot = {
                    "id": scenario_id,
                    "status": status,
                    "passed": False,
                    "runs": 0,
                    "median_score": None,
                    "pass_rate": None,
                    "hard_gate_failures": None,
                    "technical_failures": None,
                    "infra_failures": int(status == "infra_failed"),
                    "retries": None,
                    "total_cost_usd": None,
                    "wall_time_seconds": None,
                    "context_compactions": None,
                }
                efficiency.append(
                    metric(
                        "reliability",
                        subject["id"],
                        scenario_id,
                        "missing_reports",
                        "count",
                        1,
                        {**base_extra, "passed": False, "status": status},
                    )
                )
                if status == "infra_failed":
                    efficiency.append(
                        metric(
                            "reliability",
                            subject["id"],
                            scenario_id,
                            "infra_failed",
                            "count",
                            1,
                            {**base_extra, "passed": False, "status": status},
                        )
                    )
                subject_costs.append(None)
                subject_wall_times.append(None)
                subject_context_compactions.append(None)
                scenario_snapshots.append(scenario_snapshot)
                continue

            report, scenario = validate_report(
                load_json(report_path),
                config=config,
                subject=subject,
                scenario_id=scenario_id,
                path=report_path,
                deployment=deployment,
            )
            report_execution_id = report["execution"]["execution_id"]
            if report_execution_id in result_execution_ids:
                raise CollectionError(
                    f"{report_path}: duplicate result execution id "
                    f"{report_execution_id}"
                )
            result_execution_ids.add(report_execution_id)
            contract = (
                scenario.get("case_id"),
                scenario.get("scenario_version"),
                compact_extra(scenario.get("case", {})),
                compact_extra(scenario.get("execution_policy", {})),
            )
            previous = scenario_contracts.setdefault(scenario_id, contract)
            if previous != contract:
                raise CollectionError(
                    f"{report_path}: scenario contract is incompatible with "
                    "another report"
                )
            system = report["system_under_test"]
            if system not in system_observations:
                system_observations.append(system)
            execution_report = {
                "subject_id": subject["id"],
                "scenario_id": scenario_id,
                "available": True,
                "report": report,
            }
            if isinstance(report.get("execution"), dict):
                execution_report["execution_id"] = report["execution"].get(
                    "execution_id"
                )
            if deployment is not None:
                execution_report["deployment"] = deployment
            execution_reports.append(execution_report)
            report_count += 1
            report_passed = bool(scenario.get("passed"))
            subject_passed += int(report_passed)
            aggregate = scenario.get("aggregate")
            runs = scenario.get("runs")
            if not isinstance(aggregate, dict) or not isinstance(runs, list):
                raise CollectionError(f"{report_path}: aggregate and runs are required")

            median_score = optional_number(
                aggregate.get("median_score"), f"{report_path}: median_score"
            )
            pass_rate = require_number(
                aggregate.get("pass_rate"), f"{report_path}: pass_rate"
            )
            hard_gates = int(
                require_number(
                    aggregate.get("hard_gate_failures"),
                    f"{report_path}: hard_gate_failures",
                )
            )
            technical = int(
                require_number(
                    aggregate.get("technical_failures"),
                    f"{report_path}: technical_failures",
                )
            )
            deployment_infra_failed = int(
                deployment is not None and deployment.get("status") == "infra_failed"
            )
            retry_count = sum(
                len(run.get("retry_attempts", []))
                for run in runs
                if isinstance(run, dict)
                and isinstance(run.get("retry_attempts", []), list)
            )
            wall_time_seconds = (
                sum(
                    require_number(
                        run.get("wall_time_ms"), f"{report_path}: run wall_time_ms"
                    )
                    for run in runs
                    if isinstance(run, dict)
                )
                / 1000
            )
            aggregate_cost = aggregate.get("cost")
            if not isinstance(aggregate_cost, dict):
                raise CollectionError(f"{report_path}: aggregate.cost is required")
            total_cost = optional_number(
                aggregate_cost.get("total_usd"), f"{report_path}: total cost"
            )
            subject_cost = optional_number(
                aggregate_cost.get("subject_usd"), f"{report_path}: subject cost"
            )
            judge_cost = optional_number(
                aggregate_cost.get("judge_usd"), f"{report_path}: judge cost"
            )
            score_values = [
                int(require_number(run["score"], f"{report_path}: run score"))
                for run in runs
                if isinstance(run, dict) and run.get("score") is not None
            ]
            context_compactions = sum_known(
                [
                    optional_number(
                        run.get("efficiency", {}).get("context_compactions")
                        if isinstance(run.get("efficiency"), dict)
                        else None,
                        f"{report_path}: run context_compactions",
                    )
                    for run in runs
                    if isinstance(run, dict)
                ]
            )

            system = report.get("system_under_test")
            engine_revision = (
                system.get("engine_revision")
                if isinstance(system, dict)
                else report.get("engine_revision")
            )
            if isinstance(engine_revision, str) and engine_revision:
                engine_revisions.add(engine_revision)
            if isinstance(report.get("judge"), dict):
                resolved_judge = report["judge"]

            status = semantic_result_status(
                passed=report_passed,
                hard_gate_failures=hard_gates,
                technical_failures=technical,
                infra_failures=deployment_infra_failed,
            )
            extra = {
                **base_extra,
                "judge": report.get("judge") or base_extra["judge"],
                "engine_revision": engine_revision,
                "passed": report_passed,
                "status": status,
                "runs": len(runs),
            }

            if median_score is not None:
                score_range = (
                    f"{min(score_values)}–{max(score_values)}"
                    if len(score_values) > 1
                    else None
                )
                quality.append(
                    metric(
                        "quality",
                        subject["id"],
                        scenario_id,
                        "median_score",
                        "points",
                        median_score,
                        extra,
                        value_range=score_range,
                    )
                )
            quality.append(
                metric(
                    "quality",
                    subject["id"],
                    scenario_id,
                    "pass_rate",
                    "percent",
                    pass_rate * 100,
                    extra,
                )
            )
            efficiency.extend(
                [
                    metric(
                        "reliability",
                        subject["id"],
                        scenario_id,
                        "hard_gate_failures",
                        "count",
                        hard_gates,
                        extra,
                    ),
                    metric(
                        "reliability",
                        subject["id"],
                        scenario_id,
                        "technical_failures",
                        "count",
                        technical,
                        extra,
                    ),
                    metric(
                        "reliability",
                        subject["id"],
                        scenario_id,
                        "retry_attempts",
                        "count",
                        retry_count,
                        extra,
                    ),
                    metric(
                        "reliability",
                        subject["id"],
                        scenario_id,
                        "missing_reports",
                        "count",
                        0,
                        extra,
                    ),
                    metric(
                        "reliability",
                        subject["id"],
                        scenario_id,
                        "infra_failed",
                        "count",
                        0,
                        extra,
                    ),
                    metric(
                        "efficiency",
                        subject["id"],
                        scenario_id,
                        "wall_time_seconds",
                        "seconds",
                        wall_time_seconds,
                        extra,
                    ),
                ]
            )
            if subject_cost is not None:
                efficiency.append(
                    metric(
                        "efficiency",
                        subject["id"],
                        scenario_id,
                        "subject_cost_usd",
                        "USD",
                        subject_cost,
                        extra,
                    )
                )
            if judge_cost is not None:
                efficiency.append(
                    metric(
                        "efficiency",
                        subject["id"],
                        scenario_id,
                        "judge_cost_usd",
                        "USD",
                        judge_cost,
                        extra,
                    )
                )
            if total_cost is not None:
                efficiency.append(
                    metric(
                        "efficiency",
                        subject["id"],
                        scenario_id,
                        "total_cost_usd",
                        "USD",
                        total_cost,
                        extra,
                    )
                )
            if context_compactions is not None:
                efficiency.append(
                    metric(
                        "efficiency",
                        subject["id"],
                        scenario_id,
                        "context_compactions",
                        "count",
                        context_compactions,
                        extra,
                    )
                )

            hard_gate_failures += hard_gates
            technical_failures += technical
            infra_failures += deployment_infra_failed
            retries += retry_count
            subject_costs.append(total_cost)
            subject_wall_times.append(wall_time_seconds)
            subject_context_compactions.append(context_compactions)
            scenario_snapshots.append(
                {
                    "id": scenario_id,
                    "status": status,
                    "passed": report_passed,
                    "runs": len(runs),
                    "median_score": median_score,
                    "pass_rate": pass_rate,
                    "hard_gate_failures": hard_gates,
                    "technical_failures": technical,
                    "infra_failures": deployment_infra_failed,
                    "retries": retry_count,
                    "total_cost_usd": total_cost,
                    "wall_time_seconds": wall_time_seconds,
                    "context_compactions": context_compactions,
                }
            )

        expected_count = len(config.scenarios)
        missing_reports = expected_count - report_count
        scenario_pass_rate = subject_passed / expected_count * 100
        report_coverage = report_count / expected_count * 100
        all_reports_present = missing_reports == 0
        total_cost = sum_known(subject_costs)
        total_wall_time = sum_known(subject_wall_times)
        total_context_compactions = sum_known(subject_context_compactions)
        suite_passed = (
            all_reports_present
            and subject_passed == expected_count
            and technical_failures == 0
            and infra_failures == 0
        )
        suite_status = semantic_result_status(
            passed=suite_passed,
            hard_gate_failures=hard_gate_failures,
            technical_failures=technical_failures,
            infra_failures=infra_failures,
            complete=all_reports_present,
        )
        engine_revision = (
            next(iter(engine_revisions)) if len(engine_revisions) == 1 else None
        )
        suite_release, suite_stack = stack_metadata(config, None)
        if len(release_observations) == 1:
            suite_release = release_observations[0]
        if len(stack_observations) == 1:
            suite_stack = stack_observations[0]
        suite_extra = {
            "execution": execution,
            "lane": config.lane,
            "generated_at": config.generated_at,
            "source": {
                "sha": config.source_sha,
                "ref": config.source_ref,
                "repository": config.repository,
            },
            "workflow_url": config.workflow_url,
            "release": suite_release,
            "stack": suite_stack,
            "subject": subject,
            "judge": resolved_judge
            or {"model": config.judge_model, "provider": config.judge_provider},
            "engine_revision": engine_revision,
            "scenario": "suite",
            "requested_runs": config.requested_runs,
            "passed": suite_passed,
            "status": suite_status,
            "expected_reports": expected_count,
            "received_reports": report_count,
            "infra_failures": infra_failures,
        }
        quality.extend(
            [
                metric(
                    "quality",
                    subject["id"],
                    "suite",
                    "scenario_pass_rate",
                    "percent",
                    scenario_pass_rate,
                    suite_extra,
                ),
                metric(
                    "quality",
                    subject["id"],
                    "suite",
                    "report_coverage",
                    "percent",
                    report_coverage,
                    suite_extra,
                ),
            ]
        )
        efficiency.extend(
            [
                metric(
                    "reliability",
                    subject["id"],
                    "suite",
                    "hard_gate_failures",
                    "count",
                    hard_gate_failures,
                    suite_extra,
                ),
                metric(
                    "reliability",
                    subject["id"],
                    "suite",
                    "technical_failures",
                    "count",
                    technical_failures,
                    suite_extra,
                ),
                metric(
                    "reliability",
                    subject["id"],
                    "suite",
                    "retry_attempts",
                    "count",
                    retries,
                    suite_extra,
                ),
                metric(
                    "reliability",
                    subject["id"],
                    "suite",
                    "missing_reports",
                    "count",
                    missing_reports,
                    suite_extra,
                ),
                metric(
                    "reliability",
                    subject["id"],
                    "suite",
                    "infra_failed",
                    "count",
                    infra_failures,
                    suite_extra,
                ),
            ]
        )
        if total_cost is not None:
            efficiency.append(
                metric(
                    "efficiency",
                    subject["id"],
                    "suite",
                    "total_cost_usd",
                    "USD",
                    total_cost,
                    suite_extra,
                )
            )
        if total_wall_time is not None:
            efficiency.append(
                metric(
                    "efficiency",
                    subject["id"],
                    "suite",
                    "wall_time_seconds",
                    "seconds",
                    total_wall_time,
                    suite_extra,
                )
            )
        if total_context_compactions is not None:
            efficiency.append(
                metric(
                    "efficiency",
                    subject["id"],
                    "suite",
                    "context_compactions",
                    "count",
                    total_context_compactions,
                    suite_extra,
                )
            )

        snapshot_subjects.append(
            {
                **subject,
                "judge": suite_extra["judge"],
                "engine_revision": engine_revision,
                "passed": suite_passed,
                "expected_reports": expected_count,
                "received_reports": report_count,
                "scenario_pass_rate": scenario_pass_rate / 100,
                "report_coverage": report_coverage / 100,
                "hard_gate_failures": hard_gate_failures,
                "technical_failures": technical_failures,
                "infra_failures": infra_failures,
                "retry_attempts": retries,
                "total_cost_usd": total_cost,
                "wall_time_seconds": total_wall_time,
                "context_compactions": total_context_compactions,
                "scenarios": scenario_snapshots,
            }
        )

    snapshot_release, snapshot_stack = stack_metadata(config, None)
    if len(release_observations) == 1:
        snapshot_release = release_observations[0]
    if len(stack_observations) == 1:
        snapshot_stack = stack_observations[0]
    snapshot = {
        "execution": execution,
        "generated_at": config.generated_at,
        "lane": config.lane,
        "source": {
            "sha": config.source_sha,
            "ref": config.source_ref,
            "repository": config.repository,
        },
        "workflow_url": config.workflow_url,
        "release": snapshot_release,
        "stack": snapshot_stack,
        "stack_observations": stack_observations,
        "system_under_test_observations": system_observations,
        "requested_runs": config.requested_runs,
        "subjects": snapshot_subjects,
    }
    execution_detail = {
        **snapshot,
        "reports": execution_reports,
    }
    return quality, efficiency, snapshot, execution_detail


def write_outputs(
    output_dir: Path,
    quality: list[dict[str, Any]],
    efficiency: list[dict[str, Any]],
    snapshot: dict[str, Any],
    execution: dict[str, Any],
) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    for name, payload in (
        ("quality.json", quality),
        ("efficiency.json", efficiency),
        ("snapshot.json", snapshot),
        ("execution.json", execution),
    ):
        (output_dir / name).write_text(
            json.dumps(payload, indent=2, sort_keys=True) + "\n"
        )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    parser.add_argument("--reports-root", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--subjects-json", required=True)
    parser.add_argument("--scenarios-json", required=True)
    parser.add_argument("--lane", required=True)
    parser.add_argument("--requested-runs", type=int, required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--source-ref", default="")
    parser.add_argument("--repository", required=True)
    parser.add_argument("--workflow-url", required=True)
    parser.add_argument("--release-tag", default="")
    parser.add_argument("--release-worker", default="")
    parser.add_argument("--release-version", default="")
    parser.add_argument("--release-url", default="")
    parser.add_argument("--registry-tag", default="")
    parser.add_argument("--stack-mode", default="source")
    parser.add_argument("--stack-versions", default="{}")
    parser.add_argument("--stack-digest", default="")
    parser.add_argument("--judge-model", required=True)
    parser.add_argument("--judge-provider", required=True)
    parser.add_argument("--execution-run-id", required=True)
    parser.add_argument("--execution-attempt", type=int, required=True)
    parser.add_argument("--execution-event", default="")
    parser.add_argument("--execution-actor", default="")
    parser.add_argument("--generated-at")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.requested_runs < 1:
        raise CollectionError("requested runs must be positive")
    if args.execution_attempt < 1:
        raise CollectionError("execution attempt must be positive")
    generated_at = args.generated_at or datetime.now(timezone.utc).isoformat()
    config = CollectionConfig(
        reports_root=args.reports_root,
        output_dir=args.output_dir,
        subjects=parse_subjects(args.subjects_json),
        scenarios=parse_scenarios(args.scenarios_json),
        lane=require_string(args.lane, "lane"),
        requested_runs=args.requested_runs,
        source_sha=require_string(args.source_sha, "source SHA"),
        source_ref=args.source_ref,
        repository=require_string(args.repository, "repository"),
        workflow_url=require_string(args.workflow_url, "workflow URL"),
        release_tag=args.release_tag,
        release_worker=args.release_worker,
        release_version=args.release_version,
        release_url=args.release_url,
        registry_tag=args.registry_tag,
        judge_model=require_string(args.judge_model, "judge model"),
        judge_provider=require_string(args.judge_provider, "judge provider"),
        execution_run_id=require_string(args.execution_run_id, "execution run id"),
        execution_attempt=args.execution_attempt,
        execution_event=args.execution_event,
        execution_actor=args.execution_actor,
        generated_at=generated_at,
        stack_mode=require_string(args.stack_mode, "stack mode"),
        stack_versions=parse_stack_versions(args.stack_versions),
        stack_digest=args.stack_digest,
    )
    quality, efficiency, snapshot, execution = collect(config)
    write_outputs(args.output_dir, quality, efficiency, snapshot, execution)
    print(
        json.dumps(
            {
                "quality_metrics": len(quality),
                "efficiency_metrics": len(efficiency),
                "subjects": len(snapshot["subjects"]),
            }
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
