#!/usr/bin/env python3
"""Verify and safely extract an immutable Harness E2E subject archive."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import tarfile
from typing import Any


DIGEST = re.compile(r"^sha256:[0-9a-f]{64}$")
REVISION = re.compile(r"^[0-9a-f]{40}$")
REPOSITORY = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
SAFE_PATH = re.compile(r"^[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*$")
WORKER = re.compile(r"^[a-z0-9][a-z0-9_-]*$")
MAX_FILES = 512
MAX_TOTAL_BYTES = 2 * 1024 * 1024 * 1024


class VerificationError(ValueError):
    pass


def digest_bytes(data: bytes) -> str:
    return f"sha256:{hashlib.sha256(data).hexdigest()}"


def require(condition: bool, message: str) -> None:
    if not condition:
        raise VerificationError(message)


def load_manifest(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    require(isinstance(value, dict), "manifest must be a JSON object")
    allowed = {
        "repository",
        "revision",
        "created_at",
        "archive",
        "files",
        "entrypoints",
    }
    require(set(value) == allowed, "manifest fields do not match the subject artifact contract")
    require(bool(REPOSITORY.fullmatch(value["repository"])), "invalid repository identity")
    require(bool(REVISION.fullmatch(value["revision"])), "revision must be a full lowercase SHA")
    require(isinstance(value["created_at"], str) and value["created_at"], "created_at is required")

    archive = value["archive"]
    require(isinstance(archive, dict), "archive must be an object")
    require(
        set(archive) == {"sha256", "size_bytes", "media_type"},
        "archive fields do not match the subject artifact contract",
    )
    require(bool(DIGEST.fullmatch(archive["sha256"])), "invalid archive SHA-256")
    require(isinstance(archive["size_bytes"], int) and archive["size_bytes"] > 0, "invalid archive size")
    require(archive["size_bytes"] <= MAX_TOTAL_BYTES, "archive exceeds the size limit")
    require(archive["media_type"] in {"application/x-tar", "application/gzip"}, "unsupported archive media type")

    files = value["files"]
    require(isinstance(files, list) and 0 < len(files) <= MAX_FILES, "invalid file inventory size")
    observed_paths: set[str] = set()
    total_size = 0
    for item in files:
        require(isinstance(item, dict), "file inventory entries must be objects")
        require(
            set(item) == {"path", "sha256", "size_bytes", "executable"},
            "file inventory fields do not match subject-artifact contract",
        )
        path_value = item["path"]
        require(isinstance(path_value, str) and SAFE_PATH.fullmatch(path_value) is not None, "unsafe declared file path")
        require(path_value not in observed_paths, f"duplicate declared file path: {path_value}")
        observed_paths.add(path_value)
        require(bool(DIGEST.fullmatch(item["sha256"])), f"invalid SHA-256 for {path_value}")
        require(isinstance(item["size_bytes"], int) and item["size_bytes"] >= 0, f"invalid size for {path_value}")
        require(isinstance(item["executable"], bool), f"invalid executable flag for {path_value}")
        total_size += item["size_bytes"]
    require(total_size <= MAX_TOTAL_BYTES, "declared files exceed the total size limit")

    entrypoints = value["entrypoints"]
    require(isinstance(entrypoints, list) and entrypoints, "at least one entrypoint is required")
    entrypoint_workers: set[str] = set()
    for item in entrypoints:
        require(isinstance(item, dict), "entrypoints must be objects")
        require(
            set(item) == {"worker", "path", "args", "readiness_functions"},
            "entrypoint fields do not match subject-artifact contract",
        )
        require(isinstance(item["worker"], str) and WORKER.fullmatch(item["worker"]) is not None, "invalid worker id")
        require(item["worker"] not in entrypoint_workers, f"duplicate entrypoint worker: {item['worker']}")
        entrypoint_workers.add(item["worker"])
        require(item["path"] in observed_paths, f"entrypoint path is not declared: {item['path']}")
        declared = next(file for file in files if file["path"] == item["path"])
        require(declared["executable"], f"entrypoint is not executable: {item['path']}")
        require(isinstance(item["args"], list) and all(isinstance(arg, str) for arg in item["args"]), "entrypoint args must be strings")
        require(
            isinstance(item["readiness_functions"], list)
            and item["readiness_functions"]
            and all(isinstance(function, str) and "::" in function for function in item["readiness_functions"]),
            "entrypoint readiness functions are invalid",
        )
    return value


def verify_archive(manifest: dict[str, Any], archive_path: Path, extract_dir: Path) -> None:
    archive_bytes = archive_path.read_bytes()
    archive_contract = manifest["archive"]
    require(len(archive_bytes) == archive_contract["size_bytes"], "archive size does not match manifest")
    require(digest_bytes(archive_bytes) == archive_contract["sha256"], "archive SHA-256 does not match manifest")
    require(not extract_dir.exists() or not any(extract_dir.iterdir()), "extract directory must be absent or empty")
    extract_dir.mkdir(parents=True, exist_ok=True)

    declared = {item["path"]: item for item in manifest["files"]}
    observed: set[str] = set()
    mode = "r:gz" if archive_contract["media_type"] == "application/gzip" else "r:"
    with tarfile.open(archive_path, mode) as archive:
        members = archive.getmembers()
        require(len(members) <= MAX_FILES * 2, "archive has too many entries")
        for member in members:
            pure = PurePosixPath(member.name)
            require(not pure.is_absolute() and ".." not in pure.parts, f"unsafe archive path: {member.name}")
            require(member.isdir() or member.isfile(), f"unsupported archive entry type: {member.name}")
            if member.isdir():
                continue
            require(member.name in declared, f"archive contains undeclared file: {member.name}")
            require(member.name not in observed, f"archive contains duplicate file: {member.name}")
            contract = declared[member.name]
            require(member.size == contract["size_bytes"], f"archive member size mismatch: {member.name}")
            source = archive.extractfile(member)
            require(source is not None, f"cannot read archive member: {member.name}")
            data = source.read(MAX_TOTAL_BYTES + 1)
            require(len(data) == member.size, f"truncated archive member: {member.name}")
            require(digest_bytes(data) == contract["sha256"], f"file SHA-256 mismatch: {member.name}")
            destination = extract_dir.joinpath(*pure.parts)
            destination.parent.mkdir(parents=True, exist_ok=True)
            with destination.open("xb") as output:
                output.write(data)
            os.chmod(destination, 0o755 if contract["executable"] else 0o644)
            observed.add(member.name)
    missing = sorted(set(declared) - observed)
    require(not missing, f"archive is missing declared files: {', '.join(missing)}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--extract-dir", type=Path, required=True)
    parser.add_argument("--summary", type=Path)
    args = parser.parse_args()
    try:
        manifest = load_manifest(args.manifest)
        verify_archive(manifest, args.archive, args.extract_dir)
    except (OSError, json.JSONDecodeError, tarfile.TarError, VerificationError) as error:
        raise SystemExit(f"subject artifact verification failed: {error}") from error
    summary = {
        "status": "verified",
        "repository": manifest["repository"],
        "revision": manifest["revision"],
        "archive_sha256": manifest["archive"]["sha256"],
        "files": len(manifest["files"]),
        "entrypoints": [entrypoint["worker"] for entrypoint in manifest["entrypoints"]],
    }
    encoded = json.dumps(summary, indent=2, sort_keys=True) + "\n"
    if args.summary:
        args.summary.parent.mkdir(parents=True, exist_ok=True)
        args.summary.write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
