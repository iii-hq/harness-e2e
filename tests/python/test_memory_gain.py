import contextlib
import importlib.util
import io
import json
import os
import shutil
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).parents[2] / "scripts" / "memory_gain.py"
SPEC = importlib.util.spec_from_file_location("memory_gain", SCRIPT)
assert SPEC and SPEC.loader
memory_gain = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(memory_gain)

MODEL = "deepseek/deepseek-flash"
STALE, REWORDED = "memory_consolidation_stale_fact", "memory_consolidation_reworded_fact"


def scenario(scenario_id, klass, scores, prefix, measurements=(), answers=None):
    """One scenario of a results.json, a run per score; `answers` are the
    points each run's answer criteria awarded, its score when absent."""
    entry = {"scenario_id": scenario_id, "behavior_sha256": f"sha:{scenario_id}", "runs": [
        {"attempt_id": f"{prefix}-{scenario_id}-{index}", "cause": "evaluated", "technical": "valid",
         "completion": "completed", "score": score, "cost": {"subject_usd": 0.01}, "wall_time_ms": 1000,
         "scenario_measurements": [{"id": key, "value": value, "unit": "count", "origin": "observed"}
                                   for key, value in [*measurements, ("memory_answer_points", answer)]]}
        for index, (score, answer) in enumerate(zip(scores, answers or scores))
    ]}
    if klass:
        entry["class"] = klass
    return entry


def measure(entry, index, key, value):
    """Run `index` of a scenario entry with measurement `key` set to `value`."""
    run = entry["runs"][index]
    run["scenario_measurements"] = [m for m in run["scenario_measurements"] if m["id"] != key] + [
        {"id": key, "value": value, "unit": "count", "origin": "observed"}]


def write_arm(directory, scenarios):
    """An arm as a run's root bundle lays it out: a group's results and the
    root's, which repeats them."""
    for place in ("campaign/case-a", "campaign"):
        (directory / place).mkdir(parents=True, exist_ok=True)
        (directory / place / "results.json").write_text(json.dumps({"schema": "results", "scenarios": scenarios}))


def arms(off_tokens=0, consolidated=True):
    """The scenarios of each arm: memory raises four lookup answers from 20 to
    80, the two consolidation cases among them, and the long message answers
    nothing on either arm. The synthesis case answers the same on both arms,
    and only memory's run scores its rule injection. Every consolidated run
    superseded one memory, by the semantic judge."""
    def build(prefix, lookup, rule, measurements=()):
        return [
            scenario("memory_value_project", "lookup", lookup, prefix, measurements),
            scenario("memory_rule_fit", "synthesis", rule, prefix, measurements, answers=[30, 30, 30]),
            scenario("memory_small_talk", None, [100, 100, 100], prefix, measurements),
            scenario("memory_precise_port", "lookup", lookup, prefix, measurements),
            scenario("memory_precise_long_message", "lookup", [30, 30, 30], prefix, measurements,
                     answers=[0, 0, 0]),
            scenario(STALE, "lookup", lookup, prefix, measurements),
            scenario(REWORDED, "lookup", lookup, prefix, measurements),
        ]
    built = {
        "memory-off": build("off", [20, 20, 20], [30, 30, 30], [("memory_injected_tokens", off_tokens)]),
        "memory": build("on", [80, 80, 80], [60, 60, 60], [("memory_injected_tokens", 12)]),
    }
    if consolidated:
        built["memory-consolidated"] = build("cons", [80, 80, 80], [60, 60, 60], [
            ("memory_consolidation_superseded", 1), ("memory_consolidation_llm_calls", 0),
            ("memory_consolidation_notes", 1), ("memory_consolidation_semantic", 1)])
    return built


