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
        for container, keeper, command in [
                ('short', '7', 'pwd'), (CONTAINER, '1', 'pwd'),
                (CONTAINER, '7', 'x' * 65537)]:
            with self.assertRaises(ValueError):
                EXEC.execute(container, keeper, command)

    def test_real_output_bound_and_nonzero_command_feedback(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            docker = root / 'docker'
            docker.write_text('''#!/bin/sh
if [ "$2" = --env ]; then
  [ "$3" = III_TELEMETRY_ENABLED=false ] || exit 99
  shift 3
  set -- exec "$@"
fi
if [ "$1" = rm ]; then touch "$REMOVED"; exit; fi
if [ "$1" = exec ] && [ "$3" = /usr/bin/python3 ]; then
  touch "$CLEANED"
  [ -z "$CLEANUP_FAIL" ]
  exit
fi
if [ "$7" = overflow ]; then head -c 262145 /dev/zero
elif [ "$7" = timeout ]; then while :; do :; done
else echo test-failed; exit 1
fi
''')
            docker.chmod(0o755)
            removed = root / 'removed'
            cleaned = root / 'cleaned'
            environment = {'PATH': f'{root}:{os.environ["PATH"]}',
                           'REMOVED': str(removed), 'CLEANED': str(cleaned),
                           'CLEANUP_FAIL': ''}
            with patch.dict(os.environ, environment):
                result = EXEC.execute(CONTAINER, '7', 'false')
                self.assertEqual(result['exit_code'], 1)
                self.assertIn('test-failed', result['stdout'])
                self.assertFalse(removed.exists())
                result = EXEC.execute(CONTAINER, '7', 'overflow')
                self.assertEqual(result['exit_code'], 125)
                self.assertIn('output exceeded', result['stderr'])
                self.assertTrue(cleaned.exists())
                cleaned.unlink()
                with patch.object(EXEC, 'COMMAND_TIMEOUT', .01):
                    result = EXEC.execute(CONTAINER, '7', 'timeout')
                self.assertEqual(result['exit_code'], 124)
                self.assertIn('timed out after 120 seconds', result['stderr'])
                self.assertTrue(cleaned.exists())
                cleaned.unlink()
                with patch.dict(os.environ, {'CLEANUP_FAIL': '1'}), \
                        self.assertRaisesRegex(RuntimeError, 'candidate cleanup failed'):
                    EXEC.execute(CONTAINER, '7', 'overflow')
                self.assertTrue(removed.exists())
