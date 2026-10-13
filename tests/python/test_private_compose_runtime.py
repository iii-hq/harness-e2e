"""Exercise the production launcher without downloading or running the iii CLI."""
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest

import yaml

from test_exact_stack_campaign import MODULE, ROOT, campaign_contract, catalog, lock_of


# Only this fake executable ever crosses the launcher engine/Compose boundary.
FAKE_III = r'''#!/usr/bin/env python3
import hashlib, json, os, pathlib, subprocess, sys, time
import yaml
args = sys.argv[1:]
def record(kind, **details):
    with open(os.environ['FAKE_CALLS'], 'a') as log:
        log.write(json.dumps({'kind': kind, 'cwd': os.getcwd(), **details}) + '\n')
def answer(value):
    print(json.dumps(value))
def option(name):
    return args[args.index(name) + 1]
def manifest_path():
    for arg in args:
        if arg.startswith('file='):
            return pathlib.Path(arg[5:])
    return pathlib.Path(json.loads(option('--json'))['file'])
if args == ['--version']:
    print('iii ' + os.environ['FAKE_VERSION'])
    sys.exit(0)
if args[0] == 'project':
    project = pathlib.Path(option('--directory'))
    shutil_source = pathlib.Path(os.environ['FAKE_TEMPLATE'])
    import shutil
    shutil.copytree(shutil_source, project, dirs_exist_ok=True)
    record('template', project=str(project))
    sys.exit(0)
if args[0] != 'trigger':
    record('daemon', args=args, state=os.environ.get('III_COMPOSE_STATE_DIR'))
    while True:
        time.sleep(1)
function = args[1]
if function == 'engine::workers::list':
    answer({'workers': []})
elif function == 'compose::list':
    answer({'projects': []})
elif function.startswith('compose::'):
    path = manifest_path()
    content = yaml.safe_load(path.read_text())
    lock = path.with_suffix('.lock')
    record(function, file=str(path), manifest=path.read_text(),
           lock=lock.read_text() if lock.exists() else None, args=args)
    # Exact upstream seam: III_COMPOSE_DIR is the canonical manifest parent.
    secret = path.resolve().parent / 'data/secrets'
    secret.mkdir(parents=True, exist_ok=True)
    if not os.environ.get('FAKE_EMPTY_SECRET'):
        (secret / 'sentinel').write_text('synthetic-secret-never-upload')
    for name, container in content['containers'].items():
        source = container['worker']
        if source.startswith('path://./'):
            folder = path.parent / source.removeprefix('path://')
            work = path.parent / container['working_dir'] if 'working_dir' in container else folder
            record('local', name=name, source=str(folder), cwd=str(work),
                   source_bytes=(folder / 'source.txt').read_text(),
                   build_bytes=(folder / 'bin/built').read_text(), scripts=container['scripts'])
        if source.startswith('path://./') and function == 'compose::up':
            # Run only test-owned shell fixtures, with the pinned upstream cwd
            # precedence. This proves relative hook/run commands still work.
            for phase in ('pre_run', 'run'):
                command = container.get('scripts', {}).get(phase)
                if command:
                    result = subprocess.run(command, shell=True, cwd=work,
                                            check=True, capture_output=True, text=True)
                    record('local-command', name=name, phase=phase, command=command,
                           output=result.stdout, cwd=str(work))
        if source.startswith('path:///'):
            record('absolute-build', source=source, scripts=container.get('scripts'),
                   working_dir=container.get('working_dir'))
    if function == 'compose::add':
        if os.environ.get('FAKE_FAIL') == 'add':
            print('synthetic add failure', file=sys.stderr)
            sys.exit(9)
        if not lock.exists():
            lock.write_text(os.environ['FAKE_LOCK'])
        # An actual add mutates the project: export must retain these bytes.
        content['assembled_by_fake'] = True
        path.write_text(yaml.safe_dump(content, sort_keys=False))
    if function == 'compose::up' and json.loads(option('--json'))['frozen']:
        assert lock.read_text() == os.environ['FAKE_LOCK'], 'wrong adjacent frozen lock'
    if function == 'compose::down':
        content['down_by_fake'] = True
        path.write_text(yaml.safe_dump(content, sort_keys=False))
    answer({'status': 'ok'})
elif function == 'e2e::scenarios-list':
    answer(json.loads(os.environ['FAKE_CATALOG']))
elif function == 'e2e::run':
    request = json.loads(option('--json'))
    native = pathlib.Path(os.environ['HARNESS_E2E_ARTIFACTS_DIR']) / 'native/execution-1'
    journal = native / 'journal/events/00000001.json'
    journal.parent.mkdir(parents=True, exist_ok=True)
    journal.write_bytes(b'{"event":"synthetic-native-fact"}\n')
    checkpoint = native / '.workflow-state/state.json'
    checkpoint.parent.mkdir()
    checkpoint.write_bytes(b'{"checkpoint":"unchanged"}\n')
    for name in ('results.json', 'manifest.json', 'observation.json'):
        (native / name).write_text(json.dumps({'synthetic': name}) + '\n')
    answer({'execution_id': 'execution-1'})
elif function == 'e2e::status':
    answer({'execution_id':'execution-1', 'phase':'completed', 'terminal':True})
elif function == 'e2e::results-get':
    if os.environ.get('FAKE_FAIL') == 'results':
        print('synthetic results-get failure', file=sys.stderr)
        sys.exit(8)
    native = pathlib.Path(os.environ['HARNESS_E2E_ARTIFACTS_DIR']) / 'native/execution-1'
    answer({'execution_id':'execution-1', 'result_path':'execution-1/results.json', 'observation':{'evidence':{
        name.removesuffix('.json') + '_sha256':'sha256:' + hashlib.sha256((native / name).read_bytes()).hexdigest()
        for name in ('results.json', 'manifest.json')}}})
elif function in ('e2e::archive', 'e2e::archive-head'):
    answer({'status':'ok'})
else:
    raise SystemExit('unimplemented fake boundary: ' + function)
'''


