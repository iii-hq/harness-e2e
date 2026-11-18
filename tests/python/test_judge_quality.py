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


def evidence(slice_id, correct, cases=None, model="m", tokens=None, p_gold=None, groups=None):
    """A slice's evidence: one single-question case per entry of `correct`
    unless `cases` says how many cases share the decisions (decision i in case
    i % cases); `groups` gives decision i the JevBench group groups[i]."""
    count = cases or len(correct)
    decisions = [{"id": f"{slice_id}-{i % count}/q{i // count}", "kind": "noul", "answered": True, "correct": ok,
                  "gold_yes": i % 2 == 0, "p_gold": (p_gold or {}).get(i, 0.9 if ok else 0.1),
                  "model": model, "input_tokens": (tokens or {}).get(i, 10), "ms": 1.0}
                 for i, ok in enumerate(correct)]
    for decision, name in zip(decisions, groups or []):
        decision["group"] = name
    return {"schema": "judge-quality/v1", "provider": "laya", "version": "0.3.2", "config": {"model": "laya"},
            "cards": [{"name": "laya", "release_date": "abc"}], "slice": slice_id,
            "cases": [{"id": f"{slice_id}-{i}", "model": model, "ms": 1.0} for i in range(count)],
            "decisions": decisions, "errors": {},
            "measurements": [{"id": "cases", "value": count, "unit": "count"},
                             {"id": "decisions", "value": len(correct), "unit": "count"},
                             {"id": "accuracy", "value": sum(correct) / len(correct), "unit": "ratio"}]}


