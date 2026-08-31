#!/usr/bin/env python3
"""Build immutable metadata for campaign assets shipped with a runner release."""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
from typing import Any


ROOT = pathlib.Path(__file__).resolve().parents[1]


def canonical_sha256(value: Any) -> str:
    payload = json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    return f"sha256:{hashlib.sha256(payload).hexdigest()}"


def asset(path: pathlib.Path) -> dict[str, str]:
    path = path.resolve()
    value = json.loads(path.read_text(encoding="utf-8"))
    return {
        "path": str(path.relative_to(ROOT)),
        "sha256": canonical_sha256(value),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--catalog", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    metadata = {
        "schema": "harness-e2e-campaign-assets/v1",
        "runner": {
            "name": "harness-e2e",
            "version": args.version,
            "revision": args.revision,
        },
        "catalog": asset(args.catalog),
        "scoring_profiles": [asset(path) for path in sorted((ROOT / "config/scoring").glob("*.json"))],
        "campaigns": [asset(path) for path in sorted((ROOT / "config/campaigns").glob("*.json"))],
        "fault_profiles": [asset(path) for path in sorted((ROOT / "config/profiles").glob("weekly-*.json"))],
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
