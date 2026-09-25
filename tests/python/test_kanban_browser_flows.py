"""Full C3/C4 evaluator flows against a real browser and controlled HTTP app."""
import json
import os
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]

# CI-only: board_settings switches stores twice (more round trips than any
# other check), so a loaded CI runner can need more than the 8s every other
# check uses. This is unset in production scoring, which always sees 8s.
KANBAN_PROBE_ENV = {**os.environ, 'HARNESS_E2E_KANBAN_PROBE_SLOW_TIMEOUT_MS': '20000'}


class KanbanBrowserFlows(unittest.TestCase):
    def test_valid_presentation_and_independent_negative_controls(self):
        if not (ROOT / 'dashboard/node_modules/playwright/index.mjs').exists():
            self.skipTest('dashboard Playwright is not installed')
        completed = subprocess.run(
            ['node', str(ROOT / 'tests/fixtures/kanban-evaluator/browser.mjs')],
            cwd=ROOT, text=True, capture_output=True, timeout=180, env=KANBAN_PROBE_ENV,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(json.loads(completed.stdout), {
            'c3': 11, 'c4': 12,
            'negativeControls': ['wrong total', 'missing modal focus', 'lost creation fields'],
        })
