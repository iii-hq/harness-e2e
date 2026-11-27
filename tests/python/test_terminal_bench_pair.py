import contextlib
import importlib.util
import io
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/terminal_bench/tb_pair.py"
FIX = Path(__file__).resolve().parent / "fixtures/terminal_bench_pair"


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / f"scripts/terminal_bench/{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


PAIR, SCAN = load("tb_pair"), load("tb_trace_scan")

R1_COUNTS = {"trials": 15, "infra_cells": 2, "stop_cells": 1, "parity_failures": 2, "parity_unknown": 1,
             "retried_trials": 1, "missing_trials": 1, "unpaired_cells": 1, "rounds_void": 1,
             "valid_pairs": 4, "iii_wins": 1, "codex_wins": 1, "ties": 2}
R2_COUNTS = {"trials": 6, "infra_cells": 0, "stop_cells": 0, "parity_failures": 0, "parity_unknown": 0,
             "retried_trials": 0, "missing_trials": 0, "unpaired_cells": 0, "rounds_void": 0,
             "valid_pairs": 3, "iii_wins": 1, "codex_wins": 1, "ties": 1}
JOB = {"path": "job", "planned": 40, "retried": 0}


def run(*args):
    p = subprocess.run([sys.executable, "-I", str(SCRIPT), *map(str, args)], capture_output=True, text=True)
    return p.returncode, p.stdout, json.loads(p.stdout.strip().splitlines()[-1])


def summary(*jobs):
    return PAIR.summarize(*PAIR.load_rounds([str(FIX / j) for j in jobs]))


def cell(task, arm, **kw):
    return {"round": 1, "task": task, "arm": arm, "cell": "valid", "reward": 1.0, "reward_valid": True,
            "parity": True, "scan": True if arm == "iii" else None, "reasoning_generations": 1,
            "exception": None, **kw}


def edit(trial, fn):
    path = Path(trial, "result.json")
    res = json.loads(path.read_text())
    fn(res)
    path.write_text(json.dumps(res))


