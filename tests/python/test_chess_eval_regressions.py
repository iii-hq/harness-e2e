import json
import os
import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "tests" / "fixtures" / "chess-eval" / "negative-control.mjs"
PLAYWRIGHT = Path(os.environ.get(
    "PLAYWRIGHT_MODULE_PATH",
    ROOT / "dashboard" / "node_modules" / "playwright" / "index.mjs",
))


class ChessEvalRegressionsTest(unittest.TestCase):
    def test_chess_eval_requires_visible_full_fen_board(self):
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
        self.assertFalse(result["invisible"]["passed"])
        self.assertTrue(result["invisible"]["pieces_match"])
        self.assertEqual(result["invisible"]["piece_count"], 32)
        self.assertTrue(result["visible"]["passed"])
        self.assertTrue(result["variedPieceStyle"]["passed"])
        self.assertFalse(result["clipped"]["passed"])
        self.assertFalse(result["identicalBackgrounds"]["passed"])
        self.assertTrue(result["mixedGradients"]["passed"])
        self.assertFalse(result["sameColorGradients"]["passed"])
        self.assertFalse(result["transparentGradients"]["passed"])
        self.assertFalse(result["brokenBackgrounds"]["passed"])
        self.assertTrue(result["svgPieces"]["passed"])
        self.assertFalse(result["brokenImages"]["passed"])
        self.assertTrue(result["visibleImages"]["passed"])
        self.assertFalse(result["transparentImages"]["passed"])


if __name__ == "__main__":
    unittest.main()
