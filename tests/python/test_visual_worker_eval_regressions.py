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
            self.fail("dashboard Playwright must be installed for evaluator controls")
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
        self.assertEqual(result["controls"], {"outboundFakeResults": 100, "inboundFakeRequests": 100})
        self.assertEqual(result["rpc"]["invalid"], 100)
        self.assertEqual(result["rpc"]["sequence"], 800)
        self.assertEqual(
            [packet["invocation_id"] for packet in result["rpc"]["requests"]],
            [value for replay in range(100) for value in (f"before-{replay}", f"domain-{replay}", f"canvas-{replay}", f"after-{replay}")],
        )
        self.assertEqual(
            [packet["invocation_id"] for packet in result["rpc"]["results"]],
            [value for replay in range(100) for value in (f"before-{replay}", f"domain-{replay}", f"canvas-{replay}", f"after-{replay}")],
        )
        packets = sorted(result["rpc"]["requests"] + result["rpc"]["results"], key=lambda packet: packet["sequence"])
        self.assertEqual([packet["sequence"] for packet in packets], list(range(1, 801)))
        self.assertEqual(
            [packet["invocation_id"] for packet in packets],
            [value for replay in range(100) for value in (
                f"before-{replay}", f"before-{replay}", f"domain-{replay}", f"domain-{replay}",
                f"canvas-{replay}", f"canvas-{replay}", f"after-{replay}", f"after-{replay}",
            )],
        )


if __name__ == "__main__":
    unittest.main()
