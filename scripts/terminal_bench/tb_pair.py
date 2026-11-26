"""Paired summary of iii harness vs Codex CLI Harbor rounds (one mixed job dir per round).

    python3 -I tb_pair.py [--gate | --pilot] <job dir> [<job dir> ...]

Each trial is one cell, classified by phase and the same way for both arms:
  stop   account quota or auth error on either arm; reported, never counted
  infra  failed before the agent's first model response, or Harbor's verifier failed;
         its task and round are dropped from both arms
  valid  everything else after the first model response; the graded reward, or 0 without one
A (task, round) pair counts only when both arms' cells are valid. Each round is gated on its own
(gate_reasons); a round that fails is void: plain mode lists it on a VOID line and leaves it out of
the VERDICT, which is the outcome of the paired bootstrap over tasks of the counted rounds, or
inconclusive when dropping Harbor-invalid cells gives another outcome. DROPPED lines end with the
trial dir, for the per-round infra resume. The last stdout line is a JSON object of counts over
every round given. --gate exits 1 when any round fails its gate. --pilot prints one line per
trial and PILOT_READY: yes|no.
"""
import argparse
import glob
import json
import math
import os
import random
import re
import statistics
import sys
from collections import Counter
from datetime import datetime

EFFORT, CODEX_MODEL, CODEX_CLI = "xhigh", "gpt-6.2-sol", "0.170.0"
MARGIN, SEED, BOOT = -0.08, 20261124, 10_000
# Per arm per round, after the round's one infra pass.
INFRA_MAX, NO_EVIDENCE_MAX = 3, 2
# Pilot flag, not a failure: iii / Codex median reasoning tokens per generation with reasoning.
REASONING_FLAG = (0.5, 2.0)
STOP_TYPES = {"AgentAuthenticationError", "AuthenticationError", "ApiUsageLimitError", "ApiRateLimitError"}
# Only the backend's error code: Harbor's codex messages embed the task instruction, which may say "rate limit".
STOP_TEXT = re.compile(r"usage_limit_reached")
# Harbor grades these invalid (error_policy.py): the host or the verifier failed after the agent ran.
HOST_FAILURES = {"VerifierTimeoutError", "RewardFileNotFoundError", "RewardFileEmptyError",
                 "VerifierOutputParseError", "AddTestsDirError", "DownloadVerifierDirError"}
ARMS = ("iii", "codex")
SECONDARY = ("input_tokens", "output_tokens", "generations", "reasoning_generations", "tool_calls", "wall_s")


def _json(path, default=None):
    try:
        with open(path) as f:
            return json.load(f)
    except FileNotFoundError:
        return default
    except json.JSONDecodeError as e:  # Harbor writes result.json in place: a killed run can leave it cut
        print(f"tb_pair: unreadable {path}: {e}", file=sys.stderr)
        return default


def _jsonl(path):
    with open(path) as f:
        for line in f:
            try:
                yield json.loads(line)
            except json.JSONDecodeError:  # blank or cut last line of a killed run
                continue


def model_response(m):
    """The adapter's rule (iii_harbor_agent.model_response): harness 1.10 stores an empty placeholder per call."""
    return m.get("role") == "assistant" and (bool(m.get("content")) or ((m.get("usage") or {}).get("output") or 0) > 0)


