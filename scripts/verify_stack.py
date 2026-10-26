#!/usr/bin/env python3
"""Run a suite (`pr` unless `--suite` says) on a candidate stack from another
repository's pipeline and say what it measured.

The caller names what changed: workers at a commit (`--pins`, `--commit`),
released versions (`--versions`), or an iii release (`--iii`). This tool builds
the stack from `stacks/default.yaml`, or the one `--stack` names, dispatches `exact-stack-e2e.yml` without an execution (its reports only go to
artifacts), waits for the run and reads `execution-summary.json`:

    passed        every group measured, its objective outcome `passed` and
                  every one of its runs completed (`completion_rate` 1.0)
    failed        a measured group's objective outcome is `failed`, or it
                  passed with runs that did not complete
    not_measured  the run produced no summary, a group is inconclusive, its
                  infrastructure was invalid or it has no completion
                  evidence, or the run never finished
    skipped       nothing the caller changed is part of the harness graph (or
                  of the stack `--stack` names)

A green workflow proves nothing here: the campaign is advisory, so the
verdict is read from the summary. Only `passed` and `skipped` exit 0, unless
`--exit-zero` leaves the decision to the caller, which reads `status`.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import secrets
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
#: Where the named stacks live.
STACKS = DEFAULT_STACK.parent
RUN_URL = re.compile(r"/actions/runs/(\d+)")
POLL_SECONDS = 20
DEADLINE_SECONDS = 45 * 60
#: The API's bad moments (5xx, a dropped connection): worth another try.
#: Anything else (a 4xx: inputs, credentials, permissions) is not going to
#: pass and still fails the measurement at once.
UNAVAILABLE = re.compile(r"HTTP 5\d\d\b|error connecting|connection reset|\bEOF\b|timed? ?out", re.IGNORECASE)
#: A failed `gh run view` worth another poll: those, and a run GitHub has only
#: just created, which is not readable for a moment (404).
TRANSIENT = re.compile(rf"HTTP 404\b|{UNAVAILABLE.pattern}", re.IGNORECASE)
#: How long to wait, after a dispatch GitHub did not answer, before looking
#: for its run and dispatching again: two minutes in all.
#: ponytail: a run GitHub lists more than 20 s after its 5xx is dispatched
#: twice; cancel the other runs with the same request if that shows up.
DISPATCH_WAITS = (20, 40, 60)
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


def build_stack(pins: list[str], commit: str, iii: str, graph: set[str], stack: str = "default",
                versions: dict[str, str] | None = None) -> dict[str, Any] | None:
    """The `stack` of stacks/ with the pinned workers built from `commit` and
    `versions` of its workers installed.

    On the default stack a pinned worker outside the harness graph is not part
    of what runs, so it is dropped, and the harness is always pinned with the
    rest, at the same commit, so the Registry does not bring a second copy of a
    worker that is being built from source. On a named stack a pin is one of
    its containers, built with that container's settings kept; the others stay
    as the stack has them. No pin left, and no version or iii release asked
    for, means nothing to verify (None).
    """
    named = stack != "default"
    built = load_yaml((STACKS / f"{stack}.yaml" if named else DEFAULT_STACK).read_text())
    containers = built["containers"]
    if iii:
        built["iii"] = iii
    for name, version in (versions or {}).items():
        if name not in containers:
            raise VerifyError(f"the {stack} stack declares no {name} to install {version} of")
        containers[name]["version"] = version
    pinned = sorted(set(pins) & (set(containers) if named else graph))
    if pins and not pinned and not (versions or iii):
        return None
    for name in sorted(set(pinned) if named or not pinned else {APPLICATION, *pinned}):
        kept = {key: value for key, value in containers.get(name, {}).items()
                if named and key not in ("worker", "version")}
        containers[name] = {"worker": f"package://{name}", **kept, "repository": WORKERS_REPOSITORY,
                            "commit": commit}
    return built


def marker(suite: str) -> str:
    """The comment marker of `suite`: the `pr` suite keeps the original one."""
    return MARKER if suite == "pr" else f"<!-- harness-e2e-verify:{suite} -->"


def pinned(stack: dict[str, Any]) -> list[str]:
    """The containers the stack builds from a commit."""
    return sorted(name for name, container in stack["containers"].items()
                  if isinstance(container, dict) and "commit" in container)


def run_id_of(output: str) -> int:
    match = RUN_URL.search(output)
    if not match:
        raise VerifyError(f"gh did not print the run URL: {output.strip()!r}")
    return int(match[1])


def find_run(request: str, token: str | None) -> int | None:
    """The run whose title carries `request`, if a dispatch created it."""
    runs = json.loads(gh("run", "list", "-R", EXECUTOR_REPOSITORY, "-w", WORKFLOW, "-e", "workflow_dispatch",
                         "-L", "100", "--json", "databaseId,displayTitle", token=token))
    return next((run["databaseId"] for run in runs if run["displayTitle"].endswith(f" · {request}")), None)


def dispatch(stack: dict[str, Any], suite: str, model: str, token: str | None, stop: list[bool]) -> int:
    """Start the run and say its id, through the API's bad moments.

    A 5xx does not say the run was not created, and dispatching again blind
    would pay for the suite twice. So before another dispatch the run is
    looked for by `request`, which its title carries, and while that lookup
    cannot answer either, nothing is dispatched. Once the job is cancelled
    (`stop`), nothing more is dispatched either: the run is only looked for,
    so that it can be cancelled too.
    """
    import yaml

    request = secrets.token_hex(4)
    args = ("workflow", "run", WORKFLOW, "-R", EXECUTOR_REPOSITORY, "--ref", "main",
            "-f", f"suite={suite}", "-f", f"model={model}", "-f", f"request={request}",
            "-f", f"stack={yaml.safe_dump(stack, sort_keys=False)}")
    failure: VerifyError | None = None
    for attempt, wait in enumerate((0, *DISPATCH_WAITS)):
        time.sleep(wait)
        try:
            if failure and (found := find_run(request, token)):
                return found
            # The last round only looks, and so does every round once the job
            # is cancelled: a dispatch nobody looks for after would leave a run
            # nobody reads.
            if attempt < len(DISPATCH_WAITS) and not (failure and stop):
                return run_id_of(gh(*args, token=token, merged=True))
        except VerifyError as error:
            if not UNAVAILABLE.search(str(error)):
                raise
            failure = error
    raise failure


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

    # A passed objective whose runs did not all complete (a stall the scoring
    # does not see) is no pass: it fails, and the rate says why. Without
    # completion evidence (null) there is nothing to say either way.
    passed = [g for g in groups if measured(g) and g["objective_outcome"] == "passed" and g.get("completion_rate") == 1.0]
    failed = [name(g) for g in groups if measured(g) and g["objective_outcome"] == "failed"]
    failed += [f"{name(g)} ({g['completion_rate']:.0%} of runs completed)" for g in groups
               if measured(g) and g["objective_outcome"] == "passed" and g.get("completion_rate") not in (None, 1.0)]
    unmeasured = [name(g) for g in groups if not measured(g)
                  or (g["objective_outcome"] == "passed" and g.get("completion_rate") is None)]
    planned = max(planned, len(groups))
    # Nothing measured is never a pass: 0 of 0 groups says nothing about the stack.
    complete = planned > 0 and not unmeasured and len(passed) == planned
    status = "failed" if failed else "passed" if complete else "not_measured"
    return {"status": status, "passed": len(passed), "planned": planned,
            "failed": failed, "not_measured": unmeasured}


def label(built: list[str], commit: str, versions: dict[str, str], iii: str, suite: str) -> str:
    """What the stack measured: the workers built from `commit`, the released
    versions it installs (a worker built from the commit runs no version) and
    the iii release, after the suite when it is not `pr`."""
    named = " + ".join(filter(None, [f"{', '.join(built)} @ {commit[:7]}" if built else "",
                                     *(f"{name} {version}" for name, version in versions.items()
                                       if name not in built),
                                     f"iii {iii}" if iii else ""]))
    return named if suite == "pr" else f"{suite} · {named}"


def describe(result: dict[str, Any], label: str, run_url: str, mark: str = MARKER) -> str:
    lines = [mark, f"### {ICONS[result['status']]} Harness E2E · {label}"]
    if result["status"] == "skipped":
        lines.append(result.get("reason") or "Nothing this change touches is part of the harness graph.")
    else:
        lines.append(f"{result['passed']}/{result['planned']} passed")
    if result.get("failed"):
        lines.append(f"- failed: {'; '.join(result['failed'])}")
    if result.get("not_measured"):
        lines.append(f"- not measured: {'; '.join(result['not_measured'])}")
    if result.get("reason") and result["status"] != "skipped":
        lines.append(f"- {result['reason']}")
    if run_url:
        lines.append(f"[Run]({run_url})")
    return "\n".join(lines)


def comment(target: str, body: str, token: str | None, mark: str = MARKER) -> None:
    """Create or update the one comment carrying `mark` on `owner/repo#N`."""
    repository, _, number = target.partition("#")
    listing = gh("api", "--paginate", f"repos/{repository}/issues/{number}/comments",
                 "-q", f'.[] | select(.body | contains("{mark}")) | .id', token=token).split()
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

    run_id = dispatch(stack, suite, model, token, stop)
    dispatched.append(run_id)
    run_url = f"https://github.com/{EXECUTOR_REPOSITORY}/actions/runs/{run_id}"
    if stop:
        cancelled()

    deadline = time.monotonic() + DEADLINE_SECONDS
    while True:
        try:
            run = json.loads(gh("run", "view", str(run_id), "-R", EXECUTOR_REPOSITORY,
                                "--json", "status,attempt,conclusion", token=token))
            lookup_error = ""
        except VerifyError as error:
            if not TRANSIENT.search(str(error)):
                raise
            run, lookup_error = {"status": "unknown"}, str(error)
        if run["status"] == "completed":
            break
        if time.monotonic() > deadline:
            cancel()
            last = f" (last lookup: {lookup_error})" if lookup_error else ""
            return {"status": "not_measured", "passed": 0, "planned": 0, "failed": [], "not_measured": [],
                    "reason": f"the run did not finish in {DEADLINE_SECONDS // 60} minutes{last}"}, run_url
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
    parser.add_argument("--versions", default="{}", help="JSON object of released worker versions to install")
    parser.add_argument("--suite", default="pr")
    parser.add_argument("--stack", default="default", help="the stack of stacks/ the suite runs on")
    parser.add_argument("--model", default="deepseek/deepseek-flash")
    parser.add_argument("--comment-pr", default="", metavar="OWNER/REPO#N")
    parser.add_argument("--exit-zero", action="store_true", help="exit 0 whatever the verdict")
    args = parser.parse_args()

    pins = json.loads(args.pins)
    versions = json.loads(args.versions)
    if pins and not args.commit:
        parser.error("--pins needs --commit")
    if not pins and not args.iii and not versions:
        parser.error("nothing to verify: give --pins, --versions or --iii")
    token = os.environ.get("GH_TOKEN")
    run_url, stack = "", None
    try:
        graph = graph_workers() if pins and args.stack == "default" else set()
        stack = build_stack(pins, args.commit, args.iii, graph, args.stack, versions)
        if stack is None:
            result = {"status": "skipped", "passed": 0, "planned": 0, "failed": [], "not_measured": []}
            if args.stack != "default":
                result["reason"] = f"Nothing this change touches is in the {args.stack} stack."
        else:
            result, run_url = measure(stack, args.suite, args.model, token)
    # Whatever went wrong (the Registry, gh, the summary), the caller still
    # needs a verdict and the PR its comment.
    except Exception as error:  # noqa: BLE001
        result = {"status": "not_measured", "passed": 0, "planned": 0, "failed": [], "not_measured": [],
                  "reason": f"{type(error).__name__}: {error}"}

    built = pinned(stack) if stack else pins
    mark = marker(args.suite)
    body = describe(result, label(built, args.commit, versions, args.iii, args.suite), run_url, mark)
    emit(result, body, run_url)
    if args.comment_pr and result["status"] != "skipped":
        try:
            comment(args.comment_pr, body, os.environ.get("COMMENT_GH_TOKEN") or token, mark)
        except VerifyError as error:
            print(f"::warning::could not update the PR comment: {error}", file=sys.stderr)
    return 0 if args.exit_zero or result["status"] in ("passed", "skipped") else 1


if __name__ == "__main__":
    sys.exit(main())
