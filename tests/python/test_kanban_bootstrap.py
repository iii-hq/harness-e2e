import importlib.util
import io
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
sys.path.insert(0, str(ROOT / 'scripts'))
import check_kanban_catalog  # noqa: E402


class KanbanBootstrapTest(unittest.TestCase):
    def test_runtime_contract_is_pinned(self):
        self.assertEqual(
            bootstrap.IMAGE,
            'mcr.microsoft.com/playwright@sha256:cf0daee9b994042e011bc29f20cdff1a9f682a039b43fcd738f7d8a9d3bcd9d6',
        )
        self.assertEqual(bootstrap.PLAYWRIGHT_MODULE, 'playwright/index.mjs')
        self.assertEqual(bootstrap.FIXTURE_REVISION, 'main')
        tools = ROOT / 'scripts/kanban_eval/tools'
        self.assertTrue((tools / 'package.json').is_file())
        self.assertTrue((tools / 'package-lock.json').is_file())
        package = (tools / 'package.json').read_text()
        self.assertIn('"@pnpm/linux-x64": "10.18.2"', package)
        self.assertIn('"playwright": "1.61.1"', package)

    def test_ci_only_checks_out_and_bootstraps_kanban_groups(self):
        workflow = (ROOT / '.github/workflows/exact-stack-e2e.yml').read_text()
        executor = (ROOT / 'scripts/executor.sh').read_text()
        group = (ROOT / 'scripts/run_exact_stack_group.sh').read_text()
        # Checked out in full, anonymously, only for a Kanban group, at the
        # commit preparation named, which the bootstrap knows as main.
        self.assertIn('checkout iii-hq/kanban-e2e-fixture "${kanban:?preparation named no kanban-e2e-fixture commit}"',
                      executor)
        self.assertNotIn('${kanban:-main}', executor)
        self.assertIn('git -C target/kanban-fixture update-ref refs/heads/main HEAD', executor)
        self.assertNotIn('KANBAN_FIXTURE_REPOSITORY', workflow + executor)
        self.assertIn('scripts/run_in_image.sh prepare fixtures', workflow)
        # The group runs with the executor image's Node.
        self.assertIn('node-v24.18.0-linux-x64', (ROOT / 'Dockerfile').read_text())
        for step in workflow.split('\n      - '):
            if 'uses: actions/checkout@' in step:
                self.assertIn('persist-credentials: false', step)
        self.assertIn('if [[ "$campaign_group_id" == case-kanban-* ]]', group)
        # Its containers are the group's own Docker daemon's: they mount the
        # executor image's node where it is.
        self.assertIn('--node "$(realpath "$(command -v node)")"', group)
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
            # Below the runtime root: a group sees the checkout read-only.
            tools = runtime.resolve() / 'tools'
            commands = []

            def fake_run(*command, **_kwargs):
                commands.append(command)

            original_is_file = Path.is_file
            playwright = tools / 'node_modules/playwright/index.mjs'

            def is_file(path):
                return path == playwright or original_is_file(path)

            with mock.patch.object(bootstrap, 'run', side_effect=fake_run), \
                 mock.patch.object(bootstrap.subprocess, 'check_output', side_effect=[
                     'a' * 40 + '\n', 'a' * 40 + '\n', json.dumps([bootstrap.IMAGE]),
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
            self.assertEqual(commands[1][-2:], ('--prefix', str(tools)))
            for name in ('package.json', 'package-lock.json'):
                self.assertEqual((tools / name).read_bytes(), (ROOT / 'scripts/kanban_eval/tools' / name).read_bytes())
            self.assertEqual(json.loads(output.read_text())['browser-dependencies'], str(tools / 'node_modules'))
            self.assertIn('--store-dir', commands[2])
            self.assertIn('--ignore-scripts', commands[2])
            self.assertFalse(any('--with-deps' in command for command in commands))
            self.assertEqual(check_output.call_args_list[1].args[0],
                             ['git', '-C', str(fixture.resolve()), 'rev-parse', '--verify', 'main^{commit}'])
            self.assertEqual(check_output.call_args_list[2].args[0][0:4],
                             ['docker', 'image', 'inspect', '--format'])
            self.assertEqual(Path(json.loads(output.read_text())['fixture']), fixture.resolve())
            self.assertEqual(json.loads(output.read_text())['browsers'], '/ms-playwright')

    def test_bootstrap_rejects_head_that_differs_from_the_resolved_fixture_revision(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'scenarios').mkdir()
            (root / 'kanban').mkdir()
            (root / 'scenarios/catalog.json').write_bytes((ROOT / 'src/scenarios/kanban/catalog.json').read_bytes())
            (root / 'kanban/pnpm-lock.yaml').write_text('lockfileVersion: 9\n')
            with mock.patch.object(bootstrap.subprocess, 'check_output', side_effect=[
                'a' * 40 + '\n', 'b' * 40 + '\n',
            ]), mock.patch.object(bootstrap, 'run') as run, mock.patch.object(bootstrap.sys, 'argv', [
                'bootstrap.py', '--fixture', str(root), '--iii', sys.executable,
                '--runtime-root', str(root / 'runtime'), '--output', str(root / 'runtime.json'),
                '--node', sys.executable, '--npm', sys.executable,
            ]):
                with self.assertRaisesRegex(ValueError, 'fixture revision mismatch'):
                    bootstrap.main()
                run.assert_not_called()


class KanbanCatalogCheckTest(unittest.TestCase):
    def check(self, catalogs):
        """Runs the check with the fixture answering `catalogs` by commit."""
        heads = {'main': 'c' * 40}

        def get(url, token=None):
            return {'sha': heads.get(url.rsplit('/', 1)[1], url.rsplit('/', 1)[1])}

        def fetch(repository, revision):
            return catalogs[revision]

        with mock.patch.object(check_kanban_catalog.prepare, 'get_json', side_effect=get), \
                mock.patch.object(check_kanban_catalog, 'fetch', side_effect=fetch), \
                mock.patch('sys.stdout', new_callable=io.StringIO) as stdout:
            drifted = check_kanban_catalog.main()
        return drifted, stdout.getvalue()

    def test_the_head_and_the_pin_are_compared_byte_for_byte_and_a_drift_is_only_a_warning(self):
        embedded = (ROOT / 'src/scenarios/kanban/catalog.json').read_bytes()
        pin = check_kanban_catalog.prepare.FIXTURES['kanban-e2e-fixture'][3]
        drifted, output = self.check({'c' * 40: embedded, pin: embedded})
        self.assertEqual(drifted, 0)
        self.assertNotIn('::warning', output)
        # The same JSON, reserialized, is still a drift: the bootstrap
        # compares bytes.
        reserialized = json.dumps(json.loads(embedded), indent=2).encode()
        drifted, output = self.check({'c' * 40: reserialized, pin: embedded})
        self.assertEqual(drifted, 1)
        self.assertIn('::warning file=src/scenarios/kanban/catalog.json,title=Kanban catalog drift::', output)
        self.assertIn(f'iii-hq/kanban-e2e-fixture@{"c" * 40} (the head of main)', output)
        self.assertIn(f'{pin} (the pinned fallback): catalog matches', output)

    def test_a_fixture_it_cannot_read_is_a_warning_too(self):
        with mock.patch.object(check_kanban_catalog.prepare, 'get_json',
                               side_effect=check_kanban_catalog.prepare.ResolutionError('HTTP 503')), \
                mock.patch('sys.stdout', new_callable=io.StringIO) as stdout:
            self.assertEqual(check_kanban_catalog.main(), 0)
        self.assertEqual(stdout.getvalue().count('could not be read: HTTP 503'), 2)


if __name__ == '__main__':
    unittest.main()
