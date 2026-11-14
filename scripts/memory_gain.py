#!/usr/bin/env python3
"""Say how much better the memory cases go with memory than without it, per
model and per class, from the `memory-gain` runs of each arm.

An arm is the `memory-gain` suite run on one stack: `memory` (memory on),
`memory-off` and `memory-consolidated`. `--runs` lists one run per model and
arm, the arm named by its stack:

    python3 scripts/memory_gain.py --output DIR --runs '[{"model":
      "deepseek/deepseek-flash", "arm": "memory-off",
      "run_url": "https://github.com/iii-hq/harness-e2e/actions/runs/1"}]'

Each run's root bundle is downloaded and its results.json files split into
rows of the cases that have a `class`: all of them, each class (lookup,
synthesis) and those of the `memory-precise` module. Each run's score becomes
the points its answer criteria awarded (its `memory_answer_points`
measurement), so the memory-off arm loses nothing for the injection it never
makes. `harness-e2e compare --json` (HARNESS_E2E_BIN, or on PATH) then
compares memory-off with memory, and memory with memory-consolidated, row by
row: every figure and verdict is the runner's. The only sums here check the
arms, from each run's measurements: the off arm must inject nothing, and the
consolidated arm says what its passes did.

It writes DIR/memory-gain.md and DIR/memory-gain.json, prints the markdown and
appends it to the job summary. It exits 1 when a model has no quality figure
for memory against memory-off, or its memory-off arm injected.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, Callable

sys.path.insert(0, str(Path(__file__).resolve().parent))
from verify_stack import EXECUTOR_REPOSITORY, VerifyError, gh, run_id_of  # noqa: E402

OFF, ON, CONSOLIDATED = "memory-off", "memory", "memory-consolidated"
PLAN = Path(__file__).resolve().parent.parent / "config" / "test-plan.json"
INJECTED = ("memory_injected_tokens", "memory_rules_tokens", "memory_reminder_tokens")
PASS = {"superseded": "memory_consolidation_superseded", "llm_calls": "memory_consolidation_llm_calls",
        "notes": "memory_consolidation_notes"}
ANSWER = "memory_answer_points"
# shortcut: memory cuts a user line past 12,000 characters to its first and
# last 6,000, so this case scores the same on every arm; give it back to the
# rows once memory reads the whole line.
PROBES = {"memory_precise_long_message"}


def precise() -> set[str]:
    """The cases of the `memory-precise` module."""
    modules = json.loads(PLAN.read_text())["modules"]
    return set(next(module["scenarios"] for module in modules if module["id"] == "memory-precise"))


def rows() -> dict[str, Callable[[dict[str, Any]], bool]]:
    cases = precise()

    def scored(scenario: dict[str, Any]) -> bool:
        return scenario.get("class") in ("lookup", "synthesis") and scenario.get("scenario_id") not in PROBES

    return {
        "all": scored,
        "lookup": lambda scenario: scored(scenario) and scenario["class"] == "lookup",
        "synthesis": lambda scenario: scored(scenario) and scenario["class"] == "synthesis",
        "precise": lambda scenario: scored(scenario) and scenario.get("scenario_id") in cases,
    }


def answered(run: dict[str, Any]) -> dict[str, Any]:
    """The run scored on its answer criteria alone; no score without them."""
    points = next((measurement["value"] for measurement in run.get("scenario_measurements") or []
                   if measurement.get("id") == ANSWER), None)
    return {**run, "score": points}


def download(run_url: str, directory: Path) -> None:
    """The run's root bundle, as verify_stack.py reads it."""
    run_id = run_id_of(run_url)
    attempt = json.loads(gh("run", "view", str(run_id), "-R", EXECUTOR_REPOSITORY, "--json", "attempt"))["attempt"]
    gh("run", "download", str(run_id), "-R", EXECUTOR_REPOSITORY, "-n",
       f"e2e-observation-{run_id}-gh-{attempt}", "-D", str(directory))


def documents(arm: Path) -> list[tuple[Path, dict[str, Any]]]:
    """Every results.json below `arm` that parses to an object."""
    found = []
    for path in sorted(arm.rglob("results.json")):
        try:
            document = json.loads(path.read_text())
        except (OSError, ValueError):
            continue
        if isinstance(document, dict):
            found.append((path, document))
    return found


