#!/usr/bin/env python3
"""Publish native SWE journey reports and persist a bounded campaign receipt."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import re
import tempfile
from typing import Any, Callable

from publish_swe_service import (
    ALLOWED_REPOSITORY,
    SAFE_EXECUTION_ID,
    SAFE_RUN_ID,
    publish,
)


REPORT_NAME = "swe_service_report.json"
REPORT_SCHEMA = "swe-service-report/v1"
RECEIPT_SCHEMA = "swe-campaign-publication/v1"
MAX_REPORTS = 256
MAX_CLASSIFICATION_BYTES = 16 * 1024 * 1024
SAFE_FRAGMENT = re.compile(r"[^A-Za-z0-9_.-]+")


class CampaignPublishError(RuntimeError):
    """Raised when campaign-level publication input is unsafe."""


def _safe_error(error: Exception, reports_root: pathlib.Path) -> str:
    message = str(error).replace(str(reports_root), "[reports-dir]")
    message = re.sub(
        r"(?i)\b(token|secret|password|authorization)\b\s*[:=]\s*\S+",
        r"\1=[redacted]",
        message,
    )
    message = "".join(
        character
        for character in message
        if character in "\n\t" or ord(character) >= 32
    )
    return message[:512] or type(error).__name__


def _report_paths(reports_dir: pathlib.Path) -> list[pathlib.Path]:
    if not reports_dir.exists():
        return []
    if not reports_dir.is_dir():
        raise CampaignPublishError(
            "reports directory does not exist or is not a directory"
        )
    paths = sorted(
        reports_dir.rglob(REPORT_NAME),
        key=lambda path: path.relative_to(reports_dir).as_posix(),
    )
    if len(paths) > MAX_REPORTS:
        raise CampaignPublishError(
            f"reports directory contains more than {MAX_REPORTS} native reports"
        )
    return paths


def _inspect_report(
    path: pathlib.Path, reports_dir: pathlib.Path
) -> tuple[dict[str, Any], dict[str, Any] | None, str | None]:
    relative = path.relative_to(reports_dir).as_posix()
    receipt: dict[str, Any] = {"path": relative}
    try:
        if path.is_symlink():
            raise CampaignPublishError("native report must not be a symbolic link")
        if path.stat().st_size > MAX_CLASSIFICATION_BYTES:
            raise CampaignPublishError("native report exceeds the classification limit")
        raw = path.read_bytes()
        value = json.loads(raw)
        if not isinstance(value, dict):
            raise CampaignPublishError("native report must contain a JSON object")
    except (OSError, UnicodeError, json.JSONDecodeError, CampaignPublishError) as error:
        receipt.update(status="error", error=_safe_error(error, reports_dir))
        return receipt, None, None

    if value.get("schema") != REPORT_SCHEMA:
        receipt.update(status="ignored", reason="unsupported report schema")
        return receipt, None, None
    if value.get("mode") != "journey":
        receipt.update(status="ignored", reason="not a journey report")
        return receipt, None, None
    run_id = value.get("run_id")
    if isinstance(run_id, str) and SAFE_RUN_ID.fullmatch(run_id):
        receipt["run_id"] = run_id
    return receipt, value, hashlib.sha256(raw).hexdigest()


def _derived_execution_id(base: str, run_id: Any, index: int, total: int) -> str:
    if total == 1:
        return base
    raw_run_id = run_id if isinstance(run_id, str) else "invalid-run"
    fragment = SAFE_FRAGMENT.sub("-", raw_run_id).strip("-._")[:32] or "report"
    suffix = f"-{fragment}-{index:03d}"
    candidate = f"{base}{suffix}"
    if len(candidate) <= 96 and SAFE_EXECUTION_ID.fullmatch(candidate):
        return candidate
    digest = hashlib.sha256(
        f"{base}\0{raw_run_id}\0{index}".encode("utf-8", "replace")
    ).hexdigest()[:12]
    suffix = f"-{digest}-{index:03d}"
    candidate = f"{base[: 96 - len(suffix)]}{suffix}"
    if not SAFE_EXECUTION_ID.fullmatch(candidate):
        raise CampaignPublishError("could not derive a safe publication execution id")
    return candidate


def publish_campaign(
    *,
    reports_dir: pathlib.Path,
    execution_id: str,
    run_url: str,
    publish_one: Callable[..., dict[str, Any]] = publish,
) -> tuple[dict[str, Any], int]:
    if not SAFE_EXECUTION_ID.fullmatch(execution_id):
        raise CampaignPublishError("execution id is not a safe bounded identifier")

    paths = _report_paths(reports_dir)
    inspected = [_inspect_report(path, reports_dir) for path in paths]
    journey_total = sum(value is not None for _, value, _ in inspected)
    canonical_runs: dict[str, tuple[str, str]] = {}
    conflicting_runs: set[str] = set()
    for receipt, value, content_digest in inspected:
        if value is None or not isinstance(value.get("run_id"), str):
            continue
        run_id = value["run_id"]
        canonical = canonical_runs.get(run_id)
        if canonical is None:
            canonical_runs[run_id] = (content_digest or "", receipt["path"])
        elif canonical[0] == content_digest:
            receipt.update(status="duplicate", duplicate_of=canonical[1])
        else:
            conflicting_runs.add(run_id)

    if conflicting_runs:
        for receipt, value, _ in inspected:
            if value is not None:
                receipt.update(
                    status="error",
                    error=(
                        "conflicting duplicate run_id"
                        if value.get("run_id") in conflicting_runs
                        else "publication aborted because another run_id conflicts"
                    ),
                )
        receipts = [receipt for receipt, _, _ in inspected]
        error_count = sum(receipt["status"] == "error" for receipt in receipts)
        result = {
            "schema": RECEIPT_SCHEMA,
            "execution_id": execution_id,
            "status": "failed",
            "report_count": len(paths),
            "journey_count": journey_total,
            "published_count": 0,
            "ignored_count": sum(
                receipt["status"] == "ignored" for receipt in receipts
            ),
            "duplicate_count": 0,
            "error_count": error_count,
            "reports": receipts,
        }
        return result, 1

    duplicate_count = sum(
        receipt.get("status") == "duplicate" for receipt, _, _ in inspected
    )
    unique_journey_total = journey_total - duplicate_count
    journey_index = 0
    receipts: list[dict[str, Any]] = []
    for path, (receipt, value, _) in zip(paths, inspected, strict=True):
        if value is None:
            receipts.append(receipt)
            continue
        if receipt.get("status") == "duplicate":
            receipts.append(receipt)
            continue
        journey_index += 1
        publication_id = _derived_execution_id(
            execution_id, value.get("run_id"), journey_index, unique_journey_total
        )
        receipt["execution_id"] = publication_id
        try:
            receipt["publication"] = publish_one(
                repository=ALLOWED_REPOSITORY,
                execution_id=publication_id,
                report_path=path,
                run_url=run_url,
            )
            receipt["status"] = "published"
        except Exception as error:
            receipt.update(status="error", error=_safe_error(error, reports_dir))
        receipts.append(receipt)

    error_count = sum(receipt["status"] == "error" for receipt in receipts)
    published_count = sum(receipt["status"] == "published" for receipt in receipts)
    ignored_count = sum(receipt["status"] == "ignored" for receipt in receipts)
    if not paths or (journey_total == 0 and error_count == 0):
        status = "skipped"
    elif error_count and published_count:
        status = "partial_failure"
    elif error_count:
        status = "failed"
    else:
        status = "completed"
    result = {
        "schema": RECEIPT_SCHEMA,
        "execution_id": execution_id,
        "status": status,
        "report_count": len(paths),
        "journey_count": journey_total,
        "published_count": published_count,
        "ignored_count": ignored_count,
        "duplicate_count": duplicate_count,
        "error_count": error_count,
        "reports": receipts,
    }
    return result, 1 if error_count else 0


def _write_receipt(path: pathlib.Path, receipt: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
            json.dump(receipt, stream, indent=2, sort_keys=True)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--reports-dir", type=pathlib.Path, required=True)
    parser.add_argument("--execution-id", required=True)
    parser.add_argument("--run-url", default="")
    parser.add_argument("--output", type=pathlib.Path, required=True)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        receipt, exit_code = publish_campaign(
            reports_dir=args.reports_dir,
            execution_id=args.execution_id,
            run_url=args.run_url,
            publish_one=publish,
        )
    except Exception as error:
        receipt = {
            "schema": RECEIPT_SCHEMA,
            "execution_id": args.execution_id,
            "status": "failed",
            "report_count": 0,
            "journey_count": 0,
            "published_count": 0,
            "ignored_count": 0,
            "duplicate_count": 0,
            "error_count": 1,
            "reports": [],
            "error": _safe_error(error, args.reports_dir),
        }
        exit_code = 1
    _write_receipt(args.output, receipt)
    print(json.dumps(receipt, indent=2, sort_keys=True))
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