class TbPairTest(unittest.TestCase):
    def test_gate_fails_on_each_planted_failure(self):
        code, out, counts = run("--gate", FIX / "r1")
        self.assertEqual(code, 1)
        self.assertEqual(counts, R1_COUNTS)
        self.assertIn("GATE: round 1 fail: stop cells 1; effort parity failures 2; no passing trace scan; "
                      "retried trials 1; missing trials 1; unpaired cells 1", out)

    def test_clean_round_passes_the_gate(self):
        code, out, counts = run("--gate", FIX / "r2")
        self.assertEqual((code, counts), (0, R2_COUNTS))
        self.assertIn("GATE: round 1 ok", out)

    def test_plain_mode_leaves_a_void_round_out_of_the_verdict(self):
        code, out, counts = run(FIX / "r1", FIX / "r2")
        self.assertEqual(code, 0)
        self.assertEqual(counts, {k: R1_COUNTS[k] + R2_COUNTS[k] for k in R1_COUNTS})
        self.assertIn("VOID: round 1 %s: stop cells 1" % (FIX / "r1"), out)
        self.assertIn("tasks with a valid pair 3, unpaired cells 0", out)
        self.assertNotIn("zeta", out)
        code, out, _ = run(FIX / "r1")
        self.assertEqual(code, 0)
        self.assertIn("VERDICT: none, no round passed its gate", out)

    def test_infra_cap_is_per_arm_per_round(self):
        trials = [cell(f"t{i}", a) for i in range(10) for a in ("iii", "codex")]
        for t in trials[:6:2]:
            t.update(cell="infra", reward=None)
        s = PAIR.summarize(trials, {1: {**JOB, "planned": 20}})
        self.assertEqual(s["reasons"][1], [])
        trials[6].update(cell="infra", reward=None)
        s = PAIR.summarize(trials, {1: {**JOB, "planned": 20}})
        self.assertEqual(s["reasons"][1], ["iii infra cells 4"])

    def test_cells_without_effort_evidence_are_allowed_up_to_two_per_arm(self):
        trials = [cell(f"t{i}", a) for i in range(10) for a in ("iii", "codex")]
        for t in trials[1:6:2]:
            t["parity"] = None
        self.assertEqual(PAIR.summarize(trials, {1: {**JOB, "planned": 20}})["reasons"][1],
                         ["codex valid cells without effort evidence 3"])
        trials[1]["parity"] = True
        s = PAIR.summarize(trials, {1: {**JOB, "planned": 20}})
        self.assertEqual((s["reasons"][1], s["counts"]["parity_unknown"]), ([], 2))

    def test_cells_are_classified_by_phase_for_both_arms(self):
        trials, _ = PAIR.load_rounds([str(FIX / "r1")])
        got = {(t["task"], t["arm"]): (t["cell"], t["reward"]) for t in trials}
        self.assertEqual(got[("alpha", "codex")], ("valid", 0.0))  # failed after its first model response
        self.assertEqual(got[("beta", "iii")], ("valid", 0.0))
        self.assertEqual(got[("delta", "iii")], ("infra", None))  # engine unreachable in setup
        self.assertEqual(got[("eta", "codex")], ("infra", None))  # install failed, Harbor graded it 0
        self.assertEqual(got[("eps", "codex")], ("stop", None))
        self.assertEqual(got[("gamma", "codex")], ("valid", 1.0))  # no rollout copied: codex.txt shows a response
        self.assertNotIn(("theta", "codex"), got)  # its result.json is empty: a missing trial
        parity = {(t["task"], t["arm"]): t["parity"] for t in trials}
        self.assertEqual((parity[("gamma", "codex")], parity[("zeta", "codex")], parity[("alpha", "codex")]),
                         (None, False, True))

    def test_a_verifier_failure_after_the_agent_ran_is_infra(self):
        with tempfile.TemporaryDirectory() as tmp:
            trial = Path(tmp, "job/gamma__iii")
            shutil.copytree(FIX / "r1/gamma__iii", trial)
            for etype in sorted(PAIR.HOST_FAILURES):
                edit(trial, lambda res: res.update(verifier_result=None, exception_info={
                    "exception_type": etype, "exception_message": "planted"}))
                self.assertEqual(PAIR.load_trial(str(trial), 1)["cell"], "infra", etype)

    def test_rounds_pool_per_task_before_the_paired_difference(self):
        s = summary("r1", "r2")
        self.assertEqual(s["counts"], {k: R1_COUNTS[k] + R2_COUNTS[k] for k in R1_COUNTS})
        self.assertEqual(s["diffs"], {"alpha": 0.5, "beta": -1.0, "gamma": 0.5, "zeta": 0.0})
        self.assertEqual(s["mean_diff"], 0.0)
        self.assertEqual(s["outcome"], "inconclusive")
        self.assertNotIn("mcnemar_p", s)
        self.assertEqual(summary("r1")["mcnemar_p"], 1.0)

    def test_secondary_ratios_are_paired_over_valid_pairs(self):
        rows = {m: (mi, mc, ratio, n) for m, mi, mc, ratio, n in PAIR.secondary(summary("r1")["pairs"])}
        self.assertEqual(rows["input_tokens"], (2000, 1000, 2.0, 4))
        self.assertEqual(rows["wall_s"], (100, 50, 2.0, 4))

    def test_outcome_three_ways_and_margin(self):
        self.assertEqual(PAIR.outcome(-0.2, -0.01), "worse")
        self.assertEqual(PAIR.outcome(-0.05, -0.01), "worse")
        self.assertEqual(PAIR.outcome(0.01, 0.2), "better")
        self.assertEqual(PAIR.outcome(-0.05, 0.1), "non_inferior")
        self.assertEqual(PAIR.outcome(-0.2, 0.1), "inconclusive")
        self.assertEqual(PAIR.outcome(*PAIR.bootstrap([0.0] * 20)), "non_inferior")
        self.assertEqual(PAIR.outcome(*PAIR.bootstrap([1.0] * 20)), "better")
        diffs = [1.0, -1.0, 0.0, 0.0, 1.0, 0.5]
        self.assertEqual(PAIR.bootstrap(diffs), PAIR.bootstrap(diffs))

    def test_drop_invalid_sensitivity_can_only_make_the_verdict_inconclusive(self):
        trials = [cell(f"t{i}", a) for i in range(20) for a in ("iii", "codex")]
        s = PAIR.summarize(trials, {1: JOB})
        self.assertEqual((s["outcome"], s["strict_outcome"], s["verdict"], s["mdd"]),
                         ("non_inferior", "non_inferior", "non_inferior", 0.0))
        for t in trials[:8:2]:  # iii loses 4 tasks to a provider error after its first model response
            t.update(reward=0.0, reward_valid=False, exception="ApiOverloadedError")
        s = PAIR.summarize(trials, {1: JOB})
        self.assertEqual((s["outcome"], s["strict_outcome"], s["verdict"]), ("worse", "non_inferior", "inconclusive"))
        self.assertAlmostEqual(s["mdd"], 2.80 * (3.2 / 19) ** 0.5 / 20 ** 0.5)
        # The disagreement names what Harbor graded invalid: bucket B work, not a reason to change nothing.
        for t in trials:
            t.update(meta={}, reasoning=[], input_tokens=1, output_tokens=1, generations=1, tool_calls=1, wall_s=1,
                     path="p")
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            PAIR.report(trials, s)
        self.assertIn("HARBOR_INVALID: iii ApiOverloadedError 4", out.getvalue())
        for t in trials[::2]:  # Harbor drops every iii cell: no sensitivity reading, the primary stands
            t.update(reward=0.0, reward_valid=False)
        s = PAIR.summarize(trials, {1: JOB})
        self.assertEqual((s["outcome"], s["strict_outcome"], s["verdict"]), ("worse", None, "worse"))

    def test_exact_mcnemar(self):
        self.assertEqual(PAIR.mcnemar(0, 0), 1.0)
        self.assertEqual(PAIR.mcnemar(3, 3), 1.0)
        self.assertAlmostEqual(PAIR.mcnemar(5, 0), 0.0625)
        self.assertAlmostEqual(PAIR.mcnemar(0, 6), 0.03125)

    def test_subagent_rollout_skips_the_copied_parent_history(self):
        ev = PAIR.codex_evidence(str(FIX / "r2/alpha__codex"))
        self.assertEqual((ev["tool_calls"], ev["generations"], ev["reasoning_generations"]), (2, 3, 2))
        self.assertTrue(ev["parity"])

    def test_codex_output_counts_as_a_model_response_when_sessions_are_missing(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp, "agent/codex.txt")
            out.parent.mkdir()
            out.write_text('npm warn noise\n{"type":"item.completed","item":{"type":"error"}}\n')
            self.assertFalse(PAIR.codex_evidence(tmp)["first"])
            out.write_text('{"type":"thread.started"}\n{"type":"item.completed","item":{"type":"reasoning"}}\n')
            ev = PAIR.codex_evidence(tmp)
            self.assertTrue(ev["first"])
            self.assertFalse(ev["parity"])  # no rollout, no effort evidence

    def test_rate_limit_in_the_task_text_is_not_a_stop(self):
        with tempfile.TemporaryDirectory() as tmp:
            trial = Path(tmp, "job/alpha__codex")
            shutil.copytree(FIX / "r1/alpha__codex", trial)
            res = json.loads((trial / "result.json").read_text())
            res["exception_info"]["exception_message"] = "Command failed: codex exec -- 'Fix the rate limiter'"
            (trial / "result.json").write_text(json.dumps(res))
            self.assertEqual(PAIR.load_trial(str(trial), 1)["cell"], "valid")
            res["exception_info"]["exception_message"] += ' {"type":"turn.failed","code":"usage_limit_reached"}'
            (trial / "result.json").write_text(json.dumps(res))
            self.assertEqual(PAIR.load_trial(str(trial), 1)["cell"], "stop")

    def test_output_tokens_alone_prove_a_model_response(self):
        with tempfile.TemporaryDirectory() as tmp:
            trial = Path(tmp, "job/beta__iii")
            shutil.copytree(FIX / "r1/beta__iii", trial)
            # transcript collection cut short
            edit(trial, lambda res: res["agent_result"]["metadata"].update(first_model_response=False))
            self.assertEqual(PAIR.load_trial(str(trial), 1)["cell"], "valid")
            # Every provider call failed: the adapter saw only empty placeholders, and no output tokens.
            edit(trial, lambda res: (res["agent_result"].update(n_output_tokens=0), res["exception_info"].update(
                exception_type="ApiError", exception_message="harness turn failed: router/provider_rejected")))
            self.assertEqual(PAIR.load_trial(str(trial), 1)["cell"], "infra")

    def test_iii_round_without_any_reasoning_generation_fails_parity(self):
        with tempfile.TemporaryDirectory() as tmp:
            job = Path(tmp, "r2")
            shutil.copytree(FIX / "r2", job)
            for path in job.glob("*__iii/result.json"):
                res = json.loads(path.read_text())
                res["agent_result"]["metadata"]["reasoning_generations"] = 0
                path.write_text(json.dumps(res))
            code, out, counts = run("--gate", job)
        self.assertEqual((code, counts["parity_failures"]), (1, 1))
        self.assertIn("PARITY_FAIL: round 1 iii * no reasoning generation in the round", out)

    def test_pilot_mode(self):
        code, out, _ = run("--pilot", FIX / "r2")
        self.assertEqual(code, 0)
        self.assertIn("PILOT_READY: yes trials=6", out)
        self.assertIn("REASONING: median tokens per reasoning generation iii 9 codex 12 ratio 0.75", out)
        self.assertNotIn("FLAG:", out)
        code, out, _ = run("--pilot", FIX / "r1")
        self.assertEqual(code, 1)
        self.assertIn("PILOT_READY: no trials=15", out)
        self.assertIn("codex zeta reward 1.0", out)
        self.assertRegex(out, r"iii zeta .* FAIL: effort parity wrong")

    def test_pilot_needs_both_arms_the_trace_scan_and_the_packages(self):
        with tempfile.TemporaryDirectory() as tmp:
            job = Path(tmp, "pilot")
            shutil.copytree(FIX / "r2", job)
            only_iii = Path(tmp, "only-iii")
            shutil.copytree(job, only_iii, ignore=shutil.ignore_patterns("*__codex"))
            code, out, counts = run("--pilot", only_iii)
            self.assertEqual((code, counts["missing_trials"], counts["unpaired_cells"]), (1, 3, 3))
            self.assertIn("GATE: round 1 fail: missing trials 3; unpaired cells 3", out)
            Path(job, "alpha__iii/agent/trace-scan.json").unlink()
            edit(job / "beta__iii", lambda res: res["agent_result"]["metadata"].update(packages_installed=False))
            code, out, _ = run("--pilot", job)
            self.assertEqual(code, 1)
            self.assertRegex(out, r"iii alpha .* FAIL: trace scan missing or no evidence")
            self.assertRegex(out, r"iii beta .* FAIL: packages not installed")
            code, out, _ = run(job)
            self.assertIn("round 1: valid iii trials without packages_installed 1", out)

    def test_a_failing_trace_scan_or_an_api_key_login_fails_parity(self):
        with tempfile.TemporaryDirectory() as tmp:
            job = Path(tmp, "r2")
            shutil.copytree(FIX / "r2", job)
            scan = job / "alpha__iii/agent/trace-scan.json"
            scan.write_text(json.dumps({**json.loads(scan.read_text()), "ok": False, "why": ["step at low"]}))
            rollout = next((job / "beta__codex").rglob("rollout-*.jsonl"))
            rows = [json.loads(line) for line in rollout.read_text().splitlines()]
            for row in rows:
                row["payload"].pop("rate_limits", None)
            rollout.write_text("".join(json.dumps(row) + "\n" for row in rows))
            code, out, counts = run("--gate", job)
            self.assertEqual((code, counts["parity_failures"]), (1, 2))
            self.assertIn("PARITY_FAIL: round 1 iii alpha", out)
            self.assertIn("PARITY_FAIL: round 1 codex beta", out)
            for path in job.glob("*__iii/agent/trace-scan.json"):
                path.unlink()
            code, out, _ = run("--gate", job)
            self.assertIn("no passing trace scan", out)

    def test_an_iii_trial_on_another_model_fails_parity(self):
        with tempfile.TemporaryDirectory() as tmp:
            job = Path(tmp, "r2")
            shutil.copytree(FIX / "r2", job)
            result = job / "alpha__iii/result.json"
            r = json.loads(result.read_text())
            r["agent_result"]["metadata"]["model"] = "codex/gpt-5.5"
            result.write_text(json.dumps(r))
            code, out, counts = run("--gate", job)
            self.assertEqual((code, counts["parity_failures"]), (1, 1))
            self.assertIn("PARITY_FAIL: round 1 iii alpha", out)

    def test_trace_scan_reads_every_span_when_the_tree_times_out(self):
        spans = [{"name": "s%d" % i} for i in range(1200)]

        def call(fn, payload):
            if fn == "engine::traces::tree":
                raise RuntimeError("timed out")
            if not payload.get("search_all_spans"):  # what the engine answers without the flag
                return {"spans": spans[:1], "total": 1}
            return {"spans": spans[payload["offset"]:payload["offset"] + payload["limit"]], "total": len(spans)}
        self.assertEqual(len(SCAN.fetch_tree(call, "T")["roots"]), 1200)

    def test_trace_scan_reads_the_level_downstream_of_the_harness(self):
        def span(name, payload, cut=False, children=()):
            return {"name": name, "children": list(children), "events": [{"name": "iii.invocation.input", "attributes": [
                ["iii.payload.json", json.dumps(payload)], ["iii.payload.truncated", "true" if cut else "false"]]}]}

        def step(n, level="xhigh"):
            return span(SCAN.STEP_SPAN, {"scope": "harness_turn", "value": {
                "stream_request_id": f"t1:{n}", "options": {"thinking_level": level}}})

        def provider(level="xhigh", cut=False):
            return span(SCAN.PROVIDER_SPAN, {"messages": ["x" * 10], **({} if cut or level is None else {
                "thinking_level": level})}, cut=cut)
        good = {"roots": [span("execute harness::turn", {}, children=[step(0), step(1), provider(), provider(cut=True)])]}
        counts = SCAN.scan(good)
        self.assertEqual(counts, {"steps": 2, "step_levels": {"xhigh": 2}, "provider_inputs": {"xhigh": 1, "cut": 1}})
        self.assertEqual(SCAN.verdict(counts), (True, []))
        self.assertEqual(SCAN.verdict(SCAN.scan({"roots": [step(0), step(1, "low"), provider()]})), (False, ["step at low"]))
        self.assertEqual(SCAN.verdict(SCAN.scan({"roots": [step(0), provider(None)]})),
                         (False, ["provider input at absent"]))
        self.assertEqual(SCAN.verdict(SCAN.scan({"roots": [step(0), provider(cut=True)]})),
                         (None, ["no uncut provider input"]))
        cut_with = lambda level: span(SCAN.PROVIDER_SPAN, {"messages": ["x"], "thinking_level": level}, cut=True)
        self.assertEqual(SCAN.verdict(SCAN.scan({"roots": [step(0), cut_with("xhigh")]})),
                         (None, ["no uncut provider input"]))
        self.assertEqual(SCAN.verdict(SCAN.scan({"roots": [step(0), provider(), cut_with("low")]})),
                         (False, ["provider input at low"]))
        self.assertEqual(SCAN.verdict(SCAN.scan({"roots": [span("execute harness::send", {})]})),
                         (None, ["no turn step record"]))
        with tempfile.TemporaryDirectory() as tmp:
            job = Path(tmp, "r2")
            shutil.copytree(FIX / "r2", job)
            for path in job.glob("*__iii/agent/trace-scan.json"):
                path.unlink()
            for task, sid in (("beta", "s_beta"), ("gamma", "s_gamma")):
                edit(job / f"{task}__iii", lambda res, sid=sid: res["agent_result"]["metadata"].update(session_id=sid))
            trees = {"T1": good, "T2": {"roots": [step(0, "low"), provider()]}}
            calls = []

            def call(fn, payload):
                calls.append(fn)
                if fn == "engine::traces::list":
                    return {"total": 2, "traces": [{"trace_id": "T1", "trace_tags": {"iii.session.id": "s_0123456789ab"}},
                                                   {"trace_id": "T2", "trace_tags": {"iii.session.id": "s_beta"}}]}
                return trees[payload["trace_id"]]
            with contextlib.redirect_stdout(io.StringIO()) as out:
                code = SCAN.main([str(job)], call=call)
            self.assertEqual(code, 1)
            self.assertIn("TRACE_SCAN: fail trials=3 failed=1", out.getvalue())
            read = {t: json.loads((job / f"{t}__iii/agent/trace-scan.json").read_text()) for t in ("alpha", "beta", "gamma")}
            self.assertEqual([read[t]["ok"] for t in ("alpha", "beta", "gamma")], [True, False, None])
            self.assertEqual(read["gamma"]["why"], ["no harness::send trace for the session"])
            self.assertEqual(calls.count("engine::traces::list"), 1)


if __name__ == "__main__":
    unittest.main()
