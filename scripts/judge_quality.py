#!/usr/bin/env python3
"""Compare local judge providers, or two builds of one, on the judge_quality_*
slices, from each arm's local results directory.

An arm is one provider build with one configuration, run on its own stack
(README, "Judge quality"):

    python3 scripts/judge_quality.py --output DIR [--baseline ARM] \\
      [--paired A B] --runs '[{"arm": "laya-main", "path": "target/judge-quality/laya-main"},
                               {"arm": "laya-branch", "path": "target/judge-quality/laya-branch"}]'

Each arm's results.json files give each slice's last run, its behavior_sha256
and its `judge_quality_evaluation` artifact, which holds the measurements and
one record per decision. Arms whose digests or decision ids differ for a slice
are refused. One table per slice, one column per arm; accuracy cells carry a
Wilson 95% interval with n capped at the slice's cases (five typed-decisions
questions share one state).

--baseline: per slice, b (baseline right, arm wrong) and c (the reverse); on
the pooled JevBench tiers, the exact two-sided sign test and a verdict per arm
(worse, better, cannot tell). --paired A B: per slice, the flips between A and
B with their sign test, the decisions whose answer changed, the largest
|delta p_gold| among decisions that did not flip, and the wrong counts on noul
decisions whose gold is "no"; overall, on the decisions whose routed model
differs, and on those whose input_tokens are equal or differ.

Writes DIR/judge-quality.md and .json and prints the markdown. Exits 1 when an
arm lacks a slice, on a refusal, or when an arm is worse than the baseline.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
from memory_gain import documents  # noqa: E402

SLICES = ["judge_quality_jevbench_easy", "judge_quality_jevbench_original",
          "judge_quality_jevbench_hard", "judge_quality_typed_decisions"]
JEVBENCH = SLICES[:3]
POOLED = "jevbench (pooled)"
ARTIFACT = "judge_quality_evaluation"
ROWS = ["cases", "decisions", "answered", "accuracy", "majority_accuracy", "agreed_n", "accuracy_agreed",
        "choice_n", "choice_top1", "choice_nll", "choice_ece", "score_n", "score_accuracy", "score_mae",
        "noul_n", "noul_accuracy", "noul_accuracy_at_0_8", "noul_brier", "noul_auroc", "input_tokens_max",
        "case_ms_p50", "case_ms_p95"]
# Each accuracy row and the count it is a share of.
SHARE_OF = {"accuracy": "decisions", "accuracy_agreed": "agreed_n", "choice_top1": "choice_n",
            "score_accuracy": "score_n", "noul_accuracy": "noul_n", "noul_accuracy_at_0_8": "noul_n"}
NOTES = [
    "typed-decisions gold is a teacher's argmax that the teacher itself confirms on 59.4% of decisions: "
    "read accuracy_agreed and majority_accuracy, never pool it with JevBench. laya's typed-decisions "
    "checkpoint was trained on its train split.",
    "decider's training generators were written from the family names JevBench publishes, so JevBench "
    "favours decider.",
    "Read every b + c against the baseline arm run twice (its b + c is the noise floor).",
]


def wilson(p: float, n: int) -> tuple[float, float]:
    z = 1.959964
    denominator = 1 + z * z / n
    centre = (p + z * z / (2 * n)) / denominator
    half = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / denominator
    return centre - half, centre + half


def sign_test(b: int, c: int) -> float:
    n = b + c
    return min(1.0, 2 * sum(math.comb(n, k) for k in range(min(b, c) + 1)) / 2 ** n) if n else 1.0


def load_arm(path: Path) -> dict[str, dict[str, Any]]:
    """Each slice's last run in the arm: {sha, evidence}."""
    slices: dict[str, dict[str, Any]] = {}
    for results, document in documents(path):
        for scenario in document.get("scenarios") or []:
            if scenario.get("scenario_id") not in SLICES or not scenario.get("runs"):
                continue
            artifact = next((deliverable.get("artifact") or {} for deliverable in scenario["runs"][-1].get(
                "deliverables") or [] if deliverable.get("id") == ARTIFACT), {})
            found = [place / artifact["path"] for place in (results.parent, path) if artifact.get("path")]
            found = [place for place in found if place.is_file()]
            if found:
                slices[scenario["scenario_id"]] = {"sha": scenario.get("behavior_sha256"),
                                                   "evidence": json.loads(found[0].read_text())}
    return slices


