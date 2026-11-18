#!/usr/bin/env python3
"""Fetch the group observation bundles the finalizer aggregates.

It lists the jobs of this attempt of the run and the run's artifacts, picks
one bundle per group job, writes the pick to target/selected-group-artifacts.json
and downloads each bundle into target/downloaded-groups/<artifact name>/.

A completed group job, named `<campaign> · <group>` and the only job with
that name, gets the single unexpired artifact named
`e2e-observation-<EXECUTION_KEY>-<campaign>-<group>-gh-<K>` with
CONTRACT_ATTEMPT <= K <= GITHUB_RUN_ATTEMPT that was created while the job
ran. A rerun copies the jobs it does not run again with new ids but keeps
their execution times, so each job gets the bundle of its last real
execution, and a rerun of the finalizer alone gets every group's. The attempt
range refuses bundles made under an older contract (a full rerun prepares a
new one); the time window refuses older evidence for a rerun that uploaded
none. A group job without a bundle is not an error: restore reads it as never
observed.

Every request is asked again on 429, 408, 5xx, a broken connection, a body
that does not match its digest, and a 403 that is a rate limit, waiting what
GitHub names (Retry-After, or until X-RateLimit-Reset) and never less than
2, 4, 8 ... 60 s. A rate limit that names no wait waits 60 s, as GitHub
asks. Any other answer is definitive. A listing or a bundle that
cannot be fetched stops everything: the step fails, nothing is aggregated,
and target/harness-e2e-campaign/execution-summary.json says why.
"""

from __future__ import annotations

import hashlib
import http.client
import io
import json
import os
import re
import shutil
import sys
import time
import urllib.error
import urllib.request
import zipfile
from collections import Counter
from datetime import datetime
from pathlib import Path
from typing import Any

PAGE = 50  # shortcut: GitHub answers 502 to 100 copied jobs a page; lower it if 50 starts failing
ATTEMPTS = 7  # a request: waits of 2, 4, 8, 16, 32, 60 s when GitHub names none and it is no rate limit
BUDGET = 600  # seconds of waiting in the whole fetch; the job has 30 minutes
SELECTED = Path("target/selected-group-artifacts.json")
BUNDLES = Path("target/downloaded-groups")
SUMMARY = Path("target/harness-e2e-campaign/execution-summary.json")

waited = 0.0


class FetchError(Exception):
    """A listing or bundle that could not be fetched, and why."""


class KeepTokenAtHome(urllib.request.HTTPRedirectHandler):
    """The zip endpoint redirects to a signed blob URL, which must not get the token."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        request = super().redirect_request(req, fp, code, msg, headers, newurl)
        if request is not None:
            request.remove_header("Authorization")
        return request


OPENER = urllib.request.build_opener(KeepTokenAtHome)


def api(path: str) -> str:
    base = os.environ.get("GITHUB_API_URL", "https://api.github.com")
    return f"{base}/repos/{os.environ['GITHUB_REPOSITORY']}/{path}"


def named_wait(headers: Any) -> float | None:
    """The seconds GitHub asked to wait, or None when it named no wait."""
    try:
        if headers.get("Retry-After") is not None:
            return max(0.0, float(headers["Retry-After"]))
        if headers.get("X-RateLimit-Remaining") == "0":
            return max(0.0, float(headers.get("X-RateLimit-Reset") or 0) - time.time())
    except ValueError:
        pass
    return None


def get(url: str, what: str, digest: str | None = None) -> bytes:
    """The body `url` answers, asked again while the answer is transient.
    Messages name `what`, never the URL: a redirected one carries a signature."""
    global waited
    for attempt in range(1, ATTEMPTS + 1):
        # A new Request each time: urllib counts redirects on it, and would
        # read the same redirect on every retry as a loop.
        request = urllib.request.Request(url, headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {os.environ['GITHUB_TOKEN']}",
            "X-GitHub-Api-Version": "2022-11-28",
        })
        named = 0.0
        try:
            with OPENER.open(request, timeout=60) as response:
                body = response.read()
            if digest is None or digest == "sha256:" + hashlib.sha256(body).hexdigest():
                return body
            cause = "a body that does not match its digest"
        except urllib.error.HTTPError as error:
            try:
                text = error.read().decode(errors="replace").lower()
            except (OSError, http.client.HTTPException):
                text = ""
            limited = error.code == 403 and (
                error.headers.get("Retry-After") is not None
                or error.headers.get("X-RateLimit-Remaining") == "0"
                or "rate limit" in text
            )
            if error.code not in (408, 429) and error.code < 500 and not limited:
                raise FetchError(f"{what} answered HTTP {error.code}") from None
            named = named_wait(error.headers)
            if named is None:
                # GitHub: a rate limit that names no wait is waited out for at least a minute.
                named = 60.0 if limited or error.code == 429 else 0.0
            cause = f"HTTP {error.code}"
        except (OSError, http.client.HTTPException) as error:
            cause = f"{type(error).__name__}: {error}"
        if attempt == ATTEMPTS:
            raise FetchError(f"{what} answered {cause}, {ATTEMPTS} times")
        wait = max(named, min(60, 2 ** attempt))
        if waited + wait > BUDGET:
            raise FetchError(f"{what} answered {cause} and asked for {wait:g}s more than the {BUDGET}s to wait")
        print(f"::warning::{what} answered {cause}; asking again in {wait:g}s", file=sys.stderr)
        time.sleep(wait)
        waited += wait


def listing(path: str, key: str) -> list[dict]:
    """Every item of a paginated GitHub listing."""
    items: list[dict] = []
    page = 1
    while True:
        what = f"the {key} listing, page {page}"
        body = get(api(f"{path}?per_page={PAGE}&page={page}"), what)
        # Not asked again: a 200 is definitive, and a truncated body already fails in get() as IncompleteRead.
        try:
            batch = json.loads(body)[key]
            if not isinstance(batch, list):
                raise TypeError
        except (ValueError, KeyError, TypeError):
            raise FetchError(f"{what} answered no {key} list") from None
        items += batch
        if len(batch) < PAGE:
            return items
        page += 1


def select(jobs: list[dict], artifacts: list[dict], execution_key: str,
           first_attempt: int, current_attempt: int) -> dict[str, dict]:
    """Each group job's bundle, by job name (the rule in the module docstring)."""
    names = Counter(job["name"] for job in jobs)
    selected = {}
    for job in jobs:
        if job["status"] != "completed" or names[job["name"]] != 1 or " · " not in job["name"]:
            continue
        campaign, group = job["name"].split(" · ", 1)
        prefix = f"e2e-observation-{execution_key}-{campaign}-{group}-gh-"
        try:
            started = datetime.fromisoformat(job["started_at"].replace("Z", "+00:00"))
            completed = datetime.fromisoformat(job["completed_at"].replace("Z", "+00:00"))
        except (AttributeError, TypeError, ValueError):
            continue
        if started.tzinfo is None or completed.tzinfo is None or started > completed:
            continue
        matches = []
        for artifact in artifacts:
            match = re.fullmatch(re.escape(prefix) + r"([1-9][0-9]*)", artifact["name"])
            if not match or artifact["expired"] or not first_attempt <= int(match[1]) <= current_attempt:
                continue
            try:
                created = datetime.fromisoformat(artifact["created_at"].replace("Z", "+00:00"))
            except (AttributeError, TypeError, ValueError):
                continue
            # Partial reruns copy successful jobs with new IDs/attempts
            # but retain their execution times. Never substitute older
            # evidence for a rerun that failed to upload an artifact.
            if created.tzinfo is not None and started <= created <= completed:
                matches.append({"id": artifact["id"], "name": artifact["name"], "run_attempt": int(match[1]),
                                "digest": artifact.get("digest")})
        if len(matches) == 1:
            selected[job["name"]] = matches[0]
    return selected


