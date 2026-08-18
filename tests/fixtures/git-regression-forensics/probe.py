#!/usr/bin/env python3
"""Classify one checkout without revealing where the regression began."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import sys
import time


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("repository", type=Path)
    parser.add_argument("--trace", type=Path, required=True)
    args = parser.parse_args()

    repository = args.repository.resolve()
    revision = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=repository,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()

    started = time.monotonic_ns()
    executed = subprocess.run(
        [sys.executable, "get_pi.py"],
        cwd=repository,
        check=False,
        capture_output=True,
        text=True,
    )
    duration_ms = (time.monotonic_ns() - started) // 1_000_000
    observed = executed.stdout.strip()
    passed = executed.returncode == 0 and observed == "3.14"
    record = {
        "revision": revision,
        "passed": passed,
        "program_exit_code": executed.returncode,
        "observed_stdout": observed,
        "duration_ms": duration_ms,
    }
    args.trace.parent.mkdir(parents=True, exist_ok=True)
    with args.trace.open("a", encoding="utf-8") as stream:
        stream.write(json.dumps(record, sort_keys=True) + "\n")
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
