import json
import os
import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "tests" / "fixtures" / "visual-worker-eval" / "regressions.mjs"
PLAYWRIGHT = Path(os.environ.get(
    "PLAYWRIGHT_MODULE_PATH",
    ROOT / "dashboard" / "node_modules" / "playwright" / "index.mjs",
))


class VisualWorkerEvalRegressionsTest(unittest.TestCase):
    def test_theme_and_canvas_checks_measure_stable_painted_output(self):
        if not PLAYWRIGHT.exists():
            self.skipTest("dashboard Playwright is not installed")
        env = {**os.environ, "PLAYWRIGHT_MODULE_PATH": PLAYWRIGHT.as_uri()}
        completed = subprocess.run(
            ["node", str(FIXTURE)],
            cwd=ROOT,
            env=env,
            text=True,
            capture_output=True,
            timeout=20,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        result = json.loads(completed.stdout)
        self.assertFalse(result["oldTheme"]["passed"])
        self.assertTrue(result["stableTheme"]["passed"])
        self.assertFalse(result["hiddenLabel"]["rendered_graph"])
        self.assertFalse(result["coveredLabel"]["rendered_graph"])
        self.assertFalse(result["sameColorLabel"]["rendered_graph"])
        self.assertFalse(result["transparentAncestorLabel"]["rendered_graph"])
        self.assertTrue(result["visibleLabel"]["label_painted"])
        self.assertEqual(result["rpc"]["invalid"], 1)
        self.assertEqual(result["rpc"]["requests"][0]["invocation_id"], "one")
        self.assertEqual(result["rpc"]["results"][0]["invocation_id"], "one")


if __name__ == "__main__":
    unittest.main()