def decisions(evidence: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {decision["id"]: decision for decision in evidence["decisions"]}


def answer(decision: dict[str, Any]) -> Any:
    """What the arm answered: None when unanswered, the noul side, else the
    most likely label."""
    if not decision.get("answered"):
        return None
    if decision.get("gold_yes") is not None:
        return decision["gold_yes"] == decision["correct"]
    # shortcut: a choice's returned label is not kept on the decision, so its argmax stands in; store the
    # label if a provider ever answers off its own argmax.
    probabilities = decision.get("probabilities") or {}
    return max(probabilities, key=probabilities.get, default=None)


def pair(a: dict[str, dict[str, Any]], b: dict[str, dict[str, Any]], ids: list[str]) -> dict[str, Any]:
    """Flips from arm a to arm b over `ids`: b counts a right and b wrong;
    `changed` also counts answers that moved without changing correctness."""
    worse = sum(1 for i in ids if a[i]["correct"] and not b[i]["correct"])
    better = sum(1 for i in ids if b[i]["correct"] and not a[i]["correct"])
    changed = sum(1 for i in ids if a[i]["correct"] != b[i]["correct"] or answer(a[i]) != answer(b[i]))
    deltas = [abs(a[i]["p_gold"] - b[i]["p_gold"]) for i in ids if a[i]["correct"] == b[i]["correct"]
              and a[i].get("p_gold") is not None and b[i].get("p_gold") is not None]
    no = [i for i in ids if a[i].get("gold_yes") is False]
    return {"n": len(ids), "b": worse, "c": better, "p": sign_test(worse, better), "changed": changed,
            "max_delta_p_gold": max(deltas, default=None),
            "noul_gold_no_wrong": [sum(1 for i in no if not arm[i]["correct"]) for arm in (a, b)]}


PAIRED_SUBSETS = ["all", "model_differs", "input_tokens_equal", "input_tokens_differ"]


def paired(a: dict[str, dict[str, Any]], b: dict[str, dict[str, Any]]) -> dict[str, Any]:
    ids = sorted(a)
    same_tokens = {i: a[i].get("input_tokens") == b[i].get("input_tokens") for i in ids}
    return {
        "all": pair(a, b, ids),
        "model_differs": pair(a, b, [i for i in ids if a[i].get("model") != b[i].get("model")]),
        "input_tokens_equal": pair(a, b, [i for i in ids if same_tokens[i]]),
        "input_tokens_differ": pair(a, b, [i for i in ids if not same_tokens[i]]),
    }


def cell(measurements: dict[str, float], row: str, cases: int) -> str:
    if row not in measurements:
        return ""
    value = measurements[row]
    if row in SHARE_OF and measurements.get(SHARE_OF[row]):
        n = int(min(measurements[SHARE_OF[row]], cases))
        low, high = wilson(value, n)
        return f"{value:.3f} [{low:.3f}, {high:.3f}] n={n}"
    return f"{value:.0f}" if float(value).is_integer() else f"{value:.3f}"


def pair_text(result: dict[str, Any]) -> str:
    delta = result["max_delta_p_gold"]
    return (f"n={result['n']} b={result['b']} c={result['c']} p={result['p']:.4f} changed={result['changed']} "
            f"max|dp_gold|={'-' if delta is None else f'{delta:.2e}'} "
            f"noul gold-no wrong={'/'.join(map(str, result['noul_gold_no_wrong']))}")


def report(arms: dict[str, dict[str, dict[str, Any]]], baseline: str | None,
           paired_arms: list[str] | None) -> dict[str, Any]:
    out: dict[str, Any] = {"arms": {}, "slices": {}, "baseline": {}, "paired": {}, "errors": []}
    for arm, slices in arms.items():
        evidence = next(iter(slices.values()), {}).get("evidence", {})
        out["arms"][arm] = {key: evidence.get(key) for key in ("provider", "version", "config")}
        out["arms"][arm]["release_dates"] = {card.get("name"): card.get("release_date")
                                             for card in evidence.get("cards") or []}
        out["errors"] += [f"{arm}: {slice_id} not measured" for slice_id in SLICES if slice_id not in slices]
    for slice_id in SLICES:
        present = {arm: slices[slice_id] for arm, slices in arms.items() if slice_id in slices}
        if len({entry["sha"] for entry in present.values()}) > 1:
            digests = ", ".join(f"{arm} {entry['sha']}" for arm, entry in present.items())
            out["errors"].append(f"refused: {slice_id} has different definitions ({digests})")
        elif len({tuple(sorted(decisions(entry["evidence"]))) for entry in present.values()}) > 1:
            out["errors"].append(f"refused: {slice_id} has different decision ids across arms")
        else:
            out["slices"][slice_id] = present
    for arm in arms:
        if all(slice_id in out["slices"] and arm in out["slices"][slice_id] for slice_id in JEVBENCH):
            pooled = [d for s in JEVBENCH for d in out["slices"][s][arm]["evidence"]["decisions"]]
            out["arms"][arm]["pooled_accuracy"] = (sum(d["correct"] for d in pooled), len(pooled))
    if baseline:
        for arm in (arm for arm in arms if arm != baseline):
            result, pooled_a, pooled_b = {}, {}, {}
            for slice_id, present in out["slices"].items():
                if baseline in present and arm in present:
                    a, b = decisions(present[baseline]["evidence"]), decisions(present[arm]["evidence"])
                    result[slice_id] = pair(a, b, sorted(a))
                    if slice_id in JEVBENCH:
                        pooled_a.update(a)
                        pooled_b.update(b)
            if all(slice_id in result for slice_id in JEVBENCH):
                pooled = result[POOLED] = pair(pooled_a, pooled_b, sorted(pooled_a))
                verdict = "cannot tell"
                if pooled["p"] < 0.05:
                    verdict = "worse" if pooled["b"] > pooled["c"] else "better"
                pooled["verdict"] = verdict
                if verdict == "worse":
                    out["errors"].append(f"{arm} is worse than {baseline} on pooled JevBench")
            out["baseline"][arm] = result
    if paired_arms:
        a_arm, b_arm = paired_arms
        for slice_id, present in out["slices"].items():
            if a_arm in present and b_arm in present:
                out["paired"][slice_id] = paired(decisions(present[a_arm]["evidence"]),
                                                 decisions(present[b_arm]["evidence"]))
    return out


def markdown(out: dict[str, Any], arms: list[str], baseline: str | None, paired_arms: list[str] | None) -> str:
    lines = ["# Judge quality", "", "| Arm | Provider | Version | Config | Release dates |", "|---|---|---|---|---|"]
    for arm in arms:
        info = out["arms"][arm]
        lines.append(f"| {arm} | {info['provider']} | {info['version']} | `{json.dumps(info['config'])}` | "
                     f"{', '.join(f'{k} {v}' for k, v in info['release_dates'].items())} |")
    for slice_id in SLICES:
        present = out["slices"].get(slice_id, {})
        lines += ["", f"## {slice_id}", "", "| | " + " | ".join(arms) + " |", "|---" * (len(arms) + 1) + "|"]
        for row in ROWS:
            values = []
            for arm in arms:
                if arm not in present:
                    values.append("not measured")
                    continue
                evidence = present[arm]["evidence"]
                measured = {m["id"]: m["value"] for m in evidence["measurements"]}
                values.append(cell(measured, row, int(measured.get("cases", 0))))
            if any(values):
                lines.append(f"| {row} | " + " | ".join(values) + " |")
        mixes = []
        for arm in arms:
            models: dict[str, int] = {}
            for record in present[arm]["evidence"]["cases"] if arm in present else []:
                models[record.get("model") or "none"] = models.get(record.get("model") or "none", 0) + 1
            mixes.append(", ".join(f"{model} {count}" for model, count in sorted(models.items())))
        lines.append("| models | " + " | ".join(mixes) + " |")
    lines += ["", f"## {POOLED}", "", "| Arm | Accuracy |", "|---|---|"]
    for arm in arms:
        if "pooled_accuracy" in out["arms"][arm]:
            correct, n = out["arms"][arm]["pooled_accuracy"]
            low, high = wilson(correct / n, n)
            lines.append(f"| {arm} | {correct / n:.3f} [{low:.3f}, {high:.3f}] n={n} |")
    if baseline:
        lines += ["", f"## Against {baseline}", "", "b: baseline right, arm wrong. c: the reverse.", ""]
        for arm, result in out["baseline"].items():
            for slice_id, numbers in result.items():
                verdict = f" **{numbers['verdict']}**" if "verdict" in numbers else ""
                lines.append(f"- {arm}, {slice_id}: b={numbers['b']} c={numbers['c']}"
                             + (f" p={numbers['p']:.4f}{verdict}" if slice_id == POOLED else ""))
    if paired_arms:
        lines += ["", f"## Paired: {paired_arms[0]} against {paired_arms[1]}", "",
                  f"b: {paired_arms[0]} right, {paired_arms[1]} wrong. c: the reverse.", ""]
        for slice_id, result in out["paired"].items():
            lines.append(f"- {slice_id}")
            for subset in PAIRED_SUBSETS:
                lines.append(f"  - {subset}: {pair_text(result[subset])}")
    lines += ["", "## Notes", ""] + [f"- {note}" for note in NOTES]
    if out["errors"]:
        lines += ["", "## Errors", ""] + [f"- {error}" for error in out["errors"]]
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--runs", required=True, help="JSON list of {arm, path}")
    parser.add_argument("--output", required=True, type=Path, help="where judge-quality.md and .json go")
    parser.add_argument("--baseline", help="the arm every other arm is paired with")
    parser.add_argument("--paired", nargs=2, metavar=("A", "B"), help="two arms to pair in detail")
    args = parser.parse_args(argv)
    runs = json.loads(args.runs)
    arms = {run["arm"]: load_arm(Path(run["path"]).expanduser()) for run in runs}
    for name in [args.baseline, *(args.paired or [])]:
        if name and name not in arms:
            parser.error(f"{name} is not an arm of --runs")
    out = report(arms, args.baseline, args.paired)
    text = markdown(out, list(arms), args.baseline, args.paired)
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / "judge-quality.md").write_text(text)
    serializable = {**out, "slices": {slice_id: {arm: entry["sha"] for arm, entry in present.items()}
                                      for slice_id, present in out["slices"].items()}}
    (args.output / "judge-quality.json").write_text(json.dumps(serializable, indent=2) + "\n")
    print(text)
    return 1 if out["errors"] else 0


if __name__ == "__main__":
    sys.exit(main())
