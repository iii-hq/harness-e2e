#!/usr/bin/env python3
"""Decide whether consecutive shadow windows satisfy cutover parity."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def evaluate(paths: list[Path], required: int) -> dict:
    records = [json.loads(path.read_text(encoding="utf-8")) for path in paths]
    recent = records[-required:]
    identities = [
        (
            record.get("primary", {}).get("execution_id"),
            record.get("shadow", {}).get("execution_id"),
        )
        for record in recent
    ]
    unique = len(set(identities)) == len(identities)
    reasons = []
    if len(recent) < required:
        reasons.append(f"requires {required} consecutive windows; observed {len(recent)}")
    if not unique:
        reasons.append("shadow window execution identities are not unique")
    failed = [index for index, record in enumerate(recent, start=1) if not record.get("equivalent")]
    if failed:
        reasons.append(f"non-equivalent recent windows: {failed}")
    return {
        "schema_version": 1,
        "required_windows": required,
        "observed_windows": len(recent),
        "ready_for_cutover": len(recent) == required and unique and not failed,
        "windows": [
            {
                "primary_execution_id": primary,
                "shadow_execution_id": shadow,
                "equivalent": bool(record.get("equivalent")),
            }
            for (primary, shadow), record in zip(identities, recent, strict=True)
        ],
        "reasons": reasons,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("comparisons", nargs="+", type=Path)
    parser.add_argument("--required", type=int, default=3)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--require-ready", action="store_true")
    args = parser.parse_args()
    if args.required < 1:
        parser.error("--required must be positive")
    result = evaluate(args.comparisons, args.required)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=2, sort_keys=True))
    return 1 if args.require_ready and not result["ready_for_cutover"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
