#!/usr/bin/env python3
"""Validate, collect, and package a harness-e2e Registry release.

The workflow owns network publication. This helper deliberately never reads the
Registry API key so release metadata can be inspected and tested without
secrets.
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import re
import subprocess
import sys
import time
from typing import Any


WORKER_NAME = "harness-e2e"
SEMVER_CORE = r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"
VERSION_RE = re.compile(rf"^(?P<core>{SEMVER_CORE})(?P<experimental>-experimental)?$")
TAG_RE = re.compile(rf"^harness-e2e/v(?P<version>{SEMVER_CORE}(?:-experimental)?)$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
SCHEMA_KEYS = frozenset(
    {"type", "properties", "$ref", "allOf", "anyOf", "oneOf", "enum", "items", "const"}
)
TARGETS = (
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "i686-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-gnu",
    "armv7-unknown-linux-gnueabihf",
)


def read_json(path: str | pathlib.Path) -> dict[str, Any]:
    value = json.loads(pathlib.Path(path).read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path} must contain a JSON object")
    return value


def read_yaml(path: pathlib.Path) -> Any:
    import yaml

    return yaml.safe_load(path.read_text(encoding="utf-8"))


def release_is_experimental(version: str) -> bool:
    if not VERSION_RE.fullmatch(version):
        raise ValueError("version must be MAJOR.MINOR.PATCH with an optional -experimental suffix")
    return version.endswith("-experimental")


def git(*args: str, root: pathlib.Path) -> str:
    return subprocess.run(
        ["git", "-C", str(root), *args],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def github_output(**values: str) -> None:
    output_path = os.environ.get("GITHUB_OUTPUT")
    if not output_path:
        return
    with pathlib.Path(output_path).open("a", encoding="utf-8") as output:
        for key, value in values.items():
            output.write(f"{key}={value}\n")


def validate_release(root: pathlib.Path, tag: str, event_sha: str) -> dict[str, str]:
    match = TAG_RE.fullmatch(tag)
    if not match:
        raise ValueError("release tag must match harness-e2e/vX.Y.Z with an optional -experimental suffix")
    version = match.group("version")
    release_is_experimental(version)

    import tomllib

    cargo = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    package = cargo.get("package") or {}
    if package.get("name") != WORKER_NAME:
        raise ValueError(f"Cargo package name must be {WORKER_NAME}")
    if package.get("version") != version:
        raise ValueError(
            f"tag version {version} does not match Cargo.toml version {package.get('version')!r}"
        )

    manifest = read_yaml(root / "iii.worker.yaml")
    if not isinstance(manifest, dict):
        raise ValueError("iii.worker.yaml must contain an object")
    expected = {
        "iii": "v1",
        "name": WORKER_NAME,
        "language": "rust",
        "deploy": "binary",
        "manifest": "Cargo.toml",
        "bin": WORKER_NAME,
    }
    for key, value in expected.items():
        if manifest.get(key) != value:
            raise ValueError(f"iii.worker.yaml {key!r} must be {value!r}")
    if manifest.get("interface_smoke") is False:
        raise ValueError("harness-e2e must not opt out of Registry interface smoke")
    dependencies = manifest.get("dependencies")
    if not isinstance(dependencies, dict) or not dependencies:
        raise ValueError("iii.worker.yaml dependencies must be a non-empty map")
    if not all(isinstance(name, str) and isinstance(constraint, str) for name, constraint in dependencies.items()):
        raise ValueError("iii.worker.yaml dependency names and constraints must be strings")

    config = read_yaml(root / "config.yaml")
    if not isinstance(config, dict):
        raise ValueError("config.yaml must contain an object")

    head_commit = git("rev-parse", "HEAD^{commit}", root=root)
    tag_commit = git("rev-parse", f"{tag}^{{commit}}", root=root)
    tag_object = git("rev-parse", tag, root=root)
    if head_commit != tag_commit:
        raise ValueError(f"checked out commit {head_commit} differs from tag commit {tag_commit}")
    if event_sha and event_sha not in {tag_object, tag_commit}:
        raise ValueError(
            f"GitHub event SHA {event_sha} is neither the tag object nor tagged commit"
        )

    result = {"tag": tag, "version": version, "commit_sha": tag_commit}
    github_output(**result)
    return result


def iii_trigger(function_id: str, payload: dict[str, Any]) -> dict[str, Any]:
    result = subprocess.run(
        ["iii", "trigger", function_id, "--json", json.dumps(payload)],
        check=True,
        capture_output=True,
        text=True,
        timeout=30,
    )
    value = json.loads(result.stdout)
    if not isinstance(value, dict):
        raise ValueError(f"{function_id} returned a non-object payload")
    return value


def list_rows(payload: dict[str, Any], key: str) -> list[dict[str, Any]]:
    rows = payload.get(key) or []
    if not isinstance(rows, list):
        raise ValueError(f"{key} must be an array")
    return [row for row in rows if isinstance(row, dict)]


def schema(value: Any) -> dict[str, Any]:
    return value if isinstance(value, dict) else {}


def schema_is_typed(value: Any) -> bool:
    return isinstance(value, dict) and any(key in value for key in SCHEMA_KEYS)


def function_id(row: dict[str, Any]) -> str:
    value = row.get("function_id") or row.get("id")
    return value if isinstance(value, str) else ""


def unwrap_detail(value: dict[str, Any], key: str) -> dict[str, Any]:
    nested = value.get(key)
    return nested if isinstance(nested, dict) else value


def collect_interface(
    worker: str,
    prefix: str,
    baseline_workers: dict[str, Any],
    baseline_triggers: dict[str, Any],
    wait_seconds: int,
) -> dict[str, Any]:
    baseline_names = {
        value
        for row in list_rows(baseline_workers, "workers")
        for value in (row.get("name"), row.get("id"))
        if isinstance(value, str) and value
    }
    deadline = time.monotonic() + wait_seconds
    functions: list[dict[str, Any]] = []
    workers: list[dict[str, Any]] = []
    while time.monotonic() <= deadline:
        workers = list_rows(iii_trigger("engine::workers::list", {}), "workers")
        functions = list_rows(
            iii_trigger("engine::functions::list", {"include_internal": True}), "functions"
        )
        if any(function_id(row).startswith(prefix) for row in functions):
            break
        time.sleep(2)
    else:
        raise RuntimeError(f"worker did not register any {prefix}* functions in {wait_seconds}s")

    new_names = {
        value
        for row in workers
        for value in (row.get("name"), row.get("id"))
        if isinstance(value, str) and value and value not in baseline_names
    }
    matched_names = {
        str(row.get("worker_name"))
        for row in functions
        if function_id(row).startswith(prefix) and row.get("worker_name")
    }
    target_names = new_names | matched_names | {worker}
    target_rows = [
        row
        for row in functions
        if row.get("worker_name") in target_names or function_id(row).startswith(prefix)
    ]

    normalized_functions: list[dict[str, Any]] = []
    violations: list[str] = []
    seen: set[str] = set()
    for row in target_rows:
        fid = function_id(row)
        if not fid or fid in seen:
            continue
        seen.add(fid)
        detail = unwrap_detail(
            iii_trigger("engine::functions::info", {"function_id": fid}), "function"
        )
        request_schema = schema(detail.get("request_schema"))
        response_schema = schema(detail.get("response_schema"))
        if not schema_is_typed(request_schema):
            violations.append(f"{fid}.request_schema")
        if not schema_is_typed(response_schema):
            violations.append(f"{fid}.response_schema")
        metadata = detail.get("metadata") if isinstance(detail.get("metadata"), dict) else {}
        registry_name = metadata.get("registry_name") or metadata.get("name") or fid
        normalized_functions.append(
            {
                "name": registry_name,
                "description": detail.get("description") if isinstance(detail.get("description"), str) else "",
                "request_schema": request_schema,
                "response_schema": response_schema,
                "metadata": metadata,
            }
        )
    if not normalized_functions:
        raise RuntimeError("interface collection produced no functions")
    if violations:
        raise RuntimeError("untyped function schemas: " + ", ".join(violations))

    baseline_trigger_ids = {
        row.get("id")
        for row in list_rows(baseline_triggers, "triggers")
        if isinstance(row.get("id"), str)
    }
    trigger_rows = list_rows(
        iii_trigger("engine::triggers::list", {"include_internal": False}), "triggers"
    )
    normalized_triggers: list[dict[str, Any]] = []
    for row in trigger_rows:
        trigger_id = row.get("id")
        if not isinstance(trigger_id, str) or trigger_id in baseline_trigger_ids:
            continue
        detail = unwrap_detail(
            iii_trigger("engine::triggers::info", {"id": trigger_id}), "trigger"
        )
        normalized_triggers.append(
            {
                "name": trigger_id,
                "description": detail.get("description") if isinstance(detail.get("description"), str) else "",
                "invocation_schema": schema(detail.get("configuration_schema")),
                "return_schema": schema(detail.get("request_schema")),
                "metadata": {},
            }
        )

    return {"functions": normalized_functions, "triggers": normalized_triggers}


def normalize_dependencies(raw: Any) -> list[dict[str, str]]:
    if not isinstance(raw, dict):
        raise ValueError("dependencies must be a map")
    return [{"name": name, "version": version} for name, version in raw.items()]


def build_payload(
    root: pathlib.Path,
    version: str,
    tag: str,
    repo_url: str,
    interface: dict[str, Any],
    checksums_dir: pathlib.Path,
) -> dict[str, Any]:
    if tag != f"harness-e2e/v{version}":
        raise ValueError("tag and version do not identify the same release")
    manifest = read_yaml(root / "iii.worker.yaml")
    config = read_yaml(root / "config.yaml")
    if not isinstance(manifest, dict) or not isinstance(config, dict):
        raise ValueError("worker manifest and config must be objects")

    binaries: dict[str, dict[str, str]] = {}
    for target in TARGETS:
        checksum_path = checksums_dir / f"{WORKER_NAME}-{target}.sha256"
        if not checksum_path.is_file():
            raise ValueError(f"missing checksum asset {checksum_path.name}")
        digest = checksum_path.read_text(encoding="utf-8").split()[0].lower()
        if not SHA256_RE.fullmatch(digest):
            raise ValueError(f"invalid SHA-256 in {checksum_path.name}")
        extension = "zip" if "windows" in target else "tar.gz"
        binaries[target] = {
            "url": f"{repo_url}/releases/download/{tag}/{WORKER_NAME}-{target}.{extension}",
            "sha256": digest,
        }

    functions = interface.get("functions")
    if not isinstance(functions, list) or not functions:
        raise ValueError("worker interface must contain at least one function")
    violations = [
        f"{function.get('name', '<unknown>')}.{field}"
        for function in functions
        if isinstance(function, dict)
        for field in ("request_schema", "response_schema")
        if not schema_is_typed(function.get(field))
    ]
    if violations:
        raise ValueError("untyped function schemas: " + ", ".join(violations))

    return {
        "worker_name": WORKER_NAME,
        "version": version,
        "tag": "next",
        "type": "binary",
        "readme": (root / "README.md").read_text(encoding="utf-8"),
        "repo": repo_url,
        "description": manifest.get("description", ""),
        "license": manifest.get("license", ""),
        "dependencies": normalize_dependencies(manifest.get("dependencies")),
        "config": config,
        "functions": functions,
        "triggers": interface.get("triggers") or [],
        "experimental": release_is_experimental(version),
        "tags": manifest.get("tags") or [],
        "binaries": binaries,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    validate = subparsers.add_parser("validate")
    validate.add_argument("--root", default=".")
    validate.add_argument("--tag", required=True)
    validate.add_argument("--event-sha", default="")

    collect = subparsers.add_parser("collect-interface")
    collect.add_argument("--worker", default=WORKER_NAME)
    collect.add_argument("--prefix", default="e2e::")
    collect.add_argument("--baseline-workers", required=True)
    collect.add_argument("--baseline-triggers", required=True)
    collect.add_argument("--wait-seconds", type=int, default=120)
    collect.add_argument("--out", required=True)

    payload = subparsers.add_parser("build-payload")
    payload.add_argument("--root", default=".")
    payload.add_argument("--version", required=True)
    payload.add_argument("--tag", required=True)
    payload.add_argument("--repo-url", required=True)
    payload.add_argument("--interface", required=True)
    payload.add_argument("--checksums-dir", required=True)
    payload.add_argument("--out", required=True)

    args = parser.parse_args()
    try:
        if args.command == "validate":
            print(json.dumps(validate_release(pathlib.Path(args.root), args.tag, args.event_sha), indent=2))
        elif args.command == "collect-interface":
            interface = collect_interface(
                worker=args.worker,
                prefix=args.prefix,
                baseline_workers=read_json(args.baseline_workers),
                baseline_triggers=read_json(args.baseline_triggers),
                wait_seconds=args.wait_seconds,
            )
            pathlib.Path(args.out).write_text(json.dumps(interface, indent=2) + "\n", encoding="utf-8")
            print(json.dumps({"functions": len(interface["functions"]), "triggers": len(interface["triggers"])}, indent=2))
        elif args.command == "build-payload":
            release_payload = build_payload(
                root=pathlib.Path(args.root),
                version=args.version,
                tag=args.tag,
                repo_url=args.repo_url,
                interface=read_json(args.interface),
                checksums_dir=pathlib.Path(args.checksums_dir),
            )
            pathlib.Path(args.out).write_text(json.dumps(release_payload, indent=2) + "\n", encoding="utf-8")
            print(json.dumps({"worker_name": WORKER_NAME, "version": args.version, "tag": "next", "targets": list(TARGETS)}, indent=2))
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
