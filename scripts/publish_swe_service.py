#!/usr/bin/env python3
"""Publish a bounded, sanitized SWE journey result as one draft pull request."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import pathlib
import re
import shlex
import subprocess
import sys
from typing import Any
from urllib.parse import parse_qsl, urlencode, urlparse


ALLOWED_REPOSITORY = "iii-hq/e2e-fixture"
SCHEMA = "swe-service-report/v1"
SCENARIO_ID = "swe_service_journey"
SAFE_EXECUTION_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,95}$")
SAFE_RUN_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$")
FULL_SHA = re.compile(r"^[0-9a-f]{40}$")
MAX_REPORT_BYTES = 16 * 1024 * 1024
MAX_PATCH_BYTES = 2 * 1024 * 1024
MAX_CHECKPOINTS = 64
MAX_ELAPSED_MS = 24 * 60 * 60 * 1000
TERMINAL_STATUSES = {
    "completed",
    "capability_failure",
    "resource_limit",
    "cancelled",
    "infrastructure_error",
    "deadline",
}
TICKET_IDS = {
    1: "swe_config_isolation",
    2: "swe_cache_invalidation",
    3: "swe_batch_replay",
    4: "swe_replay_recovery",
    5: "swe_contract_migration",
    6: "swe_tenant_isolation",
    7: "swe_replay_performance",
    8: "swe_release_handoff",
}
SECRET_PATTERNS = (
    re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----"),
    re.compile(r"\bgh[pousr]_[A-Za-z0-9_]{20,}\b"),
    re.compile(r"\bAKIA[0-9A-Z]{16}\b"),
    re.compile(r"(?i)\bauthorization\s*:\s*bearer\s+\S+"),
    re.compile(
        r"(?i)\b(?:api[_-]?key|api[_-]?token|access[_-]?token|password|secret)\b"
        r"\s*[:=]\s*['\"]?(?!<|\$\{|example|redacted|placeholder)[^\s'\"]{8,}"
    ),
)
ABSOLUTE_PATH = re.compile(
    r"(?:(?<![A-Za-z0-9_.-])/(?:Users|home|private|tmp|var|etc)/[^\s'\"]+|"
    r"\b[A-Za-z]:\\[^\r\n]+)"
)


class PublishError(RuntimeError):
    """Raised when publication input or GitHub state is unsafe."""


def gh(
    method: str,
    endpoint: str,
    payload: dict[str, Any] | None = None,
    *,
    allow_not_found: bool = False,
) -> Any:
    """Call the GitHub API without a shell, sending mutation data as JSON stdin."""

    command = ["gh", "api", "--method", method, endpoint]
    encoded = None
    if payload is not None:
        command.extend(["--input", "-"])
        encoded = json.dumps(payload, separators=(",", ":"), sort_keys=True)
    completed = subprocess.run(
        command,
        input=encoded,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        stderr = completed.stderr.strip()
        if allow_not_found and ("HTTP 404" in stderr or "status 404" in stderr.lower()):
            return None
        raise PublishError(f"gh api {method} {endpoint} failed: {stderr}")
    if not completed.stdout.strip():
        return None
    try:
        return json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise PublishError(f"gh api {method} {endpoint} returned invalid JSON") from error


def _is_plain_int(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _require_sha(value: Any, field: str) -> str:
    if not isinstance(value, str) or not FULL_SHA.fullmatch(value):
        raise PublishError(f"{field} must be a full lowercase Git SHA")
    return value


def _require_bounded_int(
    value: Any, field: str, *, minimum: int, maximum: int
) -> int:
    if not _is_plain_int(value) or not minimum <= value <= maximum:
        raise PublishError(f"{field} must be an integer from {minimum} to {maximum}")
    return value


def _validate_patch_path(raw_path: str) -> None:
    if raw_path == "/dev/null":
        return
    if raw_path.startswith(("a/", "b/")):
        raw_path = raw_path[2:]
    path = pathlib.PurePosixPath(raw_path)
    if (
        not raw_path
        or raw_path.startswith("/")
        or "\\" in raw_path
        or "\x00" in raw_path
        or any(part in {"", ".", ".."} for part in path.parts)
    ):
        raise PublishError(f"unsafe patch path: {raw_path!r}")


def _header_path(line: str, prefix: str) -> str:
    value = line[len(prefix) :].strip()
    if prefix in {"--- ", "+++ "}:
        value = value.split("\t", 1)[0]
    try:
        pieces = shlex.split(value)
    except ValueError as error:
        raise PublishError("unsafe patch path quoting") from error
    if len(pieces) != 1:
        raise PublishError("unsafe patch path header")
    return pieces[0]


def _validate_public_text(value: str, field: str) -> None:
    if "\x00" in value or any(ord(character) < 9 for character in value):
        raise PublishError(f"{field} contains control characters")
    if ABSOLUTE_PATH.search(value):
        raise PublishError(f"{field} contains an absolute path")
    if any(pattern.search(value) for pattern in SECRET_PATTERNS):
        raise PublishError(f"{field} contains a possible secret")


def _validate_patch(patch: Any, accepted_tickets: list[int]) -> str:
    if not isinstance(patch, str):
        raise PublishError("accepted_patch must be text")
    if len(patch.encode("utf-8")) > MAX_PATCH_BYTES:
        raise PublishError("accepted_patch exceeds the publication limit")
    if not accepted_tickets and patch:
        raise PublishError("accepted_patch must be empty when no tickets were accepted")
    _validate_public_text(patch, "accepted_patch")
    prefixes = ("--- ", "+++ ", "rename from ", "rename to ", "copy from ", "copy to ")
    for line in patch.splitlines():
        if line.startswith("diff --git "):
            try:
                pieces = shlex.split(line)
            except ValueError as error:
                raise PublishError("unsafe patch path quoting") from error
            if len(pieces) != 4:
                raise PublishError("unsafe patch path header")
            _validate_patch_path(pieces[2])
            _validate_patch_path(pieces[3])
        elif line.startswith(prefixes):
            prefix = next(item for item in prefixes if line.startswith(item))
            _validate_patch_path(_header_path(line, prefix))
    return patch


def _validate_checkpoint(record: Any, index: int) -> dict[str, Any]:
    if not isinstance(record, dict):
        raise PublishError(f"checkpoint {index} must be an object")
    ticket = _require_bounded_int(record.get("ticket"), f"checkpoint {index} ticket", minimum=1, maximum=8)
    ticket_id = record.get("id")
    if ticket_id != TICKET_IDS[ticket]:
        raise PublishError(f"checkpoint {index} id does not match ticket {ticket}")
    if not isinstance(record.get("accepted"), bool):
        raise PublishError(f"checkpoint {index} accepted must be boolean")
    accepted = record["accepted"]
    submitted_head = record.get("head_sha")
    head_valid = isinstance(submitted_head, str) and FULL_SHA.fullmatch(submitted_head)
    if accepted and not head_valid:
        raise PublishError(
            f"accepted checkpoint {index} head_sha must be a full lowercase Git SHA"
        )
    return {
        "ticket": ticket,
        "id": ticket_id,
        "head_sha": submitted_head if head_valid else None,
        "head_status": "valid" if head_valid else "invalid",
        "accepted": accepted,
        "attempt": _require_bounded_int(
            record.get("attempt"), f"checkpoint {index} attempt", minimum=1, maximum=16
        ),
    }


def load_report(path: pathlib.Path) -> dict[str, Any]:
    """Load and validate the trusted controller's bounded journey report."""

    try:
        if path.stat().st_size > MAX_REPORT_BYTES:
            raise PublishError("SWE service report exceeds the publication limit")
        report = json.loads(path.read_text(encoding="utf-8"))
    except PublishError:
        raise
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise PublishError(f"cannot read SWE service report: {error}") from error
    if not isinstance(report, dict):
        raise PublishError("SWE service report must be an object")

    required = {
        "schema", "scenario_id", "mode", "fixture_revision", "run_id",
        "initial_head", "accepted_head", "accepted_tickets", "checkpoints",
        "terminal_status", "terminal_ticket", "elapsed_ms", "accepted_patch",
        "unaccepted_patch",
    }
    missing = sorted(required - set(report))
    if missing:
        raise PublishError(f"SWE service report is missing: {', '.join(missing)}")
    if report["schema"] != SCHEMA:
        raise PublishError(f"unsupported report schema; expected {SCHEMA}")
    if report["scenario_id"] != SCENARIO_ID:
        raise PublishError(f"publisher only accepts scenario {SCENARIO_ID}")
    if report["mode"] != "journey":
        raise PublishError("publisher only accepts journey mode")

    _require_sha(report["fixture_revision"], "fixture_revision")
    _require_sha(report["initial_head"], "initial_head")
    _require_sha(report["accepted_head"], "accepted_head")
    if not isinstance(report["run_id"], str) or not SAFE_RUN_ID.fullmatch(report["run_id"]):
        raise PublishError("run_id is not a safe bounded identifier")

    accepted_tickets = report["accepted_tickets"]
    if (
        not isinstance(accepted_tickets, list)
        or len(accepted_tickets) > 8
        or any(not _is_plain_int(ticket) for ticket in accepted_tickets)
        or accepted_tickets != list(range(1, len(accepted_tickets) + 1))
    ):
        raise PublishError("accepted_tickets must be an ordered prefix from ticket 1")

    raw_checkpoints = report["checkpoints"]
    if not isinstance(raw_checkpoints, list) or len(raw_checkpoints) > MAX_CHECKPOINTS:
        raise PublishError(f"checkpoints must contain at most {MAX_CHECKPOINTS} records")
    checkpoints = [
        _validate_checkpoint(record, index)
        for index, record in enumerate(raw_checkpoints, start=1)
    ]
    tickets_in_order = [record["ticket"] for record in checkpoints]
    if tickets_in_order != sorted(tickets_in_order):
        raise PublishError("checkpoints must be ordered by ticket")
    attempts_by_ticket: dict[int, list[int]] = {}
    for record in checkpoints:
        attempts_by_ticket.setdefault(record["ticket"], []).append(record["attempt"])
    if any(attempts != sorted(set(attempts)) for attempts in attempts_by_ticket.values()):
        raise PublishError("checkpoint attempts must be unique and ordered per ticket")
    accepted_records = [record for record in checkpoints if record["accepted"]]
    if [record["ticket"] for record in accepted_records] != accepted_tickets:
        raise PublishError("accepted checkpoints do not match accepted_tickets")
    expected_head = accepted_records[-1]["head_sha"] if accepted_records else report["initial_head"]
    if report["accepted_head"] != expected_head:
        raise PublishError("accepted_head does not match the accepted checkpoint prefix")

    if report["terminal_status"] not in TERMINAL_STATUSES:
        raise PublishError("unsupported terminal_status")
    terminal_ticket = report["terminal_ticket"]
    if terminal_ticket is not None:
        _require_bounded_int(terminal_ticket, "terminal_ticket", minimum=1, maximum=8)
    _require_bounded_int(report["elapsed_ms"], "elapsed_ms", minimum=0, maximum=MAX_ELAPSED_MS)
    _validate_patch(report["accepted_patch"], accepted_tickets)
    if not isinstance(report["unaccepted_patch"], str):
        raise PublishError("unaccepted_patch must be text")
    return {**report, "checkpoints": checkpoints}