def download(artifact: dict) -> None:
    # shortcut: a bundle is read into memory (about 250 KB each), write it to a file if bundles grow near the runner's memory
    body = get(api(f"actions/artifacts/{artifact['id']}/zip"), artifact["name"], artifact.get("digest"))
    try:
        with zipfile.ZipFile(io.BytesIO(body)) as archive:
            archive.extractall(BUNDLES / artifact["name"])
    except zipfile.BadZipFile as error:
        raise FetchError(f"{artifact['name']} is not a zip: {error}") from None


def failed(code: str, cause: str, selected: int, downloaded: int) -> int:
    """Record why nothing is aggregated, where the finalizer's report reads it."""
    message = f"the group artifacts could not be fetched: {cause}"
    if code == "group_artifact_download_failed":
        message += f" ({downloaded} of {selected} downloaded)"
    SUMMARY.parent.mkdir(parents=True, exist_ok=True)
    SUMMARY.write_text(json.dumps({
        "campaigns": [],
        "unavailable": f"{message}; nothing was aggregated",
        "outcome": "infra_failed",
        "phase": "workflow_artifact_download",
        "failure": {"phase": "workflow_artifact_download", "code": code, "message": message,
                    "selected": selected, "downloaded": downloaded},
    }, indent=2) + "\n")
    print(f"::error::{message}; nothing was aggregated", file=sys.stderr)
    return 1


def main() -> int:
    global waited
    waited = 0.0
    shutil.rmtree(BUNDLES, ignore_errors=True)
    SELECTED.unlink(missing_ok=True)
    attempt = int(os.environ["GITHUB_RUN_ATTEMPT"])
    run = f"actions/runs/{os.environ['GITHUB_RUN_ID']}"
    try:
        jobs = listing(f"{run}/attempts/{attempt}/jobs", "jobs")
        artifacts = listing(f"{run}/artifacts", "artifacts")
    except FetchError as error:
        return failed("group_artifact_listing_failed", str(error), 0, 0)
    selected = select(jobs, artifacts, os.environ["EXECUTION_KEY"], int(os.environ["CONTRACT_ATTEMPT"]), attempt)
    SELECTED.parent.mkdir(parents=True, exist_ok=True)
    SELECTED.write_text(json.dumps(selected, indent=2) + "\n")
    groups = sorted({job["name"] for job in jobs if " · " in job["name"]})
    print(f"selected {len(selected)} of {len(groups)} group jobs")
    for name in groups:
        if name not in selected:
            print(f"  no artifact for {name}")
    # shortcut: one download at a time (bursts trip GitHub's secondary limit; 111 bundles take about a minute), use a small pool if a campaign outgrows the job's 30 minutes
    for count, artifact in enumerate(selected.values()):
        try:
            download(artifact)
        except FetchError as error:
            return failed("group_artifact_download_failed", str(error), len(selected), count)
    print(f"downloaded {len(selected)} group artifacts")
    return 0


if __name__ == "__main__":
    sys.exit(main())