def codex_evidence(trial):
    ev = {"first": False, "generations": 0, "reasoning_generations": 0, "tool_calls": 0, "ctx": [], "cli": set(),
          "plans": set(), "reasoning": [], "scan": None}
    for path in sorted(glob.glob(os.path.join(trial, "agent", "sessions", "**", "rollout-*.jsonl"), recursive=True)):
        start, pending = None, False
        for row in _jsonl(path):
            kind, p = row.get("type"), row.get("payload") or {}
            if kind == "session_meta":
                start = p.get("subagent_history_start_ordinal")
                ev["cli"].add(p.get("cli_version"))
                continue
            # A sub-agent rollout copies its parent's history first; Harbor's codex.py skips it the same way.
            if isinstance(start, int) and not (isinstance(row.get("ordinal"), int) and row["ordinal"] >= start):
                continue
            if kind == "turn_context":
                ev["ctx"].append((p.get("model"), p.get("effort")))
            elif kind == "response_item":
                t = p.get("type") or ""
                if t == "reasoning" or t.endswith("_call") or (t == "message" and p.get("role") == "assistant"):
                    ev["first"] = pending = True
                    ev["tool_calls"] += t.endswith("_call")
            elif kind == "event_msg" and p.get("type") == "token_count":
                # Codex 0.170.0 reports the ChatGPT plan here; an API-key login has none.
                ev["plans"].add((p.get("rate_limits") or {}).get("plan_type"))
                if pending:  # a token_count after model output closes one model call, as in Harbor's codex.py
                    pending = False
                    ev["generations"] += 1
                    usage = (p.get("info") or {}).get("last_token_usage") or {}
                    ev["reasoning"].append(usage.get("reasoning_output_tokens") or 0)
                    ev["reasoning_generations"] += ev["reasoning"][-1] > 0
    out = os.path.join(trial, "agent", "codex.txt")
    if not ev["first"] and os.path.exists(out):  # sessions are copied in a cleanup step that can fail
        ev["first"] = any(str(e.get("type", "")).startswith("item.") and (e.get("item") or {}).get("type") != "error"
                          for e in _jsonl(out) if isinstance(e, dict))
    # True, False (wrong effort or no plan login), or None: no rollout or no token_count to read.
    if ev["ctx"] and set(ev["ctx"]) != {(CODEX_MODEL, EFFORT)}:
        ev["parity"] = False
    else:
        ev["parity"] = any(ev["plans"]) if ev["ctx"] and ev["plans"] else None
    return ev


def iii_evidence(meta, trial):
    transcript = _json(os.path.join(trial, "agent", "transcript.json"), {}) or {}
    gens = [g for g in ((m.get("message") or {}) for m in transcript.get("messages") or []) if model_response(g)]
    # thinking_level is the adapter's own request; tb_trace_scan.py reads what reached the provider.
    scan = (_json(os.path.join(trial, "agent", "trace-scan.json"), {}) or {}).get("ok")
    return {"first": bool(meta.get("first_model_response")), "generations": meta.get("generations"),
            "reasoning_generations": meta.get("reasoning_generations"), "tool_calls": meta.get("function_calls"),
            "reasoning": [(g.get("usage") or {}).get("reasoning") or 0 for g in gens], "scan": scan,
            # The adapter takes any --model, so the comparison model is checked here too.
            "parity": (meta.get("provider"), meta.get("model")) == ("openai-codex", "codex/" + CODEX_MODEL)
            and meta.get("thinking_level") == EFFORT and scan is not False}


def _seconds(span):
    try:
        return (datetime.fromisoformat(span["finished_at"]) - datetime.fromisoformat(span["started_at"])).total_seconds()
    except (TypeError, KeyError, ValueError):
        return None


def load_trial(trial, rnd):
    r = _json(os.path.join(trial, "result.json"))
    if r is None:  # cut or empty: counted as a missing trial
        return None
    name = (r.get("agent_info") or {}).get("name") or ((r.get("config") or {}).get("agent") or {}).get("name") or ""
    arm = "codex" if name == "codex" else "iii" if "iii" in name.lower() else None
    if arm is None:
        sys.exit(f"tb_pair: {trial}: agent {name!r} is neither arm")
    a = r.get("agent_result") or {}
    ev = codex_evidence(trial) if arm == "codex" else iii_evidence(a.get("metadata") or {}, trial)
    # Output tokens prove a model response even when the transcript or rollout copy was cut short.
    ev["first"] = ev["first"] or (a.get("n_output_tokens") or 0) > 0
    exc = r.get("exception_info") or {}
    etype = exc.get("exception_type")
    if etype in STOP_TYPES or (etype and STOP_TEXT.search(exc.get("exception_message") or "")):
        cell = "stop"
    # CancelledError is an interrupted job, not an arm's outcome; Harbor's resume re-runs it.
    elif etype and (not ev["first"] or etype == "CancelledError" or etype in HOST_FAILURES):
        cell = "infra"
    else:
        cell = "valid"
    reward = ((r.get("verifier_result") or {}).get("rewards") or {}).get("reward") or 0.0
    return {**ev, "path": trial, "round": rnd, "task": r["task_name"], "arm": arm, "cell": cell, "exception": etype,
            "reward": float(reward) if cell == "valid" else None, "reward_valid": r.get("reward_valid"),
            "input_tokens": a.get("n_input_tokens"), "cache_tokens": a.get("n_cache_tokens"),
            "output_tokens": a.get("n_output_tokens"), "wall_s": _seconds(r.get("agent_execution")),
            "meta": a.get("metadata") or {}}