def _canonical_projection(report: dict[str, Any], execution_id: str) -> dict[str, Any]:
    checkpoints = [
        _validate_checkpoint(record, index)
        for index, record in enumerate(report["checkpoints"], start=1)
    ]
    return {
        "schema": SCHEMA,
        "scenario_id": SCENARIO_ID,
        "mode": "journey",
        "execution_id": execution_id,
        "fixture_revision": report["fixture_revision"],
        "run_id": report["run_id"],
        "initial_head": report["initial_head"],
        "accepted_head": report["accepted_head"],
        "accepted_tickets": report["accepted_tickets"],
        "checkpoints": [
            {
                "ticket": record["ticket"], "id": record["id"],
                "head_sha": record["head_sha"], "head_status": record["head_status"],
                "accepted": record["accepted"],
                "attempt": record["attempt"],
            }
            for record in checkpoints
        ],
        "terminal_status": report["terminal_status"],
        "terminal_ticket": report["terminal_ticket"],
        "elapsed_ms": report["elapsed_ms"],
        "hidden_probe_details": "retained only in the trusted Harness evidence archive",
    }


def public_projection(report: dict[str, Any], execution_id: str) -> dict[str, Any]:
    """Return the explicit public whitelist and its accepted-evidence digest."""

    projection = _canonical_projection(report, execution_id)
    canonical = json.dumps(
        projection, separators=(",", ":"), sort_keys=True, ensure_ascii=False
    ).encode("utf-8")
    patch = report["accepted_patch"].encode("utf-8")
    report_digest = hashlib.sha256(canonical).hexdigest()
    patch_digest = hashlib.sha256(patch).hexdigest()
    digest = hashlib.sha256(
        len(canonical).to_bytes(8, "big") + canonical + len(patch).to_bytes(8, "big") + patch
    ).hexdigest()
    return {
        **projection,
        "report_digest": f"sha256:{report_digest}",
        "accepted_patch_digest": f"sha256:{patch_digest}",
        "publication_digest": f"sha256:{digest}",
    }


