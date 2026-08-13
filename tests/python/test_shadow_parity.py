from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import compare_shadow_results as parity
import evaluate_shadow_windows as windows


def report(execution_id: str, e2e_revision: str = "1" * 40) -> dict:
    return {
        "schema_version": 2,
        "execution": {"execution_id": execution_id},
        "system_under_test": {
            "stack": {
                "mode": "source",
                "workers_repository": "iii-hq/workers",
                "workers_revision": "a" * 40,
            },
            "e2e_revision": e2e_revision,
        },
        "subject": {"provider": "provider", "model": "model"},
        "judge": None,
        "judge_protocol": None,
        "scenarios": [
            {
                "scenario_id": "coordination.1",
                "scenario_version": 1,
                "case_id": "coordination.1:v1:seed-1",
                "case": {"seed": 1, "inputs_sha256": "sha256:" + "b" * 64},
                "execution_policy": {"max_turns": 5},
                "passed": True,
                "aggregate": {
                    "runs": 1,
                    "scored_runs": 1,
                    "passed_runs": 1,
                    "required_passes": 1,
                    "hard_gate_failures": 0,
                    "technical_failures": 0,
                },
                "runs": [
                    {
                        "run_id": execution_id,
                        "session_id": f"session-{execution_id}",
                        "wall_time_ms": 100 if execution_id == "primary" else 150,
                        "status": "passed",
                        "dimensions": [
                            {"dimension": "deliverable", "passed": True},
                            {"dimension": "structural_integrity", "passed": True},
                        ],
                        "hard_gates": [
                            {
                                "id": "artifact",
                                "dimension": "deliverable",
                                "passed": True,
                                "reason": "volatile wording",
                            }
                        ],
                        "deliverables": [
                            {
                                "id": "result",
                                "kind": "state_bundle",
                                "content_sha256": "sha256:" + "c" * 64,
                                "schema_valid": True,
                                "provenance_valid": True,
                                "invariants": [{"id": "complete", "passed": True}],
                            }
                        ],
                    }
                ],
            }
        ],
    }


class ShadowParityTests(unittest.TestCase):
    def test_ignores_volatile_identity_time_and_e2e_revision(self) -> None:
        result = parity.compare(report("primary"), report("shadow", "2" * 40))
        self.assertTrue(result["equivalent"])
        self.assertEqual(result["case_count"], 1)

    def test_reports_independent_semantic_mismatches(self) -> None:
        shadow = report("shadow")
        shadow["scenarios"][0]["runs"][0]["dimensions"][0]["passed"] = False
        shadow["scenarios"][0]["runs"][0]["deliverables"][0]["content_sha256"] = (
            "sha256:" + "d" * 64
        )
        result = parity.compare(report("primary"), shadow)
        self.assertFalse(result["equivalent"])
        self.assertEqual(result["mismatches"][0]["field"], "cases.coordination.1::coordination.1:v1:seed-1.runs")

    def test_requires_three_unique_consecutive_equivalent_windows(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            paths = []
            for index in range(3):
                path = root / f"window-{index}.json"
                path.write_text(
                    json.dumps(
                        {
                            "equivalent": True,
                            "primary": {"execution_id": f"primary-{index}"},
                            "shadow": {"execution_id": f"shadow-{index}"},
                        }
                    ),
                    encoding="utf-8",
                )
                paths.append(path)
            result = windows.evaluate(paths, 3)
            self.assertTrue(result["ready_for_cutover"])

            duplicate = windows.evaluate([paths[0], paths[1], paths[1]], 3)
            self.assertFalse(duplicate["ready_for_cutover"])
            self.assertIn("not unique", duplicate["reasons"][0])


if __name__ == "__main__":
    unittest.main()