def split(arm: Path, keep: Callable[[dict[str, Any]], bool], out: Path) -> bool:
    """Every results.json below `arm`, at the same path below `out`, with only
    the scenarios `keep` accepts, each run scored on its answer alone, and
    every other field kept; whether any scenario was kept."""
    kept = False
    for path, document in documents(arm):
        scenarios = [{**scenario, "runs": [answered(run) for run in scenario.get("runs") or []]}
                     for scenario in document.get("scenarios") or []
                     if isinstance(scenario, dict) and keep(scenario)]
        if not scenarios:
            continue
        target = out / path.relative_to(arm)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(json.dumps({**document, "scenarios": scenarios}))
        kept = True
    return kept


def compare(a: Path, b: Path) -> dict[str, Any]:
    """`harness-e2e compare A B --json`: B against A."""
    binary = os.environ.get("HARNESS_E2E_BIN") or shutil.which("harness-e2e")
    if not binary:
        raise VerifyError("set HARNESS_E2E_BIN or put harness-e2e on PATH")
    done = subprocess.run([binary, "compare", str(a), str(b), "--json"], capture_output=True, text=True)
    if done.returncode:
        raise VerifyError(f"harness-e2e compare failed: {done.stderr.strip() or done.stdout.strip()}")
    return json.loads(done.stdout)


def measured(arm: Path, keep: Callable[[dict[str, Any]], bool] = lambda scenario: True) -> list[dict[str, float]]:
    """Each run's measurements in the scenarios `keep` accepts, a run read
    once by its attempt id: a root bundle repeats its groups' results."""
    seen: dict[tuple[str, str], dict[str, float]] = {}
    unnamed: list[dict[str, float]] = []
    for _, document in documents(arm):
        for scenario in document.get("scenarios") or []:
            for run in scenario.get("runs") or [] if isinstance(scenario, dict) and keep(scenario) else []:
                values = {measurement["id"]: measurement["value"]
                          for measurement in run.get("scenario_measurements") or []}
                attempt = run.get("attempt_id") or run.get("run_id")
                if attempt:
                    seen.setdefault((scenario.get("scenario_id"), attempt), values)
                else:
                    unnamed.append(values)
    return [*seen.values(), *unnamed]


def gain(arms: dict[str, Path], work: Path) -> dict[str, Any]:
    """One model's rows and arm checks."""
    report: dict[str, Any] = {"rows": {}, "errors": []}
    for row, keep in rows().items():
        sides = {arm: work / row / arm for arm in arms}
        present = {arm for arm in arms if split(arms[arm], keep, sides[arm])}
        compared: dict[str, Any] = {}
        for name, (a, b) in (("on_vs_off", (OFF, ON)), ("consolidated_vs_on", (ON, CONSOLIDATED))):
            if not {a, b} <= present:
                compared[name] = None
                continue
            try:
                compared[name] = compare(sides[a], sides[b])
            except (VerifyError, ValueError) as error:
                report["errors"].append(f"{row}, {b} against {a}: {error}")
                compared[name] = None
        report["rows"][row] = compared
    if OFF in arms:
        report["off_injected_tokens"] = sum(run.get(key, 0) for run in measured(arms[OFF]) for key in INJECTED)
    if CONSOLIDATED in arms:
        # The runs the rows compare whose setup finished: a run without
        # measurements never reached the consolidation step.
        runs = [run for run in measured(arms[CONSOLIDATED], rows()["all"]) if run]
        passes = [run for run in runs if PASS["superseded"] in run]
        report["consolidation"] = {"runs": len(runs), "consolidated": len(passes),
                                   **{key: sum(run.get(found, 0) for run in passes) for key, found in PASS.items()}}
        if passes and len(passes) < len(runs):
            report["errors"].append(f"{len(runs) - len(passes)} memory-consolidated runs never consolidated: "
                                    "memory-consolidated against memory mixes them in")
    return report


def number(value: float | None, sign: bool = False) -> str:
    return "-" if value is None else f"{value:+.1f}" if sign else f"{value:.1f}"


def verdict(comparison: dict[str, Any]) -> str:
    said = (comparison.get("verdict") or "unavailable").replace("_", " ")
    return f"{said} ({comparison['qualifier']})" if comparison.get("qualifier") else said


