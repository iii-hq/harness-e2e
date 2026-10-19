#!/usr/bin/env python3
"""Compare two arms of a Harness A/B: executions whose stacks differ only in
the harness worker (stacks/harness-ab/).

    python3 scripts/harness_ab.py A_DIR B_DIR [--label-a 1.8.44] [--label-b 1.8.50]

Each directory holds the observation artifacts of one arm, as `gh run
download` leaves them: every `results.json` below it is read, and a run seen
twice (a root bundle carries its groups' results too) counts once. Only
technically valid runs count, on both arms alike: an infrastructure failure is
not the harness's. A scenario is compared only where both arms ran the same
definition (`behavior_sha256`).

Per metric it prints arm A, arm B, B − A and B / A − 1, with 95% intervals
from a bootstrap stratified by scenario (runs resampled within each scenario
and arm). Completion and score are the mean over scenarios, each scenario
weighing the same; tokens, cost, time and call errors are the sum over
scenarios of each one's mean, what one pass over them takes.
"""

from __future__ import annotations

import argparse
import json
import random
import sys
from collections import defaultdict
from pathlib import Path

#: (key, label, how scenarios combine)
METRICS = (
    ("completed", "Conclusão (fração)", "mean"),
    ("score", "Nota", "mean"),
    ("input_tokens", "Tokens de entrada", "sum"),
    ("output_tokens", "Tokens de saída", "sum"),
    ("cost_usd", "Custo (US$)", "sum"),
    ("wall_s", "Tempo (s)", "sum"),
    ("call_errors", "Erros de chamada", "sum"),
)


def runs_of(directory: Path) -> dict[str, dict]:
    """Every valid run below `directory`, by run id."""
    runs = {}
    for path in sorted(directory.rglob("results.json")):
        try:
            results = json.loads(path.read_text())
        except (OSError, ValueError):
            continue
        for scenario in (results.get("scenarios") if isinstance(results, dict) else None) or []:
            for run in scenario.get("runs") or []:
                if run.get("technical") != "valid":
                    continue
                totals = (run.get("metrics") or {}).get("totals") or {}
                efficiency = run.get("efficiency") or {}
                runs[run.get("attempt_id") or run.get("run_id") or f"{path}:{len(runs)}"] = {
                    "scenario": scenario["scenario_id"],
                    "behavior": scenario.get("behavior_sha256"),
                    "completed": float(run.get("completion") == "completed"),
                    "score": run.get("score"),
                    "input_tokens": totals.get("input_tokens", efficiency.get("input_tokens")),
                    "output_tokens": totals.get("output_tokens", efficiency.get("output_tokens")),
                    "cost_usd": (run.get("cost") or {}).get("subject_usd", efficiency.get("cost_usd")),
                    "wall_s": None if run.get("wall_time_ms") is None else run["wall_time_ms"] / 1000,
                    "call_errors": totals.get("function_call_errors", efficiency.get("function_call_errors")),
                }
    return runs


def cells(runs: dict[str, dict]) -> dict[tuple[str, str], list[dict]]:
    by = defaultdict(list)
    for run in runs.values():
        by[(run["scenario"], run["behavior"])].append(run)
    return by


def mean(values: list[float]) -> float | None:
    return sum(values) / len(values) if values else None


def combine(per_scenario: list[float | None], how: str) -> float | None:
    values = [value for value in per_scenario if value is not None]
    if len(values) != len(per_scenario) or not values:
        return None
    return sum(values) if how == "sum" else sum(values) / len(values)


def statistic(arm: list[list[dict]], key: str, how: str) -> float | None:
    return combine([mean([run[key] for run in runs if run[key] is not None]) for runs in arm], how)


def interval(values: list[float]) -> tuple[float, float] | None:
    values = sorted(value for value in values if value is not None)
    if len(values) < 100:
        return None
    return values[int(0.025 * len(values))], values[int(0.975 * len(values)) - 1]


