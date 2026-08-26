#!/usr/bin/env python3
"""Offline fixture launcher for engineering-ticket benchmarks.

The reviewed source is either a local Git repository or an immutable Git
bundle. The benchmark receives only a disposable clone path and an opaque lease
id; cleanup resolves the path from the owned lease record.
"""

from __future__ import annotations

import argparse
import contextlib
import fcntl
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import uuid
from collections.abc import Sequence
from typing import Any


REPOSITORY_ENV = "HARNESS_E2E_ENGINEERING_FIXTURE_REPOSITORY"
ROOT_ENV = "HARNESS_E2E_ENGINEERING_FIXTURE_ROOT"
DEFAULT_ROOT = pathlib.Path("/var/tmp/iii-harness-e2e/engineering-ticket")
SAFE_EXECUTION_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$")
FULL_SHA = re.compile(r"^[0-9a-fA-F]{40}$")
LEASE_ID = re.compile(r"^[0-9a-f]{32}$")


class LauncherError(RuntimeError):
    pass


def _run_git(repository: pathlib.Path, *args: str) -> str:
    completed = subprocess.run(
        ["git", "-C", str(repository), *args],
        check=False,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise LauncherError(f"git {' '.join(args)} failed in {repository}: {detail}")
    return completed.stdout.strip()


def _configured_root(environ: dict[str, str] | os._Environ[str]) -> pathlib.Path:
    raw = environ.get(ROOT_ENV)
    root = pathlib.Path(raw) if raw else DEFAULT_ROOT
    if not root.is_absolute() or root == pathlib.Path("/"):
        raise LauncherError(f"{ROOT_ENV} must be a non-root absolute path")
    home = pathlib.Path.home().resolve()
    resolved = root.resolve(strict=False)
    if resolved == home:
        raise LauncherError(f"{ROOT_ENV} cannot be the home directory")
    return resolved


def _configured_repository(
    environ: dict[str, str] | os._Environ[str],
) -> pathlib.Path:
    raw = environ.get(REPOSITORY_ENV)
    if not raw:
        raise LauncherError(f"{REPOSITORY_ENV} is required")
    repository = pathlib.Path(raw)
    if not repository.is_absolute():
        raise LauncherError(f"{REPOSITORY_ENV} must be absolute")
    try:
        repository = repository.resolve(strict=True)
    except FileNotFoundError as error:
        raise LauncherError(f"reviewed fixture repository does not exist: {repository}") from error
    if repository.is_dir():
        _run_git(repository, "rev-parse", "--git-dir")
    elif repository.is_file():
        completed = subprocess.run(
            ["git", "bundle", "verify", str(repository)],
            check=False,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        if completed.returncode != 0:
            detail = completed.stderr.strip() or completed.stdout.strip()
            raise LauncherError(f"invalid fixture bundle {repository}: {detail}")
    else:
        raise LauncherError(f"reviewed fixture source is not a repository or bundle: {repository}")
    return repository


@contextlib.contextmanager
def _lease_lock(root: pathlib.Path):
    leases = root / "leases"
    leases.mkdir(parents=True, exist_ok=True)
    lock_path = leases / ".lock"
    with lock_path.open("a+", encoding="utf-8") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        yield


def _write_lease(path: pathlib.Path, value: dict[str, Any]) -> None:
    temporary = path.with_suffix(".json.tmp")
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    descriptor = os.open(temporary, flags, 0o600)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
            json.dump(value, stream, sort_keys=True)
            stream.write("\n")
        temporary.replace(path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def _owned_worktree(root: pathlib.Path, path: pathlib.Path) -> pathlib.Path:
    worktrees = (root / "worktrees").resolve(strict=False)
    candidate = path.resolve(strict=False)
    if candidate.parent != worktrees or candidate == worktrees:
        raise LauncherError(f"lease path is outside the protected worktree root: {candidate}")
    return candidate


def prepare(
    execution_id: str,
    revision: str,
    *,
    environ: dict[str, str] | os._Environ[str] = os.environ,
) -> dict[str, str]:
    if not SAFE_EXECUTION_ID.fullmatch(execution_id):
        raise LauncherError("execution id is not safe")
    if not FULL_SHA.fullmatch(revision):
        raise LauncherError("revision must be a full 40-character Git SHA")
    revision = revision.lower()
    repository = _configured_repository(environ)
    root = _configured_root(environ)
    branch = f"e2e/{execution_id}"
    branch_check = subprocess.run(
        ["git", "check-ref-format", "--branch", branch],
        check=False,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if branch_check.returncode != 0:
        detail = branch_check.stderr.strip() or branch_check.stdout.strip()
        raise LauncherError(f"fixture branch is invalid: {detail}")
    worktrees = root / "worktrees"
    leases = root / "leases"
    worktrees.mkdir(parents=True, exist_ok=True)
    leases.mkdir(parents=True, exist_ok=True)
    root.chmod(0o700)
    worktrees.chmod(0o700)
    leases.chmod(0o700)
    lease_id = uuid.uuid4().hex
    worktree = _owned_worktree(root, worktrees / f"{execution_id}-{lease_id}")
    lease_path = leases / f"{lease_id}.json"
    try:
        clone_args = ["git", "clone", "--quiet", "--no-checkout"]
        if repository.is_dir():
            clone_args.append("--shared")
        clone_args.extend([str(repository), str(worktree)])
        cloned = subprocess.run(
            clone_args,
            check=False,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        if cloned.returncode != 0:
            detail = cloned.stderr.strip() or cloned.stdout.strip()
            raise LauncherError(f"local fixture clone failed: {detail}")
        _run_git(worktree, "remote", "remove", "origin")
        _run_git(worktree, "checkout", "--quiet", "-b", branch, revision)
        expected_ref = f"refs/heads/{branch}"
        refs = _run_git(worktree, "for-each-ref", "--format=%(refname)").splitlines()
        for ref in refs:
            if ref != expected_ref:
                _run_git(worktree, "update-ref", "-d", ref)
        _run_git(worktree, "config", "--local", "user.name", "Harness E2E")
        _run_git(
            worktree,
            "config",
            "--local",
            "user.email",
            "harness-e2e@example.invalid",
        )
        if _run_git(worktree, "rev-parse", "HEAD").lower() != revision:
            raise LauncherError("disposable fixture HEAD differs from requested revision")
        if _run_git(worktree, "status", "--porcelain=v1", "--untracked-files=all"):
            raise LauncherError("disposable fixture is dirty after checkout")
        if _run_git(worktree, "remote"):
            raise LauncherError("disposable fixture retained a Git remote")
        retained_refs = _run_git(
            worktree,
            "for-each-ref",
            "--sort=refname",
            "--format=%(refname)",
        ).splitlines()
        if retained_refs != [expected_ref]:
            raise LauncherError("disposable fixture did not retain exactly one branch")
        lease = {
            "lease_id": lease_id,
            "path": str(worktree),
            "revision": revision,
            "execution_id": execution_id,
        }
        with _lease_lock(root):
            _write_lease(lease_path, lease)
        return {key: str(value) for key, value in lease.items()}
    except BaseException:
        if worktree.exists() and not worktree.is_symlink():
            shutil.rmtree(worktree)
        raise


def cleanup(
    lease_id: str,
    *,
    environ: dict[str, str] | os._Environ[str] = os.environ,
) -> dict[str, str | bool]:
    if not LEASE_ID.fullmatch(lease_id):
        raise LauncherError("lease id is not valid")
    root = _configured_root(environ)
    lease_path = root / "leases" / f"{lease_id}.json"
    with _lease_lock(root):
        try:
            value = json.loads(lease_path.read_text(encoding="utf-8"))
        except FileNotFoundError as error:
            raise LauncherError(f"unknown fixture lease {lease_id}") from error
        if value.get("lease_id") != lease_id or not isinstance(value.get("path"), str):
            raise LauncherError(f"fixture lease {lease_id} is malformed")
        worktree = _owned_worktree(root, pathlib.Path(value["path"]))
        if worktree.is_symlink():
            raise LauncherError(f"refusing symlink fixture cleanup: {worktree}")
        if worktree.exists():
            shutil.rmtree(worktree)
        lease_path.unlink()
    return {"lease_id": lease_id, "removed": True}


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__)
    commands = root.add_subparsers(dest="command", required=True)
    prepare_command = commands.add_parser("prepare")
    prepare_command.add_argument("--execution-id", required=True)
    prepare_command.add_argument("--revision", required=True)
    cleanup_command = commands.add_parser("cleanup")
    cleanup_command.add_argument("--lease-id", required=True)
    return root


def main(argv: Sequence[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command == "prepare":
            result = prepare(args.execution_id, args.revision)
        else:
            result = cleanup(args.lease_id)
    except LauncherError as error:
        print(f"engineering-ticket-fixture: {error}", file=sys.stderr)
        return 2
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