def load_rounds(job_dirs):
    trials, jobs = [], {}
    for rnd, job in enumerate(job_dirs, 1):
        if not os.path.isdir(job):
            sys.exit(f"tb_pair: {job} is not a directory")
        res = _json(os.path.join(job, "result.json"), {}) or {}
        found = [load_trial(os.path.dirname(p), rnd) for p in sorted(glob.glob(os.path.join(job, "*", "result.json")))]
        trials += [t for t in found if t]
        jobs[rnd] = {"path": job, "planned": res.get("n_total_trials"),
                     "retried": (res.get("stats") or {}).get("n_retries") or 0}
    return trials, jobs


def bootstrap(diffs, seed=SEED, n=BOOT):
    """95% percentile interval of the mean over tasks, resampling tasks with a fixed seed."""
    rng, k = random.Random(seed), len(diffs)
    means = sorted(sum(rng.choices(diffs, k=k)) / k for _ in range(n))
    return means[int(0.025 * n)], means[int(0.975 * n) - 1]


def outcome(lo, hi):
    """Worse takes precedence: an interval wholly below 0 is worse even above the margin."""
    if hi < 0:
        return "worse"
    if lo > 0:
        return "better"
    return "non_inferior" if lo > MARGIN else "inconclusive"


def mcnemar(b, c):
    """Exact two-sided McNemar p from the discordant counts."""
    n = b + c
    return min(1.0, 2 * sum(math.comb(n, i) for i in range(min(b, c) + 1)) / 2 ** n) if n else 1.0


def parity_failures(trials):
    bad = [t for t in trials if t["cell"] == "valid" and t["parity"] is False]
    for rnd in sorted({t["round"] for t in trials}):
        valid = [t for t in trials if t["round"] == rnd and t["arm"] == "iii" and t["cell"] == "valid"]
        if valid and not any((t["reasoning_generations"] or 0) > 0 for t in valid):
            bad.append({"round": rnd, "arm": "iii", "task": "*", "why": "no reasoning generation in the round"})
    return bad


def task_diffs(pairs):
    by_task = {}
    for c in pairs:
        by_task.setdefault(c["iii"]["task"], []).append(c)
    return by_task, {task: statistics.fmean(c["iii"]["reward"] for c in cs) - statistics.fmean(c["codex"]["reward"] for c in cs)
                     for task, cs in by_task.items()}


def round_counts(trials, job):
    cells = {}
    for t in trials:
        if t["arm"] in cells.setdefault(t["task"], {}):
            sys.exit(f"tb_pair: two trials for {(t['round'], t['task'], t['arm'])}; n_attempts must be 1")
        cells[t["task"]][t["arm"]] = t
    pairs = [c for c in cells.values() if all(c.get(a, {}).get("cell") == "valid" for a in ARMS)]
    return pairs, {
        "trials": len(trials),
        "infra_cells": sum(t["cell"] == "infra" for t in trials),
        "stop_cells": sum(t["cell"] == "stop" for t in trials),
        "parity_failures": len(parity_failures(trials)),
        "parity_unknown": sum(t["cell"] == "valid" and t["parity"] is None for t in trials),
        "retried_trials": job["retried"],
        "missing_trials": max((job["planned"] or 0) - len(trials), 0),
        "unpaired_cells": sum(len(c) < 2 for c in cells.values()),
        "valid_pairs": len(pairs),
        "iii_wins": sum(c["iii"]["reward"] > c["codex"]["reward"] for c in pairs),
        "codex_wins": sum(c["iii"]["reward"] < c["codex"]["reward"] for c in pairs),
        "ties": sum(c["iii"]["reward"] == c["codex"]["reward"] for c in pairs),
    }


