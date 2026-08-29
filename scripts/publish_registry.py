#!/usr/bin/env python3
"""Publish harness-e2e to the current Registry with exact effect readback."""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from typing import Any, Literal


MAX_ATTEMPTS = 3


class PublicationError(RuntimeError):
    pass


class TransportError(RuntimeError):
    pass


@dataclass(frozen=True)
class Proof:
    state: Literal["present", "absent", "unknown", "divergent"]
    detail: str


def request_json(
    method: str,
    url: str,
    payload: dict[str, Any] | None = None,
    *,
    api_key: str | None = None,
) -> tuple[int, dict[str, Any]]:
    body = json.dumps(payload, separators=(",", ":")).encode() if payload is not None else None
    headers = {"Accept": "application/json"}
    if payload is not None:
        headers["Content-Type"] = "application/json"
    if api_key:
        headers["X-API-Key"] = api_key
    request = urllib.request.Request(url, data=body, headers=headers, method=method)
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            raw = response.read().decode()
            return response.status, json.loads(raw) if raw else {}
    except urllib.error.HTTPError as error:
        raw = error.read().decode(errors="replace")
        try:
            parsed = json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            parsed = {"error": raw}
        return error.code, parsed
    except (TimeoutError, urllib.error.URLError, OSError) as error:
        raise TransportError(str(error)) from error


def _object(value: Any, field: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise PublicationError(f"{field} must be an object")
    return value


def _read_payload(path: pathlib.Path, worker: str, version: str) -> dict[str, Any]:
    payload = _object(json.loads(path.read_text(encoding="utf-8")), "payload")
    if payload.get("worker_name") != worker or payload.get("version") != version:
        raise PublicationError("payload identity differs from the requested release")
    if payload.get("type") != "binary" or payload.get("tag") != "next":
        raise PublicationError("harness-e2e publication must be binary on next")
    if not isinstance(payload.get("binaries"), dict) or not payload["binaries"]:
        raise PublicationError("payload binaries must be non-empty")
    if "package_descriptor" in payload or "descriptor_sha256" in payload or "channel" in payload:
        raise PublicationError("payload contains fields unsupported by the current Registry")
    return payload


def _resolved_node(body: dict[str, Any], worker: str, version: str) -> dict[str, Any] | None:
    root = body.get("root")
    graph = body.get("graph")
    if not isinstance(root, dict) or root.get("name") != worker or root.get("version") != version:
        return None
    if not isinstance(graph, list):
        return None
    matches = [row for row in graph if isinstance(row, dict) and row.get("name") == worker]
    return matches[0] if len(matches) == 1 and matches[0].get("version") == version else None


def _absent(status: int, body: dict[str, Any]) -> bool:
    error = body.get("error")
    code = error.get("code") if isinstance(error, dict) else None
    return status == 404 or (status == 422 and code in {"worker_not_found", "version_not_found"})


def prove(api_url: str, worker: str, version: str, payload: dict[str, Any]) -> Proof:
    base = api_url.rstrip("/")
    encoded = urllib.parse.quote(worker, safe="")
    try:
        exact_status, exact = request_json("POST", f"{base}/resolve", {"worker": worker, "version": version})
        next_status, next_body = request_json("POST", f"{base}/resolve", {"worker": worker, "version": "next"})
        versions_status, versions = request_json("GET", f"{base}/w/{encoded}/versions")
    except TransportError as error:
        return Proof("unknown", f"Registry readback transport failed: {error}")

    if _absent(exact_status, exact):
        return Proof("absent", "exact version is absent")
    if exact_status != 200 or next_status != 200 or versions_status != 200:
        return Proof(
            "unknown",
            f"readback returned exact={exact_status}, next={next_status}, versions={versions_status}",
        )

    node = _resolved_node(exact, worker, version)
    next_node = _resolved_node(next_body, worker, version)
    if node is None or next_node is None:
        return Proof("divergent", "exact or next resolution does not identify the requested version")
    expected_dependencies = {
        row["name"]: row["version"]
        for row in payload.get("dependencies", [])
        if isinstance(row, dict) and isinstance(row.get("name"), str) and isinstance(row.get("version"), str)
    }
    expected = {
        "type": payload["type"],
        "dependencies": expected_dependencies,
        "binaries": payload["binaries"],
    }
    actual = {key: node.get(key) for key in expected}
    if actual != expected:
        return Proof("divergent", "resolved type, dependencies, or binaries differ from prepared payload")

    rows = versions.get("versions")
    if not isinstance(rows, list):
        return Proof("divergent", "versions readback has no versions array")
    tagged = [
        row.get("version")
        for row in rows
        if isinstance(row, dict) and isinstance(row.get("tags"), list) and "next" in row["tags"]
    ]
    if tagged != [version]:
        return Proof("divergent", f"raw next pointer is {tagged}, expected {[version]}")
    return Proof("present", "exact version, artifacts, dependencies, and next pointer match")


def publish(api_url: str, api_key: str, worker: str, version: str, payload: dict[str, Any]) -> dict[str, Any]:
    last = "publication was not attempted"
    for attempt in range(1, MAX_ATTEMPTS + 1):
        transport = None
        try:
            status, response = request_json("POST", f"{api_url.rstrip('/')}/publish", payload, api_key=api_key)
        except TransportError as error:
            status, response, transport = 0, {}, str(error)

        if status not in {0, 200, 201, 409} and not 500 <= status <= 599:
            raise PublicationError(f"publish failed with HTTP {status}: {json.dumps(response, sort_keys=True)}")
        proof = prove(api_url, worker, version, payload)
        if proof.state == "present":
            state = "unchanged" if status == 409 else ("changed" if status in {200, 201} else "recovered")
            return {"state": state, "attempt": attempt, "proof": proof.detail}
        if proof.state in {"divergent", "unknown"}:
            raise PublicationError(f"publish effect is {proof.state}: {proof.detail}")
        if status == 409:
            raise PublicationError("HTTP 409 is not idempotent: exact candidate effect is absent")
        last = f"{transport}; {proof.detail}" if transport else proof.detail
        if attempt < MAX_ATTEMPTS:
            time.sleep(attempt)
    raise PublicationError(f"publish remained absent after {MAX_ATTEMPTS} attempts: {last}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--api-url", required=True)
    parser.add_argument("--worker", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--payload", type=pathlib.Path, required=True)
    parser.add_argument("--out", type=pathlib.Path, required=True)
    args = parser.parse_args()
    try:
        key = os.environ.get("WORKERS_REGISTRY_API_KEY", "")
        if not key:
            raise PublicationError("WORKERS_REGISTRY_API_KEY is required")
        payload = _read_payload(args.payload, args.worker, args.version)
        receipt = publish(args.api_url, key, args.worker, args.version, payload)
        args.out.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        print(json.dumps(receipt, sort_keys=True))
        return 0
    except (OSError, json.JSONDecodeError, PublicationError) as error:
        print(f"registry publication: {error}", file=os.sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
