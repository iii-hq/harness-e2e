"""Exercise the production launcher without downloading or running the iii CLI."""
import hashlib
import io
import json
import os
import re
from pathlib import Path
import shutil
import signal
import subprocess
import tarfile
import tempfile
import time
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
def error(code, message):
    print('Error: ' + json.dumps({'code': code, 'message': message, 'stacktrace': None}, indent=2), file=sys.stderr)
    sys.exit(1)
if os.environ.get('FAKE_HANG') == function:
    # A call that never answers: only the launcher's own bound ends it.
    if os.environ.get('FAKE_BREAK_LOCK'):
        # Exporting the lock will fail, and the launcher's status with it.
        for lock in pathlib.Path(os.environ['TMPDIR']).glob('harness-e2e-compose.*/project/worker-compose.lock'):
            lock.unlink()
            lock.mkdir()
    record('hang', function=function)
    time.sleep(3600)
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
    if function == 'compose::up' and os.environ.get('FAKE_ORPHANS'):
        # Workers Compose starts in a process group of their own outlive it:
        # one by its cwd, one by its command line under the runtime root.
        orphans = pathlib.Path(os.environ['FAKE_ORPHANS'])
        earlier = [int(pid) for pid in orphans.read_text().split()] if orphans.exists() else []
        def alive(pid):
            try:
                return pathlib.Path(f'/proc/{pid}/status').read_text().split('State:')[1].split()[0] != 'Z'
            except (OSError, IndexError):
                return False
        containers = pathlib.Path(os.environ['FAKE_CONTAINERS'])
        record('orphans', alive=[pid for pid in earlier if alive(pid)],
               containers=containers.read_text().split() if containers.exists() else [])
        sleeper = [sys.executable, '-c', 'import time; time.sleep(3600)']
        for command, cwd in ((sleeper, path.parent), (sleeper + [str(path.parent / 'worker')], '/')):
            child = subprocess.Popen(command, cwd=cwd, start_new_session=True,
                                     stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            with orphans.open('a') as file:
                file.write(f'{child.pid}\n')
        with containers.open('a') as file:
            file.write(f'container-{len(earlier)}\n')
    if function == 'compose::up' and (os.environ.get('FAKE_FAIL') == 'up'
                                      or (os.environ.get('FAKE_FAIL') == 'up-once'
                                          and not os.environ.get('HARNESS_E2E_GROUP_ATTEMPT'))):
        # What iii printed on 2026-10-14, when GitHub answered 500 for ade.
        print('Error: ' + json.dumps({'code': 'PACKAGE_DOWNLOAD_FAILED', 'message': (
            "container 'ade': could not download https://api.workers.iii.dev/download/ade/1.9.60/"
            "x86_64-unknown-linux-gnu: HTTP status server error (500 Internal Server Error) for url "
            "(https://github.com/iii-hq/workers/releases/download/build-881c3e0e19c38764747df73a9dc1e48aaa68e4ce/"
            "ade-x86_64-unknown-linux-gnu.tar.gz)"), 'stacktrace': None}, indent=2), file=sys.stderr)
        sys.exit(1)
    if function == 'compose::up' and json.loads(option('--json'))['frozen']:
        assert lock.read_text() == os.environ['FAKE_LOCK'], 'wrong adjacent frozen lock'
    if function == 'compose::down':
        content['down_by_fake'] = True
        path.write_text(yaml.safe_dump(content, sort_keys=False))
    answer({'status': 'ok'})
elif function == 'e2e::scenarios-list':
    if os.environ.get('FAKE_FAIL') == 'catalog':
        error('function_not_found', 'Function e2e::scenarios-list not found in namespace e2e.')
    answer(json.loads(os.environ['FAKE_CATALOG']))
elif function == 'e2e::run':
    if os.environ.get('FAKE_FAIL') == 'run':
        error('ADMISSION_TIMEOUT', 'the runner did not admit the run in time')
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
    phase = os.environ.get('FAKE_STATUS', 'completed')
    answer({'execution_id':'execution-1', 'phase':phase, 'terminal':True,
            **({'error': 'the subject gave up'} if phase != 'completed' else {})})
elif function == 'e2e::results-get':
    if os.environ.get('FAKE_FAIL') == 'results':
        print('synthetic results-get failure', file=sys.stderr)
        sys.exit(8)
    native = pathlib.Path(os.environ['HARNESS_E2E_ARTIFACTS_DIR']) / 'native/execution-1'
    answer({'execution_id':'execution-1', 'result_path':'execution-1/results.json', 'observation':{'evidence':{
        name.removesuffix('.json') + '_sha256':'sha256:' + hashlib.sha256((native / name).read_bytes()).hexdigest()
        for name in ('results.json', 'manifest.json')}}})
elif function in ('e2e::archive', 'e2e::archive-head'):
    if os.environ.get('FAKE_FAIL') == 'archive':
        error('ARCHIVE_UNAVAILABLE', 'longitudinal storage is not configured')
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
        tools = self.tools = self.root / 'tools'
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
        # The launcher waits with sleep(1); this one only says how long.
        self.sleeps = self.root / 'sleeps'
        sleep = tools / 'sleep'
        sleep.write_text('#!/bin/sh\nprintf \'%s\\n\' "$*" >>"$FAKE_SLEEPS"\n'
                         # Interrupted while it waits that long, as a cancelled job is.
                         '[ "$1" != "${FAKE_SLEEP_TERM:-}" ] || kill -TERM "$PPID"\n')
        sleep.chmod(0o755)
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
            'FAKE_SLEEPS':str(self.sleeps),
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
        # The launcher's own evidence never holds the key: nothing withheld.
        self.assertEqual(bundle['withheld'], [])
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
        # Once for each attempt: nothing had run, so the group was tried again.
        self.assertEqual(len([row for row in rows if row['kind'] == 'compose::down']), 2)
        self.assertFalse(any(row['kind'] == 'compose::up' for row in rows))
        failure = json.loads((self.artifacts / 'failure.json').read_text())
        self.assertEqual(failure['phase'], 'project_assembly')
        self.assertEqual(failure['function'], 'compose::add')
        self.assertEqual(failure['attempt'], 2)
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

    def test_old_manifest_under_artifacts_withholds_secret_sentinel_and_empty_directory(self):
        Path(self.env['HARNESS_E2E_CONTRACT']).write_text(json.dumps(self.contract))
        manifest = self.artifacts / 'stack/worker-compose.yaml'
        manifest.parent.mkdir()
        manifest.write_text('containers: {}\n')
        secret = manifest.parent / 'data/secrets'
        withheld = [{'path': 'stack/data/secrets', 'withheld': 'credential', 'reason': 'reserved credential path'}]
        for empty, packaged in ((False, 'module'), (True, 'module'), (False, 'command')):
            with self.subTest(empty=empty, packaged=packaged):
                secret.mkdir(parents=True, exist_ok=True)
                if not empty:
                    (secret / 'sentinel').write_text('synthetic-secret-never-upload')
                if packaged == 'module':
                    bundle = MODULE.package_bundle(self.artifacts, self.contract, {})
                else:
                    result = subprocess.run(
                        ['python3', str(self.repo / 'scripts/exact_stack_campaign.py'), 'package',
                         '--root', str(self.artifacts), '--contract', self.env['HARNESS_E2E_CONTRACT'],
                         '--workflow', '{}', '--output', str(self.root / 'bundle.json')],
                        env=self.env, capture_output=True, text=True, timeout=5)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertIn('::warning::withheld stack/data/secrets from the evidence', result.stdout)
                    bundle = json.loads((self.root / 'bundle.json').read_text())
                self.assertEqual(bundle['withheld'], withheld)
                self.assertFalse(secret.exists())
                self.assertEqual([entry['path'] for entry in bundle['files']], ['stack/worker-compose.yaml'])
                self.assertNotIn('synthetic-secret-never-upload', json.dumps(bundle))

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

    def test_introduced_credential_paths_are_withheld_and_symlinks_still_fail_after_launcher(self):
        self.assertEqual(self.launch().returncode, 0)
        self.assert_private()
        for name in ('secrets', '.env'):
            path = self.artifacts / name
            path.mkdir()
            bundle = MODULE.package_bundle(self.artifacts, self.contract, {})
            self.assertEqual([entry['path'] for entry in bundle['withheld']], [name])
            self.assertFalse(path.exists())
        # A symlink is still refused: what it points at cannot be vouched for.
        link = self.artifacts / 'escape'
        link.symlink_to(self.root)
        with self.assertRaisesRegex(ValueError, 'symlink'):
            MODULE.package_bundle(self.artifacts, self.contract, {})

    def test_readiness_is_a_deadline_however_long_each_probe_hangs(self):
        """Each probe is stopped after 6 s (`--timeout-ms 5000`, and the
        process a second later), and the wait counts seconds, not probes: a
        deadline of W seconds gives up within W + 6 + 1. With the default
        300 s that is 307 s; counting probes, it once took 155 minutes."""
        wait = 2
        self.env['HARNESS_E2E_WAIT_SECONDS'] = str(wait)
        # A second attempt only repeats this; see the test below.
        self.env['HARNESS_E2E_GROUP_ATTEMPT'] = '2'
        for hung, phase, what in (('engine::workers::list', 'engine_start', 'iii engine'),
                                  ('compose::list', 'compose_start', 'iii compose'),
                                  ('e2e::scenarios-list', 'runner_readiness', 'E2E runner')):
            with self.subTest(hung=hung):
                shutil.rmtree(self.artifacts)
                self.artifacts.mkdir()
                self.calls.unlink(missing_ok=True)
                self.env['FAKE_HANG'] = hung
                started = time.monotonic()
                result = self.launch()
                elapsed = time.monotonic() - started
                self.assertNotEqual(result.returncode, 0)
                failure = json.loads((self.artifacts / 'failure.json').read_text())
                self.assertEqual(failure['phase'], phase)
                self.assertTrue(failure['error'].startswith(what), failure['error'])
                gave_up = int(re.search(rf'within {wait}s \(gave up after (\d+)s\)', failure['error'])[1])
                self.assertGreaterEqual(gave_up, wait)
                self.assertLessEqual(gave_up, wait + 6 + 1)
                # The rest is the launcher starting and taking the stack down.
                self.assertLess(elapsed, wait + 6 + 1 + 10)
                self.assertEqual(failure['function'], hung)

    def test_a_stack_that_cannot_start_is_tried_once_more_and_says_what_iii_answered(self):
        self.env['FAKE_FAIL'] = 'up'
        result = self.launch()
        self.assertNotEqual(result.returncode, 0)
        rows = self.observed()
        # Three compose::up a try (#281), two tries, each taken down after.
        self.assertEqual([row['kind'] for row in rows if row['kind'] in ('compose::up', 'compose::down')],
                         ['compose::up'] * 3 + ['compose::down'] + ['compose::up'] * 3 + ['compose::down'])
        self.assertIn('60', self.sleeps.read_text().splitlines())
        self.assertIn('trying the group once more in 60s', result.stderr)
        failure = json.loads((self.artifacts / 'failure.json').read_text())
        # What Release Control already reads, as it was.
        self.assertEqual(failure['phase'], 'project_start')
        self.assertEqual(failure['outcome'], 'infra_failed')
        self.assertEqual(failure['exit_code'], 1)
        self.assertIn('compose::up exhausted 3 attempts', failure['error'])
        # And what iii answered, not the line of the launcher.
        self.assertEqual(failure['code'], 'PACKAGE_DOWNLOAD_FAILED')
        self.assertEqual(failure['function'], 'compose::up')
        self.assertTrue(failure['message'].startswith("container 'ade': could not download"), failure['message'])
        self.assertIn('ade/1.9.60', failure['message'])
        self.assertIn('500 Internal Server Error', failure['message'])
        self.assertLessEqual(len(failure['message']), 300)
        self.assertEqual(failure['attempt'], 2)
        [earlier] = failure['earlier_attempts']
        self.assertEqual((earlier['attempt'], earlier['phase'], earlier['code']),
                         (1, 'project_start', 'PACKAGE_DOWNLOAD_FAILED'))
        # The first attempt's evidence stays, apart; the bundle takes both.
        self.assertEqual(json.loads((self.artifacts / 'attempts/1/failure.json').read_text()), earlier)
        self.assertTrue((self.artifacts / 'attempts/1/logs/compose-up-attempt-3.log').is_file())
        self.assertTrue((self.artifacts / 'logs/compose-up-attempt-3.log').is_file())
        bundle = MODULE.package_bundle(self.artifacts, self.contract, {},
                                       {'DEEPSEEK_API_KEY': self.env['DEEPSEEK_API_KEY']})
        self.assertIn('attempts/1/failure.json', {entry['path'] for entry in bundle['files']})
        self.assertNotIn(self.env['DEEPSEEK_API_KEY'], (self.artifacts / 'failure.json').read_text())

    def test_a_second_attempt_still_finds_the_declared_local_sources(self):
        source = self.artifacts / 'stack/local'
        (source / 'bin').mkdir(parents=True)
        (source / 'source.txt').write_text('existing local source')
        (source / 'bin/built').write_text('existing build')
        self.contract['runtime']['compose']['containers']['local'] = {
            'worker':'path://./local', 'working_dir':'./local', 'scripts':{'run':'cat ./bin/built'}}
        self.env['FAKE_FAIL'] = 'up'
        result = self.launch()
        self.assertNotEqual(result.returncode, 0)
        rows = self.observed()
        # Both attempts got as far as starting the stack, the local source staged each time.
        self.assertEqual(len([row for row in rows if row['kind'] == 'compose::up']), 6)
        staged = [row for row in rows if row['kind'] == 'local']
        self.assertTrue(staged)
        self.assertTrue(all(row['source_bytes'] == 'existing local source' for row in staged))
        self.assertEqual(json.loads((self.artifacts / 'failure.json').read_text())['phase'], 'project_start')
        self.assertEqual((self.artifacts / 'attempts/1/stack/local/source.txt').read_text(), 'existing local source')

    def test_a_group_whose_run_started_is_never_tried_again(self):
        self.env['FAKE_FAIL'] = 'results'
        result = self.launch()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len([row for row in self.observed() if row['kind'] == 'compose::up']), 1)
        self.assertFalse((self.artifacts / 'attempts').exists())
        self.assertNotIn('once more', result.stderr)
        failure = json.loads((self.artifacts / 'failure.json').read_text())
        self.assertEqual((failure['phase'], failure['attempt'], failure['function']),
                         ('results', 1, 'e2e::results-get'))
        self.assertNotIn('earlier_attempts', failure)

    def test_a_run_the_runner_did_not_admit_is_never_tried_again(self):
        # Whether the runner started it is unknown: the result is not drawn twice.
        self.env['FAKE_FAIL'] = 'run'
        result = self.launch()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len([row for row in self.observed() if row['kind'] == 'compose::up']), 1)
        self.assertFalse((self.artifacts / 'attempts').exists())
        failure = json.loads((self.artifacts / 'failure.json').read_text())
        self.assertEqual((failure['phase'], failure['attempt'], failure['function'], failure['code']),
                         ('execution', 1, 'e2e::run', 'ADMISSION_TIMEOUT'))

    def wait_for_hang(self, process):
        """Until the launcher's call that never answers has started."""
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline and process.poll() is None:
            if self.calls.exists() and any(row['kind'] == 'hang' for row in self.observed()):
                return
            time.sleep(0.1)
        self.fail('the hung call never started')

    def test_an_interrupted_group_is_never_tried_again(self):
        """Ctrl-C reaches the whole process group, as a terminal sends it: the
        call in flight (in the foreground) stops at once, not at its timeout.
        TERM reaches the launcher alone, as a stopped container's does."""
        self.env['HARNESS_E2E_WAIT_SECONDS'] = '60'
        # The last: cleanup fails after the interruption (the lock cannot be
        # exported), which must not turn it into a failure worth a retry.
        for signal_name, status, hung, exit_status in (
                ('SIGINT', 130, 'engine::workers::list', 130), ('SIGTERM', 143, 'engine::workers::list', 143),
                ('SIGTERM', 143, 'e2e::scenarios-list', 1)):
            with self.subTest(signal=signal_name, hung=hung):
                self.env['FAKE_HANG'] = hung
                if exit_status != status:
                    self.env['FAKE_BREAK_LOCK'] = '1'
                shutil.rmtree(self.artifacts)
                self.artifacts.mkdir()
                self.calls.unlink(missing_ok=True)
                self.sleeps.unlink(missing_ok=True)
                Path(self.env['HARNESS_E2E_CONTRACT']).write_text(json.dumps(self.contract))
                process = subprocess.Popen(['bash', str(self.repo / 'scripts/run_exact_stack_group.sh')], env=self.env,
                                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                                           start_new_session=True)
                self.wait_for_hang(process)
                sent = time.monotonic()
                if signal_name == 'SIGINT':
                    os.killpg(process.pid, signal.SIGINT)
                else:
                    process.send_signal(signal.SIGTERM)
                _, stderr = process.communicate(timeout=30)
                self.assertEqual(process.returncode, exit_status, stderr)
                if signal_name == 'SIGINT':
                    # The probe would have waited out its 6 s.
                    self.assertLess(time.monotonic() - sent, 4)
                self.assertFalse((self.artifacts / 'attempts').exists())
                self.assertNotIn('once more', stderr)
                self.assertNotIn('60', self.sleeps.read_text().split() if self.sleeps.exists() else [])
                failure = json.loads((self.artifacts / 'failure.json').read_text())
                phase = 'engine_start' if hung == 'engine::workers::list' else 'runner_readiness'
                self.assertEqual((failure['phase'], failure['attempt'], failure['exit_code']), (phase, 1, status))
                self.assertFalse(list(self.tmp.glob('harness-e2e-compose.*')))

    def test_cancelled_while_it_waits_to_try_again_the_group_keeps_its_evidence(self):
        self.env['FAKE_FAIL'] = 'up'
        self.env['FAKE_SLEEP_TERM'] = '60'
        result = self.launch()
        self.assertEqual(result.returncode, -signal.SIGTERM, result.stderr)
        # Nothing moved: the first attempt's failure is the group's.
        self.assertFalse((self.artifacts / 'attempts').exists())
        failure = json.loads((self.artifacts / 'failure.json').read_text())
        self.assertEqual((failure['phase'], failure['attempt'], failure['code']),
                         ('project_start', 1, 'PACKAGE_DOWNLOAD_FAILED'))
        self.assertTrue((self.artifacts / 'logs/compose-up-attempt-3.log').is_file())
        self.assertEqual(len([row for row in self.observed() if row['kind'] == 'compose::up']), 3)

    def test_what_the_first_attempt_left_running_is_gone_before_the_second(self):
        """Compose starts each worker in a process group of its own, with no
        parent-death signal: killing Compose leaves them connected to the next
        engine on the port. Before the second attempt every process under
        the runtime root, and every container of the group's own daemon, goes."""
        orphans = self.root / 'orphans'
        containers = self.root / 'containers'
        docker_log = self.root / 'docker.log'
        docker = self.tools / 'docker'
        docker.write_text('#!/bin/sh\nprintf \'%s\\n\' "$*" >>"$FAKE_DOCKER_LOG"\n'
                          'case "$1" in ps) cat "$FAKE_CONTAINERS" 2>/dev/null ;; rm) : >"$FAKE_CONTAINERS" ;; esac\n')
        docker.chmod(0o755)
        self.env.update({'FAKE_ORPHANS': str(orphans), 'FAKE_CONTAINERS': str(containers),
                         'FAKE_DOCKER_LOG': str(docker_log), 'HARNESS_E2E_OWN_DOCKER': '1',
                         # The runner never registers, so the stack is up when the attempt fails.
                         'FAKE_FAIL': 'catalog', 'HARNESS_E2E_WAIT_SECONDS': '1'})
        result = self.launch()
        self.assertNotEqual(result.returncode, 0)
        seen = [row for row in self.observed() if row['kind'] == 'orphans']
        self.assertEqual(len(seen), 2, 'one compose::up an attempt')
        first = [int(pid) for pid in orphans.read_text().split()]
        self.assertEqual(len(first), 4)
        # When the second attempt started its stack, nothing of the first ran.
        self.assertEqual(seen[1], {**seen[1], 'alive': [], 'containers': []})
        self.assertIn('rm -f container-0', docker_log.read_text())
        # Nor of the second, once it ended.
        for pid in first:
            state = Path(f'/proc/{pid}/status')
            self.assertFalse(state.exists() and '\nState:\tZ' not in state.read_text(), pid)
        self.assertEqual(json.loads((self.artifacts / 'failure.json').read_text())['phase'], 'runner_readiness')

    def test_a_group_tried_again_says_so_at_the_top_and_in_its_shard(self):
        self.env['FAKE_FAIL'] = 'up-once'
        result = self.launch()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.artifacts / 'failure.json').exists())
        marker = json.loads((self.artifacts / 'stack/attempt.json').read_text())
        self.assertEqual(marker['attempt'], 2)
        [earlier] = marker['earlier_attempts']
        self.assertEqual((earlier['attempt'], earlier['phase'], earlier['code'], earlier['function']),
                         (1, 'project_start', 'PACKAGE_DOWNLOAD_FAILED', 'compose::up'))
        self.assertEqual(earlier, json.loads((self.artifacts / 'attempts/1/failure.json').read_text()))
        payload = self.root / 'shard.json'
        reported = subprocess.run(
            ['python3', str(ROOT / 'scripts/report_execution.py'), 'shard', '--execution-id', 'e-1',
             '--oidc-audience', 'a', '--artifacts', str(self.artifacts), '--campaign-id', 'c', '--group-id', 'g',
             '--outcome', 'success', '--output', str(payload)],
            env={'PATH': os.environ['PATH']}, capture_output=True, text=True)
        self.assertEqual(reported.returncode, 0, reported.stderr)
        self.assertEqual(json.loads(payload.read_text())['group']['attempts'], marker)

    def test_a_failure_the_launcher_tolerated_is_never_named_as_the_cause(self):
        # Archiving is best effort; the run itself ended in failure after it.
        self.env.update({'FAKE_FAIL': 'archive', 'FAKE_STATUS': 'failed'})
        result = self.launch()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('ARCHIVE_UNAVAILABLE', (self.artifacts / 'logs/archive.log').read_text())
        failure = json.loads((self.artifacts / 'failure.json').read_text())
        self.assertEqual((failure['phase'], failure['error']), ('results', 'the subject gave up'))
        for key in ('function', 'code', 'message'):
            self.assertNotIn(key, failure)

    def test_the_failure_record_never_holds_a_credential(self):
        key = 'sk-deepseek-0123456789abcdef'
        stderr = 'Error: ' + json.dumps({'code': 'PROVIDER_REJECTED', 'message': 'x' * 280 + f' key={key} rest'})
        earlier = [{'attempt': 1, 'phase': 'project_start', 'error': f'compose::up said {key}'}]
        record = MODULE.failure_record('project_start', 1, 2, f'compose::up failed with {key}', None, 'compose::up',
                                       stderr, earlier, {'DEEPSEEK_API_KEY': key})
        text = json.dumps(record)
        # Cut after redaction: not even the key's first characters remain.
        self.assertNotIn('sk-deep', text)
        self.assertEqual(record['error'], 'compose::up failed with [redacted:DEEPSEEK_API_KEY]')
        self.assertEqual(record['earlier_attempts'][0]['error'], 'compose::up said [redacted:DEEPSEEK_API_KEY]')
        self.assertTrue(record['message'].startswith('x' * 280 + ' key=[redac'))
        # The command takes them from what the group received.
        log = self.root / 'stderr.log'
        log.write_text(stderr)
        output = self.root / 'failure.json'
        subprocess.run(['python3', str(self.repo / 'scripts/exact_stack_campaign.py'), 'failure',
                        '--output', str(output), '--phase', 'project_start', '--exit-code', '1',
                        '--iii-function', 'compose::up', '--iii-stderr', str(log)],
                       env={'PATH': os.environ['PATH'], 'DEEPSEEK_API_KEY': key}, check=True)
        self.assertNotIn('sk-deep', output.read_text())
        self.assertIn(' key=[redacted:DEEP', output.read_text())

    def test_the_failure_record_keeps_what_iii_answered_and_never_more_than_its_start(self):
        progress = 'resolving ade@1.9.60\nError: {\n  "code": "X_FAILED",\n  "message": "first"\n}\n'
        cases = (
            (progress + 'Error: {"code": "PACKAGE_DOWNLOAD_FAILED", "message": "  two\\n  lines  "}\n',
             {'code': 'PACKAGE_DOWNLOAD_FAILED', 'message': 'two lines'}),
            ('Error: Timed out waiting for the engine (no response within the timeout).\nmore\n',
             {'message': 'Timed out waiting for the engine (no response within the timeout).'}),
            ('Error: {"code": "D310", "message": "' + 'x' * 400 + '"}', {'code': 'D310', 'message': 'x' * 299 + '…'}),
            ('nothing iii said', {}),
            ('', {}),
        )
        for stderr, expected in cases:
            with self.subTest(stderr=stderr[:30]):
                self.assertEqual(MODULE.iii_error(stderr), expected)
        record = MODULE.failure_record('project_start', 1, 1, None, 128, 'compose::status', cases[0][0])
        self.assertEqual(record['error'],
                         'compose::status failed during project_start: PACKAGE_DOWNLOAD_FAILED: two lines')
        record = MODULE.failure_record('project_start', 1, 1, 'compose::up exhausted 3 attempts', 128, 'compose::up')
        self.assertEqual((record['error'], record['function']), ('compose::up exhausted 3 attempts', 'compose::up'))
        self.assertNotIn('code', record)
        record = MODULE.failure_record('materialization', 2, 1, None, 561)
        self.assertEqual(record, {'phase': 'materialization', 'outcome': 'infra_failed', 'attempt': 1, 'exit_code': 2,
                                  'error': 'command failed during materialization at line 561 (exit 2)'})


if __name__ == '__main__':
    unittest.main()