def gate_reasons(trials, job, c):
    reasons = []
    if c["stop_cells"]:
        reasons.append("stop cells %d" % c["stop_cells"])
    if c["parity_failures"]:
        reasons.append("effort parity failures %d" % c["parity_failures"])
    for a in ARMS:
        mine = [t for t in trials if t["arm"] == a]
        n = sum(t["cell"] == "infra" for t in mine)
        if n > INFRA_MAX:
            reasons.append("%s infra cells %d" % (a, n))
        n = sum(t["cell"] == "valid" and t["parity"] is None for t in mine)
        if n > NO_EVIDENCE_MAX:
            reasons.append("%s valid cells without effort evidence %d" % (a, n))
    iii_valid = [t for t in trials if t["arm"] == "iii" and t["cell"] == "valid"]
    if iii_valid and not any(t["scan"] for t in iii_valid):
        reasons.append("no passing trace scan")
    if c["retried_trials"]:
        reasons.append("retried trials %d" % c["retried_trials"])
    if job["planned"] is None:
        reasons.append("planned trial count unknown")
    if c["missing_trials"]:
        reasons.append("missing trials %d" % c["missing_trials"])
    if c["unpaired_cells"]:
        reasons.append("unpaired cells %d" % c["unpaired_cells"])
    return reasons


def summarize(trials, jobs):
    per_round, pairs, reasons = [], [], {}
    for rnd, job in jobs.items():
        mine = [t for t in trials if t["round"] == rnd]
        p, c = round_counts(mine, job)
        pairs += p
        per_round.append(c)
        reasons[rnd] = gate_reasons(mine, job, c)
    counts = {k: sum(c[k] for c in per_round) for k in per_round[0]} if per_round else {}
    counts["rounds_void"] = sum(bool(r) for r in reasons.values())
    by_task, diffs = task_diffs(pairs)
    s = {"counts": counts, "by_task": by_task, "diffs": diffs, "pairs": pairs, "reasons": reasons,
         "rounds": len(jobs), "unpaired": counts.get("unpaired_cells", 0)}
    if diffs:
        s["mean_diff"] = statistics.fmean(diffs.values())
        s["interval"] = bootstrap([diffs[k] for k in sorted(diffs)])
        s["outcome"] = outcome(*s["interval"])
        # Realized mdd of that interval (two-sided 95%, 80% power) from the observed spread over tasks.
        s["mdd"] = 2.80 * statistics.stdev(diffs.values()) / math.sqrt(len(diffs)) if len(diffs) > 1 else None
        # Sensitivity: Harbor's own rule also drops every cell it graded invalid, e.g. a provider error mid-run.
        _, strict = task_diffs([c for c in pairs if all(c[a]["reward_valid"] for a in ARMS)])
        s["strict_outcome"] = outcome(*bootstrap([strict[k] for k in sorted(strict)])) if strict else None
        s["verdict"] = s["outcome"] if s["strict_outcome"] in (None, s["outcome"]) else "inconclusive"
    if s["rounds"] == 1:
        s["mcnemar_p"] = mcnemar(counts["iii_wins"], counts["codex_wins"])
    return s


def secondary(pairs):
    """Per arm means over valid pairs, and the geometric mean over tasks of the iii / codex ratio."""
    rows = []
    for m in SECONDARY:
        both = [c for c in pairs if c["iii"][m] is not None and c["codex"][m] is not None]
        logs = {}
        for c in both:
            if c["iii"][m] > 0 and c["codex"][m] > 0:
                logs.setdefault(c["iii"]["task"], []).append(math.log(c["iii"][m] / c["codex"][m]))
        ratio = math.exp(statistics.fmean(statistics.fmean(v) for v in logs.values())) if logs else None
        means = [statistics.fmean(c[a][m] for c in both) if both else None for a in ARMS]
        rows.append((m, *means, ratio, len(logs)))
    return rows


def reasoning(trials):
    """Generations, median reasoning tokens over those with reasoning, and the zero-reasoning share."""
    r = [x for t in trials for x in t["reasoning"]]
    pos = [x for x in r if x > 0]
    return len(r), statistics.median(pos) if pos else None, 1 - len(pos) / len(r) if r else None


