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
        })