class MemoryGainTest(unittest.TestCase):
    def run_main(self, built, compare=None, unmeasured=()):
        """main() on runs whose downloads are the built arms, plus `unmeasured`
        records; its exit code and report."""
        runs = [{"model": MODEL, "arm": arm, "run_url": f"https://github.com/iii-hq/harness-e2e/actions/runs/{i}"}
                for i, arm in enumerate(built)] + list(unmeasured)

        def download(run_url, directory):
            write_arm(directory, built[runs[int(run_url.rsplit("/", 1)[1])]["arm"]])

        with tempfile.TemporaryDirectory() as output, \
                patch.object(memory_gain, "download", download), \
                patch.dict(os.environ, {"GITHUB_STEP_SUMMARY": ""}), \
                contextlib.redirect_stdout(io.StringIO()):
            patches = [patch.object(memory_gain, "compare", compare)] if compare else []
            with contextlib.ExitStack() as stack:
                for active in patches:
                    stack.enter_context(active)
                code = memory_gain.main(["--runs", json.dumps(runs), "--output", output])
            report = json.loads(Path(output, "memory-gain.json").read_text())
            text = Path(output, "memory-gain.md").read_text()
        return code, report, text

    @staticmethod
    def fake_compare(a, b):
        return {"verdict": "better", "qualifier": None, "quality": {"state": "improved", "a": 1.0, "b": 2.0,
                "delta": 1.0, "tests": 1}, "cost": {"relative": 0.0}, "tests": [], "dropped": [], "warnings": []}

    def test_split_keeps_what_the_row_accepts_and_says_when_nothing_is_left(self):
        with tempfile.TemporaryDirectory() as directory:
            arm, out = Path(directory, "arm"), Path(directory, "out")
            write_arm(arm, arms()["memory"])
            self.assertTrue(memory_gain.split(arm, lambda s: s.get("class") == "lookup", out / "lookup"))
            kept = json.loads((out / "lookup/campaign/case-a/results.json").read_text())
            self.assertEqual([s["scenario_id"] for s in kept["scenarios"]],
                             ["memory_value_project", "memory_precise_port", "memory_precise_long_message",
                              STALE, REWORDED])
            self.assertEqual(kept["schema"], "results")
            self.assertFalse(memory_gain.split(arm, lambda s: s.get("class") == "nothing", out / "none"))
            self.assertFalse((out / "none").exists())

    def test_a_run_scores_its_answer_points_and_no_score_without_them(self):
        run = {"score": 60, "scenario_measurements": [{"id": "memory_answer_points", "value": 30}]}
        self.assertEqual(memory_gain.answered(run)["score"], 30)
        self.assertIsNone(memory_gain.answered({"score": 60, "scenario_measurements": []})["score"])
        with tempfile.TemporaryDirectory() as directory:
            arm, out = Path(directory, "arm"), Path(directory, "out")
            write_arm(arm, arms()["memory"])
            memory_gain.split(arm, memory_gain.rows()["synthesis"], out)
            kept = json.loads((out / "campaign/results.json").read_text())
            self.assertEqual([run["score"] for run in kept["scenarios"][0]["runs"]], [30, 30, 30])

    def test_the_rows_hold_the_classed_cases_and_the_module_rows_read_the_plan(self):
        self.assertEqual(len(memory_gain.module("memory-precise")), 6)
        self.assertEqual(memory_gain.module("memory-consolidation"), [STALE, REWORDED])
        rows = memory_gain.rows()
        self.assertTrue(rows["precise"]({"scenario_id": "memory_precise_port", "class": "lookup"}))
        self.assertFalse(rows["precise"]({"scenario_id": "memory_value_project", "class": "lookup"}))
        long_message = {"scenario_id": "memory_precise_long_message", "class": "lookup"}
        self.assertEqual([name for name, row in rows.items() if row(long_message)], ["all", "lookup", "precise"])
        # A consolidation case counts in its own row only.
        stale = {"scenario_id": STALE, "class": "lookup"}
        self.assertEqual([name for name, row in rows.items() if row(stale)], ["consolidation"])
        # A case without a class is in no row.
        for row in rows.values():
            self.assertFalse(row({"scenario_id": "memory_small_talk"}))

    def test_a_model_without_its_memory_off_arm_has_no_gain(self):
        built = arms()
        del built["memory-off"]
        code, report, text = self.run_main(built, self.fake_compare)
        self.assertEqual(code, 1)
        self.assertIsNone(report[MODEL]["rows"]["all"]["on_vs_off"])
        self.assertIn("no quality figure for memory against memory-off", text)

    def test_an_off_arm_that_injected_fails_the_report(self):
        code, report, text = self.run_main(arms(off_tokens=7), self.fake_compare)
        self.assertEqual(code, 1)
        # The root bundle repeats each run: counted once.
        self.assertEqual(report[MODEL]["off_injected_tokens"], 7 * 21)
        self.assertIn("the memory-off arm injected 147 tokens: injection was not off", text)

    def test_the_consolidation_line_counts_the_compared_runs_that_consolidated(self):
        code, report, text = self.run_main(arms(), self.fake_compare)
        self.assertEqual(code, 0)
        # The six compared cases, three runs each: no small talk.
        self.assertEqual(report[MODEL]["consolidation"], {"runs": 18, "consolidated": 18, "superseded": 18,
                                                          "llm_calls": 0, "notes": 18})
        self.assertIn("consolidation ran in 18 of 18 runs, superseded 18 memories, 0 model calls, 18 notes", text)
        self.assertIn("the memory-off arm injected nothing", text)
        self.assertEqual(report[MODEL]["errors"], [])
        # The consolidation cases have a row and a line each.
        self.assertIsNotNone(report[MODEL]["rows"]["consolidation"]["consolidated_vs_on"])
        self.assertIn("| consolidation |", text)
        self.assertEqual(report[MODEL]["consolidation_cases"][STALE],
                         [{"superseded": 1, "semantic": 1, "notes": 1}] * 3)
        self.assertIn(f"- {REWORDED}, per consolidated run: superseded 1, 1, 1; semantic 1, 1, 1; notes 1, 1, 1",
                      text)
        # No run recorded its setup's ranking: no line.
        self.assertEqual(report[MODEL]["minilm"], {})
        self.assertNotIn("set up on MiniLM", text)

    def test_a_compared_run_that_never_consolidated_is_an_error(self):
        built = arms()
        unconsolidated = scenario("memory_rule_fit", "synthesis", [60, 60, 60], "cons")
        # A run whose setup failed has no measurements and is not counted.
        unconsolidated["runs"][2]["scenario_measurements"] = []
        built["memory-consolidated"][1] = unconsolidated
        code, report, text = self.run_main(built, self.fake_compare)
        self.assertEqual(code, 1)
        self.assertEqual(report[MODEL]["consolidation"]["runs"], 17)
        self.assertEqual(report[MODEL]["consolidation"]["consolidated"], 15)
        self.assertIn("2 memory-consolidated runs never consolidated", text)

    def test_a_local_arm_is_read_where_it_is(self):
        local_model = "anthropic/claude-haiku-5-5"
        with tempfile.TemporaryDirectory() as local:
            records = []
            for arm, scenarios in arms().items():
                write_arm(Path(local, arm), scenarios)
                records.append({"model": local_model, "arm": arm, "path": str(Path(local, arm))})
            # A results.json names its directory.
            records[0]["path"] = str(Path(local, records[0]["arm"], "campaign", "results.json"))
            records.append({"model": "other/model", "arm": "memory", "path": str(Path(local, "missing"))})
            code, report, text = self.run_main(arms(), self.fake_compare, records)
        self.assertEqual(report[local_model]["errors"], [])
        self.assertEqual(report[local_model]["consolidation"], report[MODEL]["consolidation"])
        self.assertEqual(report[local_model]["off_injected_tokens"], 0)
        self.assertIn(f"## {local_model}", text)
        self.assertEqual(report["other/model"]["errors"][0], f"memory: no results.json at {Path(local, 'missing')}")
        self.assertEqual(code, 1)

    def test_a_model_whose_arms_started_no_run_is_reported_and_fails(self):
        unmeasured = [{"model": "other/model", "arm": arm, "run_url": ""} for arm in ("memory", "memory-off")]
        code, report, text = self.run_main(arms(), self.fake_compare, unmeasured)
        self.assertEqual(code, 1)
        self.assertEqual(report[MODEL]["errors"], [])
        self.assertIn("memory-off: not measured: the arm started no run", report["other/model"]["errors"])
        self.assertIn("## other/model", text)

    def test_a_consolidated_arm_whose_runs_never_consolidated_fails(self):
        built = arms()
        built["memory-consolidated"] = arms()["memory"]
        code, report, text = self.run_main(built, self.fake_compare)
        self.assertEqual(code, 1)
        self.assertIn("no run consolidated: this arm is a second memory-on arm", text)
        self.assertIn("18 memory-consolidated runs never consolidated: memory-consolidated against memory "
                      "mixes them in", report[MODEL]["errors"])
        self.assertIn(f"{STALE}: no memory-consolidated run consolidated", report[MODEL]["errors"])
        self.assertIn(f"- {REWORDED}: no consolidated run", text)

    def test_a_consolidation_case_missing_from_the_consolidated_arm_fails(self):
        built = arms()
        built["memory-consolidated"] = [s for s in built["memory-consolidated"] if s["scenario_id"] != REWORDED]
        code, report, _ = self.run_main(built, self.fake_compare)
        self.assertEqual(code, 1)
        self.assertEqual(report[MODEL]["errors"], [f"{REWORDED}: no memory-consolidated run consolidated"])

    def test_a_consolidation_case_pass_that_superseded_nothing_fails(self):
        built = arms()
        stale = next(s for s in built["memory-consolidated"] if s["scenario_id"] == STALE)
        measure(stale, 1, "memory_consolidation_superseded", 0)
        code, report, text = self.run_main(built, self.fake_compare)
        self.assertEqual(code, 1)
        self.assertEqual(report[MODEL]["errors"], [f"{STALE}: 1 of 3 consolidated runs superseded nothing"])
        self.assertIn(f"- {STALE}, per consolidated run: superseded 1, 0, 1; semantic 1, 1, 1", text)

    def test_a_consolidation_case_pass_with_no_semantic_action_fails(self):
        built = arms()
        reworded = next(s for s in built["memory-consolidated"] if s["scenario_id"] == REWORDED)
        for index in (0, 2):
            measure(reworded, index, "memory_consolidation_semantic", 0)
        # A pass from a runner that predates the count took none either.
        stale = next(s for s in built["memory-consolidated"] if s["scenario_id"] == STALE)
        stale["runs"][0]["scenario_measurements"] = [
            m for m in stale["runs"][0]["scenario_measurements"] if m["id"] != "memory_consolidation_semantic"]
        code, report, _ = self.run_main(built, self.fake_compare)
        self.assertEqual(code, 1)
        self.assertEqual(report[MODEL]["errors"], [f"{STALE}: 1 of 3 consolidated runs took no semantic action",
                                                   f"{REWORDED}: 2 of 3 consolidated runs took no semantic action"])

    def test_the_minilm_line_counts_each_arms_setups_and_fails_nothing(self):
        built = arms()
        for arm, scenarios in built.items():
            for entry in scenarios:
                for index in range(3):
                    measure(entry, index, "memory_setup_retrieval_minilm", 1)
        # Two consolidated runs set up lexical; small talk has no class, so its run is not counted.
        measure(built["memory-consolidated"][0], 0, "memory_setup_retrieval_minilm", 0)
        measure(built["memory-consolidated"][3], 2, "memory_setup_retrieval_minilm", 0)
        measure(built["memory"][2], 0, "memory_setup_retrieval_minilm", 0)
        code, report, text = self.run_main(built, self.fake_compare)
        self.assertEqual(code, 0, text)
        self.assertEqual(report[MODEL]["errors"], [])
        self.assertEqual(report[MODEL]["minilm"]["memory-consolidated"], {"runs": 18, "minilm": 16})
        self.assertIn("- set up on MiniLM: 18 of 18 memory-off runs, 18 of 18 memory runs, "
                      "16 of 18 memory-consolidated runs", text)

    @unittest.skipUnless(os.environ.get("HARNESS_E2E_BIN") or shutil.which("harness-e2e"), "needs the runner")
    def test_the_lookup_gain_is_the_runners_delta(self):
        code, report, text = self.run_main(arms())
        self.assertEqual(code, 0, text)
        lookup = report[MODEL]["rows"]["lookup"]["on_vs_off"]
        # The long message gains nothing and counts; the consolidation cases
        # are in their own row: (60 + 60 + 0) / 3.
        self.assertEqual(lookup["quality"]["delta"], 40.0)
        self.assertEqual(lookup["quality"]["tests"], 3)
        with tempfile.TemporaryDirectory() as directory:
            sides = []
            for arm in ("memory-off", "memory"):
                write_arm(Path(directory, arm, "raw"), arms()[arm])
                memory_gain.split(Path(directory, arm, "raw"), memory_gain.rows()["lookup"],
                                  Path(directory, arm, "lookup"))
                sides.append(Path(directory, arm, "lookup"))
            self.assertEqual(lookup["quality"], memory_gain.compare(*sides)["quality"])
        # No class, no row: small talk is in none.
        self.assertEqual(report[MODEL]["rows"]["all"]["on_vs_off"]["quality"]["tests"], 4)
        # The same answers on both arms gain nothing, though memory's runs
        # also scored the rule injection (60 against 30).
        self.assertEqual(report[MODEL]["rows"]["synthesis"]["on_vs_off"]["quality"]["delta"], 0.0)

    @unittest.skipUnless(os.environ.get("HARNESS_E2E_BIN") or shutil.which("harness-e2e"), "needs the runner")
    def test_the_consolidation_cases_lift_only_their_own_row(self):
        built = arms()
        # Only a consolidated bank answers the consolidation cases well.
        for entry in built["memory-consolidated"]:
            if entry["scenario_id"] in (STALE, REWORDED):
                for index in range(3):
                    measure(entry, index, "memory_answer_points", 100)
        code, report, text = self.run_main(built)
        self.assertEqual(code, 0, text)
        rows = report[MODEL]["rows"]
        for name in ("all", "lookup"):
            self.assertEqual(rows[name]["consolidated_vs_on"]["quality"]["delta"], 0.0, name)
        self.assertEqual(rows["consolidation"]["consolidated_vs_on"]["quality"]["delta"], 20.0)
        self.assertEqual(rows["consolidation"]["consolidated_vs_on"]["quality"]["tests"], 2)


if __name__ == "__main__":
    unittest.main()
