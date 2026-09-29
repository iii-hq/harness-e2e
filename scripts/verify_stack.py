#!/usr/bin/env python3
"""Run the `pr` suite on a candidate stack from another repository's pipeline
and say what it measured.

The caller names what changed: workers at a commit (`--pins`, `--commit`), or
an iii release (`--iii`). This tool builds the stack from `stacks/default.yaml`,
dispatches `exact-stack-e2e.yml` without an execution (its reports only go to
artifacts), waits for the run and reads `execution-summary.json`:

    passed        every group measured and its objective outcome is `passed`
    failed        a measured group's objective outcome is `failed`
    not_measured  the run produced no summary, a group is inconclusive or its
                  infrastructure was invalid, or the run never finished
    skipped       nothing the caller changed is part of the harness graph

A green workflow proves nothing here: the campaign is advisory, so the
verdict is read from the summary. Only `passed` and `skipped` exit 0, unless
`--exit-zero` leaves the decision to the caller, which reads `status`.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
from exact_stack_campaign import APPLICATION, DEFAULT_STACK, load_yaml  # noqa: E402
from prepare_execution import release_graph  # noqa: E402

EXECUTOR_REPOSITORY = "iii-hq/harness-e2e"
#: Where the workers are built from. The Registry no longer points a release at
#: a GitHub asset, so the executor cannot find the repository by itself.
WORKERS_REPOSITORY = "iii-hq/workers"
WORKFLOW = "exact-stack-e2e.yml"
#: Tells the comment this tool owns apart from every other one on the PR.
MARKER = "<!-- harness-e2e-verify -->"
RUN_URL = re.compile(r"/actions/runs/(\d+)")
POLL_SECONDS = 20
DEADLINE_SECONDS = 45 * 60
ICONS = {"passed": "🟢", "failed": "🔴", "not_measured": "🟡", "skipped": "⚪"}


class VerifyError(RuntimeError):
    """The run could not be started, finished or read."""


def gh(*args: str, token: str | None = None, merged: bool = False) -> str:
    """What `gh` printed; `merged` adds stderr, where some commands put their link."""
    env = {**os.environ, **({"GH_TOKEN": token} if token else {})}
    done = subprocess.run(["gh", *args], capture_output=True, text=True, env=env)
    if done.returncode:
        raise VerifyError(f"gh {' '.join(args[:3])} failed: {done.stderr.strip() or done.stdout.strip()}")
    return done.stdout + done.stderr if merged else done.stdout


def graph_workers() -> set[str]:
    """The workers the harness release is made of, engine built-ins left out."""
    nodes = release_graph(APPLICATION).get("graph") or []
    return {node["name"] for node in nodes if node.get("type") != "engine"}


def build_stack(pins: list[str], commit: str, iii: str, graph: set[str]) -> dict[str, Any] | None:
    """The default stack with the pinned workers built from `commit`.

    A pinned worker outside the harness graph is not part of what runs, so it
    is dropped; none left means nothing to verify (None). The harness is
    always pinned with them, at the same commit, so the Registry does not
    bring a second copy of a worker that is being built from source.
    """
    stack = load_yaml(DEFAULT_STACK.read_text())
    if iii:
        stack["iii"] = iii
    pinned = sorted(set(pins) & graph)
    if pins and not pinned:
        return None
    if pinned:
        for name in sorted({APPLICATION, *pinned}):
            stack["containers"][name] = {"worker": f"package://{name}", "repository": WORKERS_REPOSITORY,
                                         "commit": commit}
    return stack


def pinned(stack: dict[str, Any]) -> list[str]:
    """The containers the stack builds from a commit."""
    return sorted(name for name, container in stack["containers"].items()
                  if isinstance(container, dict) and "commit" in container)


def run_id_of(output: str) -> int:
    match = RUN_URL.search(output)
    if not match:
        raise VerifyError(f"gh did not print the run URL: {output.strip()!r}")
    return int(match[1])


def verdict(summary: dict[str, Any]) -> dict[str, Any]:
    """What the summary says: a status and the groups behind it."""
    campaigns = summary.get("campaigns") or []
    groups = [group for campaign in campaigns for group in campaign.get("groups") or []]
    planned = sum((campaign.get("scoring") or {}).get("expected_groups") or len(campaign.get("groups") or [])
                  for campaign in campaigns)

    def measured(group: dict[str, Any]) -> bool:
        return (group.get("infrastructure_valid") is True and group.get("report_state") == "complete"
                and group.get("objective_outcome") in ("passed", "failed"))

    def name(group: dict[str, Any]) -> str:
        return ", ".join(group.get("scenarios") or []) or str(group.get("group_id"))

    passed = [g for g in groups if measured(g) and g["objective_outcome"] == "passed"]
    failed = [name(g) for g in groups if measured(g) and g["objective_outcome"] == "failed"]
    unmeasured = [name(g) for g in groups if not measured(g)]
    planned = max(planned, len(groups))
    # Nothing measured is never a pass: 0 of 0 groups says nothing about the stack.
    complete = planned > 0 and not unmeasured and len(passed) == planned
    status = "failed" if failed else "passed" if complete else "not_measured"
    return {"status": status, "passed": len(passed), "planned": planned,
            "failed": failed, "not_measured": unmeasured}


def describe(result: dict[str, Any], label: str, run_url: str) -> str:
    lines = [MARKER, f"### {ICONS[result['status']]} Harness E2E · {label}"]
    if result["status"] == "skipped":
        lines.append("Nothing this change touches is part of the harness graph.")
    else:
        lines.append(f"{result['passed']}/{result['planned']} passed")
    if result.get("failed"):
        lines.append(f"- failed: {'; '.join(result['failed'])}")
    if result.get("not_measured"):
        lines.append(f"- not measured: {'; '.join(result['not_measured'])}")
    if result.get("reason"):
        lines.append(f"- {result['reason']}")
    if run_url:
        lines.append(f"[Run]({run_url})")
    return "\n".join(lines)


def comment(target: str, body: str, token: str | None) -> None:
    """Create or update the one comment carrying MARKER on `owner/repo#N`."""
    repository, _, number = target.partition("#")
    listing = gh("api", "--paginate", f"repos/{repository}/issues/{number}/comments",
                 "-q", f'.[] | select(.body | contains("{MARKER}")) | .id', token=token).split()
    if listing:
        gh("api", "-X", "PATCH", f"repos/{repository}/issues/comments/{listing[0]}", "-f", f"body={body}", token=token)
    else:
        gh("api", "-X", "POST", f"repos/{repository}/issues/{number}/comments", "-f", f"body={body}", token=token)


def emit(result: dict[str, Any], body: str, run_url: str) -> None:
    """Outputs for the next steps of the calling job, and its summary page."""
    summary = f"{result['passed']}/{result['planned']}"
    if result.get("failed"):
        summary += f" · failed: {'; '.join(result['failed'])}"
    if result.get("not_measured"):
        summary += f" · not measured: {'; '.join(result['not_measured'])}"
    if result.get("reason"):
        summary += f" · {result['reason']}"
    fields = {"status": result["status"], "summary": " ".join(summary.split()), "run_url": run_url}
    if path := os.environ.get("GITHUB_OUTPUT"):
        with open(path, "a") as out:
            out.write("".join(f"{key}={value}\n" for key, value in fields.items()))
    if path := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(path, "a") as out:
            out.write(body + "\n")
    print(body)


def measure(stack: dict[str, Any], suite: str, model: str, token: str | None) -> tuple[dict[str, Any], str]:
    import yaml

    dispatched: list[int] = []
    stop: list[bool] = []

    def cancel() -> None:
        # GitHub refuses to cancel a run it has only just queued (409) for a
        # few seconds; a cancelled job leaves about ten before it is killed.
        for _ in range(8):
            done = subprocess.run(["gh", "run", "cancel", str(dispatched[0]), "-R", EXECUTOR_REPOSITORY],
                                  capture_output=True, env={**os.environ, **({"GH_TOKEN": token} if token else {})})
            if done.returncode == 0:
                return
            time.sleep(1)

    # A newer push cancels the calling job; the run it started should not go
    # on. A signal while the dispatch is still in flight waits for its run id:
    # exiting then would leave a run nobody can name.
    def cancelled(*_: Any) -> None:
        if not dispatched:
            stop.append(True)
            return
        cancel()
        sys.exit(143)

    signal.signal(signal.SIGTERM, cancelled)
    signal.signal(signal.SIGINT, cancelled)

    started = gh("workflow", "run", WORKFLOW, "-R", EXECUTOR_REPOSITORY, "--ref", "main",
                 "-f", f"suite={suite}", "-f", f"model={model}",
                 "-f", f"stack={yaml.safe_dump(stack, sort_keys=False)}", token=token, merged=True)
    run_id = run_id_of(started)
    dispatched.append(run_id)
    run_url = f"https://github.com/{EXECUTOR_REPOSITORY}/actions/runs/{run_id}"
    if stop:
        cancelled()

    deadline = time.monotonic() + DEADLINE_SECONDS
    while True:
        run = json.loads(gh("run", "view", str(run_id), "-R", EXECUTOR_REPOSITORY,
                            "--json", "status,attempt,conclusion", token=token))
        if run["status"] == "completed":
            break
        if time.monotonic() > deadline:
            cancel()
            return {"status": "not_measured", "passed": 0, "planned": 0, "failed": [], "not_measured": [],
                    "reason": f"the run did not finish in {DEADLINE_SECONDS // 60} minutes"}, run_url
        time.sleep(POLL_SECONDS)

    with tempfile.TemporaryDirectory() as directory:
        try:
            gh("run", "download", str(run_id), "-R", EXECUTOR_REPOSITORY, "-n",
               f"e2e-observation-{run_id}-gh-{run['attempt']}", "-D", directory, token=token)
            summary = json.loads((Path(directory) / "execution-summary.json").read_text())
        except (VerifyError, OSError, ValueError) as error:
            return {"status": "not_measured", "passed": 0, "planned": 0, "failed": [], "not_measured": [],
                    "reason": f"the run concluded {run.get('conclusion')} and left no readable summary ({error})"}, run_url
    return verdict(summary), run_url


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--pins", default="[]", help="JSON list of the workers the change touches")
    parser.add_argument("--commit", default="", help="the commit those workers are built from")
    parser.add_argument("--iii", default="", help="an iii release version to install")
    parser.add_argument("--suite", default="pr")
    parser.add_argument("--model", default="deepseek/deepseek-flash")
    parser.add_argument("--comment-pr", default="", metavar="OWNER/REPO#N")
    parser.add_argument("--exit-zero", action="store_true", help="exit 0 whatever the verdict")
    args = parser.parse_args()

    pins = json.loads(args.pins)
    if pins and not args.commit:
        parser.error("--pins needs --commit")
    if not pins and not args.iii:
        parser.error("nothing to verify: give --pins or --iii")
    token = os.environ.get("GH_TOKEN")
    run_url, stack = "", None
    try:
        stack = build_stack(pins, args.commit, args.iii, graph_workers() if pins else set())
        if stack is None:
            result = {"status": "skipped", "passed": 0, "planned": 0, "failed": [], "not_measured": []}
        else:
            result, run_url = measure(stack, args.suite, args.model, token)
    # Whatever went wrong (the Registry, gh, the summary), the caller still
    # needs a verdict and the PR its comment.
    except Exception as error:  # noqa: BLE001
        result = {"status": "not_measured", "passed": 0, "planned": 0, "failed": [], "not_measured": [],
                  "reason": f"{type(error).__name__}: {error}"}

    built = pinned(stack) if stack else pins
    label = " + ".join(filter(None, [f"{', '.join(built)} @ {args.commit[:7]}" if built else "",
                                     f"iii {args.iii}" if args.iii else ""]))
    body = describe(result, label, run_url)
    emit(result, body, run_url)
    if args.comment_pr and result["status"] != "skipped":
        try:
            comment(args.comment_pr, body, os.environ.get("COMMENT_GH_TOKEN") or token)
        except VerifyError as error:
            print(f"::warning::could not update the PR comment: {error}", file=sys.stderr)
    return 0 if args.exit_zero or result["status"] in ("passed", "skipped") else 1


if __name__ == "__main__":
    sys.exit(main())
