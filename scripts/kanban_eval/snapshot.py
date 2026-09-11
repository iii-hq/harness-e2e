#!/usr/bin/env python3
"""Export one pinned Kanban revision without exposing source Git history."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import posixpath
import re
import subprocess
import tarfile
from typing import Any


FULL_SHA = re.compile(r"^[0-9a-f]{40}$")


class SnapshotError(RuntimeError):
    pass


def _git(repository: Path, *args: str, env: dict[str, str] | None = None) -> bytes:
    completed = subprocess.run(
        ["git", "-C", str(repository), *args],
        check=False,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
    )
    if completed.returncode:
        detail = completed.stderr.decode(errors="replace").strip()
        raise SnapshotError(f"git {' '.join(args)} failed: {detail}")
    return completed.stdout


def _safe_path(path: str) -> bool:
    normalized = posixpath.normpath(path)
    return (
        bool(path)
        and "\0" not in path
        and not PurePosixPath(path).is_absolute()
        and normalized not in {".", ".."}
        and not normalized.startswith("../")
    )


def _validate_archive(data: bytes) -> None:
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as archive:
        for member in archive.getmembers():
            if not _safe_path(member.name):
                raise SnapshotError(f"unsafe archive path: {member.name}")
            parts = PurePosixPath(member.name).parts
            if any(part in {".git", "scenarios"} for part in parts):
                raise SnapshotError(f"private path is not allowed in snapshot: {member.name}")
            if member.issym():
                raise SnapshotError(f"symlink escapes or aliases snapshot paths: {member.name}")
            elif member.islnk():
                raise SnapshotError(f"hard link escapes or aliases snapshot paths: {member.name}")
            elif not (member.isfile() or member.isdir()):
                raise SnapshotError(f"special archive entry is not allowed: {member.name}")


def _load_catalog(path: Path, expected_digest: str | None) -> tuple[dict[str, Any], str]:
    raw = path.read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    if (
        expected_digest is not None
        and expected_digest.removeprefix("sha256:") != digest
    ):
        raise SnapshotError("catalog SHA-256 does not match the expected digest")
    try:
        catalog = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise SnapshotError(f"invalid catalog: {error}") from error
    if not isinstance(catalog, dict) or catalog.get("schema") != "kanban-scenarios/v1":
        raise SnapshotError("unsupported Kanban scenario catalog")
    if not isinstance(catalog.get("shared_prompt"), str) or not isinstance(catalog.get("cases"), list):
        raise SnapshotError("invalid Kanban scenario catalog")
    return catalog, digest


def _select_case(catalog: dict[str, Any], case_id: str) -> dict[str, Any]:
    matches = [
        case
        for case in catalog["cases"]
        if isinstance(case, dict) and case.get("id") == case_id
    ]
    if len(matches) != 1:
        raise SnapshotError(f"scenario must exist exactly once: {case_id}")
    case = matches[0]
    if not all(
        isinstance(case.get(key), str)
        for key in ("base_commit", "reference_commit", "prompt")
    ):
        raise SnapshotError(f"invalid scenario: {case_id}")
    if not FULL_SHA.fullmatch(case["base_commit"]) or not FULL_SHA.fullmatch(case["reference_commit"]):
        raise SnapshotError(f"scenario commits must be full lowercase SHAs: {case_id}")
    criteria = case.get("criteria")
    if (
        not isinstance(criteria, list)
        or not criteria
        or not all(isinstance(item, str) and item.strip() for item in criteria)
    ):
        raise SnapshotError(f"invalid scenario criteria: {case_id}")
    return case


def _prompt(catalog: dict[str, Any], case: dict[str, Any]) -> str:
    criteria = "\n".join(f"- {item}" for item in case["criteria"])
    instructions = Path(__file__).with_name('instructions.md').read_text().strip()
    return f'{catalog["shared_prompt"].strip()}\n\n{case["prompt"].strip()}\n\nAcceptance criteria:\n{criteria}\n\n{instructions}\n'


def prepare(
    repo: str | Path,
    catalog_path: str | Path,
    case_id: str,
    revision_kind: str,
    destination: str | Path,
    *,
    expected_catalog_sha256: str | None = None,
) -> dict[str, str]:
    """Create an isolated one-commit repository for a trusted catalog case."""
    repository = Path(repo).resolve(strict=True)
    catalog_file = Path(catalog_path).resolve(strict=True)
    target = Path(destination)
    if target.is_symlink():
        raise SnapshotError("destination must not be a symlink")
    if target.exists() and (not target.is_dir() or any(target.iterdir())):
        raise SnapshotError("destination must be a new or empty directory")
    if revision_kind not in {"base", "reference"}:
        raise SnapshotError("revision kind must be base or reference")

    catalog, catalog_digest = _load_catalog(catalog_file, expected_catalog_sha256)
    case = _select_case(catalog, case_id)
    base_sha, reference_sha = case["base_commit"], case["reference_commit"]
    for sha in (base_sha, reference_sha):
        resolved = _git(repository, "rev-parse", f"{sha}^{{commit}}").decode().strip()
        if resolved != sha:
            raise SnapshotError(f"catalog commit did not resolve exactly: {sha}")
    parents = _git(repository, "rev-list", "--parents", "-n", "1", reference_sha).decode().split()
    if parents != [reference_sha, base_sha]:
        raise SnapshotError(f"{case_id}: reference must have exactly the base as parent")

    source_sha = base_sha if revision_kind == "base" else reference_sha
    archive_data = _git(repository, "archive", "--format=tar", source_sha)
    _validate_archive(archive_data)

    target.mkdir(parents=True, exist_ok=True)
    with tarfile.open(fileobj=io.BytesIO(archive_data), mode="r:") as archive:
        archive.extractall(target)

    git_env = {
        **os.environ,
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_CONFIG_SYSTEM": os.devnull,
        "GIT_AUTHOR_NAME": "Harness Snapshot",
        "GIT_AUTHOR_EMAIL": "snapshot@harness.invalid",
        "GIT_COMMITTER_NAME": "Harness Snapshot",
        "GIT_COMMITTER_EMAIL": "snapshot@harness.invalid",
        "GIT_AUTHOR_DATE": "2000-01-01T00:00:00Z",
        "GIT_COMMITTER_DATE": "2000-01-01T00:00:00Z",
    }
    _git(target, "init", "--quiet", "--initial-branch=main", "--template=", env=git_env)
    _git(target, "add", "--all", "--force", env=git_env)
    _git(
        target,
        "-c",
        "commit.gpgsign=false",
        "-c",
        "core.hooksPath=/dev/null",
        "commit",
        "--quiet",
        "-m",
        "Initialize scenario snapshot",
        env=git_env,
    )
    snapshot_head = _git(target, "rev-parse", "HEAD", env=git_env).decode().strip()

    return {
        "case_id": case_id,
        "revision_kind": revision_kind,
        "source_sha": source_sha,
        "base_sha": base_sha,
        "reference_sha": reference_sha,
        "catalog_sha256": catalog_digest,
        "snapshot_git_head": snapshot_head,
        "prompt": _prompt(catalog, case),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--catalog", type=Path, required=True)
    parser.add_argument("--case", required=True)
    parser.add_argument("--revision", choices=("base", "reference"), default="base")
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--expected-catalog-sha256")
    args = parser.parse_args()
    metadata = prepare(
        args.repo,
        args.catalog,
        args.case,
        args.revision,
        args.destination,
        expected_catalog_sha256=args.expected_catalog_sha256,
    )
    json.dump(metadata, fp=os.sys.stdout, indent=2)
    os.sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
