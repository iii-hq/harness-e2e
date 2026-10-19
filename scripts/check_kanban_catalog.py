#!/usr/bin/env python3
"""Warn when the Kanban fixture's catalog differs from the runner's.

scripts/kanban_eval/bootstrap.py stops every Kanban group whose fixture
checkout carries a catalog other than src/scenarios/kanban/catalog.json, byte
for byte. This reads the catalog at the commits those groups check out (the
head prepare_execution.py resolves, and the pin it falls back to) and says
ahead of time when they differ. Only a warning: a catalog that moved is a
contract change, not a reason to hold a pull request or a release back.
"""

from __future__ import annotations

import os
from pathlib import Path
import urllib.request

import prepare_execution as prepare

CATALOG = "src/scenarios/kanban/catalog.json"
EMBEDDED = Path(__file__).resolve().parents[1] / CATALOG
FIXTURE_CATALOG = "scenarios/catalog.json"


def warn(message: str) -> None:
    print(f"::warning file={CATALOG},title=Kanban catalog drift::{message}")


def fetch(repository: str, revision: str) -> bytes:
    url = f"https://raw.githubusercontent.com/{repository}/{revision}/{FIXTURE_CATALOG}"
    with urllib.request.urlopen(url, timeout=30) as response:
        return response.read()


def main() -> int:
    """How many of those commits carry another catalog; the exit stays 0."""
    repository, ref, _, pin = prepare.FIXTURES["kanban-e2e-fixture"]
    token = os.environ.get("GITHUB_TOKEN") or None
    embedded = EMBEDDED.read_bytes()
    drifted = 0
    for label, name in ((f"the head of {ref}", ref), ("the pinned fallback", pin)):
        try:
            revision = str(prepare.get_json(f"{prepare.GITHUB_API_URL}/repos/{repository}/commits/{name}",
                                            token=token)["sha"])
            catalog = fetch(repository, revision)
        except (prepare.ResolutionError, OSError, KeyError, TypeError) as error:
            warn(f"{repository}@{name} ({label}) could not be read: {error}")
            continue
        if catalog == embedded:
            print(f"{repository}@{revision} ({label}): catalog matches")
            continue
        drifted += 1
        warn(f"{repository}@{revision} ({label}) carries a {FIXTURE_CATALOG} that differs from "
             f"{CATALOG}; every Kanban group checking it out stops in the bootstrap. "
             f"Copy the runner's catalog into the fixture byte for byte, then move the pin in "
             f"scripts/prepare_execution.py FIXTURES.")
    return drifted


if __name__ == "__main__":
    main()
