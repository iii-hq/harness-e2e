#!/usr/bin/env python3
"""Publish a harness-e2e release to the Registry and promote it to latest.

``publish`` stages the build's archives in a Registry-hosted release, so the
Registry serves them from its CDN (iii-hq/registry docs/ASSET_UPLOADS.md), and
publishes the version on ``next``. ``promote`` moves ``latest`` onto that
candidate, using the raw tag pointer for compare-and-swap so an incompatible
dependency graph on an existing ``latest`` release cannot be mistaken for an
absent pointer.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any


WORKER_NAME = "harness-e2e"
SEMVER_CORE = r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"
VERSION_RE = re.compile(rf"^{SEMVER_CORE}(?:-experimental)?$")


class RegistryError(RuntimeError):
    """A safe, user-facing Registry operation failure."""


def request_json(
    method: str,
    url: str,
    payload: dict[str, Any] | None = None,
    *,
    api_key: str | None = None,
    timeout: float = 60,
) -> tuple[int, dict[str, Any]]:
    body = json.dumps(payload).encode("utf-8") if payload is not None else None
    headers = {"Content-Type": "application/json"}
    if api_key:
        headers["X-API-Key"] = api_key
    request = urllib.request.Request(url, data=body, headers=headers, method=method)
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.status, _decode_object(response.read())
    except urllib.error.HTTPError as error:
        try:
            response = _decode_object(error.read())
        except (OSError, ValueError):
            response = {"error": f"HTTP {error.code}"}
        return error.code, response
    except (OSError, ValueError) as error:
        raise RegistryError(f"Registry request failed: {error}") from error


def _decode_object(raw: bytes) -> dict[str, Any]:
    value = json.loads(raw.decode("utf-8"))
    if not isinstance(value, dict):
        raise ValueError("Registry response must be a JSON object")
    return value


def _error_code(response: dict[str, Any]) -> str | None:
    error = response.get("error")
    return error.get("code") if isinstance(error, dict) and isinstance(error.get("code"), str) else None


def resolved_root_version(response: dict[str, Any]) -> str:
    root = response.get("root")
    version = root.get("version") if isinstance(root, dict) else None
    if not isinstance(version, str) or not version:
        raise RegistryError("Registry resolve response has no root.version")
    return version


def resolve_version(
    api_url: str,
    worker: str,
    selector: str,
    *,
    allow_missing: bool = False,
) -> str | None:
    status, response = request_json(
        "POST",
        f"{api_url.rstrip('/')}/resolve",
        {"worker": worker, "version": selector},
    )
    if status == 200:
        return resolved_root_version(response)
    if allow_missing and _error_code(response) in {"version_not_found", "worker_not_found"}:
        return None
    raise RegistryError(
        f"resolve {worker}@{selector} failed with HTTP {status}: {json.dumps(response, sort_keys=True)}"
    )


def release_tag_version(
    api_url: str,
    worker: str,
    tag: str,
    *,
    allow_missing: bool = False,
) -> str | None:
    encoded_worker = urllib.parse.quote(worker, safe="")
    status, response = request_json(
        "GET",
        f"{api_url.rstrip('/')}/w/{encoded_worker}/versions",
    )
    if status == 200:
        versions = response.get("versions")
        if not isinstance(versions, list):
            raise RegistryError(f"versions response for {worker} has no versions list")
        matches = [
            entry.get("version")
            for entry in versions
            if isinstance(entry, dict) and tag in (entry.get("tags") or [])
        ]
        if len(matches) == 1 and isinstance(matches[0], str) and matches[0]:
            return matches[0]
        if not matches and allow_missing:
            return None
        if not matches:
            raise RegistryError(f"release tag {worker}@{tag} was not found")
        raise RegistryError(f"release tag {worker}@{tag} points to multiple versions")
    if allow_missing and status == 404 and _error_code(response) in {"version_not_found", "worker_not_found"}:
        return None
    raise RegistryError(
        f"list versions for {worker} failed with HTTP {status}: {json.dumps(response, sort_keys=True)}"
    )


def upload_file(url: str, headers: dict[str, str], path: Path) -> None:
    # urllib derives Content-Length from the body; a second one is rejected.
    headers = {name: value for name, value in headers.items() if name.lower() != "content-length"}
    request = urllib.request.Request(url, data=path.read_bytes(), headers=headers, method="PUT")
    try:
        with urllib.request.urlopen(request, timeout=300):
            return
    except urllib.error.HTTPError as error:
        raise RegistryError(f"upload of {path.name} failed with HTTP {error.code}: {error.read()[:300]!r}") from error
    except OSError as error:
        raise RegistryError(f"upload of {path.name} failed: {error}") from error


def publish(
    api_url: str,
    api_key: str,
    payload: dict[str, Any],
    assets: list[dict[str, Any]],
    assets_dir: Path,
) -> dict[str, Any]:
    """Open a Registry-hosted release, upload ``assets`` and publish on next.

    Every step repeats safely: an open release resumes, registering an asset
    again re-issues its upload URL, and a version that already exists is a
    replay when next resolves to it.
    """
    worker, version = payload.get("worker_name"), payload.get("version")
    if worker != WORKER_NAME:
        raise RegistryError(f"this pipeline only publishes {WORKER_NAME}")
    validate_version(version)
    releases = f"{api_url.rstrip('/')}/w/{urllib.parse.quote(worker, safe='')}/releases"
    start = {"version": version, "type": payload.get("type")}
    status, response = request_json("POST", releases, start, api_key=api_key)
    if status == 410:
        # That call closed an expired release left by an earlier attempt.
        status, response = request_json("POST", releases, start, api_key=api_key)
    if status == 409 and response.get("code") == "version_already_published":
        resolved = resolve_version(api_url, worker, "next")
        if resolved != version:
            raise RegistryError(f"{worker}@{version} is already published but next resolves to {resolved}")
        return {"worker": worker, "version": version, "release_id": None, "changed": False}
    release = response.get("release")
    release_id = release.get("id") if isinstance(release, dict) else None
    if status != 200 or not isinstance(release_id, str):
        raise RegistryError(f"release start failed with HTTP {status}: {json.dumps(response, sort_keys=True)}")

    status, response = request_json(
        "POST", f"{releases}/{urllib.parse.quote(release_id, safe='')}/assets", {"assets": assets}, api_key=api_key
    )
    if status != 200:
        raise RegistryError(f"asset registration failed with HTTP {status}: {json.dumps(response, sort_keys=True)}")
    uploads = {
        entry.get("name"): entry.get("upload")
        for entry in response.get("assets") or []
        if isinstance(entry, dict)
    }
    for asset in assets:
        upload = uploads.get(asset["name"])
        if not isinstance(upload, dict) or not isinstance(upload.get("url"), str):
            raise RegistryError(f"Registry returned no upload URL for {asset['name']}")
        upload_file(upload["url"], upload.get("headers") or {}, assets_dir / asset["name"])

    # Publishing copies every staged archive to the CDN before it answers.
    status, response = request_json(
        "POST",
        f"{api_url.rstrip('/')}/publish",
        {**payload, "release_id": release_id},
        api_key=api_key,
        timeout=300,
    )
    if status != 200:
        raise RegistryError(f"publish failed with HTTP {status}: {json.dumps(response, sort_keys=True)}")
    return {
        "worker": worker,
        "version": version,
        "release_id": release_id,
        "changed": response.get("changed") is not False,
        "registry_response": response,
    }


def promotion_payload(version: str, current_latest: str | None) -> dict[str, str]:
    payload = {"version": version, "expected_tag": "next"}
    if current_latest is not None:
        payload["expected_current_version"] = current_latest
    return payload


def validate_version(version: str) -> None:
    if not VERSION_RE.fullmatch(version):
        raise RegistryError("version must be MAJOR.MINOR.PATCH with an optional -experimental suffix")


def inspect_candidate(
    api_url: str,
    worker: str,
    expected_next: str,
    expected_latest: str | None,
) -> dict[str, Any]:
    validate_version(expected_next)
    if expected_latest is not None:
        validate_version(expected_latest)

    current_latest = release_tag_version(api_url, worker, "latest", allow_missing=True)
    current_next = release_tag_version(api_url, worker, "next", allow_missing=True)
    if current_next != expected_next:
        raise RegistryError(f"next points to {current_next}, expected {expected_next}")
    if current_latest != expected_latest:
        raise RegistryError(f"latest points to {current_latest}, expected {expected_latest}")
    resolved_next = resolve_version(api_url, worker, "next")
    if resolved_next != expected_next:
        raise RegistryError(f"next resolves to {resolved_next}, expected {expected_next}")

    return {
        "worker": worker,
        "next": current_next,
        "latest": current_latest,
        "resolved_next": resolved_next,
    }


def promote(
    api_url: str,
    api_key: str,
    worker: str,
    version: str,
    expected_next: str,
    expected_latest: str | None,
) -> dict[str, Any]:
    if worker != WORKER_NAME:
        raise RegistryError(f"this pipeline only promotes {WORKER_NAME}")
    validate_version(version)
    validate_version(expected_next)
    if expected_latest is not None:
        validate_version(expected_latest)

    # Read raw pointers for CAS. Do not resolve latest before the mutation:
    # its dependency graph may be incompatible while its tag pointer is valid.
    current_latest = release_tag_version(api_url, worker, "latest", allow_missing=True)
    current_next = release_tag_version(api_url, worker, "next", allow_missing=True)
    if current_next != expected_next:
        raise RegistryError(f"next points to {current_next}, expected {expected_next}")
    if expected_next != version:
        raise RegistryError(f"promotion target {version} does not match expected next {expected_next}")

    if current_latest == version:
        resolved_latest = resolve_version(api_url, worker, "latest")
        if resolved_latest != version:
            raise RegistryError(f"latest resolves to {resolved_latest}, expected {version}")
        return {
            "worker": worker,
            "version": version,
            "previous_latest": current_latest,
            "next": current_next,
            "latest": current_latest,
            "changed": False,
            "registry_response": {"changed": False, "idempotent": True},
        }

    if current_latest != expected_latest:
        raise RegistryError(f"latest points to {current_latest}, expected {expected_latest}")

    encoded_worker = urllib.parse.quote(worker, safe="")
    status, response = request_json(
        "PUT",
        f"{api_url.rstrip('/')}/w/{encoded_worker}/tags/latest",
        promotion_payload(version, current_latest),
        api_key=api_key,
    )
    if status != 200:
        raise RegistryError(f"promotion failed with HTTP {status}: {json.dumps(response, sort_keys=True)}")

    promoted_tag = release_tag_version(api_url, worker, "latest")
    if promoted_tag != version:
        raise RegistryError(f"latest tag points to {promoted_tag}, expected {version}")
    promoted = resolve_version(api_url, worker, "latest")
    if promoted != version:
        raise RegistryError(f"latest resolves to {promoted}, expected {version}")

    return {
        "worker": worker,
        "version": version,
        "previous_latest": current_latest,
        "next": current_next,
        "latest": promoted,
        "changed": bool(response.get("changed")),
        "registry_response": response,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    for command in ("verify", "promote"):
        subparser = subparsers.add_parser(command)
        subparser.add_argument("--api-url", required=True)
        subparser.add_argument("--worker", default=WORKER_NAME)
        subparser.add_argument("--version", required=True)
        subparser.add_argument("--expected-next-version", required=True)
        subparser.add_argument("--expected-latest-version", required=True)
        subparser.add_argument("--output", type=Path, required=True)

    publish_parser = subparsers.add_parser("publish")
    publish_parser.add_argument("--api-url", required=True)
    publish_parser.add_argument("--payload", type=Path, required=True)
    publish_parser.add_argument("--assets", type=Path, required=True)
    publish_parser.add_argument("--assets-dir", type=Path, required=True)
    publish_parser.add_argument("--output", type=Path, required=True)

    args = parser.parse_args()
    try:
        api_key = os.environ.get("WORKERS_REGISTRY_API_KEY", "")
        if args.command != "verify" and not api_key:
            raise RegistryError("WORKERS_REGISTRY_API_KEY is required")
        if args.command == "publish":
            result = publish(
                args.api_url,
                api_key,
                json.loads(args.payload.read_text(encoding="utf-8")),
                json.loads(args.assets.read_text(encoding="utf-8")),
                args.assets_dir,
            )
        elif args.worker != WORKER_NAME:
            raise RegistryError(f"this pipeline only promotes {WORKER_NAME}")
        else:
            expected_latest = None if args.expected_latest_version == "none" else args.expected_latest_version
            if args.command == "verify":
                result = inspect_candidate(
                    args.api_url,
                    args.worker,
                    args.expected_next_version,
                    expected_latest,
                )
            else:
                result = promote(
                    args.api_url,
                    api_key,
                    args.worker,
                    args.version,
                    args.expected_next_version,
                    expected_latest,
                )
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        print(json.dumps(result, sort_keys=True))
        return 0
    except (OSError, RegistryError, ValueError) as error:
        print(f"error: {error}", file=os.sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
