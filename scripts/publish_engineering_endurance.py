#!/usr/bin/env python3
"""Publish a sanitized endurance run to iii-hq/e2e-fixture.

This program is intentionally a post-run trusted publisher. The evaluated
Harness session never receives GitHub credentials. Raw hidden-probe output stays
in the Harness evidence archive; GitHub receives only decisions, measurements,
the accepted production patch, and public checkpoint topology.
"""

from __future__ import annotations

import argparse
import base64
import json
import pathlib
import re
import subprocess
from typing import Any


ALLOWED_REPOSITORY = "iii-hq/e2e-fixture"
SAFE_EXECUTION_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$")


class PublishError(RuntimeError):
    pass


def gh(method: str, endpoint: str, payload: dict[str, Any] | None = None) -> Any:
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
        raise PublishError(
            f"gh api {method} {endpoint} failed: {completed.stderr.strip()}"
        )
    if not completed.stdout.strip():
        return None
    return json.loads(completed.stdout)


def load_report(path: pathlib.Path) -> dict[str, Any]:
    try:
        report = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise PublishError(f"cannot read endurance report {path}: {error}") from error
    required = {
        "scenario_version",
        "accepted_head",
        "accepted_rungs",
        "total_rungs",
        "terminal_status",
        "checkpoints",
        "accepted_patch",
        "measurements",
    }
    missing = sorted(required - set(report))
    if missing:
        raise PublishError(f"endurance report is missing: {', '.join(missing)}")
    if report["scenario_version"] != 1 or report["total_rungs"] != 10:
        raise PublishError("unsupported endurance report contract")
    if not isinstance(report["accepted_patch"], str):
        raise PublishError("accepted_patch must be text")
    return report


def public_projection(report: dict[str, Any], execution_id: str) -> dict[str, Any]:
    checkpoints = []
    for record in report["checkpoints"]:
        evidence = record.get("evidence", {})
        checkpoints.append(
            {
                "rung": record.get("rung"),
                "ticket_id": record.get("ticket_id"),
                "attempt": record.get("attempt"),
                "requested_head": record.get("requested_head"),
                "previous_accepted_head": record.get("previous_accepted_head"),
                "duration_ms": record.get("duration_ms"),
                "accepted": record.get("accepted"),
                "evidence": {
                    "public_tests_passed": evidence.get("public_tests_passed"),
                    "hidden_probes_passed": evidence.get("hidden_probes_passed"),
                    "worktree_clean": evidence.get("worktree_clean"),
                    "branch_valid": evidence.get("branch_valid"),
                    "refs_valid": evidence.get("refs_valid"),
                    "git_config_valid": evidence.get("git_config_valid"),
                    "remotes_valid": evidence.get("remotes_valid"),
                    "ancestry_valid": evidence.get("ancestry_valid"),
                    "non_merge_commits": evidence.get("non_merge_commits"),
                    "changed_paths": evidence.get("changed_paths"),
                    "changed_lines": evidence.get("changed_lines"),
                    "scope_valid": evidence.get("scope_valid"),
                },
            }
        )
    return {
        "kind": "engineering-endurance-public-observation",
        "version": 1,
        "execution_id": execution_id,
        "scenario_version": report["scenario_version"],
        "accepted_rungs": report["accepted_rungs"],
        "total_rungs": report["total_rungs"],
        "terminal_status": report["terminal_status"],
        "terminal_rung": report.get("terminal_rung"),
        "elapsed_ms": report.get("elapsed_ms"),
        "accepted_checkpoints": report.get("accepted_checkpoints"),
        "rejected_checkpoints": report.get("rejected_checkpoints"),
        "total_changed_lines": report.get("total_changed_lines"),
        "measurements": report["measurements"],
        "checkpoints": checkpoints,
        "hidden_probe_details": "retained only in the trusted Harness evidence archive",
    }


def create_blob(repository: str, content: bytes) -> str:
    response = gh(
        "POST",
        f"repos/{repository}/git/blobs",
        {"content": base64.b64encode(content).decode("ascii"), "encoding": "base64"},
    )
    return str(response["sha"])