def markdown(models: dict[str, dict[str, Any]]) -> str:
    lines = ["# Memory gain", "",
             "A run scores the points its answer criteria awarded, nothing for what memory injected. "
             "Gain is memory's score minus memory-off's, in points, from `harness-e2e compare`. "
             "Consolidated is memory-consolidated's score minus memory's. The rows hold the cases "
             f"with a class, without {', '.join(sorted(PROBES))}: it scores the same on every arm "
             "until memory reads a user line past 12,000 characters.", ""]
    for model, report in models.items():
        lines += [f"## {model}", "",
                  "| Row | Tests | Off | On | Gain | Quality | Cost change | Verdict "
                  "| Consolidated - on | Quality | Verdict |",
                  "| --- | ---: | ---: | ---: | ---: | --- | ---: | --- | ---: | --- | --- |"]
        dropped, warnings = [0, 0], 0
        for row, compared in report["rows"].items():
            cells = [row]
            for index, name in enumerate(("on_vs_off", "consolidated_vs_on")):
                comparison = compared.get(name)
                quality = (comparison or {}).get("quality")
                if comparison:
                    dropped[index] += len(comparison.get("dropped") or [])
                    warnings += len(comparison.get("warnings") or [])
                if index == 0:
                    tests = quality["tests"] if quality else len((comparison or {}).get("tests") or [])
                    cost = (comparison or {}).get("cost")
                    cells += [str(tests), number(quality and quality["a"]), number(quality and quality["b"]),
                              number(quality and quality["delta"], sign=True),
                              quality["state"] if quality else "unavailable",
                              f"{cost['relative']:+.0%}" if cost else "unavailable"]
                else:
                    cells += [number(quality and quality["delta"], sign=True),
                              quality["state"] if quality else "unavailable"]
                cells.append(verdict(comparison) if comparison else "unavailable")
            lines.append("| " + " | ".join(cells) + " |")
        lines += ["", f"Tests left out: {dropped[0]} (memory against memory-off), {dropped[1]} "
                      f"(memory-consolidated against memory). Warnings: {warnings}."]
        if "off_injected_tokens" in report:
            injected = report["off_injected_tokens"]
            lines.append(f"- the memory-off arm injected {injected:g} tokens: injection was not off" if injected
                         else "- the memory-off arm injected nothing")
        if passes := report.get("consolidation"):
            lines.append(f"- consolidation ran in {passes['consolidated']} of {passes['runs']} runs, superseded "
                         f"{passes['superseded']:g} memories, {passes['llm_calls']:g} model calls, "
                         f"{passes['notes']:g} notes" if passes["consolidated"]
                         else "- no run consolidated: this arm is a second memory-on arm")
        lines += [f"- {error}" for error in report["errors"]]
        lines.append("")
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--runs", required=True, help="JSON list of {model, arm, run_url}")
    parser.add_argument("--output", required=True, type=Path, help="where memory-gain.md and .json go")
    args = parser.parse_args(argv)
    models: dict[str, dict[str, Any]] = {}
    failed = False
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        arms: dict[str, dict[str, Path]] = {}
        errors: dict[str, list[str]] = {}
        for index, run in enumerate(json.loads(args.runs)):
            directory = root / "runs" / str(index)
            try:
                download(run["run_url"], directory)
            except VerifyError as error:
                errors.setdefault(run["model"], []).append(f"{run['arm']}: {error}")
                continue
            arms.setdefault(run["model"], {})[run["arm"]] = directory
        for model in sorted(set(arms) | set(errors)):
            report = gain(arms.get(model, {}), root / "rows" / model.replace("/", "_"))
            report["errors"] = errors.get(model, []) + report["errors"]
            on_vs_off = report["rows"]["all"]["on_vs_off"]
            if not (on_vs_off and on_vs_off.get("quality")):
                report["errors"].append("no quality figure for memory against memory-off")
                failed = True
            if report.get("off_injected_tokens"):
                failed = True
            models[model] = report
    text = markdown(models)
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / "memory-gain.md").write_text(text)
    (args.output / "memory-gain.json").write_text(json.dumps(models, indent=2) + "\n")
    if path := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(path, "a") as out:
            out.write(text + "\n")
    print(text)
    return 1 if failed or not models else 0


if __name__ == "__main__":
    sys.exit(main())
