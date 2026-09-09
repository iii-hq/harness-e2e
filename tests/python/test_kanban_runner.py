import importlib.util
from pathlib import Path
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location(
    'kanban_runner', Path(__file__).resolve().parents[2] / 'scripts/kanban_eval/run.py')
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class KanbanRunnerTest(unittest.TestCase):
    def test_runtime_digest_changes_with_contents(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / 'runtime'
            binary.write_text('first')
            initial = runner.tree_digest(root)
            binary.write_text('other')
            self.assertNotEqual(initial, runner.tree_digest(root))

    def test_candidate_mounts_exclude_trusted_state(self):
        command = runner.candidate_command(['true'])
        for private in ('/trusted', '/evidence', '/browser-deps', '/browsers'):
            self.assertNotIn(private, command)
        self.assertIn('--unshare-all', command)
        self.assertIn('--share-net', command)
        self.assertIn('--clearenv', command)
        self.assertEqual(command[-1], 'true')

    def test_command_timeout_is_not_a_success(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(runner.subprocess.TimeoutExpired):
                runner.bounded(['/bin/sleep', '5'], Path(directory) / 'log', .01)
