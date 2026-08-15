from __future__ import annotations

import json
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from assessment_contract import AssessmentContractError, normalize_assessment_contract


class AssessmentContractTests(unittest.TestCase):
    def fixture(self, name: str) -> dict:
        return json.loads((ROOT / "tests" / "fixtures" / "results" / name).read_text())

    def test_preserves_the_shared_v3_contract_fixture(self) -> None:
        result = self.fixture("results-v3-assessment-contract.json")
        contract = normalize_assessment_contract(result)

        self.assertEqual(contract, result["assessment_contract"])
        run = contract["runs"][0]
        self.assertEqual(run["system_status"], "hard_gate_failed")
        self.assertEqual(run["ai_final_assessment"]["availability"], "unavailable")
        self.assertEqual(run["effective_status"], "hard_gate_failed")

    def test_normalizes_legacy_results_without_fabricating_status(self) -> None:
        contract = normalize_assessment_contract(
            self.fixture("results-v2-without-assessments.json")
        )

        self.assertEqual(contract["contract_version"], 1)
        self.assertEqual(contract["runs"][0]["system_status"], "unavailable")
        self.assertEqual(contract["runs"][0]["effective_status"], "unavailable")
        self.assertEqual(
            contract["runs"][0]["ai_final_assessment"]["availability"],
            "not_evaluated",
        )

    def test_rejects_v3_without_the_contract(self) -> None:
        with self.assertRaisesRegex(AssessmentContractError, "require assessment_contract"):
            normalize_assessment_contract({"schema_version": 3, "scenarios": []})


if __name__ == "__main__":
    unittest.main()