def write_arm(directory, slices, shas=None, results_dir="", started="2026-11-01T10:00:00.000Z"):
    """An arm's results directory: results.json (in `results_dir`, run at
    `started`) plus one artifact per slice, its path relative to the arm root;
    a slice whose evidence is None has a run without the artifact."""
    scenarios = []
    for slice_id, built in slices.items():
        path = f"deliverables/{results_dir or 'run'}-{slice_id}/a1/judge_quality_evaluation.json"
        if built is not None:
            (directory / path).parent.mkdir(parents=True, exist_ok=True)
            (directory / path).write_text(json.dumps(built))
        scenarios.append({"scenario_id": slice_id, "behavior_sha256": (shas or {}).get(slice_id, f"sha:{slice_id}"),
                          "runs": [{"deliverables": [{"id": "judge_quality_evaluation", "artifact": {"path": path}}]}]})
    (directory / results_dir).mkdir(parents=True, exist_ok=True)
    (directory / results_dir / "results.json").write_text(json.dumps(
        {"schema": "results", "execution": {"started_at": started}, "scenarios": scenarios}))


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

    def test_grouped_decisions_get_no_interval(self):
        slices = full()
        slices[TYPED] = evidence(TYPED, [True, False] * 1000, cases=400)
        slices[ORIGINAL] = evidence(ORIGINAL, [True, False, True, True], groups=["g1", "g1", "g2", "g2"])
        code, text, _ = self.run_main({"a": slices})
        self.assertEqual(code, 0, text)
        self.assertIn("| accuracy | 0.500 n=2000 in 400 groups |", text)
        self.assertIn("| accuracy | 0.750 n=4 in 2 groups |", text)
        self.assertIn("| a | 0.964 n=28 in 26 groups |", text)

    def test_baseline_verdicts_use_the_exact_sign_test_on_pooled_jevbench(self):
        worse = [False] * 12 + [True] * 8
        code, text, report = self.run_main({"base": full(hard=[True] * 19 + [False]), "arm": full(hard=worse)},
                                           "--baseline", "base")
        self.assertEqual(code, 1, text)
        self.assertIn("b=12 c=1 groups b=12 c=1 p=0.0034 **worse**", text)
        self.assertEqual(report["baseline"]["arm"]["jevbench (pooled)"]["b"], 12)
        self.assertEqual(report["baseline"]["arm"]["jevbench (pooled)"]["c"], 1)

        code, text, _ = self.run_main({"base": full(hard=[True] * 19 + [False]),
                                       "arm": full(hard=[False] * 3 + [True] * 17)}, "--baseline", "base")
        self.assertEqual(code, 0, text)
        self.assertIn("p=0.6250 **cannot tell**", text)

        code, text, _ = self.run_main({"base": full(hard=[True] + [False] * 12 + [True] * 7),
                                       "arm": full(hard=[False] + [True] * 19)}, "--baseline", "base")
        self.assertEqual(code, 0, text)
        self.assertIn("b=1 c=12 groups b=1 c=12 p=0.0034 **better**", text)

    def test_paraphrases_of_one_scenario_count_once_in_the_sign_test(self):
        names = ["g1", "g1", "g2", "g2", "g3", "g3"]
        base = full()
        base[ORIGINAL] = evidence(ORIGINAL, [True] * 6, groups=names)
        arm = full()
        arm[ORIGINAL] = evidence(ORIGINAL, [False] * 6, groups=names)
        code, text, report = self.run_main({"base": base, "arm": arm}, "--baseline", "base")
        self.assertEqual(code, 0, text)
        pooled = report["baseline"]["arm"]["jevbench (pooled)"]
        self.assertEqual((pooled["b"], pooled["c"], pooled["groups_b"], pooled["groups_c"]), (6, 0, 3, 0))
        self.assertAlmostEqual(pooled["p"], 0.25)
        self.assertEqual(pooled["verdict"], "cannot tell")

    def test_typed_decisions_pair_whole_cases(self):
        a = {f"s{case}/q{q}": {"id": f"s{case}/q{q}", "correct": True} for case in range(2) for q in range(5)}
        b = {i: {**decision, "correct": False} for i, decision in a.items()}
        result = judge_quality.pair(a, b, sorted(a))
        self.assertEqual((result["b"], result["groups"], result["groups_b"]), (10, 2, 2))
        self.assertAlmostEqual(result["p"], 0.5)
        # A case that loses two decisions and wins one counts for the baseline once.
        b["s0/q0"]["correct"] = b["s0/q1"]["correct"] = True
        a["s0/q2"]["correct"] = False
        mixed = judge_quality.pair(a, b, [f"s0/q{q}" for q in range(5)])
        self.assertEqual((mixed["b"], mixed["c"], mixed["groups_b"], mixed["groups_c"]), (2, 0, 1, 0))

    def test_an_artifact_path_resolves_against_the_arm_root(self):
        with tempfile.TemporaryDirectory() as root:
            write_arm(Path(root), full(), results_dir="run-1")
            slices = judge_quality.load_arm(Path(root))
        self.assertEqual(sorted(slices), sorted(judge_quality.SLICES))
        self.assertEqual(slices[HARD]["evidence"]["slice"], HARD)

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
        # decisions 10-12 read more tokens, 10 flips one way, 11 (gold no) the
        # other, 12 drifts 0.4 without flipping. With equal tokens, 5 drifts
        # 0.05 and 13 (gold no, wrong in both) goes unanswered.
        moved = b[HARD]["decisions"]
        for i in range(4):
            moved[i]["model"] = "laya-multilingual"
        moved[0]["correct"] = moved[1]["correct"] = False
        a[HARD]["decisions"][10]["correct"] = False
        for i in (10, 11, 12):
            moved[i]["input_tokens"] = 900
        moved[11].update(correct=False, p_gold=0.4)
        moved[12]["p_gold"] = 0.5
        moved[5]["p_gold"] = 0.85
        a[HARD]["decisions"][13].update(correct=False, p_gold=0.1)
        moved[13].update(answered=False, correct=False, p_gold=None)
        code, text, report = self.run_main({"a": a, "b": b}, "--paired", "a", "b")
        self.assertEqual(code, 0, text)
        hard = report["paired"][HARD]
        self.assertEqual({k: hard["all"][k] for k in ("n", "b", "c", "changed", "noul_gold_no_wrong")},
                         {"n": 20, "b": 3, "c": 1, "changed": 5, "noul_gold_no_wrong": [1, 3]})
        self.assertAlmostEqual(hard["all"]["max_delta_p_gold"], 0.4)
        self.assertEqual((hard["model_differs"]["n"], hard["model_differs"]["b"]), (4, 2))
        equal = hard["input_tokens_equal"]
        self.assertEqual((equal["n"], equal["b"], equal["c"], equal["changed"]), (17, 2, 0, 3))
        self.assertAlmostEqual(equal["max_delta_p_gold"], 0.05)
        differ = hard["input_tokens_differ"]
        self.assertEqual((differ["n"], differ["b"], differ["c"], differ["noul_gold_no_wrong"]), (3, 1, 1, [0, 1]))
        self.assertIn("model_differs: n=4 b=2 c=0 groups=4 groups b=2 c=0 p=0.5000 changed=2", text)
        self.assertIn("input_tokens_equal: n=17 b=2 c=0 groups=17 groups b=2 c=0 p=0.5000 changed=3 "
                      "max|dp_gold|=5.00e-02 noul gold-no wrong=1/2", text)

    def test_a_decision_without_input_tokens_is_in_no_token_subset(self):
        a, b = full(), full()
        del a[HARD]["decisions"][0]["input_tokens"]
        del a[HARD]["decisions"][1]["input_tokens"]
        del b[HARD]["decisions"][1]["input_tokens"]
        b[HARD]["decisions"][0]["correct"] = b[HARD]["decisions"][1]["correct"] = False
        _, _, report = self.run_main({"a": a, "b": b}, "--paired", "a", "b")
        hard = report["paired"][HARD]
        self.assertEqual((hard["all"]["n"], hard["all"]["b"]), (20, 2))
        self.assertEqual((hard["input_tokens_equal"]["n"], hard["input_tokens_equal"]["b"]), (18, 0))
        self.assertEqual(hard["input_tokens_differ"]["n"], 0)

    def test_an_answer_that_moves_between_two_wrong_labels_counts_as_changed(self):
        a = {"x": {"id": "x/q", "answered": True, "correct": False, "p_gold": 0.1,
                   "probabilities": {"a": 0.5, "b": 0.1, "c": 0.4}}}
        b = {"x": {"id": "x/q", "answered": True, "correct": False, "p_gold": 0.1,
                   "probabilities": {"a": 0.4, "b": 0.1, "c": 0.5}}}
        result = judge_quality.pair(a, b, ["x"])
        self.assertEqual((result["b"], result["c"], result["changed"], result["max_delta_p_gold"]), (0, 0, 1, 0.0))
        self.assertEqual(judge_quality.pair(a, a, ["x"])["changed"], 0)
        # A tie the two arms break differently: the returned label tells them apart.
        tied = {"a": 0.45, "b": 0.45, "c": 0.1}
        a["x"].update(probabilities=tied, choice="a")
        b["x"].update(probabilities=tied, choice="b")
        self.assertEqual(judge_quality.pair(a, b, ["x"])["changed"], 1)

    def test_each_slice_comes_from_the_latest_run_that_has_it(self):
        with tempfile.TemporaryDirectory() as root:
            old, new = full(), full(hard=[False] * 20)
            # "a-new" sorts before "b-old" but ran later; its typed run left no artifact.
            write_arm(Path(root), old, results_dir="b-old", started="2026-11-01T09:00:00.000Z")
            write_arm(Path(root), {**new, TYPED: None}, results_dir="a-new", started="2026-11-01T10:00:00.000Z")
            slices = judge_quality.load_arm(Path(root))
        self.assertFalse(any(d["correct"] for d in slices[HARD]["evidence"]["decisions"]))
        self.assertNotIn(TYPED, slices)

    def test_a_results_file_without_a_start_time_refuses_the_arm(self):
        with tempfile.TemporaryDirectory() as root:
            write_arm(Path(root), full())
            document = json.loads(Path(root, "results.json").read_text())
            del document["execution"]
            Path(root, "results.json").write_text(json.dumps(document))
            with self.assertRaisesRegex(ValueError, "execution.started_at"):
                judge_quality.load_arm(Path(root))

    def test_an_arm_whose_slices_ran_different_builds_is_refused(self):
        slices = full()
        slices[HARD]["version"] = "0.3.3"
        code, text, _ = self.run_main({"a": slices})
        self.assertEqual(code, 1)
        self.assertIn("refused: a ran its slices on different builds or configurations", text)
        self.assertIn(f"{HARD}: laya 0.3.3", text)

    def test_an_empty_arm_list_is_refused(self):
        with tempfile.TemporaryDirectory() as root, contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                judge_quality.main(["--runs", "[]", "--output", root])

    def test_sign_test_and_wilson(self):
        self.assertAlmostEqual(judge_quality.sign_test(12, 1), 0.003418, places=6)
        self.assertEqual(judge_quality.sign_test(0, 0), 1.0)
        low, high = judge_quality.wilson(30 / 48, 48)
        self.assertEqual((round(low, 3), round(high, 3)), (0.484, 0.748))


if __name__ == "__main__":
    unittest.main()
