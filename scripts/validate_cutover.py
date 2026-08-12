#!/usr/bin/env python3
"""Validate ordered Harness E2E cutover and immutable rollback evidence."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import pathlib
import re
from typing import Any


FULL_SHA = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^(?:sha256:)?[0-9a-f]{64}$")
VALID_MODES = {"legacy", "shadow", "new_path"}


class CutoverError(ValueError):
    pass


def load_json(path: pathlib.Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise CutoverError(f"{path} must contain a JSON object")
    return value


def parse_time(value: Any, label: str) -> dt.datetime:
    if not isinstance(value, str) or not value:
        raise CutoverError(f"{label} must be an RFC 3339 timestamp")
    try:
        parsed = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise CutoverError(f"{label} must be an RFC 3339 timestamp") from error
    if parsed.tzinfo is None:
        raise CutoverError(f"{label} must include a timezone")
    return parsed


def validate_policy(policy: dict[str, Any]) -> None:
    if policy.get("schema_version") != 1:
        raise CutoverError("cutover policy must use schema_version 1")
    lanes = policy.get("lane_order")
    if lanes != ["pull_request", "main", "daily", "release"]:
        raise CutoverError("lane_order must preserve pull_request, main, daily, release")
    canonical = policy.get("canonical") or {}
    if canonical.get("entrypoint") != "e2e::run":
        raise CutoverError("canonical entrypoint must be e2e::run")
    if canonical.get("results_schema_version") != 2:
        raise CutoverError("canonical results schema must be version 2")
    if canonical.get("archive_scheme") != "iii-storage":
        raise CutoverError("canonical archives must use iii-storage")
    required = canonical.get("required_identity_fields")
    if required != ["execution_id", "lane", "subject_revision", "e2e_revision"]:
        raise CutoverError("canonical identity field policy is incomplete")
    if not isinstance(policy.get("minimum_shadow_windows"), int) or policy["minimum_shadow_windows"] < 3:
        raise CutoverError("minimum_shadow_windows must be at least 3")
    if not isinstance(policy.get("rollback_window_days"), int) or policy["rollback_window_days"] < 1:
        raise CutoverError("rollback_window_days must be positive")
    legacy = policy.get("legacy") or {}
    if not str(legacy.get("workflow", "")).startswith(".github/workflows/"):
        raise CutoverError("legacy workflow must be an explicit workflow path")
    if not str(legacy.get("immutable_tag_prefix", "")).startswith("refs/tags/"):
        raise CutoverError("legacy rollback prefix must require an immutable tag")


def evaluate_cutover(
    policy: dict[str, Any],
    evidence: dict[str, Any],
    required_stage: str,
    evaluated_at: dt.datetime | None = None,
) -> dict[str, Any]:
    validate_policy(policy)
    if evidence.get("schema_version") != 1:
        raise CutoverError("cutover evidence must use schema_version 1")
    evaluated_at = evaluated_at or dt.datetime.now(dt.timezone.utc)
    if evaluated_at.tzinfo is None:
        raise CutoverError("evaluated_at must include a timezone")
    subject_revision = evidence.get("subject_revision")
    e2e_revision = evidence.get("e2e_revision")
    if not isinstance(subject_revision, str) or not FULL_SHA.fullmatch(subject_revision):
        raise CutoverError("subject_revision must be a full immutable Git SHA")
    if not isinstance(e2e_revision, str) or not FULL_SHA.fullmatch(e2e_revision):
        raise CutoverError("e2e_revision must be a full immutable Git SHA")

    lane_order: list[str] = policy["lane_order"]
    stages = lane_order + ["legacy_removal"]
    if required_stage not in stages:
        raise CutoverError(f"unsupported required stage {required_stage!r}")
    raw_lanes = evidence.get("lanes")
    if not isinstance(raw_lanes, list):
        raise CutoverError("cutover evidence lanes must be a list")
    lanes: dict[str, dict[str, Any]] = {}
    for lane in raw_lanes:
        if not isinstance(lane, dict) or lane.get("lane") not in lane_order:
            raise CutoverError("cutover evidence contains an unknown lane")
        lane_id = lane["lane"]
        if lane_id in lanes:
            raise CutoverError(f"cutover evidence repeats lane {lane_id}")
        if lane.get("mode") not in VALID_MODES:
            raise CutoverError(f"lane {lane_id} has an unsupported mode")
        lanes[lane_id] = lane
    if set(lanes) != set(lane_order):
        raise CutoverError("cutover evidence must include every lane exactly once")

    reasons: list[str] = []
    shadow_windows = evidence.get("shadow_windows")
    if not isinstance(shadow_windows, list):
        raise CutoverError("shadow_windows must be a list")
    ordered_windows: list[tuple[int, dt.datetime, str, str, int, bool]] = []
    seen_sequences: set[int] = set()
    for window in shadow_windows:
        if not isinstance(window, dict):
            raise CutoverError("shadow window must be an object")
        revision = window.get("subject_revision")
        benchmark_revision = window.get("e2e_revision")
        seed = window.get("seed")
        sequence = window.get("sequence")
        if not isinstance(revision, str) or not FULL_SHA.fullmatch(revision):
            raise CutoverError("shadow window subject_revision must be a full Git SHA")
        if not isinstance(benchmark_revision, str) or not FULL_SHA.fullmatch(benchmark_revision):
            raise CutoverError("shadow window e2e_revision must be a full Git SHA")
        if not isinstance(seed, int) or seed < 0:
            raise CutoverError("shadow window seed must be a non-negative integer")
        if not isinstance(sequence, int) or sequence < 1 or sequence in seen_sequences:
            raise CutoverError("shadow window sequence must be a unique positive integer")
        seen_sequences.add(sequence)
        completed_at = parse_time(window.get("completed_at"), "shadow window completed_at")
        if not isinstance(window.get("equivalent"), bool):
            raise CutoverError("shadow window equivalent must be boolean")
        ordered_windows.append(
            (
                sequence,
                completed_at,
                revision,
                benchmark_revision,
                seed,
                window["equivalent"],
            )
        )
    ordered_windows.sort()
    sequences = [window[0] for window in ordered_windows]
    if sequences and sequences != list(range(sequences[0], sequences[-1] + 1)):
        raise CutoverError("shadow window sequence has a gap")
    completed_times = [window[1] for window in ordered_windows]
    if completed_times != sorted(completed_times):
        raise CutoverError("shadow window timestamps do not follow their sequence")
    trailing_equivalent: set[tuple[str, str, int]] = set()
    for _, _, revision, benchmark_revision, seed, equivalent in reversed(ordered_windows):
        if not equivalent:
            break
        trailing_equivalent.add((revision, benchmark_revision, seed))
    if len(trailing_equivalent) < policy["minimum_shadow_windows"]:
        reasons.append(
            f"only {len(trailing_equivalent)} consecutive equivalent shadow windows; "
            f"{policy['minimum_shadow_windows']} required"
        )

    target_index = lane_order.index(required_stage) if required_stage in lane_order else len(lane_order) - 1
    for index, lane_id in enumerate(lane_order):
        lane = lanes[lane_id]
        if index <= target_index and lane.get("mode") != "new_path":
            reasons.append(f"lane {lane_id} has not cut over to the new path")
        if lane.get("mode") in {"shadow", "new_path"}:
            reasons.extend(
                validate_canonical_lane(
                    policy,
                    lane,
                    subject_revision=subject_revision,
                    e2e_revision=e2e_revision,
                )
            )

    if required_stage == "legacy_removal":
        reasons.extend(validate_legacy_removal(policy, evidence, lanes, evaluated_at))

    result = {
        "schema_version": 1,
        "required_stage": required_stage,
        "evaluated_at": evaluated_at.isoformat().replace("+00:00", "Z"),
        "eligible": not reasons,
        "subject_revision": subject_revision,
        "e2e_revision": e2e_revision,
        "validated_lanes": [
            {"lane": lane_id, "mode": lanes[lane_id]["mode"]} for lane_id in lane_order
        ],
        "equivalent_shadow_windows": len(trailing_equivalent),
        "reasons": reasons or ["ordered cutover evidence satisfies policy"],
    }
    canonical = json.dumps(result, sort_keys=True, separators=(",", ":")).encode()
    result["evaluation_sha256"] = hashlib.sha256(canonical).hexdigest()
    return result


def validate_canonical_lane(
    policy: dict[str, Any],
    lane: dict[str, Any],
    *,
    subject_revision: str,
    e2e_revision: str,
) -> list[str]:
    reasons: list[str] = []
    lane_id = lane["lane"]
    canonical = policy["canonical"]
    if lane.get("entrypoint") != canonical["entrypoint"]:
        reasons.append(f"lane {lane_id} does not use e2e::run")
    if lane.get("results_schema_version") != canonical["results_schema_version"]:
        reasons.append(f"lane {lane_id} does not publish canonical results v2")
    identity = lane.get("identity")
    if not isinstance(identity, dict):
        reasons.append(f"lane {lane_id} has no canonical identity")
    else:
        expected = {
            "lane": lane_id,
            "subject_revision": subject_revision,
            "e2e_revision": e2e_revision,
        }
        if not isinstance(identity.get("execution_id"), str) or not identity["execution_id"]:
            reasons.append(f"lane {lane_id} has no execution_id")
        for field, value in expected.items():
            if identity.get(field) != value:
                reasons.append(f"lane {lane_id} identity {field} differs from cutover evidence")
    archive = lane.get("archive")
    if not isinstance(archive, dict):
        reasons.append(f"lane {lane_id} has no durable archive proof")
    else:
        uri = archive.get("uri")
        digest = archive.get("sha256")
        if archive.get("availability") != "available":
            reasons.append(f"lane {lane_id} archive is not available")
        if not isinstance(digest, str) or not SHA256.fullmatch(digest):
            reasons.append(f"lane {lane_id} archive has no valid SHA-256")
        normalized = digest.removeprefix("sha256:") if isinstance(digest, str) else ""
        if not isinstance(uri, str) or not uri.startswith(f"{canonical['archive_scheme']}://"):
            reasons.append(f"lane {lane_id} archive URI is not iii-storage")
        elif f"sha256={normalized}" not in uri:
            reasons.append(f"lane {lane_id} archive URI is not bound to its SHA-256")
    return reasons


def validate_legacy_removal(
    policy: dict[str, Any],
    evidence: dict[str, Any],
    lanes: dict[str, dict[str, Any]],
    evaluated_at: dt.datetime,
) -> list[str]:
    reasons: list[str] = []
    if any(lane.get("mode") != "new_path" for lane in lanes.values()):
        reasons.append("all lanes must use the new path before legacy removal")
    consumers = evidence.get("consumers")
    if not isinstance(consumers, list) or not consumers:
        return reasons + ["active-consumer inventory is missing"]
    if any(not isinstance(consumer, dict) for consumer in consumers):
        return reasons + ["active-consumer inventory contains an invalid entry"]
    active_legacy = [
        str(consumer.get("id", "unknown"))
        for consumer in consumers
        if consumer.get("active") is True
        and (
            consumer.get("entrypoint") != policy["canonical"]["entrypoint"]
            or consumer.get("results_schema_version") != policy["canonical"]["results_schema_version"]
        )
    ]
    if active_legacy:
        reasons.append("active legacy consumers remain: " + ", ".join(sorted(active_legacy)))
    rollback = evidence.get("rollback")
    if not isinstance(rollback, dict):
        return reasons + ["rollback evidence is missing"]
    prefix = policy["legacy"]["immutable_tag_prefix"]
    if not str(rollback.get("legacy_ref", "")).startswith(prefix):
        reasons.append("legacy rollback ref is not the required immutable tag")
    if not isinstance(rollback.get("legacy_revision"), str) or not FULL_SHA.fullmatch(
        rollback["legacy_revision"]
    ):
        reasons.append("legacy rollback revision must be a full Git SHA")
    started = parse_time(rollback.get("window_started_at"), "rollback.window_started_at")
    ends = parse_time(rollback.get("window_ends_at"), "rollback.window_ends_at")
    required_end = started + dt.timedelta(days=policy["rollback_window_days"])
    if ends < required_end:
        reasons.append("rollback window is shorter than policy")
    if evaluated_at < ends:
        reasons.append("rollback window has not elapsed")
    drill = rollback.get("drill")
    if not isinstance(drill, dict) or drill.get("succeeded") is not True:
        reasons.append("rollback simulation has not succeeded")
    else:
        drill_at = parse_time(drill.get("executed_at"), "rollback.drill.executed_at")
        if drill_at < started or drill_at > evaluated_at:
            reasons.append("rollback simulation is outside the observed rollback window")
        if drill.get("restored_entrypoint") != policy["legacy"]["workflow"]:
            reasons.append("rollback simulation did not restore the declared legacy workflow")
        digest = drill.get("evidence_sha256")
        if not isinstance(digest, str) or not SHA256.fullmatch(digest):
            reasons.append("rollback simulation has no immutable evidence hash")
    return reasons


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--policy", type=pathlib.Path, required=True)
    parser.add_argument("--evidence", type=pathlib.Path, required=True)
    parser.add_argument(
        "--require-stage",
        choices=["pull_request", "main", "daily", "release", "legacy_removal"],
        required=True,
    )
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    try:
        result = evaluate_cutover(
            load_json(args.policy), load_json(args.evidence), args.require_stage
        )
    except (OSError, json.JSONDecodeError, CutoverError) as error:
        parser.error(str(error))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    if not result["eligible"]:
        for reason in result["reasons"]:
            print(f"cutover blocked: {reason}")
        return 1
    print(f"cutover stage {args.require_stage} is eligible")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
