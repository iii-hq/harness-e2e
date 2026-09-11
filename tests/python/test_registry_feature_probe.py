import json
import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tests/fixtures/registry-version-comparison/validate-feature.cjs"
DETAIL_PROBE = ROOT / "tests/fixtures/registry-version-comparison/detail-probe.cjs"
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

    @unittest.skipUnless((ROOT / "dashboard/node_modules/playwright").exists(),
                         "dashboard Playwright dependencies are not installed")
    def test_timeout_detail_uses_semantic_control_and_bounded_row_in_real_chromium(self):
        harness = r"""
const fs = require('node:fs');
const { chromium } = require('./dashboard/node_modules/playwright');
(async () => {
  const browser = await chromium.launch({headless:true});
  try {
    const source = fs.readFileSync(process.argv[1], 'utf8') + '\n' + fs.readFileSync(process.argv[2], 'utf8');
    const fixtures = JSON.parse(process.argv[3]);
    const results = {};
    for (const [name, html] of Object.entries(fixtures)) {
      const page = await browser.newPage();
      await page.setContent(html);
      results[name] = await page.evaluate(async source => {
        const AsyncFunction = Object.getPrototypeOf(async function(){}).constructor;
        let now = 0;
        const fakeDate = {now: () => (now += 1000)};
        return new AsyncFunction('capture', 'sleep', 'document', 'location', 'Date', source)(
          {kind:'detail'}, async () => {}, document, location, fakeDate
        );
      }, source);
      await page.close();
    }
    process.stdout.write(JSON.stringify(results));
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exit(1); });
"""
        fixtures = {
            "before_after": "<main><div><header>/timeoutMs</header><details><summary>before / after</summary><pre>3000 5000</pre></details></div></main>",
            "timeout_label": "<main><details><summary>timeout</summary><pre>3000 5000</pre></details></main>",
            "button_local": "<main><article><h2>timeout</h2><button aria-expanded='false' onclick=\"this.setAttribute('aria-expanded','true');this.nextElementSibling.hidden=false\">show values</button><div hidden>3000 5000</div></article></main>",
            "sibling": "<main><article><details><summary>before / after</summary><pre>unrelated 10 20</pre></details></article><article><h2>timeout</h2><p>3000 5000</p></article></main>",
            "inert": "<main><article><h2>timeout</h2><button aria-expanded='false'>before / after</button><div>3000 5000</div></article></main>",
        }
        completed = subprocess.run(
            ["node", "-e", harness, str(DETAIL_PROBE),
             str(ROOT / "tests/fixtures/registry-version-comparison/capture.cjs"), json.dumps(fixtures)],
            cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            check=True, timeout=30,
        )
        observed = json.loads(completed.stdout)
        self.assertEqual(observed["before_after"]["status"], "passed")
        self.assertEqual(observed["timeout_label"]["status"], "passed")
        self.assertEqual(observed["button_local"]["status"], "passed")
        self.assertEqual(observed["sibling"]["status"], "failed")
        self.assertEqual(observed["inert"]["status"], "failed")


if __name__ == "__main__":
    unittest.main()
