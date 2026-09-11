"""Analytics opt-out at real child boundaries; no III service or provider is started."""
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import run_e2e_campaign as campaign


def load(name, relative):
    spec = importlib.util.spec_from_file_location(name, ROOT / relative)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


linkly = load('telemetry_linkly', 'scripts/linkly_stack.py')
bootstrap = load('telemetry_bootstrap', 'scripts/kanban_eval/bootstrap.py')
kanban = load('telemetry_kanban', 'scripts/kanban_eval/run.py')
registry = load('telemetry_registry', 'tests/fixtures/registry-version-comparison/lifecycle.py')
controller = load('telemetry_controller', 'src/scenarios/swe_service/controller.py')
isolation = load('telemetry_isolation', 'src/scenarios/swe_service/isolation.py')

# Checking both generations catches an environment that is only present in a
# builder object. These children cannot emit III analytics, even in the red case.
CHECK = (
    "import os,subprocess,sys; "
    "assert os.environ.get('III_TELEMETRY_ENABLED')=='false'; "
    "assert subprocess.check_output([sys.executable,'-c',"
    "\"import os; print(os.environ.get('III_TELEMETRY_ENABLED'))\"],text=True).strip()=='false'"
)


class TelemetryBoundaryTests(unittest.TestCase):
    def test_campaign_pins_after_caller_merge_and_preserves_observability(self):
        for setting in (None, '', 'true', 'false'):
            with self.subTest(setting=setting):
                environment = {'OTEL_ENABLED': 'true', 'DEEPSEEK_API_KEY': 'test-sentinel'}
                if setting is not None:
                    environment['III_TELEMETRY_ENABLED'] = setting
                before = dict(environment)
                check = CHECK + "; assert os.environ['OTEL_ENABLED']=='true'; assert os.environ['DEEPSEEK_API_KEY']=='test-sentinel'"
                code, error = campaign._run_process(
                    [sys.executable, '-c', check], environment=environment,
                    run_process=subprocess.run,
                )
                self.assertEqual((code, error), (0, None))
                self.assertEqual(environment, before)

    def test_catalog_probe_is_opted_out_before_execution(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / 'catalog-probe'
            binary.write_text(f'#!{sys.executable}\n' + CHECK + "\nprint('{\"scenarios\":{}}')\n")
            binary.chmod(0o700)
            with patch.dict(os.environ, {'III_TELEMETRY_ENABLED': 'true'}):
                self.assertEqual(campaign.scenario_catalog(binary), {})
            campaign.scenario_catalog.cache_clear()

    def test_standalone_helpers_pin_real_children(self):
        argv = [sys.executable, '-c', CHECK]
        with patch.dict(os.environ, {'III_TELEMETRY_ENABLED': 'true'}):
            linkly.run(argv, env={'III_TELEMETRY_ENABLED': 'true'})
            bootstrap.run(*argv, env={'III_TELEMETRY_ENABLED': 'true'})
            registry.run(argv, env={'III_TELEMETRY_ENABLED': 'true'})
            controller.run(argv)
            with tempfile.TemporaryDirectory() as directory:
                self.assertEqual(kanban.bounded(argv, Path(directory) / 'child.log', 10), 0)

    def test_compose_exec_replaces_process_with_opted_out_environment(self):
        with tempfile.TemporaryDirectory() as directory:
            project = Path(directory)
            (project / 'worker-compose.yaml').write_text('namespace: test\ncontainers: {}\n')
            binary = project / 'iii-stub'
            binary.write_text(f'#!{sys.executable}\n' + CHECK + '\n')
            binary.chmod(0o700)
            result = subprocess.run(
                [sys.executable, str(ROOT / 'scripts/linkly_stack.py'), '--iii', str(binary),
                 'up', '--dir', str(project)], capture_output=True, text=True,
                env={**os.environ, 'III_TELEMETRY_ENABLED': 'true'},
            )
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_kanban_docker_environment_cannot_reenable_analytics(self):
        environment = {'III_TELEMETRY_ENABLED': 'true', 'OTEL_ENABLED': 'true'}
        argv = kanban.container_command('image', 'none', 'candidate', [], environment)
        self.assertEqual([argv[i + 1] for i, arg in enumerate(argv) if arg == '--env'],
                         ['III_TELEMETRY_ENABLED=false', 'OTEL_ENABLED=true'])
        self.assertEqual(environment['III_TELEMETRY_ENABLED'], 'true')
        self.assertIn('III_TELEMETRY_ENABLED=false', kanban.docker_exec('container', ['iii', '--version']))

    def test_swe_clean_environments_include_only_explicit_opt_out(self):
        self.assertEqual(isolation.ENV['III_TELEMETRY_ENABLED'], 'false')
        self.assertNotIn('OTEL_ENABLED', isolation.ENV)
        self.assertNotIn('DEEPSEEK_API_KEY', isolation.ENV)
        bwrap = isolation._bwrap_command('bwrap', Path('/workspace'), Path('/probes.py'))
        index = bwrap.index('III_TELEMETRY_ENABLED')
        self.assertEqual(bwrap[index - 1:index + 2], ['--setenv', 'III_TELEMETRY_ENABLED', 'false'])
        docker = isolation._docker_command('docker', 'image', Path('/workspace'), Path('/probes.py'), 'test')
        self.assertIn('III_TELEMETRY_ENABLED=false', docker)

    def test_linkly_pins_each_active_worker_and_preserves_environment(self):
        source = ['containers:', '  provider-deepseek:', '    worker: package://provider-deepseek',
                  "    env_file: ['./.env']", '    environment:',
                  '      III_TELEMETRY_ENABLED: "true"', '      OTEL_ENABLED: "true"',
                  '  harness:', '    worker: package://harness']
        result = linkly.disable_analytics(source)
        self.assertEqual(result.count('      III_TELEMETRY_ENABLED: "false"'), 2)
        self.assertIn("    env_file: ['./.env']", result)
        self.assertIn('      OTEL_ENABLED: "true"', result)
        self.assertNotIn('      III_TELEMETRY_ENABLED: "true"', result)
        self.assertEqual(linkly.disable_analytics(result), result)

    def test_registry_nested_compose_pins_resolved_service_environments(self):
        resolved = {'services': {'api': {'environment': {'III_TELEMETRY_ENABLED': 'true', 'OTEL_ENABLED': 'true'}},
                                 'web': {'image': 'image'}}}
        with tempfile.TemporaryDirectory() as directory:
            fixture = Path(directory)
            with patch.object(registry, 'container_exec', return_value=json.dumps(resolved).encode()) as execute:
                registry.disable_fixture_compose_analytics(fixture, {'container': 'private-daemon'})
            execute.assert_called_once_with({'container': 'private-daemon'},
                                           'docker compose -f /fixture/compose.yaml config --format json')
            actual = json.loads((fixture / 'compose.yaml').read_text())
            for service in actual['services'].values():
                self.assertEqual(service['environment']['III_TELEMETRY_ENABLED'], 'false')
            self.assertEqual(actual['services']['api']['environment']['OTEL_ENABLED'], 'true')

    def test_registry_build_install_and_version_probes_are_opted_out(self):
        with tempfile.TemporaryDirectory() as directory:
            dockerfile = Path(directory) / 'Dockerfile'
            dockerfile.write_text('FROM base AS tool\nRUN install-iii && iii --version\nFROM base\nRUN iii --version\n')
            registry.disable_fixture_build_analytics(dockerfile)
            lines = dockerfile.read_text().splitlines()
            for i, line in enumerate(lines):
                if line.startswith('RUN '):
                    self.assertEqual(lines[i - 1], 'ENV III_TELEMETRY_ENABLED=false')
            self.assertEqual(lines[-1], 'ENV III_TELEMETRY_ENABLED=false')

    def test_shell_entrypoints_export_before_any_install_or_version_probe(self):
        for relative in ('scripts/run_exact_stack_group.sh', 'scripts/run_exact_stack_fault.sh',
                         'scripts/install_bwrap.sh', 'supervisor/install.sh', 'supervisor/run-weekly-stress'):
            with self.subTest(path=relative):
                prefix = '\n'.join((ROOT / relative).read_text().splitlines()[:3])
                self.assertIn('export III_TELEMETRY_ENABLED=false', prefix)
                result = subprocess.run(['bash', '-c', prefix + '\nexec "$1" -c "$2"',
                                         'probe', sys.executable, CHECK],
                                        env={**os.environ, 'III_TELEMETRY_ENABLED': 'true'})
                self.assertEqual(result.returncode, 0)

    def test_every_workflow_including_reusable_has_its_own_opt_out(self):
        workflows = list((ROOT / '.github/workflows').glob('*.yml'))
        self.assertTrue(workflows)
        for path in workflows:
            self.assertIn('\nenv:\n  III_TELEMETRY_ENABLED: "false"\n', path.read_text(), path.name)

    def test_provider_secret_writer_keeps_exact_allowlist_and_private_files(self):
        source = (ROOT / 'scripts/run_exact_stack_group.sh').read_text()
        writer = re.search(r'^write_provider_secret\(\) \{.*?^\}', source, re.M | re.S).group()
        calls = re.findall(r'^write_provider_secret (\S+) (\S+)$', source, re.M)
        self.assertEqual(calls, [('provider-deepseek', 'DEEPSEEK_API_KEY'),
                                 ('provider-zai', 'ZAI_API_KEY')])
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            contract = root / 'contract.json'
            contract.write_text(json.dumps({'orchestration': {'roots': [
                {'worker': worker} for worker, _ in calls
            ]}}))
            secrets = root / 'secrets'
            secrets.mkdir()
            shell = ('set -euo pipefail\ncontract_path=$1\nsecrets_dir=$2\n'
                     + writer + '\n' + '\n'.join('write_provider_secret ' + ' '.join(call) for call in calls))
            environment = {'PATH': os.environ['PATH'], 'III_TELEMETRY_ENABLED': 'false',
                           'DEEPSEEK_API_KEY': 'deepseek-sentinel', 'ZAI_API_KEY': 'zai-sentinel',
                           'UNRELATED_SECRET': 'must-not-be-forwarded'}
            subprocess.run(['bash', '-c', shell, 'provider-test', str(contract), str(secrets)],
                           env=environment, check=True, capture_output=True)
            self.assertEqual({file.name for file in secrets.iterdir()},
                             {'provider-deepseek.env', 'provider-zai.env'})
            for worker, variable in calls:
                file = secrets / f'{worker}.env'
                self.assertEqual(file.read_text(), f'{variable}={environment[variable]}\n')
                self.assertEqual(file.stat().st_mode & 0o777, 0o600)


if __name__ == '__main__':
    unittest.main()
