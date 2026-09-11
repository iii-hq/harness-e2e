import json
import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tests/fixtures/registry-version-comparison/validate-feature.cjs"
METRICS = ROOT / "tests/fixtures/registry-version-comparison/metrics.json"


class RegistryFeatureProbeTests(unittest.TestCase):
    def test_probe_ids_are_known_test_two_metrics(self):
        source = SCRIPT.read_text()
        catalog = json.loads(METRICS.read_text())
        known = {metric["id"] for test in catalog["tests"] if test["id"] == 2 for metric in test["metrics"]}
        probed = {
            "implementation.same_version", "implementation.function_removal",
            "implementation.required_impact", "implementation.exact_version",
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

    def run_capture(self, timeout_row_text):
        harness = r"""
const fs = require('node:fs');
const AsyncFunction = Object.getPrototypeOf(async function(){}).constructor;
const source = fs.readFileSync(process.argv[1], 'utf8');
let open = false;
const row = {
  innerText: process.argv[2], textContent: process.argv[2], parentElement: null,
  checkVisibility: () => true,
};
const details = {
  tagName: 'DETAILS', get open() { return open; }, innerText: process.argv[2],
  textContent: process.argv[2], parentElement: row, checkVisibility: () => true,
};
const summary = {
  tagName: 'SUMMARY', textContent: 'before / after', innerText: 'before / after',
  parentElement: details, checkVisibility: () => true, click: () => { open = true; },
  getAttribute: () => null,
};
const body = { get innerText() { return `unrelated timeout 3000 5000\n${process.argv[2]}`; } };
const document = {
  body,
  querySelectorAll: selector => selector === 'button, summary' ? [summary] : [],
};
let now = 0;
const Date = { now: () => (now += 1000) };
const run = new AsyncFunction('capture', 'sleep', 'document', 'location', 'Date', source);
run({kind:'detail'}, async () => {}, document, {href:'http://fixture'}, Date)
  .then(result => process.stdout.write(JSON.stringify(result)));
"""
        result = subprocess.run(
            ["node", "-e", harness, str(ROOT / "tests/fixtures/registry-version-comparison/capture.cjs"), timeout_row_text],
            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True, timeout=5,
        )
        return json.loads(result.stdout)

    def test_timeout_detail_accepts_before_after_summary_and_checks_local_values(self):
        passed = self.run_capture("timeout before / after 3000 5000")
        self.assertEqual(passed["status"], "passed")
        self.assertTrue(passed["state"]["detail_expanded"])
        unrelated = self.run_capture("timeout before / after no values here")
        self.assertEqual(unrelated["status"], "failed")

    @unittest.skipUnless((ROOT / "dashboard/node_modules/playwright").exists(),
                         "dashboard Playwright dependencies are not installed")
    def test_timeout_detail_opens_real_html_details_with_playwright(self):
        harness = r"""
const fs = require('node:fs');
const { chromium } = require('./dashboard/node_modules/playwright');
(async () => {
  const browser = await chromium.launch({headless:true});
  try {
    const page = await browser.newPage();
    await page.setContent('<main><p>unrelated 3000 5000</p><article><h2>timeout</h2><details><summary>before / after</summary><pre>3000 5000</pre></details></article></main>');
    const source = fs.readFileSync(process.argv[1], 'utf8');
    const result = await page.evaluate(async source => {
      const AsyncFunction = Object.getPrototypeOf(async function(){}).constructor;
      return new AsyncFunction('capture', 'sleep', 'document', 'location', 'Date', source)(
        {kind:'detail'}, ms => new Promise(resolve => setTimeout(resolve, ms)), document, location, Date
      );
    }, source);
    process.stdout.write(JSON.stringify({result, open: await page.locator('details').getAttribute('open')}));
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exit(1); });
"""
        completed = subprocess.run(
            ["node", "-e", harness, str(ROOT / "tests/fixtures/registry-version-comparison/capture.cjs")],
            cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            check=True, timeout=15,
        )
        observed = json.loads(completed.stdout)
        self.assertEqual(observed["result"]["status"], "passed")
        self.assertEqual(observed["open"], "")


if __name__ == "__main__":
    unittest.main()
