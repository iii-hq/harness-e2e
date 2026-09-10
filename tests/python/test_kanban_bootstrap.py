import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('kanban_bootstrap', ROOT / 'scripts/kanban_eval/bootstrap.py')
bootstrap = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bootstrap)


class KanbanBootstrapTest(unittest.TestCase):
    def test_runtime_contract_is_pinned(self):
        self.assertEqual(
            bootstrap.IMAGE,
            'mcr.microsoft.com/playwright@sha256:cf0daee9b994042e011bc29f20cdff1a9f682a039b43fcd738f7d8a9d3bcd9d6',
        )
        self.assertEqual(bootstrap.PLAYWRIGHT_MODULE, 'playwright/index.mjs')
        self.assertEqual(bootstrap.FIXTURE_REVISION, '0471257a95095da7c5e9d366e26636976472e90d')
        tools = ROOT / 'scripts/kanban_eval/tools'
        self.assertTrue((tools / 'package.json').is_file())
        self.assertTrue((tools / 'package-lock.json').is_file())
        package = (tools / 'package.json').read_text()
        self.assertIn('"@pnpm/linux-x64": "10.18.2"', package)
        self.assertIn('"playwright": "1.61.1"', package)

    def test_ci_only_checks_out_and_bootstraps_kanban_groups(self):
        workflow = (ROOT / '.github/workflows/exact-stack-e2e.yml').read_text()
        group = (ROOT / 'scripts/run_exact_stack_group.sh').read_text()
        self.assertIn("startsWith(matrix.group_id, 'case-kanban-')", workflow)
        self.assertIn('repository: iii-hq/kanban-e2e-fixture', workflow)
        self.assertNotIn('KANBAN_FIXTURE_REPOSITORY', workflow)
        self.assertIn('0471257a95095da7c5e9d366e26636976472e90d', workflow)
        self.assertIn('fetch-depth: 0', workflow)
        self.assertIn('node-version: 24.18.0', workflow)
        kanban_checkout = next(
            step for step in workflow.split('\n      - ')
            if 'name: Checkout pinned Kanban fixture' in step
        )
        self.assertNotIn('token:', kanban_checkout)
        for step in workflow.split('\n      - '):
            if 'uses: actions/checkout@' in step:
                self.assertIn('persist-credentials: false', step)
        self.assertIn('if [[ "$campaign_group_id" == case-kanban-* ]]', group)
        self.assertIn('HARNESS_E2E_KANBAN_RUNTIME', group)
        self.assertIn('Kanban fixture checkout is unavailable', group)

    def test_bootstrap_fails_closed_for_missing_fixture(self):
        with self.assertRaises(FileNotFoundError):
            bootstrap.required_directory('/definitely-missing-kanban-fixture', 'fixture')

    def test_bootstrap_pulls_and_inspects_image_before_installing(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = root / 'fixture'
            (fixture / 'scenarios').mkdir(parents=True)
            (fixture / 'kanban/node_modules').mkdir(parents=True)
            (fixture / 'scenarios/catalog.json').write_bytes((ROOT / 'src/scenarios/kanban/catalog.json').read_bytes())
            (fixture / 'kanban/pnpm-lock.yaml').write_text('lockfileVersion: 9\n')
            runtime = root / 'runtime'
            output = root / 'runtime.json'
            tools = ROOT / 'scripts/kanban_eval/tools'
            commands = []

            def fake_run(*command, **_kwargs):
                commands.append(command)

            original_is_file = Path.is_file
            playwright = tools / 'node_modules/playwright/index.mjs'

            def is_file(path):
                return path == playwright or original_is_file(path)

            with mock.patch.object(bootstrap, 'run', side_effect=fake_run), \
                 mock.patch.object(bootstrap.subprocess, 'check_output', side_effect=[
                     bootstrap.FIXTURE_REVISION + '\n', json.dumps([bootstrap.IMAGE]),
                 ]) as check_output, \
                 mock.patch.object(bootstrap, 'required_file', side_effect=lambda path, _label: Path(path)), \
                 mock.patch.object(Path, 'is_file', is_file), \
                 mock.patch.object(bootstrap.sys, 'argv', [
                     'bootstrap.py', '--fixture', str(fixture), '--iii', '/bin/true',
                     '--runtime-root', str(runtime), '--output', str(output),
                     '--node', '/bin/true', '--npm', '/bin/true',
                 ]):
                bootstrap.main()

            self.assertEqual(commands[0], ('docker', 'pull', bootstrap.IMAGE))
            self.assertIn('--store-dir', commands[2])
            self.assertIn('--ignore-scripts', commands[2])
            self.assertFalse(any('--with-deps' in command for command in commands))
            self.assertEqual(check_output.call_args_list[1].args[0][0:4],
                             ['docker', 'image', 'inspect', '--format'])
            self.assertEqual(Path(json.loads(output.read_text())['fixture']), fixture.resolve())
            self.assertEqual(json.loads(output.read_text())['browsers'], '/ms-playwright')


if __name__ == '__main__':
    unittest.main()