def _f(x, spec=".3f"):
    return "-" if x is None else format(x, spec)


def report(trials, s):
    print("| task | rounds | iii | codex | iii - codex |\n|---|---|---|---|---|")
    for task in sorted(s["by_task"]):
        cs = s["by_task"][task]
        print("| %s | %d | %s | %s | %+.3f |" % (task, len(cs), _f(statistics.fmean(c["iii"]["reward"] for c in cs)),
                                              _f(statistics.fmean(c["codex"]["reward"] for c in cs)), s["diffs"][task]))
    print("\n| arm | trials | valid | infra | stop | parity fail | no effort evidence | mean reward (paired) |"
          "\n|---|---|---|---|---|---|---|---|")
    bad = parity_failures(trials)
    for a in ARMS:
        mine = [t for t in trials if t["arm"] == a]
        mean = statistics.fmean(c[a]["reward"] for c in s["pairs"]) if s["pairs"] else None
        print("| %s | %d | %d | %d | %d | %d | %d | %s |" % (
            a, len(mine), *(sum(t["cell"] == k for t in mine) for k in ("valid", "infra", "stop")),
            sum(b["arm"] == a for b in bad), sum(t["cell"] == "valid" and t["parity"] is None for t in mine), _f(mean)))
    print("\n| secondary (valid pairs) | iii mean | codex mean | iii / codex (geo mean over tasks) | tasks |\n|---|---|---|---|---|")
    for m, mi, mc, ratio, n in secondary(s["pairs"]):
        print("| %s | %s | %s | %s | %d |" % (m, _f(mi, ".1f"), _f(mc, ".1f"), _f(ratio), n))
    print("\n| reasoning (valid cells) | generations | median tokens per reasoning generation | zero-reasoning share |"
          "\n|---|---|---|---|")
    for a in ARMS:
        n, med, zero = reasoning([t for t in trials if t["arm"] == a and t["cell"] == "valid"])
        print("| %s | %d | %s | %s |" % (a, n, _f(med, ".0f"), _f(zero, ".2f")))
    for rnd in sorted({t["round"] for t in trials}):
        print("round %d: valid iii trials without packages_installed %d" % (rnd, sum(
            t["round"] == rnd and t["arm"] == "iii" and t["cell"] == "valid" and t["meta"].get("packages_installed") is not True
            for t in trials)))
    for b in bad:
        print("PARITY_FAIL: round %s %s %s %s" % (b["round"], b["arm"], b["task"], b.get("why") or b.get("path")))
    for t in trials:
        if t["cell"] != "valid":
            print("DROPPED: round %d %s %s %s %s %s" % (t["round"], t["arm"], t["task"], t["cell"], t["exception"], t["path"]))
    print("\nrounds %d, tasks with a valid pair %d, unpaired cells %d" % (s["rounds"], len(s["diffs"]), s["unpaired"]))
    if "interval" in s:
        print("pass_rate_diff (iii - codex): %+.3f, 95%% paired bootstrap over tasks [%+.3f, %+.3f] -> %s"
              % (s["mean_diff"], *s["interval"], s["outcome"]))
        print("realized mdd %s; drop-invalid sensitivity -> %s; VERDICT: %s"
              % (_f(s["mdd"]), s["strict_outcome"], s["verdict"]))
        if s["strict_outcome"] not in (None, s["outcome"]):
            # The readings disagree: name what Harbor graded invalid, which only the sensitivity drops.
            for a in ARMS:
                for exc, n in sorted(Counter(c[a]["exception"] for c in s["pairs"] if not c[a]["reward_valid"]).items(),
                                     key=str):
                    print("HARBOR_INVALID: %s %s %d" % (a, exc, n))
    if "mcnemar_p" in s:
        print("exact McNemar (one round): iii wins %d, codex wins %d, p = %.4f"
              % (s["counts"]["iii_wins"], s["counts"]["codex_wins"], s["mcnemar_p"]))


