import json
import os
import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tests/fixtures/registry-version-comparison/validate-feature.cjs"
METRICS = ROOT / "tests/fixtures/registry-version-comparison/metrics.json"


class RegistryFeatureProbeTests(unittest.TestCase):
    def test_real_browser_failures_cover_ssr_and_spa(self):
        playwright = Path(os.environ.get("PLAYWRIGHT_MODULE_PATH", ROOT / "dashboard/node_modules/playwright/index.mjs"))
        self.assertTrue(playwright.exists(), "Playwright is required for evaluator regression controls")
        result = subprocess.run(
            ["node", str(SCRIPT.with_name("failure-regressions.mjs"))], cwd=ROOT,
            env={**os.environ, "PLAYWRIGHT_MODULE_PATH": playwright.as_uri()},
            text=True, capture_output=True, timeout=900,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        result = json.loads(result.stdout)
        self.assertEqual(len(result["replays"]), 6)
        self.assertTrue(all(count == 100 for count in result["replays"].values()))
        self.assertGreaterEqual(result["actual503Responses"], 1200)

    def test_the_probe_runs_when_node_reads_it_on_stdin(self):
        # validate.py pipes the probe into `node -`, where require.main is
        # undefined: a guard on require.main === module alone ran nothing,
        # exited 0 and wrote no feature.json. With nothing to reach, the probe
        # still runs and records why.
        output = Path("/tmp/registry-validation/feature.json")
        output.unlink(missing_ok=True)
        try:
            result = subprocess.run(
                ["node", "-"], input=SCRIPT.read_text(), cwd=ROOT, text=True, capture_output=True,
                timeout=120, env={**os.environ, "TEST_API_URL": "http://127.0.0.1:9",
                                  "E2E_APP_URL": "http://127.0.0.1:9"},
            )
            self.assertTrue(output.is_file(), result.stderr)
            self.assertIn("observations", json.loads(output.read_text()))
        finally:
            output.unlink(missing_ok=True)

    def test_probe_ids_are_known_test_two_metrics(self):
        source = SCRIPT.read_text()
        catalog = json.loads(METRICS.read_text())
        known = {metric["id"] for test in catalog["tests"] if test["id"] == 2 for metric in test["metrics"]}
        probed = {
            "implementation.same_version", "implementation.function_removal",
            "implementation.required_impact",
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