def create_blob(repository: str, content: bytes) -> str:
    response = gh(
        "POST", f"repos/{repository}/git/blobs",
        {"content": base64.b64encode(content).decode("ascii"), "encoding": "base64"},
    )
    if not isinstance(response, dict) or not FULL_SHA.fullmatch(str(response.get("sha", ""))):
        raise PublishError("GitHub returned an invalid blob SHA")
    return str(response["sha"])


def _decode_content(response: Any, path: str) -> bytes:
    if not isinstance(response, dict) or response.get("encoding") != "base64":
        raise PublishError(f"existing {path} has an unsupported GitHub representation")
    try:
        encoded = response["content"]
        if not isinstance(encoded, str):
            raise TypeError("content must be text")
        return base64.b64decode("".join(encoded.split()), validate=True)
    except (KeyError, TypeError, ValueError) as error:
        raise PublishError(f"existing {path} is not valid base64") from error


def _matching_pulls(repository: str, owner: str, branch: str) -> list[dict[str, Any]]:
    query = urlencode({"state": "all", "head": f"{owner}:{branch}", "per_page": "100"})
    response = gh("GET", f"repos/{repository}/pulls?{query}")
    if not isinstance(response, list):
        raise PublishError("GitHub returned an invalid pull request listing")
    return [
        pull for pull in response
        if isinstance(pull, dict)
        and isinstance(pull.get("head"), dict)
        and pull["head"].get("ref") == branch
    ]


