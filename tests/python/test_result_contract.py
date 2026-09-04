import json
import pathlib
import sys
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from generate_result_contract import generated_files
from result_contract import contract_values


class ResultContractTests(unittest.TestCase):
    def test_checked_in_rust_and_typescript_match_authoritative_sources(self):
        for path, expected in generated_files().items():
            with self.subTest(path=str(path)):
                self.assertEqual(
                    path.read_text(encoding="utf-8"),
                    expected,
                    "Run python3 scripts/generate_result_contract.py after exporting schemas",
                )

    def test_source_change_updates_both_generated_consumers(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / "config").mkdir()
            (root / "config/results-contract.json").write_text(json.dumps({
                "schema_version": 4,
                "results_schema": "schema.json",
                "scoring_profile": "profile.json",
            }), encoding="utf-8")
            (root / "schema.json").write_text('{"type":"object"}', encoding="utf-8")
            (root / "profile.json").write_text('{"weight":1}', encoding="utf-8")
            before = contract_values(root)
            (root / "schema.json").write_text('{"type":"array"}', encoding="utf-8")
            after = contract_values(root)
            self.assertNotEqual(before["RESULT_CONTRACT_SHA256"], after["RESULT_CONTRACT_SHA256"])
            self.assertEqual(before["SCORING_PROFILE_SHA256"], after["SCORING_PROFILE_SHA256"])
            for content in generated_files(root).values():
                self.assertIn(after["RESULT_CONTRACT_SHA256"], content)
                self.assertNotIn(before["RESULT_CONTRACT_SHA256"], content)


if __name__ == "__main__":
    unittest.main()
