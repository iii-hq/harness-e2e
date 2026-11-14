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


def write_arm(directory, scenarios):
    """An arm as a run's root bundle lays it out: a group's results and the
    root's, which repeats them."""
    for place in ("campaign/case-a", "campaign"):
        (directory / place).mkdir(parents=True, exist_ok=True)
        (directory / place / "results.json").write_text(json.dumps({"schema": "results", "scenarios": scenarios}))


def arms(off_tokens=0, consolidated=True):
    """The scenarios of each arm: memory raises the lookup answers from 20 to
    80. The synthesis case answers the same on both arms, and only memory's
    run scores its rule injection."""
    def build(prefix, lookup, rule, measurements=()):
        return [
            scenario("memory_value_project", "lookup", lookup, prefix, measurements),
            scenario("memory_rule_fit", "synthesis", rule, prefix, measurements, answers=[30, 30, 30]),
            scenario("memory_small_talk", None, [100, 100, 100], prefix, measurements),
            scenario("memory_precise_port", "lookup", lookup, prefix, measurements),
            scenario("memory_precise_long_message", "lookup", [30, 30, 30], prefix, measurements,
                     answers=[0, 0, 0]),
        ]
    built = {
        "memory-off": build("off", [20, 20, 20], [30, 30, 30], [("memory_injected_tokens", off_tokens)]),
        "memory": build("on", [80, 80, 80], [60, 60, 60], [("memory_injected_tokens", 12)]),
    }
    if consolidated:
        built["memory-consolidated"] = build("cons", [80, 80, 80], [60, 60, 60], [
            ("memory_consolidation_superseded", 1), ("memory_consolidation_llm_calls", 0),
            ("memory_consolidation_notes", 1)])
    return built


class MemoryGainTest(unittest.TestCase):
    def run_main(self, built, compare=None):
        """main() on runs whose downloads are the built arms; its exit code and report."""
        runs = [{"model": MODEL, "arm": arm, "run_url": f"https://github.com/iii-hq/harness-e2e/actions/runs/{i}"}
                for i, arm in enumerate(built)]

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
                             ["memory_value_project", "memory_precise_port", "memory_precise_long_message"])
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

    def test_the_rows_hold_the_classed_cases_and_the_precise_row_reads_the_plan(self):
        self.assertEqual(len(memory_gain.precise()), 6)
        rows = memory_gain.rows()
        self.assertTrue(rows["precise"]({"scenario_id": "memory_precise_port", "class": "lookup"}))
        self.assertFalse(rows["precise"]({"scenario_id": "memory_value_project", "class": "lookup"}))
        # The probe of memory's long-line cut, and a case without a class, are in no row.
        for row in rows.values():
            self.assertFalse(row({"scenario_id": "memory_precise_long_message", "class": "lookup"}))
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
        self.assertEqual(report[MODEL]["off_injected_tokens"], 7 * 15)
        self.assertIn("the memory-off arm injected 105 tokens: injection was not off", text)

    def test_the_consolidation_line_counts_the_compared_runs_that_consolidated(self):
        code, report, text = self.run_main(arms(), self.fake_compare)
        self.assertEqual(code, 0)
        # The three compared cases, three runs each: no small talk, no probe.
        self.assertEqual(report[MODEL]["consolidation"], {"runs": 9, "consolidated": 9, "superseded": 9,
                                                          "llm_calls": 0, "notes": 9})
        self.assertIn("consolidation ran in 9 of 9 runs, superseded 9 memories, 0 model calls, 9 notes", text)
        self.assertIn("the memory-off arm injected nothing", text)
        self.assertEqual(report[MODEL]["errors"], [])

    def test_a_compared_run_that_never_consolidated_is_an_error(self):
        built = arms()
        unconsolidated = scenario("memory_rule_fit", "synthesis", [60, 60, 60], "cons")
        # A run whose setup failed has no measurements and is not counted.
        unconsolidated["runs"][2]["scenario_measurements"] = []
        built["memory-consolidated"][1] = unconsolidated
        _, report, text = self.run_main(built, self.fake_compare)
        self.assertEqual(report[MODEL]["consolidation"]["runs"], 8)
        self.assertEqual(report[MODEL]["consolidation"]["consolidated"], 6)
        self.assertIn("2 memory-consolidated runs never consolidated", text)

    def test_a_consolidated_arm_whose_runs_never_consolidated_says_so(self):
        built = arms()
        built["memory-consolidated"] = arms()["memory"]
        _, _, text = self.run_main(built, self.fake_compare)
        self.assertIn("no run consolidated: this arm is a second memory-on arm", text)

    @unittest.skipUnless(os.environ.get("HARNESS_E2E_BIN") or shutil.which("harness-e2e"), "needs the runner")
    def test_the_lookup_gain_is_the_runners_delta(self):
        code, report, text = self.run_main(arms())
        self.assertEqual(code, 0, text)
        lookup = report[MODEL]["rows"]["lookup"]["on_vs_off"]
        self.assertEqual(lookup["quality"]["delta"], 60.0)
        # The long-message probe is left out.
        self.assertEqual(lookup["quality"]["tests"], 2)
        with tempfile.TemporaryDirectory() as directory:
            sides = []
            for arm in ("memory-off", "memory"):
                write_arm(Path(directory, arm, "raw"), arms()[arm])
                memory_gain.split(Path(directory, arm, "raw"), memory_gain.rows()["lookup"],
                                  Path(directory, arm, "lookup"))
                sides.append(Path(directory, arm, "lookup"))
            self.assertEqual(lookup["quality"], memory_gain.compare(*sides)["quality"])
        # No class, no row: small talk is in none.
        self.assertEqual(report[MODEL]["rows"]["all"]["on_vs_off"]["quality"]["tests"], 3)
        # The same answers on both arms gain nothing, though memory's runs
        # also scored the rule injection (60 against 30).
        self.assertEqual(report[MODEL]["rows"]["synthesis"]["on_vs_off"]["quality"]["delta"], 0.0)


if __name__ == "__main__":
    unittest.main()
