import json
import os
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
PLAYWRIGHT = Path(os.environ.get("PLAYWRIGHT_MODULE_PATH", ROOT / "dashboard/node_modules/playwright/index.mjs"))


class TrendingFocusEvalRegressions(unittest.TestCase):
    @unittest.skipUnless(PLAYWRIGHT.is_file(), "Playwright is not installed")
    def test_visible_focus_rejects_transparent_and_imperceptible_changes(self):
        result = subprocess.run(
            ["node", str(ROOT / "tests/fixtures/trending-topics-build/focus-controls.mjs")],
            env={**os.environ, "PLAYWRIGHT_MODULE_PATH": PLAYWRIGHT.as_uri()},
            capture_output=True, text=True, timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), {
            "outline": True, "background": True, "translucent": True, "faded": False, "transparent": False,
            "lowContrast": False, "imperceptible": False,
            "pillOutline": True, "pillShadow": True, "pillInset": True, "pillPageColor": False, "pseudoInsideRing": True,
            "pillInsetShadow": True, "pseudoInsideOutline": True, "pillKeptShadow": False, "pillAddedShadow": True,
        })

    @unittest.skipUnless(PLAYWRIGHT.is_file(), "Playwright is not installed")
    def test_link_labels_skip_decorative_hidden_glyphs_but_not_hidden_words(self):
        # 0.41.0 r02 lost B06, B08 and B09 to aria-hidden arrows inside correctly named links.
        result = subprocess.run(
            ["node", str(ROOT / "tests/fixtures/trending-topics-build/label-controls.mjs")],
            env={**os.environ, "PLAYWRIGHT_MODULE_PATH": PLAYWRIGHT.as_uri()},
            capture_output=True, text=True, timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        spec = (ROOT / "tests/fixtures/trending-topics-build/acceptance.spec.mjs").read_text()
        # Only labels skip decorative glyphs; body and edition text keep every rendered character.
        self.assertEqual(spec.count("evaluate(renderedText, true)"), 1)
        self.assertEqual(spec.count("evaluate(renderedText)"), 2)