def compare(a: list[list[dict]], b: list[list[dict]], key: str, how: str,
            resamples: int, rng: random.Random) -> dict:
    base, treated = statistic(a, key, how), statistic(b, key, how)
    # One run is no sample of its own variance: no interval then.
    if min(len(runs) for runs in a + b) < 2:
        resamples = 0
    deltas, ratios = [], []
    for _ in range(resamples):
        sa = statistic([rng.choices(runs, k=len(runs)) for runs in a], key, how)
        sb = statistic([rng.choices(runs, k=len(runs)) for runs in b], key, how)
        if sa is None or sb is None:
            continue
        deltas.append(sb - sa)
        if sa:
            ratios.append(sb / sa - 1)
    return {
        "a": base, "b": treated,
        "delta": None if base is None or treated is None else treated - base,
        "delta_ci": interval(deltas),
        "relative": None if not base or treated is None else treated / base - 1,
        "relative_ci": interval(ratios),
    }


def analyse(a_runs: dict[str, dict], b_runs: dict[str, dict], resamples: int = 10000, seed: int = 0) -> dict:
    a_cells, b_cells = cells(a_runs), cells(b_runs)
    shared = sorted(set(a_cells) & set(b_cells))
    unmatched = sorted({scenario for scenario, _ in set(a_cells) ^ set(b_cells)})
    rng = random.Random(seed)
    a, b = [a_cells[cell] for cell in shared], [b_cells[cell] for cell in shared]
    return {
        "scenarios": [
            {"scenario": scenario, "behavior": behavior, "n_a": len(a_cells[(scenario, behavior)]),
             "n_b": len(b_cells[(scenario, behavior)]),
             "metrics": {key: compare([a_cells[(scenario, behavior)]], [b_cells[(scenario, behavior)]], key, "mean",
                                      resamples // 5, rng) for key, _, _ in METRICS}}
            for scenario, behavior in shared
        ],
        "pooled": {key: compare(a, b, key, how, resamples, rng) for key, _, how in METRICS},
        "unmatched": unmatched,
    }


def number(value: float | None, digits: int = 3) -> str:
    return "—" if value is None else f"{value:,.{digits}f}".replace(",", " ")


def percent(value: float | None, ci: tuple[float, float] | None) -> str:
    if value is None:
        return "—"
    text = f"{value:+.1%}"
    return text + (f" [{ci[0]:+.1%}, {ci[1]:+.1%}]" if ci else "")


def report(result: dict, label_a: str, label_b: str) -> str:
    lines = [f"## {label_b} contra {label_a}", "",
             f"| Métrica | {label_a} | {label_b} | Δ (IC 95%) | Δ relativo (IC 95%) |", "|---|---|---|---|---|"]
    for key, label, _ in METRICS:
        row = result["pooled"][key]
        ci = row["delta_ci"]
        delta = number(row["delta"]) + (f" [{number(ci[0])}, {number(ci[1])}]" if ci else "")
        lines.append(f"| {label} | {number(row['a'])} | {number(row['b'])} | {delta} | "
                     f"{percent(row['relative'], row['relative_ci'])} |")
    lines += ["", "### Por cenário (mesma behavior_sha256)", "",
              "| Cenário | n | Conclusão | Nota | Tokens de saída | Custo | Tempo | Erros de chamada |",
              "|---|---|---|---|---|---|---|---|"]
    for scenario in result["scenarios"]:
        metrics = scenario["metrics"]

        def cell(key: str) -> str:
            row = metrics[key]
            return f"{number(row['a'], 2)} → {number(row['b'], 2)}"

        lines.append(f"| {scenario['scenario']} | {scenario['n_a']}/{scenario['n_b']} | {cell('completed')} | "
                     f"{cell('score')} | {percent(metrics['output_tokens']['relative'], metrics['output_tokens']['relative_ci'])} | "
                     f"{percent(metrics['cost_usd']['relative'], metrics['cost_usd']['relative_ci'])} | "
                     f"{percent(metrics['wall_s']['relative'], metrics['wall_s']['relative_ci'])} | {cell('call_errors')} |")
    if result["unmatched"]:
        lines += ["", "Fora da comparação (só um braço, ou definições diferentes): " + ", ".join(result["unmatched"])]
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("a", type=Path)
    parser.add_argument("b", type=Path)
    parser.add_argument("--label-a", default="A")
    parser.add_argument("--label-b", default="B")
    parser.add_argument("--resamples", type=int, default=10000)
    parser.add_argument("--json", type=Path, help="also write the numbers here")
    args = parser.parse_args()
    result = analyse(runs_of(args.a), runs_of(args.b), args.resamples)
    if args.json:
        args.json.write_text(json.dumps(result, indent=2) + "\n")
    sys.stdout.write(report(result, args.label_a, args.label_b))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