def pilot(trials):
    ok_all = bool(trials)
    for t in sorted(trials, key=lambda t: (t["task"], t["arm"])):
        bad = [] if t["cell"] == "valid" and t["reward_valid"] else ["cell %s, reward_valid %s, %s" % (
            t["cell"], t["reward_valid"], t["exception"])]
        if t["parity"] is not True:
            bad.append("effort parity %s" % ("no evidence" if t["parity"] is None else "wrong"))
        if not (t["reasoning_generations"] or 0) > 0:
            bad.append("no reasoning generation")
        if not t["input_tokens"]:
            bad.append("no input tokens")
        if not t["tool_calls"]:
            bad.append("no tool call")
        if t["arm"] == "codex" and t["cli"] != {CODEX_CLI}:
            bad.append("cli version %s" % sorted(map(str, t["cli"])))
        if t["arm"] == "iii" and t["scan"] is not True:
            bad.append("trace scan %s" % {None: "missing or no evidence", False: "failed"}[t["scan"]])
        if t["arm"] == "iii" and t["meta"].get("packages_installed") is not True:
            bad.append("packages not installed")
        n, med, zero = reasoning([t])
        extra = "ctx %s plans %s cli %s" % (sorted(set(t["ctx"])), sorted(map(str, t["plans"])), sorted(map(str, t["cli"]))) \
            if t["arm"] == "codex" else "models %s thinking %s scan %s base_tools %s packages %s" % (
                t["meta"].get("models_seen"), t["meta"].get("thinking_level"), t["scan"],
                t["meta"].get("base_tools"), t["meta"].get("packages_installed"))
        print(t["arm"], t["task"], "reward", t["reward"], "in", t["input_tokens"], "cache", t["cache_tokens"],
              "out", t["output_tokens"], "gens", t["generations"], "rgens", t["reasoning_generations"],
              "reasoning median", _f(med, ".0f"), "zero share", _f(zero, ".2f"), "tools", t["tool_calls"],
              "wall_s", _f(t["wall_s"], ".0f"), extra, "OK" if not bad else "FAIL: " + "; ".join(bad))
        ok_all = ok_all and not bad
    meds = {a: reasoning([t for t in trials if t["arm"] == a])[1] for a in ARMS}
    ratio = meds["iii"] / meds["codex"] if meds["iii"] and meds["codex"] else None
    print("REASONING: median tokens per reasoning generation iii %s codex %s ratio %s"
          % (_f(meds["iii"], ".0f"), _f(meds["codex"], ".0f"), _f(ratio, ".2f")))
    if ratio is None or not REASONING_FLAG[0] <= ratio <= REASONING_FLAG[1]:
        print("FLAG: reasoning ratio outside [%g, %g]: the owner reads the pilot report before approving" % REASONING_FLAG)
    return ok_all


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    mode = ap.add_mutually_exclusive_group()
    mode.add_argument("--gate", action="store_true", help="exit 1 when any round fails its gate")
    mode.add_argument("--pilot", action="store_true", help="per-trial pilot_ready check")
    ap.add_argument("jobs", nargs="+", help="Harbor job dirs, one per round")
    args = ap.parse_args(argv)
    trials, jobs = load_rounds(args.jobs)
    s = summarize(trials, jobs)
    failed = {r: why for r, why in s["reasons"].items() if why}
    code = 0
    if args.pilot:
        ready = pilot(trials) and not failed
        for r, why in failed.items():
            print("GATE: round %d fail: %s" % (r, "; ".join(why)))
        print("PILOT_READY: %s trials=%d" % ("yes" if ready else "no", len(trials)))
        code = 0 if ready else 1
    elif args.gate:
        report(trials, s)
        for r in sorted(jobs):
            print("GATE: round %d %s" % (r, "fail: " + "; ".join(failed[r]) if r in failed else "ok"))
        code = 1 if failed else 0
    else:
        for r, why in failed.items():
            print("VOID: round %d %s: %s" % (r, jobs[r]["path"], "; ".join(why)))
        kept = {r: j for r, j in jobs.items() if r not in failed}
        if kept:
            report([t for t in trials if t["round"] in kept], summarize([t for t in trials if t["round"] in kept], kept))
        else:
            print("VERDICT: none, no round passed its gate")
    print(json.dumps(s["counts"], sort_keys=True))
    return code


if __name__ == "__main__":
    sys.exit(main())