class PrivateComposeRuntimeTests(unittest.TestCase):
    def setUp(self):
        base = Path(os.environ.get('HARNESS_E2E_TEST_ROOT', ROOT / 'target/private-runtime-tests'))
        base.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=base)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / 'repo'
        (self.repo / 'scripts').mkdir(parents=True)
        for name in ('run_exact_stack_group.sh', 'exact_stack_campaign.py', 'extract_kanban_reports.py'):
            shutil.copyfile(ROOT / 'scripts' / name, self.repo / 'scripts' / name)
        shutil.copytree(ROOT / 'config', self.repo / 'config')
        self.artifacts = self.repo / 'target/artifacts'
        self.artifacts.mkdir(parents=True)
        self.tmp = self.root / 'private-tmp'
        self.tmp.mkdir()
        self.calls = self.root / 'calls.jsonl'
        tools = self.root / 'tools'
        tools.mkdir()
        archive = self.root / 'fake-cli.tar.gz'
        payload = FAKE_III.encode()
        with tarfile.open(archive, 'w:gz') as tar:
            member = tarfile.TarInfo('iii')
            member.size = len(payload)
            member.mode = 0o755
            tar.addfile(member, io.BytesIO(payload))
        # Download interception still exercises production archive hashing and
        # extraction. There is no network fallback and no real CLI in this tar.
        curl = tools / 'curl'
        curl.write_text('#!/usr/bin/env python3\nimport os, shutil, sys\n'
                        "shutil.copyfile(os.environ['FAKE_ARCHIVE'], sys.argv[sys.argv.index('-o') + 1])\n")
        curl.chmod(0o755)
        self.contract = campaign_contract()
        self.contract['runtime']['cli']['sha256'] = 'sha256:' + hashlib.sha256(archive.read_bytes()).hexdigest()
        self.contract['runtime']['compose'] = {'containers': {
            name: {'worker': 'package://' + name, 'version': version}
            for name, version in {'harness':'1.9.0', 'harness-e2e':'0.6.0-experimental', 'provider-deepseek':'1.0.0'}.items()
        }}
        self.lock = lock_of({'harness':'1.9.0', 'harness-e2e':'0.6.0-experimental', 'provider-deepseek':'1.0.0'})
        self.lock['graphs'] = {'harness':['harness'], 'harness-e2e':['harness-e2e'], 'provider-deepseek':['provider-deepseek']}
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith(('HARNESS_E2E_', 'FAKE_')) and key not in MODULE.credential_catalog()[1]}
        self.env.update({
            'PATH':str(tools) + os.pathsep + os.environ['PATH'], 'TMPDIR':str(self.tmp),
            'HARNESS_E2E_CONTRACT':str(self.root / 'contract.json'),
            'HARNESS_E2E_ARTIFACTS_DIR':str(self.artifacts), 'HARNESS_E2E_CAMPAIGN_GROUP_ID':'daily-core',
            'HARNESS_E2E_WAIT_SECONDS':'5', 'FAKE_CALLS':str(self.calls), 'FAKE_ARCHIVE':str(archive),
            'FAKE_VERSION':self.contract['runtime']['cli']['version'], 'FAKE_CATALOG':json.dumps(catalog()),
            'FAKE_LOCK':yaml.safe_dump(self.lock, sort_keys=False),
            'DEEPSEEK_API_KEY':'synthetic-secret-never-upload',
        })

    def launch(self):
        Path(self.env['HARNESS_E2E_CONTRACT']).write_text(json.dumps(self.contract))
        result = subprocess.run(['bash', str(self.repo / 'scripts/run_exact_stack_group.sh')],
                                env=self.env, capture_output=True, text=True, timeout=30)
        self.assertFalse(list(self.tmp.glob('harness-e2e-compose.*')), result.stderr)
        return result

    def observed(self):
        return [json.loads(line) for line in self.calls.read_text().splitlines()]

    def assert_private(self):
        rows = self.observed()
        for row in rows:
            if row['kind'].startswith('compose::'):
                self.assertTrue(Path(row['file']).is_relative_to(self.tmp), row)
                self.assertFalse(Path(row['file']).is_relative_to(self.artifacts), row)
            if row['kind'] == 'daemon' and row['args'][0] == 'compose':
                self.assertTrue(Path(row['state']).is_relative_to(self.tmp))
                self.assertTrue(Path(row['cwd']).is_relative_to(self.tmp))
        down = [row for row in rows if row['kind'] == 'compose::down']
        self.assertEqual(len(down), 1)
        self.assertTrue((self.artifacts / 'stack/worker-compose.lock').is_file())
        self.assertEqual((self.artifacts / 'stack/worker-compose.lock').read_text(), self.env['FAKE_LOCK'])
        bundle = MODULE.package_bundle(self.artifacts, self.contract, {}, {'DEEPSEEK_API_KEY':self.env['DEEPSEEK_API_KEY']})
        paths = {entry['path'] for entry in bundle['files']}
        self.assertFalse(any(path.endswith('.env') or '/secrets/' in path or 'compose-state/' in path for path in paths))
        return rows, paths

    def test_non_template_launcher_keeps_runtime_private_and_evidence_factual(self):
        result = self.launch()
        self.assertEqual(result.returncode, 0, result.stderr)
        rows, paths = self.assert_private()
        self.assertEqual([row['kind'] for row in rows if row['kind'].startswith('compose::')],
                         ['compose::add', 'compose::up', 'compose::status', 'compose::down'])
        before = (self.artifacts / 'stack/worker-compose.yaml').read_bytes()
        final = (self.artifacts / 'stack/worker-compose-final.yaml').read_bytes()
        up = next(row for row in rows if row['kind'] == 'compose::up')
        down = next(row for row in rows if row['kind'] == 'compose::down')
        self.assertEqual(before, up['manifest'].encode())
        self.assertEqual(yaml.safe_load(final), {**yaml.safe_load(down['manifest']), 'down_by_fake': True})
        self.assertTrue(yaml.safe_load(before)['assembled_by_fake'])
        self.assertNotIn('down_by_fake', yaml.safe_load(before))
        self.assertTrue(yaml.safe_load(final)['down_by_fake'])
        self.assertEqual(json.loads((self.artifacts / 'compose-evidence.json').read_text())['compose_sha256'],
                         'sha256:' + hashlib.sha256(before).hexdigest())
        native = self.artifacts / 'native/execution-1'
        self.assertIn('native/execution-1/.workflow-state/state.json', paths)
        for name in ('results.json', 'manifest.json', 'observation.json'):
            self.assertEqual((native / name).read_bytes(), (self.artifacts / name).read_bytes())

    def test_frozen_uses_exact_adjacent_lock_without_add(self):
        self.contract['runtime']['lock'] = self.lock
        self.env['FAKE_EMPTY_SECRET'] = '1'
        result = self.launch()
        self.assertEqual(result.returncode, 0, result.stderr)
        rows, _ = self.assert_private()
        self.assertFalse(any(row['kind'] == 'compose::add' for row in rows))
        up = next(row for row in rows if row['kind'] == 'compose::up')
        self.assertEqual(json.loads(up['args'][up['args'].index('--json') + 1])['frozen'], True)
        self.assertEqual(up['lock'], self.env['FAKE_LOCK'])

    def test_assemble_only_exports_manifest_and_lock_and_never_starts_groups(self):
        self.env['HARNESS_E2E_ASSEMBLE_ONLY'] = '1'
        # Even a selected template is not scaffolded for execution assembly.
        self.contract['runtime']['template'] = {'id':'fixture', 'revision':'a' * 40}
        result = self.launch()
        self.assertEqual(result.returncode, 0, result.stderr)
        rows, paths = self.assert_private()
        self.assertFalse(any(row['kind'] in ('compose::up', 'template') for row in rows))
        self.assertTrue(yaml.safe_load((self.artifacts / 'stack/worker-compose.yaml').read_text())['assembled_by_fake'])
        self.assertNotIn('native/execution-1/results.json', paths)

    def test_template_preserves_local_source_build_and_private_cwd(self):
        template = self.repo / 'target/execution-template'
        template.mkdir()
        subprocess.run(['git', 'init', '-q', str(template)], check=True)
        # No commit: immutable empty tree object suffices for fake revision.
        revision = subprocess.check_output(['git', '-C', str(template), 'hash-object', '-t', 'tree', '/dev/null'], text=True).strip()
        # Fake the single read-only HEAD boundary, never bypass other git use.
        git = self.root / 'tools/git'
        git.write_text('#!/usr/bin/env bash\n[[ "$*" == *"rev-parse HEAD" ]] || exit 77\n'
                       + "printf '%s\\n' " + revision + '\n')
        git.chmod(0o755)
        scaffold = template / 'iii'
        source = scaffold / 'local'
        (source / 'bin').mkdir(parents=True)
        (source / 'source.txt').write_text('template source')
        (source / 'bin/built').write_text('template build')
        (source / 'build.sh').write_text('printf \"template build\" > bin/built\n')
        (scaffold / 'worker-compose.yaml').write_text(yaml.safe_dump({'containers':{
            'harness':{'worker':'package://harness'},
            'local':{'worker':'path://./local', 'scripts':{'run':'cat ./bin/built', 'pre_run':'sh ./build.sh'}}}}))
        self.contract['runtime']['template'] = {'id':'fixture', 'revision':revision}
        self.env['FAKE_TEMPLATE'] = str(scaffold)
        result = self.launch()
        self.assertEqual(result.returncode, 0, result.stderr)
        rows, _ = self.assert_private()
        local = next(row for row in rows if row['kind'] == 'local')
        self.assertTrue(Path(local['source']).is_relative_to(self.tmp))
        self.assertEqual(local['cwd'], local['source'])
        self.assertEqual(local['source_bytes'], 'template source')
        self.assertEqual(local['build_bytes'], 'template build')
        self.assertEqual(local['scripts'], {'run':'cat ./bin/built', 'pre_run':'sh ./build.sh'})
        commands = [row for row in rows if row['kind'] == 'local-command']
        self.assertEqual([row['phase'] for row in commands], ['pre_run', 'run'])
        self.assertEqual(commands[1]['output'], 'template build')

    def test_non_template_relative_sources_and_absolute_commit_builds_survive(self):
        source = self.artifacts / 'stack/local'
        (source / 'bin').mkdir(parents=True)
        (source / 'source.txt').write_text('existing local source')
        (source / 'bin/built').write_text('existing build')
        (source / 'build.sh').write_text('printf \"existing build\" > bin/built\n')
        self.contract['runtime']['compose']['containers']['local'] = {
            'worker':'path://./local', 'working_dir':'./local',
            'scripts':{'run':'cat ./bin/built', 'pre_run':'sh ./build.sh'}}
        self.contract['runtime']['compose']['containers']['pinned'] = {
            'worker':'path:///build/commit', 'working_dir':'.', 'scripts':{'run':'exec /build/commit/bin/worker'}}
        result = self.launch()
        self.assertEqual(result.returncode, 0, result.stderr)
        rows, _ = self.assert_private()
        local = next(row for row in rows if row['kind'] == 'local')
        self.assertEqual(local['source_bytes'], 'existing local source')
        self.assertEqual(local['build_bytes'], 'existing build')
        self.assertEqual(local['cwd'], local['source'])
        commands = [row for row in rows if row['kind'] == 'local-command']
        self.assertEqual([row['phase'] for row in commands], ['pre_run', 'run'])
        self.assertEqual(commands[1]['output'], 'existing build')
        absolute = next(row for row in rows if row['kind'] == 'absolute-build')
        self.assertEqual(absolute['source'], 'path:///build/commit')
        self.assertEqual(absolute['scripts']['run'], 'exec /build/commit/bin/worker')
        self.assertEqual(absolute['working_dir'], '.')

    def test_relative_scripts_from_explicit_project_cwd_are_not_silently_rebased(self):
        source = self.artifacts / 'stack/local'
        (source / 'bin').mkdir(parents=True)
        (source / 'source.txt').write_text('root-cwd source')
        (source / 'bin/built').write_text('root-cwd build')
        (source / 'build.sh').write_text('printf "root-cwd build" > local/bin/built\n')
        scripts = {'pre_run':'sh ./local/build.sh', 'run':'cat ./local/bin/built'}
        self.contract['runtime']['compose']['containers']['local'] = {
            'worker':'path://./local', 'working_dir':'.', 'scripts':scripts}
        result = self.launch()
        self.assertEqual(result.returncode, 0, result.stderr)
        rows, _ = self.assert_private()
        local = next(row for row in rows if row['kind'] == 'local')
        self.assertEqual(Path(local['cwd']), Path(local['source']).parent)
        self.assertEqual(local['scripts'], scripts)
        commands = [row for row in rows if row['kind'] == 'local-command']
        self.assertEqual([row['phase'] for row in commands], ['pre_run', 'run'])
        self.assertEqual(commands[1]['output'], 'root-cwd build')

    def test_add_failure_down_and_private_cleanup(self):
        self.env['FAKE_FAIL'] = 'add'
        result = self.launch()
        self.assertNotEqual(result.returncode, 0)
        rows = self.observed()
        self.assertEqual(len([row for row in rows if row['kind'] == 'compose::down']), 1)
        self.assertFalse(any(row['kind'] == 'compose::up' for row in rows))
        failure = json.loads((self.artifacts / 'failure.json').read_text())
        self.assertEqual(failure['phase'], 'project_assembly')
        MODULE.package_bundle(self.artifacts, self.contract, {})

    def test_results_failure_preserves_committed_native_bytes_and_hidden_checkpoint(self):
        self.env['FAKE_FAIL'] = 'results'
        result = self.launch()
        self.assertNotEqual(result.returncode, 0)
        _, paths = self.assert_private()
        native = self.artifacts / 'native/execution-1'
        self.assertEqual((native / 'journal/events/00000001.json').read_bytes(), b'{"event":"synthetic-native-fact"}\n')
        self.assertEqual((native / '.workflow-state/state.json').read_bytes(), b'{"checkpoint":"unchanged"}\n')
        self.assertIn('native/execution-1/results.json', paths)
        self.assertEqual(json.loads((self.artifacts / 'failure.json').read_text())['phase'], 'results')

    def test_tmpdir_overlap_via_symlink_rejected_before_download_or_credentials(self):
        link = self.root / 'overlap'
        link.symlink_to(self.artifacts, target_is_directory=True)
        self.env['TMPDIR'] = str(link)
        result = self.launch()
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn('must not overlap', result.stdout)
        self.assertFalse(self.calls.exists())
        self.assertFalse(list(self.artifacts.rglob('*.env')))
        self.assertFalse(list(self.artifacts.glob('harness-e2e-compose.*')))

    def test_old_manifest_under_artifacts_rejects_secret_sentinel_and_empty_directory(self):
        Path(self.env['HARNESS_E2E_CONTRACT']).write_text(json.dumps(self.contract))
        manifest = self.artifacts / 'stack/worker-compose.yaml'
        manifest.parent.mkdir()
        manifest.write_text('containers: {}\n')
        secret = manifest.parent / 'data/secrets'
        secret.mkdir(parents=True)
        for empty in (False, True):
            with self.subTest(empty=empty):
                sentinel = secret / 'sentinel'
                if empty:
                    sentinel.unlink()
                else:
                    sentinel.write_text('synthetic-secret-never-upload')
                with self.assertRaisesRegex(ValueError, 'reserved credential path: stack/data/secrets'):
                    MODULE.package_bundle(self.artifacts, self.contract, {})
                result = subprocess.run(
                    ['python3', str(self.repo / 'scripts/exact_stack_campaign.py'), 'package',
                     '--root', str(self.artifacts), '--contract', self.env['HARNESS_E2E_CONTRACT'],
                     '--workflow', '{}', '--output', str(self.root / 'bundle.json')],
                    env=self.env, capture_output=True, text=True, timeout=5)
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn('reserved credential path: stack/data/secrets', result.stdout)
                self.assertFalse((self.root / 'bundle.json').exists())

    def test_actual_manifest_and_lock_layout_validation_rejects_symlink_and_path_escapes(self):
        project = self.tmp / 'project'
        project.mkdir()
        compose = project / 'worker-compose.yaml'
        MODULE.validate_runtime_layout(self.artifacts, self.tmp, self.repo / 'target', compose)
        for path in (self.artifacts / 'stack/worker-compose.yaml', project / '../../escaped.yaml'):
            with self.assertRaisesRegex(ValueError, 'inside the private runtime'):
                MODULE.validate_runtime_layout(self.artifacts, self.tmp, self.repo / 'target', path)
        compose.with_suffix('.lock').symlink_to(self.artifacts / 'lock')
        with self.assertRaisesRegex(ValueError, 'inside the private runtime'):
            MODULE.validate_runtime_layout(self.artifacts, self.tmp, self.repo / 'target', compose)

    def test_exports_refuse_source_and_destination_symlink_escapes(self):
        project = self.tmp / 'project'
        project.mkdir()
        compose = project / 'worker-compose.yaml'
        compose.write_text('containers: {}\n')
        export = lambda: MODULE.export_compose_files(compose, self.tmp, self.artifacts, self.repo / 'target')
        stack = self.artifacts / 'stack'
        stack.symlink_to(self.root, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, 'symlinks'):
            export()
        stack.unlink()
        secret = project / 'private'
        secret.write_text('synthetic-secret-never-upload')
        compose.unlink()
        compose.symlink_to(secret)
        with self.assertRaisesRegex(ValueError, 'private runtime'):
            export()
        self.assertFalse((self.root / 'worker-compose.yaml').exists())

    def test_local_source_symlink_escape_fails_before_engine_start(self):
        source = self.artifacts / 'stack/local'
        source.mkdir(parents=True)
        (source / 'escape').symlink_to(self.root)
        self.contract['runtime']['compose']['containers']['local'] = {'worker':'path://./local'}
        result = self.launch()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('source contains a symlink', result.stdout)
        self.assertFalse(self.calls.exists())

    def test_exports_reject_non_regular_files_and_internal_parent_symlinks(self):
        project = self.tmp / 'project'
        project.mkdir()
        compose = project / 'worker-compose.yaml'
        compose.mkdir()
        with self.assertRaisesRegex(ValueError, 'regular file'):
            MODULE.export_compose_files(compose, self.tmp, self.artifacts, self.repo / 'target')
        compose.rmdir()
        actual = self.tmp / 'actual'
        actual.mkdir()
        (actual / compose.name).write_text('containers: {}\n')
        project.rmdir()
        project.symlink_to(actual, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, 'symlinks'):
            MODULE.export_compose_files(compose, self.tmp, self.artifacts, self.repo / 'target')
        self.assertFalse((self.artifacts / 'stack').exists())

    def test_local_source_or_separate_relative_cwd_escape_is_rejected(self):
        source = self.artifacts / 'stack/local'
        source.mkdir(parents=True)
        manifest = {'containers':{'local':{'worker':'path://./../outside'}}}
        project = self.tmp / 'project'
        project.mkdir()
        with self.assertRaisesRegex(ValueError, 'inside its project'):
            MODULE.stage_relative_workers(manifest, source.parent, project)
        manifest['containers']['local'] = {'worker':'path://./local', 'working_dir':'../outside'}
        with self.assertRaisesRegex(ValueError, 'inside its source directory'):
            MODULE.stage_relative_workers(manifest, source.parent, project)
        self.assertFalse((project / 'local').exists())

    def test_introduced_artifact_credentials_and_symlinks_still_fail_after_launcher(self):
        self.assertEqual(self.launch().returncode, 0)
        self.assert_private()
        for name in ('secrets', '.env'):
            path = self.artifacts / name
            path.mkdir()
            with self.assertRaisesRegex(ValueError, 'reserved credential path'):
                MODULE.package_bundle(self.artifacts, self.contract, {})
            path.rmdir()
        link = self.artifacts / 'escape'
        link.symlink_to(self.root)
        with self.assertRaisesRegex(ValueError, 'symlink'):
            MODULE.package_bundle(self.artifacts, self.contract, {})


if __name__ == '__main__':
    unittest.main()