def _validate_run_url(run_url: str) -> None:
    if not isinstance(run_url, str) or len(run_url) > 2048:
        raise PublishError("run URL is not bounded")
    if not run_url:
        return
    parsed = urlparse(run_url)
    if (
        parsed.scheme not in {"http", "https"} or not parsed.netloc
        or parsed.username is not None or parsed.password is not None
    ):
        raise PublishError("run URL must be an HTTP(S) URL without credentials")
    sensitive_query_keys = {
        "access_token", "api_key", "api_token", "auth", "authorization",
        "credential", "key", "password", "secret", "signature", "token",
    }
    if any(key.lower() in sensitive_query_keys for key, _ in parse_qsl(parsed.query)):
        raise PublishError("run URL contains a sensitive query parameter")
    _validate_public_text(run_url, "run URL")


def _create_draft_pull(
    *,
    repository: str,
    branch: str,
    default_branch: str,
    execution_id: str,
    report: dict[str, Any],
    projection: dict[str, Any],
    run_url: str,
) -> dict[str, Any]:
    accepted = len(report["accepted_tickets"])
    source_line = f"- Source run: {run_url}\n" if run_url else ""
    body = (
        "SWE service journey result.\n\n"
        f"- Accepted tickets: `{accepted}/8`\n"
        f"- Terminal status: `{report['terminal_status']}`\n"
        f"- Runtime: `{report['elapsed_ms']} ms`\n"
        f"{source_line}"
        f"- Evidence digest: `{projection['publication_digest']}`\n\n"
        "Hidden probe details and unaccepted changes remain in the trusted Harness evidence archive. "
        "This pull request contains the sanitized report and accepted patch."
    )
    pull = gh(
        "POST", f"repos/{repository}/pulls",
        {
            "title": f"SWE service result: {execution_id} ({accepted}/8)",
            "head": branch, "base": default_branch, "body": body, "draft": True,
        },
    )
    if not isinstance(pull, dict) or not isinstance(pull.get("html_url"), str):
        raise PublishError("GitHub returned an invalid pull request")
    return pull


