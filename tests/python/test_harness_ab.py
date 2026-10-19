import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).parents[2] / "scripts" / "harness_ab.py"
SPEC = importlib.util.spec_from_file_location("harness_ab", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def results(scenario, behavior, runs):
    return {"scenarios": [{"scenario_id": scenario, "behavior_sha256": behavior, "runs": [
        {"attempt_id": attempt, "technical": technical, "completion": "completed" if done else "failed",
         "score": score, "cost": {"subject_usd": cost}, "wall_time_ms": 1000,
         "metrics": {"totals": {"input_tokens": 100, "output_tokens": out, "function_call_errors": 0}}}
        for attempt, technical, done, score, cost, out in runs
    ]}]}


class HarnessAbTest(unittest.TestCase):
    def test_arms_compare_on_the_same_definition_and_valid_runs_only(self):
        with tempfile.TemporaryDirectory() as directory:
            a, b = Path(directory, "a"), Path(directory, "b")
            for arm, cost, out, prefix in ((a, 0.2, 1000, "a"), (b, 0.1, 600, "b")):
                runs = [(f"{prefix}{i}", "valid", True, 90, cost, out) for i in range(5)]
                # An infrastructure failure counts on neither arm.
                runs.append((f"{prefix}x", "technical_invalid", False, 0, 9.0, 9))
                (arm / "g1").mkdir(parents=True)
                (arm / "g1/results.json").write_text(json.dumps(results("kanban", "sha:k", runs)))
                # The root bundle repeats a group's results: counted once.
                (arm / "root").mkdir()
                (arm / "root/results.json").write_text(json.dumps(results("kanban", "sha:k", runs)))
            # A definition only one arm ran is left out.
            (b / "g2").mkdir()
            (b / "g2/results.json").write_text(json.dumps(results("registry", "sha:r", [("r1", "valid", True, 50, 1, 1)])))
            result = MODULE.analyse(MODULE.runs_of(a), MODULE.runs_of(b), resamples=500)
        self.assertEqual(result["unmatched"], ["registry"])
        self.assertEqual([(s["scenario"], s["n_a"], s["n_b"]) for s in result["scenarios"]], [("kanban", 5, 5)])
        cost = result["pooled"]["cost_usd"]
        self.assertAlmostEqual(cost["relative"], -0.5)
        self.assertAlmostEqual(cost["relative_ci"][0], -0.5)
        self.assertAlmostEqual(result["pooled"]["output_tokens"]["delta"], -400)
        self.assertEqual(result["pooled"]["completed"]["delta"], 0)
        self.assertIn("| Custo (US$) | 0.200 | 0.100 |", MODULE.report(result, "1.8.44", "1.8.50"))


if __name__ == "__main__":
    unittest.main()
