import json
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "repository-tasks/registry-version-comparison/validate-feature.cjs"
METRICS = ROOT / "repository-tasks/registry-version-comparison/metrics.json"


class RegistryFeatureProbeTests(unittest.TestCase):
    def test_probe_ids_are_known_test_two_metrics(self):
        source = SCRIPT.read_text()
        catalog = json.loads(METRICS.read_text())
        known = {metric["id"] for test in catalog["tests"] if test["id"] == 2 for metric in test["metrics"]}
        probed = {
            "implementation.same_version", "implementation.function_removal",
            "implementation.required_impact", "implementation.object_order",
            "implementation.required_order", "implementation.enum_order",
            "implementation.config_array_order", "implementation.missing_metadata",
            "implementation.worker_lookup", "implementation.reverse_kinds",
            "implementation.reverse_values", "implementation.reverse_impact",
            "implementation.shared_url", "implementation.stale_results",
            "implementation.patch_application", "implementation.invalid_version",
            "implementation.missing_worker", "implementation.missing_version",
            "implementation.history", "implementation.expanded_detail",
            "implementation.keyboard_selectors", "implementation.versions_regression",
            "implementation.readme_regression", "implementation.api_reference_regression",
            "implementation.download_regression",
        }
        self.assertEqual(probed, known)
        self.assertIn("feature.json", source)

    def test_probe_has_bounded_runtime_and_factual_output(self):
        source = SCRIPT.read_text()
        self.assertIn("AbortSignal.timeout", source)
        self.assertIn("status: 'unavailable'", source)
        self.assertIn("status: 'measured'", source)
        self.assertIn("/tmp/registry-validation", source)


if __name__ == "__main__":
    unittest.main()