def publish(
    *, repository: str, execution_id: str, report_path: pathlib.Path, run_url: str
) -> dict[str, Any]:
    if repository != ALLOWED_REPOSITORY:
        raise PublishError(f"repository must be exactly {ALLOWED_REPOSITORY}")
    if not SAFE_EXECUTION_ID.fullmatch(execution_id):
        raise PublishError("execution id is not safe")
    report = load_report(report_path)
    projection = public_projection(report, execution_id)

    repository_data = gh("GET", f"repos/{repository}")
    default_branch = repository_data["default_branch"]
    base_ref = gh("GET", f"repos/{repository}/git/ref/heads/{default_branch}")
    base_sha = base_ref["object"]["sha"]
    base_commit = gh("GET", f"repos/{repository}/git/commits/{base_sha}")
    branch = f"benchmark-runs/endurance/{execution_id}"

    report_bytes = (json.dumps(projection, indent=2, sort_keys=True) + "\n").encode()
    patch_bytes = report["accepted_patch"].encode("utf-8")
    report_blob = create_blob(repository, report_bytes)
    patch_blob = create_blob(repository, patch_bytes)
    tree = gh(
        "POST",
        f"repos/{repository}/git/trees",
        {
            "base_tree": base_commit["tree"]["sha"],
            "tree": [
                {
                    "path": f"benchmark-runs/endurance/{execution_id}/report.json",
                    "mode": "100644",
                    "type": "blob",
                    "sha": report_blob,
                },
                {
                    "path": f"benchmark-runs/endurance/{execution_id}/accepted.patch",
                    "mode": "100644",
                    "type": "blob",
                    "sha": patch_blob,
                },
            ],
        },
    )
    commit = gh(
        "POST",
        f"repos/{repository}/git/commits",
        {
            "message": f"Record engineering endurance run {execution_id}",
            "tree": tree["sha"],
            "parents": [base_sha],
        },
    )
    head_sha = commit["sha"]
    gh(
        "POST",
        f"repos/{repository}/git/refs",
        {"ref": f"refs/heads/{branch}", "sha": head_sha},
    )

    terminal = report["terminal_status"] or "incomplete"
    body = (
        "Automated, advisory engineering endurance observation.\n\n"
        f"- Accepted rungs: `{report['accepted_rungs']}/{report['total_rungs']}`\n"
        f"- Terminal status: `{terminal}`\n"
        f"- Terminal rung: `{report.get('terminal_rung')}`\n"
        f"- Runtime: `{report.get('elapsed_ms')} ms`\n"
        f"- Source run: {run_url or 'not supplied'}\n\n"
        "The evaluated session had no GitHub credentials. Hidden probe details remain in the "
        "trusted Harness archive; this PR contains a sanitized report and the accepted patch."
    )
    pull = gh(
        "POST",
        f"repos/{repository}/pulls",
        {
            "title": f"Engineering endurance: {execution_id} ({report['accepted_rungs']}/10)",
            "head": branch,
            "base": default_branch,
            "body": body,
            "draft": True,
        },
    )

    accepted_by_rung = {
        int(record["rung"]): record
        for record in report["checkpoints"]
        if record.get("accepted")
    }
    for rung in range(1, int(report["accepted_rungs"]) + 1):
        record = accepted_by_rung[rung]
        gh(
            "POST",
            f"repos/{repository}/check-runs",
            {
                "name": f"Harness E2E endurance / rung {rung:02d}",
                "head_sha": head_sha,
                "status": "completed",
                "conclusion": "success",
                "details_url": run_url or pull["html_url"],
                "output": {
                    "title": f"Rung {rung} accepted",
                    "summary": (
                        f"Ticket `{record['ticket_id']}` passed public and cumulative hidden "
                        f"validation on attempt {record['attempt']}."
                    ),
                },
            },
        )
    if terminal != "completed":
        boundary = report.get("terminal_rung") or int(report["accepted_rungs"]) + 1
        gh(
            "POST",
            f"repos/{repository}/check-runs",
            {
                "name": f"Harness E2E endurance / boundary {int(boundary):02d}",
                "head_sha": head_sha,
                "status": "completed",
                "conclusion": "neutral",
                "details_url": run_url or pull["html_url"],
                "output": {
                    "title": "Observed capability boundary",
                    "summary": f"Terminal benchmark status: `{terminal}`. This signal is advisory.",
                },
            },
        )
    return {
        "repository": repository,
        "branch": branch,
        "commit_sha": head_sha,
        "pull_request": pull["html_url"],
        "accepted_rungs": report["accepted_rungs"],
        "terminal_status": terminal,
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository", default=ALLOWED_REPOSITORY)
    parser.add_argument("--execution-id", required=True)
    parser.add_argument("--report", type=pathlib.Path, required=True)
    parser.add_argument("--run-url", default="")
    parser.add_argument("--output", type=pathlib.Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    result = publish(
        repository=args.repository,
        execution_id=args.execution_id,
        report_path=args.report,
        run_url=args.run_url,
    )
    encoded = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
