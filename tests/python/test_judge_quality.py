import contextlib
import importlib.util
import io
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).parents[2] / "scripts" / "judge_quality.py"
SPEC = importlib.util.spec_from_file_location("judge_quality", SCRIPT)
assert SPEC and SPEC.loader
judge_quality = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(judge_quality)

EASY, ORIGINAL, HARD, TYPED = judge_quality.SLICES


def evidence(slice_id, correct, cases=None, model="m", tokens=None, p_gold=None):
    """A slice's evidence: one single-question case per entry of `correct`
    unless `cases` says how many cases share the decisions."""
    decisions = [{"id": f"{slice_id}-{i}/q", "kind": "noul", "answered": True, "correct": ok,
                  "gold_yes": i % 2 == 0, "p_gold": (p_gold or {}).get(i, 0.9 if ok else 0.1),
                  "model": model, "input_tokens": (tokens or {}).get(i, 10), "ms": 1.0}
                 for i, ok in enumerate(correct)]
    count = cases or len(correct)
    return {"schema": "judge-quality/v1", "provider": "laya", "version": "0.3.2", "config": {"model": "laya"},
            "cards": [{"name": "laya", "release_date": "abc"}], "slice": slice_id,
            "cases": [{"id": f"{slice_id}-{i}", "model": model, "ms": 1.0} for i in range(count)],
            "decisions": decisions, "errors": {},
            "measurements": [{"id": "cases", "value": count, "unit": "count"},
                             {"id": "decisions", "value": len(correct), "unit": "count"},
                             {"id": "accuracy", "value": sum(correct) / len(correct), "unit": "ratio"}]}


def write_arm(directory, slices, shas=None):
    """An arm's results directory: results.json plus one artifact per slice."""
    scenarios = []
    for slice_id, built in slices.items():
        path = f"deliverables/run-{slice_id}/a1/judge_quality_evaluation.json"
        (directory / path).parent.mkdir(parents=True, exist_ok=True)
        (directory / path).write_text(json.dumps(built))
        scenarios.append({"scenario_id": slice_id, "behavior_sha256": (shas or {}).get(slice_id, f"sha:{slice_id}"),
                          "runs": [{"deliverables": [{"id": "judge_quality_evaluation", "artifact": {"path": path}}]}]})
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "results.json").write_text(json.dumps({"schema": "results", "scenarios": scenarios}))


def full(easy=None, hard=None, typed=None, **kwargs):
    """All four slices; JevBench tiers right unless given."""
    return {EASY: evidence(EASY, easy or [True] * 4, **kwargs), ORIGINAL: evidence(ORIGINAL, [True] * 4, **kwargs),
            HARD: evidence(HARD, hard or [True] * 20, **kwargs), TYPED: evidence(TYPED, typed or [True] * 10, **kwargs)}


