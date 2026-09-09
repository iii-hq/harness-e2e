import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location(
    'kanban_runner', Path(__file__).resolve().parents[2] / 'scripts/kanban_eval/run.py')
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class KanbanRunnerTest(unittest.TestCase):
    def test_probe_requires_complete_consistent_evidence_and_normal_exit(self):
        with tempfile.TemporaryDirectory() as directory:
            evidence = Path(directory)
            result = {'schema': 'kanban-evaluation/v1', 'case_id': 'test',
                      'status': 'passed', 'functional_status': 'passed',
                      'checks': [{'id': 'criterion_1', 'status': 'passed', 'detail': 'observed'}]}
            coverage = {'schema': 'kanban-evaluation-coverage/v1', 'case_id': 'test',
                        'complete': True, 'criteria': result['checks']}
            (evidence / 'result.json').write_text(json.dumps(result))
            with self.assertRaisesRegex(RuntimeError, 'incomplete evidence'):
                runner.probe_result(evidence, 'test', 0)
            (evidence / 'coverage.json').write_text(json.dumps(coverage))
            for code in (1, 2, -9):
                with self.subTest(returncode=code), self.assertRaises(RuntimeError):
                    runner.probe_result(evidence, 'test', code)
            self.assertEqual(runner.probe_result(evidence, 'test', 0), result)
            with self.assertRaisesRegex(RuntimeError, 'inconsistent coverage'):
                runner.probe_result(evidence, 'another-case', 0)
            result['checks'][0]['status'] = 'failed'
            (evidence / 'result.json').write_text(json.dumps(result))
            (evidence / 'coverage.json').write_text(json.dumps(coverage))
            with self.assertRaisesRegex(RuntimeError, 'inconsistent verdict'):
                runner.probe_result(evidence, 'test', 0)
            result.update(status='failed', functional_status='failed')
            (evidence / 'result.json').write_text(json.dumps(result))
            self.assertEqual(runner.probe_result(evidence, 'test', 0)['status'], 'failed')

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
