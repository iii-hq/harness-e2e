import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location('kanban_exec', Path(__file__).resolve().parents[2] / 'scripts/kanban_eval/exec.py')
EXEC = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(EXEC)
CONTAINER = 'a' * 64


class KanbanExecTest(unittest.TestCase):
    def test_rejects_untrusted_container_and_oversized_commands(self):
        for container, command in [('short', 'pwd'), (CONTAINER, 'x' * 65537)]:
            with self.assertRaises(ValueError):
                EXEC.execute(container, command)

    def test_real_output_bound_and_nonzero_command_feedback(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            docker = root / 'docker'
            docker.write_text('#!/bin/sh\nif [ "$1" = rm ]; then touch "$REMOVED"; exit; fi\nif [ "$7" = overflow ]; then head -c 262145 /dev/zero; else echo test-failed; exit 1; fi\n')
            docker.chmod(0o755)
            removed = root / 'removed'
            with patch.dict(os.environ, {'PATH': f'{root}:{os.environ["PATH"]}', 'REMOVED': str(removed)}):
                result = EXEC.execute(CONTAINER, 'false')
                self.assertEqual(result['exit_code'], 1)
                self.assertIn('test-failed', result['stdout'])
                self.assertFalse(removed.exists())
                with self.assertRaisesRegex(RuntimeError, 'output exceeded'):
                    EXEC.execute(CONTAINER, 'overflow')
                self.assertTrue(removed.exists())
