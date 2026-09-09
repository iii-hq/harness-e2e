import importlib.util
import json
from pathlib import Path
import tempfile
import types
import unittest
from unittest import mock


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

    def test_candidate_container_policy_is_bounded_and_offline(self):
        command = runner.container_command('sha256:image', 'none', 'candidate', [
            ('/host/workspace', '/workspace', True),
            ('/host/dependencies', '/workspace/kanban/node_modules', False),
        ])
        for item in ('--pull', 'never', '--read-only', '--cap-drop', 'ALL',
                     'no-new-privileges=true', '--pids-limit', '256', '--memory',
                     '2g', '--cpus', '2', 'kanban-eval.role=candidate'):
            self.assertIn(item, command)
        self.assertEqual(command[command.index('--network') + 1], 'none')
        self.assertIn('type=bind,src=/host/workspace,dst=/workspace', command)
        self.assertIn('type=bind,src=/host/dependencies,dst=/workspace/kanban/node_modules,readonly', command)
        for private in ('/trusted', '/evidence', '/browser-deps', '/browsers', '/var/run/docker.sock'):
            self.assertNotIn(private, command)

    def test_evaluator_shares_only_candidate_network(self):
        command = runner.container_command('sha256:image', 'container:candidate-id', 'evaluator', [
            ('/trusted', '/trusted', False), ('/evidence', '/evidence', True),
        ])
        self.assertEqual(command[command.index('--network') + 1], 'container:candidate-id')
        self.assertIn('kanban-eval.role=evaluator', command)
        self.assertNotIn('/workspace', command)

    def test_model_workspace_has_explicit_tmpfs_bounds(self):
        command = runner.container_command('sha256:image', 'none', 'candidate', [], bounded_workspace=True)
        for path, size in (('/workspace', '256m'), ('/data', '64m'), ('/runtime-state', '64m')):
            self.assertTrue(any(value.startswith(f'{path}:rw,nosuid,nodev,size={size},uid=') for value in command))
        self.assertNotIn('--mount', command)

    def test_command_timeout_is_not_a_success(self):
        with tempfile.TemporaryDirectory() as directory:
            with mock.patch.object(runner, 'remove_container') as remove:
                with self.assertRaises(runner.subprocess.TimeoutExpired):
                    runner.bounded(['/bin/sleep', '5'], Path(directory) / 'log', .01, 'owned-id')
            remove.assert_called_once_with('owned-id')

    def test_command_log_cannot_exceed_host_file_limit(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / 'log'
            code = runner.bounded(['/usr/bin/python3', '-c',
                                   "import os; os.write(1,b'x'*(16*1024**2)); os.write(1,b'x')"], log, 10)
            self.assertNotEqual(code, 0)
            self.assertEqual(log.stat().st_size, 16 * 1024 ** 2)

    def test_interruption_writes_infrastructure_verdict(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ('fixture', 'dependencies', 'browser-dependencies', 'browsers'):
                (root / name).mkdir()
            for name in ('catalog', 'node', 'iii', 'pnpm'):
                (root / name).write_text('input')
            output = root / 'output'
            argv = ['run.py', '--fixture', str(root / 'fixture'), '--catalog', str(root / 'catalog'),
                    '--case', 'test', '--revision', 'base', '--output', str(output),
                    '--image', 'sha256:image', '--node', str(root / 'node'), '--iii', str(root / 'iii'),
                    '--pnpm', str(root / 'pnpm'), '--dependencies', str(root / 'dependencies'),
                    '--browser-dependencies', str(root / 'browser-dependencies'), '--browsers', str(root / 'browsers')]
            snapshot = types.SimpleNamespace(prepare=mock.Mock(side_effect=KeyboardInterrupt))
            with mock.patch.object(runner.sys, 'argv', argv), mock.patch.dict(runner.sys.modules, {'snapshot': snapshot}):
                self.assertEqual(runner.main(), 2)
            result = json.loads((output / 'evidence/result.json').read_text())
            self.assertEqual(result['status'], 'infrastructure_failed')
            self.assertEqual(result['error'], 'KeyboardInterrupt')
