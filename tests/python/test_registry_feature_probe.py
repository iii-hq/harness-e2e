import json
import os
import re
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
        self.assertEqual(result["uppercase"], {"ssr:correct": 10, "ssr:stale": 10, "spa:correct": 10, "spa:stale": 10})
        self.assertGreaterEqual(result["actual503Responses"], 1200)

    def test_expanded_detail_reads_only_visible_row_or_controlled_values(self):
        playwright = Path(os.environ.get("PLAYWRIGHT_MODULE_PATH", ROOT / "dashboard/node_modules/playwright/index.mjs"))
        self.assertTrue(playwright.exists(), "Playwright is required for evaluator regression controls")
        result = subprocess.run(
            ["node", str(SCRIPT.with_name("disclosure-regressions.mjs"))], cwd=ROOT,
            env={**os.environ, "PLAYWRIGHT_MODULE_PATH": playwright.as_uri()},
            text=True, capture_output=True, timeout=300,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        accepted = {name for name, passed in json.loads(result.stdout)["disclosure"].items() if passed}
        self.assertEqual(accepted, {"nested", "controlled_sibling"})

    def test_config_array_order_accepts_a_whole_or_per_index_reversal(self):
        # One changed array and one changed entry per index are both ordered JSON Pointer diffs.
        script = """
const { configArrayReordered } = require(process.argv[1]);
const c = (path, before, after, kind = 'changed', area = 'config') => ({ area, path, kind, before, after });
const cases = {
  whole: [c('/required', ['one', 'two'], ['two', 'one'])],
  per_index: [c('/required/0', 'one', 'two'), c('/required/1', 'two', 'one')],
  set_semantics: [],
  unchanged: [c('/required', ['one', 'two'], ['one', 'two'])],
  one_index: [c('/required/0', 'one', 'two')],
  wrong_values: [c('/required/0', 'one', 'two'), c('/required/1', 'one', 'two')],
  wrong_area: [c('/required', ['one', 'two'], ['two', 'one'], 'changed', 'functions')],
  swapped: [c('/required/0', 'two', 'one'), c('/required/1', 'one', 'two')],
  per_index_removed: [c('/required/0', 'one', 'two', 'removed'), c('/required/1', 'two', 'one', 'removed')],
  per_index_added: [c('/required/0', 'one', 'two', 'added'), c('/required/1', 'two', 'one', 'added')],
};
console.log(JSON.stringify(Object.fromEntries(Object.entries(cases).map(
  ([name, changes]) => [name, configArrayReordered({ status: 200, body: { changes } })]))));
"""
        result = subprocess.run(["node", "-e", script, str(SCRIPT)], cwd=ROOT, text=True,
                                capture_output=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)
        accepted = {name for name, passed in json.loads(result.stdout).items() if passed}
        self.assertEqual(accepted, {"whole", "per_index"})
        # The probe scores the case through this helper, not an inline copy of the old check.
        self.assertIn("? configArrayReordered(result)", SCRIPT.read_text())

    def test_the_probe_browser_opens_the_app_on_localhost(self):
        # The fixture's Next 16 dev server refuses its HMR socket to a 127.0.0.1 origin, and a page
        # opened there never hydrated: 0.41.0 registry-implementation r01's select never fetched its pair.
        script = """
const { appOrigin } = require(process.argv[1]);
console.log(JSON.stringify(['http://127.0.0.1:43000', 'http://127.0.0.1:43000/workers', 'http://localhost:43000',
  'http://127.0.0.10:43000', 'http://web:43000', undefined].map(appOrigin)));
"""
        result = subprocess.run(["node", "-e", script, str(SCRIPT)], cwd=ROOT, text=True,
                                capture_output=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), ["http://localhost:43000", "http://localhost:43000/workers",
                                                     "http://localhost:43000", "http://127.0.0.10:43000",
                                                     "http://web:43000", None])
        self.assertIn("const app = appOrigin(process.env.E2E_APP_URL);", SCRIPT.read_text())

    def test_keyboard_selector_waits_for_a_controlled_select_to_settle(self):
        # Hydrated, 0.41.0 r01's controlled select kept 2.0.0 until router.push landed on 1.1.0;
        # reading right after Enter failed a working selector.
        playwright = Path(os.environ.get("PLAYWRIGHT_MODULE_PATH", ROOT / "dashboard/node_modules/playwright/index.mjs"))
        self.assertTrue(playwright.exists(), "Playwright is required for evaluator regression controls")
        script = """
const { keyboardSelectChanges } = require(process.argv[1]);
const { chromium } = require(process.argv[2]);
const select = (handler = '', attrs = '') => `<select id="s" ${attrs}><option>a</option><option>b</option></select><script>${handler}</script>`;
const pages = {
  native: select(),
  controlled_late: select("let v='a';s.addEventListener('change',()=>{const n=s.value;s.value=v;setTimeout(()=>{v=n;s.value=n},800)})"),
  always_reverts: select("s.addEventListener('change',()=>{s.value='a'})"),
  disabled: select('', 'disabled'),
};
(async () => {
  const browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
  const out = {};
  try {
    for (const [name, html] of Object.entries(pages)) {
      const page = await browser.newPage();
      await page.setContent(html);
      out[name] = await keyboardSelectChanges(page, page.locator('select'), 2000);
      await page.close();
    }
  } finally { await browser.close(); }
  console.log(JSON.stringify(out));
})();
"""
        result = subprocess.run(["node", "-e", script, str(SCRIPT), str(playwright.parent)], cwd=ROOT, text=True,
                                capture_output=True, timeout=120)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout),
                         {"native": True, "controlled_late": True, "always_reverts": False, "disabled": False})
        self.assertIn("keyboard = await keyboardSelectChanges(page, selects.nth(1), timeout);", SCRIPT.read_text())

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

    def test_the_tester_is_asked_each_check_s_metric_question(self):
        # The tester sees only the prompt's questions, so they must be the ones
        # the metrics state, for every check the probe grades it on.
        prompt = (SCRIPT.parent / "test-4-verification.md").read_text()
        asked = dict(re.findall(r"^- `(implementation\.\w+)`: (.+)$", prompt, re.M))
        catalog = json.loads(METRICS.read_text())
        questions = {metric["id"]: metric["question"] for test in catalog["tests"]
                     if test["id"] == 2 for metric in test["metrics"]}
        self.assertEqual(len(asked), 23)
        self.assertEqual(asked, {check: questions[check] for check in asked})

    def test_probe_has_bounded_runtime_and_factual_output(self):
        source = SCRIPT.read_text()
        self.assertIn("AbortSignal.timeout", source)
        self.assertIn("status: 'unavailable'", source)
        self.assertIn("status: 'measured'", source)
        self.assertIn("/tmp/registry-validation", source)


if __name__ == "__main__":
    unittest.main()