class JudgeQualityTest(unittest.TestCase):
    def run_main(self, arms, *extra, shas=None):
        with tempfile.TemporaryDirectory() as root:
            runs = []
            for name, slices in arms.items():
                write_arm(Path(root, name), slices, (shas or {}).get(name))
                runs.append({"arm": name, "path": str(Path(root, name))})
            with contextlib.redirect_stdout(io.StringIO()):
                code = judge_quality.main(["--runs", json.dumps(runs), "--output", str(Path(root, "out")), *extra])
            return code, Path(root, "out", "judge-quality.md").read_text(), json.loads(
                Path(root, "out", "judge-quality.json").read_text())

    def test_two_arms_give_a_column_each_with_wilson_intervals_and_the_pooled_row(self):
        code, text, _ = self.run_main({"a": full(easy=[True] * 30 + [False] * 18), "b": full(easy=[True] * 48)})
        self.assertEqual(code, 0, text)
        self.assertIn("| | a | b |", text)
        self.assertIn("| accuracy | 0.625 [0.484, 0.748] n=48 | 1.000 ", text)
        self.assertIn("## jevbench (pooled)", text)
        self.assertIn("| a | 0.750 ", text)
        self.assertIn("| models | m 48 | m 48 |", text)

    def test_shared_states_cap_n_at_the_cases(self):
        slices = full()
        slices[TYPED] = evidence(TYPED, [True, False] * 1000, cases=400)
        code, text, _ = self.run_main({"a": slices})
        self.assertEqual(code, 0, text)
        self.assertIn("| accuracy | 0.500 [0.451, 0.549] n=400 |", text)

    def test_baseline_verdicts_use_the_exact_sign_test_on_pooled_jevbench(self):
        worse = [False] * 12 + [True] * 8
        code, text, report = self.run_main({"base": full(hard=[True] * 19 + [False]), "arm": full(hard=worse)},
                                           "--baseline", "base")
        self.assertEqual(code, 1, text)
        self.assertIn("p=0.0034 **worse**", text)
        self.assertEqual(report["baseline"]["arm"]["jevbench (pooled)"]["b"], 12)
        self.assertEqual(report["baseline"]["arm"]["jevbench (pooled)"]["c"], 1)

        code, text, _ = self.run_main({"base": full(hard=[True] * 19 + [False]),
                                       "arm": full(hard=[False] * 3 + [True] * 17)}, "--baseline", "base")
        self.assertEqual(code, 0, text)
        self.assertIn("p=0.6250 **cannot tell**", text)

    def test_a_missing_slice_reads_not_measured(self):
        slices = full()
        del slices[TYPED]
        code, text, report = self.run_main({"a": slices, "b": full()})
        self.assertEqual(code, 1)
        self.assertIn("not measured", text)
        self.assertIn(f"a: {TYPED} not measured", report["errors"])

    def test_arms_with_different_definitions_are_refused(self):
        code, text, _ = self.run_main({"a": full(), "b": full()}, shas={"b": {HARD: "sha:other"}})
        self.assertEqual(code, 1)
        self.assertIn(f"refused: {HARD} has different definitions (a sha:{HARD}, b sha:other)", text)

    def test_paired_splits_flips_by_routed_model_and_input_tokens(self):
        a = full(hard=[True] * 20)
        b = full(hard=[True] * 20)
        # In b: decisions 0-3 move to another checkpoint, two of them flip;
        # decisions 10-11 read more tokens, one flips the other way.
        moved = b[HARD]["decisions"]
        for i in range(4):
            moved[i]["model"] = "laya-multilingual"
        moved[0]["correct"] = moved[1]["correct"] = False
        a[HARD]["decisions"][10]["correct"] = False
        for i in (10, 11):
            moved[i]["input_tokens"] = 900
        moved[5]["p_gold"] = 0.85
        code, text, report = self.run_main({"a": a, "b": b}, "--paired", "a", "b")
        self.assertEqual(code, 0, text)
        hard = report["paired"][HARD]
        self.assertEqual((hard["all"]["b"], hard["all"]["c"], hard["all"]["n"]), (2, 1, 20))
        self.assertAlmostEqual(hard["all"]["max_delta_p_gold"], 0.05)
        self.assertEqual((hard["model_differs"]["n"], hard["model_differs"]["b"]), (4, 2))
        self.assertEqual((hard["input_tokens_differ"]["n"], hard["input_tokens_differ"]["c"]), (2, 1))
        self.assertEqual(hard["noul_gold_no_wrong"], [0, 1])
        self.assertIn("model_differs: n=4 b=2 c=0 p=0.5000", text)

    def test_sign_test_and_wilson(self):
        self.assertAlmostEqual(judge_quality.sign_test(12, 1), 0.003418, places=6)
        self.assertEqual(judge_quality.sign_test(0, 0), 1.0)
        low, high = judge_quality.wilson(30 / 48, 48)
        self.assertEqual((round(low, 3), round(high, 3)), (0.484, 0.748))


if __name__ == "__main__":
    unittest.main()