def publish(
    *, repository: str, execution_id: str, report_path: pathlib.Path, run_url: str
) -> dict[str, Any]:
    if repository != ALLOWED_REPOSITORY:
        raise PublishError(f"repository must be exactly {ALLOWED_REPOSITORY}")
    if not isinstance(execution_id, str) or not SAFE_EXECUTION_ID.fullmatch(execution_id):
        raise PublishError("execution id is not a safe bounded identifier")
    _validate_run_url(run_url)
    report = load_report(report_path)
    projection = public_projection(report, execution_id)
    report_bytes = (json.dumps(projection, indent=2, sort_keys=True) + "\n").encode("utf-8")
    patch_bytes = report["accepted_patch"].encode("utf-8")

    repository_data = gh("GET", f"repos/{repository}")
    if not isinstance(repository_data, dict):
        raise PublishError("GitHub returned invalid repository metadata")
    default_branch = repository_data.get("default_branch")
    owner = repository_data.get("owner", {}).get("login")
    if not isinstance(default_branch, str) or not SAFE_RUN_ID.fullmatch(default_branch):
        raise PublishError("repository default branch is unsafe")
    if not isinstance(owner, str) or not SAFE_RUN_ID.fullmatch(owner):
        raise PublishError("repository owner is unsafe")

    branch = f"feat/swe-result-{execution_id}"
    result_root = f"benchmark-runs/swe/{execution_id}"
    branch_ref = gh(
        "GET", f"repos/{repository}/git/ref/heads/{branch}", allow_not_found=True
    )
    if branch_ref is not None:
        head_sha = _require_sha(
            str(branch_ref.get("object", {}).get("sha", "")),
            "existing branch head",
        )
        report_response = gh(
            "GET", f"repos/{repository}/contents/{result_root}/report.json?ref={branch}"
        )
        patch_response = gh(
            "GET", f"repos/{repository}/contents/{result_root}/accepted.patch?ref={branch}"
        )
        existing_report = _decode_content(report_response, "report.json")
        existing_patch = _decode_content(patch_response, "accepted.patch")
        try:
            existing_projection = json.loads(existing_report)
            existing_report_digest = existing_projection["report_digest"]
            existing_digest = existing_projection["publication_digest"]
        except (json.JSONDecodeError, KeyError, TypeError) as error:
            raise PublishError("existing result has no valid report digest") from error
        if (
            existing_report_digest != projection["report_digest"]
            or existing_digest != projection["publication_digest"]
            or existing_report != report_bytes or existing_patch != patch_bytes
        ):
            raise PublishError("existing branch contains conflicting evidence")
        pulls = _matching_pulls(repository, owner, branch)
        if len(pulls) > 1:
            raise PublishError("matching evidence has more than one pull request")
        if pulls:
            pull = pulls[0]
            if pull.get("draft") is not True or pull.get("base", {}).get("ref") != default_branch:
                raise PublishError("existing result pull request is not the expected draft")
            reused = True
        else:
            pull = _create_draft_pull(
                repository=repository,
                branch=branch,
                default_branch=default_branch,
                execution_id=execution_id,
                report=report,
                projection=projection,
                run_url=run_url,
            )
            reused = False
        return {
            "repository": repository, "branch": branch, "commit_sha": head_sha,
            "pull_request": pull["html_url"],
            "publication_digest": projection["publication_digest"],
            "accepted_tickets": report["accepted_tickets"],
            "terminal_status": report["terminal_status"], "reused": reused,
        }

    existing_pulls = _matching_pulls(repository, owner, branch)
    if existing_pulls:
        raise PublishError("pull request exists without its immutable result branch")
    base_ref = gh("GET", f"repos/{repository}/git/ref/heads/{default_branch}")
    base_sha = _require_sha(base_ref.get("object", {}).get("sha"), "base branch head")
    base_commit = gh("GET", f"repos/{repository}/git/commits/{base_sha}")
    base_tree = _require_sha(base_commit.get("tree", {}).get("sha"), "base tree")

    report_blob = create_blob(repository, report_bytes)
    patch_blob = create_blob(repository, patch_bytes)
    tree = gh(
        "POST", f"repos/{repository}/git/trees",
        {
            "base_tree": base_tree,
            "tree": [
                {"path": f"{result_root}/report.json", "mode": "100644", "type": "blob", "sha": report_blob},
                {"path": f"{result_root}/accepted.patch", "mode": "100644", "type": "blob", "sha": patch_blob},
            ],
        },
    )
    tree_sha = _require_sha(tree.get("sha"), "result tree")
    commit = gh(
        "POST", f"repos/{repository}/git/commits",
        {"message": f"Record SWE service journey {execution_id}", "tree": tree_sha, "parents": [base_sha]},
    )
    head_sha = _require_sha(commit.get("sha"), "result commit")
    gh(
        "POST", f"repos/{repository}/git/refs",
        {"ref": f"refs/heads/{branch}", "sha": head_sha},
    )

    pull = _create_draft_pull(
        repository=repository,
        branch=branch,
        default_branch=default_branch,
        execution_id=execution_id,
        report=report,
        projection=projection,
        run_url=run_url,
    )
    return {
        "repository": repository, "branch": branch, "commit_sha": head_sha,
        "pull_request": pull["html_url"],
        "publication_digest": projection["publication_digest"],
        "accepted_tickets": report["accepted_tickets"],
        "terminal_status": report["terminal_status"], "reused": False,
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository", default=ALLOWED_REPOSITORY)
    parser.add_argument("--execution-id", required=True)
    parser.add_argument("--report", type=pathlib.Path, required=True)
    parser.add_argument("--run-url", default="")
    parser.add_argument("--output", type=pathlib.Path, required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        result = publish(
            repository=args.repository, execution_id=args.execution_id,
            report_path=args.report, run_url=args.run_url,
        )
        encoded = json.dumps(result, indent=2, sort_keys=True) + "\n"
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded, encoding="utf-8")
        print(encoded, end="")
        return 0
    except PublishError as error:
        print(f"publication failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
