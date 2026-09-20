"""Full C3/C4 evaluator flows against a real browser and controlled HTTP app."""
import json
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


class KanbanBrowserFlows(unittest.TestCase):
    def test_valid_presentation_and_independent_negative_controls(self):
        if not (ROOT / 'dashboard/node_modules/playwright/index.mjs').exists():
            self.skipTest('dashboard Playwright is not installed')
        completed = subprocess.run(
            ['node', str(ROOT / 'tests/fixtures/kanban-evaluator/browser.mjs')],
            cwd=ROOT, text=True, capture_output=True, timeout=180,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(json.loads(completed.stdout), {
            'c3': 11, 'c4': 12,
            'negativeControls': ['wrong total', 'missing modal focus', 'lost creation fields'],
        })
