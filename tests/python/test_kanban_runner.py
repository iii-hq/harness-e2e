import importlib.util
import json
from pathlib import Path
import subprocess
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

    def test_pinned_image_owns_system_libraries_and_browsers(self):
        args = types.SimpleNamespace(node=Path('/inputs/node'), iii=Path('/inputs/iii'),
                                     pnpm=Path('/inputs/pnpm'))
        mounts = runner.runtime_mounts(args)
        self.assertEqual({target for _source, target, _writable in mounts},
                         {'/runtime/node', '/runtime/iii', '/runtime/pnpm'})
        self.assertFalse(any(target in ('/usr', '/bin', '/lib', '/lib64', '/browsers',
                                        '/etc/fonts', '/etc/ld.so.cache') for _source, target, _writable in mounts))

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

    def test_external_subject_publishes_ready_and_observes_lifecycle_markers(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            ready = {'candidate': 'candidate-id', 'evaluator': 'evaluator-id',
                     'prompt': 'Build it', 'snapshot_git_head': 'abc123'}

            def complete(_seconds):
                (output / 'subject-complete').write_text('{"model_invoked":true}')

            with mock.patch.object(runner.time, 'sleep', side_effect=complete):
                complete = runner.wait_for_external_subject(output, ready, timeout=1)
            self.assertEqual(json.loads((output / 'ready.json').read_text()), ready)
            self.assertEqual(complete, {'model_invoked': True})

            (output / 'cancel').touch()
            with self.assertRaisesRegex(KeyboardInterrupt, 'cancelled'):
                runner.wait_for_external_subject(output, ready, timeout=1)

            (output / 'cancel').unlink()
            (output / 'subject-complete').write_text('{"model_invoked":"yes"}')
            with self.assertRaisesRegex(runner.EvaluationError, 'completion marker is invalid'):
                runner.wait_for_external_subject(output, ready, timeout=1)

    def test_external_subject_arguments_are_base_only_and_mutually_exclusive(self):
        required = ['--fixture', '/unused', '--catalog', '/unused', '--case', 'C2',
                    '--output', '/unused', '--image', 'unused', '--node', '/unused',
                    '--iii', '/unused', '--pnpm', '/unused', '--dependencies', '/unused',
                    '--browser-dependencies', '/unused', '--browsers', '/unused']
        invalid = [
            [*required, '--revision', 'base', '--external-subject',
             '--subject-model', 'deepseek-v4-flash'],
            [*required, '--revision', 'reference', '--external-subject'],
            [*required[:-1], 'ms-playwright', '--revision', 'base'],
        ]
        for argv in invalid:
            with self.subTest(argv=argv), mock.patch.object(runner.sys, 'argv', ['run.py', *argv]):
                with self.assertRaises(SystemExit):
                    runner.main()

    def test_bounded_command_stops_when_external_cancel_appears(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cancel = root / 'cancel'
            cancel.touch()
            with self.assertRaisesRegex(KeyboardInterrupt, 'cancelled'):
                runner.bounded(['/bin/sleep', '5'], root / 'log', 5, cancel=cancel)

    def test_bounded_command_polls_private_control(self):
        with tempfile.TemporaryDirectory() as directory:
            polls = []
            code = runner.bounded(['/bin/sleep', '.05'], Path(directory) / 'log', 1,
                                  poll=lambda: polls.append(True))
            self.assertEqual(code, 0)
            self.assertTrue(polls)

    def test_private_control_rejects_unknown_operations_and_processes_each_id_once(self):
        with tempfile.TemporaryDirectory() as directory:
            evidence = Path(directory)
            poll = runner.control_callback(evidence, 'candidate', 'evaluator', [mock.Mock(), 0])
            runner.atomic_json(evidence / 'control-request.json',
                               {'id': 'unknown', 'operation': 'shell', 'payload': {'command': 'id'}})
            poll()
            response = json.loads((evidence / 'control-response.json').read_text())
            self.assertEqual(response['id'], 'unknown')
            self.assertFalse(response['ok'])
            self.assertIn('unsupported control operation', response['error'])

            completed = subprocess.CompletedProcess([], 0, stdout=b'[]', stderr=b'')
            with mock.patch.object(runner.subprocess, 'run', return_value=completed) as run:
                runner.atomic_json(evidence / 'control-request.json',
                                   {'id': 'read-once', 'operation': 'read_store', 'payload': {}})
                poll()
                poll()
            run.assert_called_once()
            response = json.loads((evidence / 'control-response.json').read_text())
            self.assertEqual(response, {'id': 'read-once', 'ok': True, 'value': '[]'})

            with mock.patch.object(runner, 'restart_runtime', return_value={'ready': True}) as restart:
                runner.atomic_json(evidence / 'control-request.json', {
                    'id': 'own-defaults', 'operation': 'restart',
                    'payload': {'register_configuration': False, 'reset_configuration': True}})
                poll()
            self.assertEqual(restart.call_args.kwargs,
                             {'register_configuration': False, 'reset_configuration': True})

            with mock.patch.object(runner, 'inspect_runtime',
                                   return_value={'websocket_url': 'ws://127.0.0.1:9229/id'}):
                runner.atomic_json(evidence / 'control-request.json',
                                   {'id': 'inspect', 'operation': 'inspect_runtime', 'payload': {}})
                poll()
            response = json.loads((evidence / 'control-response.json').read_text())
            self.assertEqual(response['value'], {'websocket_url': 'ws://127.0.0.1:9229/id'})

            with mock.patch.object(runner, 'inspect_runtime',
                                   side_effect=runner.InfrastructureError('invalid inspector')):
                runner.atomic_json(evidence / 'control-request.json',
                                   {'id': 'inspect-failed', 'operation': 'inspect_runtime', 'payload': {}})
                with self.assertRaisesRegex(runner.InfrastructureError, 'invalid inspector'):
                    poll()

    def test_runtime_can_start_without_grader_configuration_registration(self):
        with tempfile.TemporaryDirectory() as directory:
            process = mock.Mock()
            with mock.patch.object(runner.subprocess, 'Popen', return_value=process) as popen:
                self.assertIs(runner.start_runtime('candidate', Path(directory) / 'runtime.log', False), process)
            command = popen.call_args.args[0]
            self.assertIn(runner.RUNTIME_ENGINE, command[-1])
            self.assertIn(runner.RUNTIME_COMPOSE, command[-1])
            self.assertNotIn('configuration::register', command[-1])

    def test_hot_reload_requires_observed_marker_and_runs_restoration(self):
        with tempfile.TemporaryDirectory() as directory:
            evidence = Path(directory)
            runtime_log = evidence / 'runtime.log'
            runtime_log.write_bytes(b'initial\n')
            marker = 'KANBAN_HOT_RELOAD_' + ('01' * 16)
            calls = []

            def docker(command, **_kwargs):
                calls.append(command)
                if marker in command:
                    with runtime_log.open('ab') as log:
                        log.write((marker + '\n').encode())
                    return subprocess.CompletedProcess(command, 0, stdout=b'{"count":1}', stderr=b'')
                return subprocess.CompletedProcess(command, 0, stdout=b'', stderr=b'')

            process = mock.Mock()
            process.poll.return_value = None
            with mock.patch.object(runner.os, 'urandom', return_value=b'\x01' * 16), \
                    mock.patch.object(runner.subprocess, 'run', side_effect=docker), \
                    mock.patch.object(runner, 'wait_runtime_ready', return_value=True):
                result = runner.hot_reload('candidate', 'evaluator', evidence, [process, 0])
            self.assertEqual(result, {'observed': True, 'source_restored': True})
            self.assertEqual(len(calls), 2)
            self.assertTrue(any('shutil.rmtree' in part for part in calls[1]))

            calls.clear()
            with mock.patch.object(runner.os, 'urandom', return_value=b'\x01' * 16), \
                    mock.patch.object(runner.subprocess, 'run', side_effect=docker), \
                    mock.patch.object(runner.time, 'monotonic', side_effect=[0, 56]), \
                    mock.patch.object(runner, 'wait_runtime_ready', return_value=True):
                result = runner.hot_reload('candidate', 'evaluator', evidence, [process, 0])
            self.assertEqual(result, {'observed': False, 'source_restored': True})

    def test_hot_reload_instruments_non_index_sources_and_excludes_generated_or_linked_trees(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / 'kanban'
            source = root / 'src/main.mts'
            generated = root / 'dist/generated.ts'
            dependency = root / 'node_modules/dependency.ts'
            source.parent.mkdir(parents=True)
            generated.parent.mkdir()
            dependency.parent.mkdir()
            source.write_text('export const app = true\n')
            generated.write_text('generated\n')
            dependency.write_text('dependency\n')
            outside = Path(directory) / 'outside'
            outside.mkdir()
            (outside / 'linked.ts').write_text('linked\n')
            (root / 'linked').symlink_to(outside, target_is_directory=True)
            backup = Path(directory) / 'backup'

            subprocess.run(['/usr/bin/python3', '-I', '-c', runner.HOT_RELOAD_PREPARE,
                            str(root), str(backup), 'marker'], check=True, stdout=subprocess.PIPE)
            self.assertIn('marker', source.read_text())
            self.assertEqual(generated.read_text(), 'generated\n')
            self.assertEqual(dependency.read_text(), 'dependency\n')
            self.assertEqual((outside / 'linked.ts').read_text(), 'linked\n')
            subprocess.run(['/usr/bin/python3', '-I', '-c', runner.HOT_RELOAD_RESTORE,
                            str(root), str(backup)], check=True)
            self.assertEqual(source.read_text(), 'export const app = true\n')

    def test_runtime_inspection_returns_only_loopback_node_debugger(self):
        signal_result = subprocess.CompletedProcess([], 0, stdout=b'', stderr=b'')
        targets = subprocess.CompletedProcess([], 0, stdout=json.dumps([{
            'webSocketDebuggerUrl': 'ws://127.0.0.1:9229/node-target'}]).encode(), stderr=b'')
        with mock.patch.object(runner.subprocess, 'run', side_effect=[signal_result, targets]) as run:
            result = runner.inspect_runtime('candidate', 'evaluator')
        self.assertEqual(result, {'websocket_url': 'ws://127.0.0.1:9229/node-target'})
        self.assertIn("int(fields[1].split(':')[1],16)==3000", run.call_args_list[0].args[0][-1])

        invalid = subprocess.CompletedProcess([], 0, stdout=json.dumps([{
            'webSocketDebuggerUrl': 'ws://example.com:9229/node-target'}]).encode(), stderr=b'')
        with mock.patch.object(runner.subprocess, 'run', side_effect=[signal_result, invalid]):
            with self.assertRaisesRegex(runner.InfrastructureError, 'invalid debugger target'):
                runner.inspect_runtime('candidate', 'evaluator')

    def test_container_cleanup_is_bounded_and_best_effort(self):
        timeout = subprocess.TimeoutExpired(['docker'], 15)
        with mock.patch.object(runner.subprocess, 'run', side_effect=timeout) as run:
            runner.remove_container('candidate-id')
        self.assertEqual(run.call_args.kwargs['timeout'], 15)

    def test_change_capture_includes_source_changes_but_not_ignored_runtime_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repo = root / 'repo'
            repo.mkdir()
            subprocess.run(['git', 'init', '--quiet', str(repo)], check=True)
            subprocess.run(['git', '-C', str(repo), 'config', 'user.name', 'Test'], check=True)
            subprocess.run(['git', '-C', str(repo), 'config', 'user.email', 'test@example.invalid'], check=True)
            (repo / '.gitignore').write_text('kanban/node_modules\nkanban/dist/\ndata/\nignored-source\n')
            (repo / 'edited').write_text('before')
            (repo / 'deleted').write_text('before')
            (repo / 'kanban/dist').mkdir(parents=True)
            (repo / 'kanban/dist/tracked.js').write_text('before')
            (repo / 'data').mkdir()
            (repo / 'data/tracked.json').write_text('before')
            subprocess.run(['git', '-C', str(repo), 'add', '.'], check=True)
            subprocess.run(['git', '-C', str(repo), 'add', '--force',
                            'kanban/dist/tracked.js', 'data/tracked.json'], check=True)
            subprocess.run(['git', '-C', str(repo), 'commit', '--quiet', '-m', 'baseline'], check=True)

            (repo / 'edited').write_text('after')
            (repo / 'deleted').unlink()
            (repo / 'created').write_text('after')
            (repo / 'ignored-source').write_text('after')
            (repo / 'kanban/dist/output.js').write_text('build output')
            (repo / 'kanban/dist/tracked.js').write_text('after')
            (repo / 'data/tickets.json').write_text('{}')
            (repo / 'data/tracked.json').write_text('after')
            dependencies = root / 'dependencies'
            dependencies.mkdir()
            (repo / 'kanban/node_modules').symlink_to(dependencies, target_is_directory=True)

            subprocess.run(runner.stage_changes_command(repo), check=True)
            staged = subprocess.check_output(
                ['git', '-C', str(repo), 'diff', '--cached', '--name-status', 'HEAD'], text=True)
            self.assertEqual(set(staged.splitlines()), {
                'A\tcreated', 'D\tdeleted', 'M\tedited', 'A\tignored-source'})

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

    def test_root_host_is_rejected_before_snapshot_or_docker(self):
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
            snapshot = types.SimpleNamespace(prepare=mock.Mock())
            with mock.patch.object(runner.sys, 'argv', argv), mock.patch.object(runner.os, 'getuid', return_value=0), \
                    mock.patch.dict(runner.sys.modules, {'snapshot': snapshot}):
                self.assertEqual(runner.main(), 2)
            snapshot.prepare.assert_not_called()
            result = json.loads((output / 'evidence/result.json').read_text())
            self.assertEqual(result['status'], 'infrastructure_failed')
            self.assertIn('non-root', result['error'])
