#!/usr/bin/env python3
"""Validate, collect, and package a harness-e2e Registry release.

The workflow owns network publication. This helper deliberately never reads the
Registry API key so release metadata can be inspected and tested without
secrets.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import os
import pathlib
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import time
import zipfile
from typing import Any


WORKER_NAME = "harness-e2e"
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


def descriptor_sha256(descriptor: dict[str, Any]) -> str:
    subject = {key: value for key, value in descriptor.items() if key != "descriptor_sha256"}
    canonical = json.dumps(
        subject,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    ).encode("utf-8")
    return hashlib.sha256(canonical).hexdigest()


def load_release_descriptor(
    path: str | pathlib.Path,
    *,
    expected_source_sha: str = "",
    expected_digest: str = "",
    expected_version: str = "",
) -> dict[str, Any]:
    descriptor = read_json(path)
    required = {
        "contract", "worker", "version", "source_sha", "release_spec_sha256",
        "public_manifest_sha256", "registry_projection_sha256", "compiler_digest",
        "descriptor_sha256", "source", "artifact", "runtime", "validation",
        "publish", "build_units", "registry_projection",
    }
    if set(descriptor) != required:
        raise ValueError(f"release descriptor must contain exactly {sorted(required)}")
    if descriptor["contract"] != "release-descriptor" or descriptor["worker"] != WORKER_NAME:
        raise ValueError("release descriptor has the wrong contract or worker")
    digest = descriptor_sha256(descriptor)
    if descriptor.get("descriptor_sha256") != digest:
        raise ValueError("release descriptor package digest is invalid")
    if expected_source_sha and descriptor.get("source_sha") != expected_source_sha:
        raise ValueError("release descriptor source SHA does not match the dispatch")
    if expected_digest and digest != expected_digest:
        raise ValueError("release descriptor digest does not match the dispatch")
    if expected_version and descriptor.get("version") != expected_version:
        raise ValueError("release descriptor version does not match the dispatch")
    artifact = descriptor.get("artifact")
    if not isinstance(artifact, dict) or artifact.get("kind") != "rust-binary":
        raise ValueError("harness-e2e release descriptor must describe a rust-binary")
    if artifact.get("binary") != WORKER_NAME or tuple(artifact.get("targets") or ()) != TARGETS:
        raise ValueError("harness-e2e release descriptor has an unexpected binary target matrix")
    if artifact.get("toolchain") != {"name": "rust", "version": "1.97.1"}:
        raise ValueError("harness-e2e release descriptor has an unexpected Rust toolchain")
    return descriptor


def safe_relative(value: Any, field: str, *, allow_dot: bool = False) -> pathlib.Path:
    if not isinstance(value, str) or not value or (value == "." and not allow_dot):
        raise ValueError(f"{field} must be a non-empty relative path")
    path = pathlib.Path(value)
    if path.is_absolute() or ".." in path.parts:
        raise ValueError(f"{field} must remain within the repository")
    return path


def argv(value: Any, field: str) -> tuple[str, ...]:
    if not isinstance(value, list) or not value or any(not isinstance(part, str) or not part for part in value):
        raise ValueError(f"{field} must be a non-empty argv")
    return tuple(value)


def frontend_specs(descriptor: dict[str, Any]) -> list[dict[str, Any]]:
    artifact = descriptor["artifact"]
    raw = artifact.get("frontends", [])
    if not isinstance(raw, list):
        raise ValueError("artifact.frontends must be an array")
    required = {
        "workspace_root",
        "source_path",
        "runtime",
        "package_manager",
        "lockfile",
        "install_command",
        "build_command",
        "outputs",
    }
    normalized: list[dict[str, Any]] = []
    for index, value in enumerate(raw):
        if not isinstance(value, dict) or set(value) != required:
            raise ValueError(f"artifact.frontends[{index}] differs from compiler contract")
        runtime = value["runtime"]
        package_manager = value["package_manager"]
        if runtime != {"name": "node", "version": "22"}:
            raise ValueError("Harness dashboard requires Node 22")
        if package_manager != {"name": "pnpm", "version": "11.13.1"}:
            raise ValueError("Harness dashboard requires pnpm 11.13.1")
        outputs = value["outputs"]
        if not isinstance(outputs, list) or not outputs:
            raise ValueError(f"artifact.frontends[{index}].outputs must be non-empty")
        normalized.append(
            {
                **value,
                "workspace_root": safe_relative(
                    value["workspace_root"],
                    f"artifact.frontends[{index}].workspace_root",
                    allow_dot=True,
                ),
                "source_path": safe_relative(value["source_path"], f"artifact.frontends[{index}].source_path"),
                "lockfile": safe_relative(value["lockfile"], f"artifact.frontends[{index}].lockfile"),
                "install_command": argv(value["install_command"], f"artifact.frontends[{index}].install_command"),
                "build_command": argv(value["build_command"], f"artifact.frontends[{index}].build_command"),
                "outputs": [safe_relative(output, f"artifact.frontends[{index}].outputs") for output in outputs],
            }
        )
    return normalized


def frontend_metadata(descriptor_path: pathlib.Path) -> dict[str, str]:
    descriptor = load_release_descriptor(descriptor_path)
    specs = frontend_specs(descriptor)
    if len(specs) != 1:
        raise ValueError("Harness release requires exactly one declared dashboard frontend")
    spec = specs[0]
    lockfile = pathlib.Path(spec["workspace_root"]) / spec["lockfile"]
    if not lockfile.is_file() or not stat.S_ISREG(lockfile.lstat().st_mode):
        raise ValueError(f"frontend lockfile is missing or not regular: {lockfile}")
    result = {
        "runtime_version": str(spec["runtime"]["version"]),
        "package_manager_version": str(spec["package_manager"]["version"]),
        "rust_toolchain_version": str(descriptor["artifact"]["toolchain"]["version"]),
        "lockfile": lockfile.as_posix(),
        "lock_sha256": hashlib.sha256(lockfile.read_bytes()).hexdigest(),
    }
    github_output(**result)
    return result


def build_frontends(descriptor_path: pathlib.Path, output: pathlib.Path) -> dict[str, Any]:
    descriptor = load_release_descriptor(descriptor_path)
    specs = frontend_specs(descriptor)
    output.mkdir(parents=True, exist_ok=False)
    repository = pathlib.Path.cwd().resolve()
    copied: list[str] = []
    installed: set[tuple[str, tuple[str, ...]]] = set()
    for spec in specs:
        workspace = (repository / spec["workspace_root"]).resolve()
        source = (repository / spec["source_path"]).resolve()
        if not workspace.is_dir() or not workspace.is_relative_to(repository):
            raise ValueError(f"frontend workspace escapes repository: {workspace}")
        if not source.is_dir() or not source.is_relative_to(repository):
            raise ValueError(f"frontend source escapes repository: {source}")
        install_identity = (workspace.as_posix(), spec["install_command"])
        if install_identity not in installed:
            subprocess.run(spec["install_command"], cwd=workspace, check=True)
            installed.add(install_identity)
        subprocess.run(spec["build_command"], cwd=source, check=True)
        for relative in spec["outputs"]:
            source_output = source / relative
            resolved = source_output.resolve(strict=True)
            if not resolved.is_relative_to(source):
                raise ValueError(f"frontend output escapes source: {source_output}")
            candidates = [source_output, *source_output.rglob("*")] if source_output.is_dir() else [source_output]
            if any(candidate.is_symlink() for candidate in candidates):
                raise ValueError(f"frontend output contains a symlink: {source_output}")
            destination = output / spec["source_path"] / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            if source_output.is_dir():
                shutil.copytree(source_output, destination)
            elif source_output.is_file():
                shutil.copy2(source_output, destination)
            else:
                raise ValueError(f"frontend output is not a file or directory: {source_output}")
            copied.append(destination.as_posix())
    return {"frontends": len(specs), "outputs": copied}


def package_binary(binary_path: pathlib.Path, target: str, output: pathlib.Path) -> dict[str, Any]:
    if target not in TARGETS:
        raise ValueError(f"unsupported Harness release target: {target}")
    if not binary_path.is_file() or binary_path.is_symlink():
        raise ValueError(f"binary is missing, not regular, or a symlink: {binary_path}")
    output.mkdir(parents=True, exist_ok=False)
    body = binary_path.read_bytes()
    executable_name = f"{WORKER_NAME}.exe" if "windows" in target else WORKER_NAME
    if "windows" in target:
        archive = output / f"{WORKER_NAME}-{target}.zip"
        member = zipfile.ZipInfo(executable_name, date_time=(1980, 1, 1, 0, 0, 0))
        member.create_system = 3
        member.external_attr = 0o755 << 16
        member.compress_type = zipfile.ZIP_DEFLATED
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as bundle:
            bundle.writestr(member, body)
    else:
        archive = output / f"{WORKER_NAME}-{target}.tar.gz"
        member = tarfile.TarInfo(executable_name)
        member.size = len(body)
        member.mode = 0o755
        member.uid = member.gid = 0
        member.uname = member.gname = ""
        member.mtime = 0
        with archive.open("wb") as raw:
            with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0, compresslevel=9) as compressed:
                with tarfile.open(fileobj=compressed, mode="w", format=tarfile.GNU_FORMAT) as bundle:
                    import io

                    bundle.addfile(member, io.BytesIO(body))
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    checksum = archive.with_name(f"{archive.name}.sha256")
    checksum.write_text(f"{digest}  {archive.name}\n", encoding="utf-8")
    return {"archive": archive.as_posix(), "sha256": digest, "size": archive.stat().st_size}


def github_output(**values: str) -> None:
    output_path = os.environ.get("GITHUB_OUTPUT")
    if not output_path:
        return
    with pathlib.Path(output_path).open("a", encoding="utf-8") as output:
        for key, value in values.items():
            output.write(f"{key}={value}\n")


def validate_release_descriptor(
    descriptor_path: str | pathlib.Path,
    source_sha: str,
    digest: str,
    version: str,
) -> dict[str, str]:
    descriptor = load_release_descriptor(
        descriptor_path,
        expected_source_sha=source_sha,
        expected_digest=digest,
        expected_version=version,
    )
    result = {
        "version": str(descriptor["version"]),
        "commit_sha": str(descriptor["source_sha"]),
        "descriptor_sha256": str(descriptor["descriptor_sha256"]),
    }
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


def build_payload(
    descriptor_path: pathlib.Path,
    tag: str,
    repo_url: str,
    interface: dict[str, Any],
    checksums_dir: pathlib.Path,
) -> dict[str, Any]:
    descriptor = load_release_descriptor(descriptor_path)
    projection = descriptor["registry_projection"]
    required_projection = {
        "worker_name", "version", "type", "description", "license", "tags",
        "dependencies", "config", "experimental", "readme",
    }
    if not isinstance(projection, dict) or set(projection) != required_projection:
        raise ValueError("release descriptor Registry projection differs from the current API")
    if (
        projection.get("worker_name") != WORKER_NAME
        or projection.get("version") != descriptor.get("version")
        or projection.get("type") != "binary"
    ):
        raise ValueError("release descriptor Registry projection identity is invalid")
    version = str(descriptor["version"])
    if tag != f"harness-e2e/v{version}":
        raise ValueError("tag and descriptor version do not identify the same release")

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
        **projection,
        "tag": "next",
        "repo": repo_url,
        "functions": functions,
        "triggers": interface.get("triggers") or [],
        "binaries": binaries,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    validate = subparsers.add_parser("validate-descriptor")
    validate.add_argument("--descriptor", required=True)
    validate.add_argument("--source-sha", required=True)
    validate.add_argument("--descriptor-sha256", required=True)
    validate.add_argument("--version", required=True)

    frontend = subparsers.add_parser("frontend-metadata")
    frontend.add_argument("--descriptor", required=True)

    build_frontend = subparsers.add_parser("build-frontends")
    build_frontend.add_argument("--descriptor", required=True)
    build_frontend.add_argument("--out", required=True)

    package = subparsers.add_parser("package-binary")
    package.add_argument("--binary", required=True)
    package.add_argument("--target", required=True)
    package.add_argument("--out", required=True)

    collect = subparsers.add_parser("collect-interface")
    collect.add_argument("--worker", default=WORKER_NAME)
    collect.add_argument("--prefix", default="e2e::")
    collect.add_argument("--baseline-workers", required=True)
    collect.add_argument("--baseline-triggers", required=True)
    collect.add_argument("--wait-seconds", type=int, default=120)
    collect.add_argument("--out", required=True)

    payload = subparsers.add_parser("build-payload")
    payload.add_argument("--descriptor", required=True)
    payload.add_argument("--tag", required=True)
    payload.add_argument("--repo-url", required=True)
    payload.add_argument("--interface", required=True)
    payload.add_argument("--checksums-dir", required=True)
    payload.add_argument("--out", required=True)

    args = parser.parse_args()
    try:
        if args.command == "validate-descriptor":
            print(
                json.dumps(
                    validate_release_descriptor(
                        args.descriptor,
                        args.source_sha,
                        args.descriptor_sha256,
                        args.version,
                    ),
                    indent=2,
                )
            )
        elif args.command == "frontend-metadata":
            print(json.dumps(frontend_metadata(pathlib.Path(args.descriptor)), indent=2))
        elif args.command == "build-frontends":
            print(json.dumps(build_frontends(pathlib.Path(args.descriptor), pathlib.Path(args.out)), indent=2))
        elif args.command == "package-binary":
            print(
                json.dumps(
                    package_binary(pathlib.Path(args.binary), args.target, pathlib.Path(args.out)),
                    indent=2,
                )
            )
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
                descriptor_path=pathlib.Path(args.descriptor),
                tag=args.tag,
                repo_url=args.repo_url,
                interface=read_json(args.interface),
                checksums_dir=pathlib.Path(args.checksums_dir),
            )
            pathlib.Path(args.out).write_text(json.dumps(release_payload, indent=2) + "\n", encoding="utf-8")
            print(json.dumps({"worker": WORKER_NAME, "channel": "next", "targets": list(TARGETS)}, indent=2))
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
