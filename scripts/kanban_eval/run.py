#!/usr/bin/env python3
"""Run trusted Kanban control probes with candidate code in a nested OS sandbox.

This is a local control runner, not a model campaign or a registered ScenarioId.
All runtime inputs are administrator-selected, never supplied by candidate code.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time
import urllib.request

ENV = {'PATH': '/runtime:/usr/bin:/bin', 'HOME': '/tmp', 'LANG': 'C.UTF-8',
       'LC_ALL': 'C.UTF-8', 'III_TELEMETRY_ENABLED': 'false', 'OTEL_ENABLED': 'false',
       'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': '/dev/null',
       'GIT_TERMINAL_PROMPT': '0', 'CI': 'true'}


class InfrastructureError(RuntimeError):
    pass


def runtime_roots():
    args = []
    for root in ('/usr', '/bin', '/lib', '/lib64'):
        if Path(root).exists():
            args += ['--ro-bind', root, root]
    return args


def candidate_command(command):
    """Private mounts, parent PIDs and host network are absent from this view."""
    args = ['/usr/bin/bwrap', '--unshare-all', '--share-net', '--die-with-parent',
            '--new-session', '--cap-drop', 'ALL', '--clearenv', *runtime_roots(),
            '--proc', '/proc', '--dev', '/dev', '--tmpfs', '/tmp',
            '--ro-bind', '/runtime', '/runtime', '--bind', '/workspace', '/workspace',
            '--bind', '/data', '/data', '--bind', '/runtime-state', '/runtime-state']
    if Path('/workspace/kanban').is_dir():
        args += ['--ro-bind', '/dependencies', '/workspace/kanban/node_modules']
    for key, value in ENV.items():
        args += ['--setenv', key, value]
    return args + ['--chdir', '/workspace', '--', *command]


def bounded(command, log, timeout):
    with open(log, 'wb') as output:
        proc = subprocess.Popen(command, env=ENV, stdin=subprocess.DEVNULL,
                                stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            return proc.wait(timeout=timeout)
        finally:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            proc.wait()


def inside(case_id):
    evidence = Path('/evidence')
    started = time.monotonic()
    # A trusted canary must be unreadable to candidate processes, even though
    # the evaluator and candidate need the same isolated loopback network.
    Path('/evidence/private-canary').write_text('private control state')
    check = """import pathlib,socket
assert pathlib.Path('/workspace/README.md').is_file()
for p in ['/trusted/run.py','/evidence/private-canary','/browser-deps','/browsers']:
 assert not pathlib.Path(p).exists(),p
assert not pathlib.Path('/proc/1/root/evidence').exists()
s=socket.socket();s.settimeout(.2)
assert s.connect_ex(('1.1.1.1',443)) != 0
print('workspace readable; trusted files, parent process and external network inaccessible')
"""
    rc = bounded(candidate_command(['/usr/bin/python3', '-I', '-c', check]),
                 evidence / 'isolation.log', 10)
    if rc:
        raise InfrastructureError('candidate isolation preflight failed; see isolation.log')
    build_checks = []
    result = {'schema': 'kanban-evaluation/v1', 'case_id': case_id,
              'status': 'failed', 'functional_status': 'failed', 'checks': build_checks}
    if not Path('/workspace/kanban/package.json').is_file():
        build_checks.append({'id': 'application_present', 'status': 'failed',
                             'detail': 'The selected snapshot contains no application.'})
        (evidence / 'result.json').write_text(json.dumps(result))
        return 0
    for step in ('typecheck', 'test', 'build'):
        try:
            code = bounded(candidate_command(['pnpm', '--dir', 'kanban', step]),
                           evidence / f'{step}.log', 90)
        except subprocess.TimeoutExpired:
            code = 124
        build_checks.append({'id': step, 'status': 'passed' if code == 0 else 'failed',
                             'detail': f'isolated command exit {code}'})
        if code:
            (evidence / 'result.json').write_text(json.dumps(result))
            return 0
    config = {'workers': [
        {'name': 'iii-worker-manager', 'config': {'host': '127.0.0.1', 'port': 50179}},
        {'name': 'configuration', 'config': {'adapter': {'name': 'fs', 'config': {'directory': '/runtime-state/configuration'}}}},
    ]}
    Path('/runtime-state/config.json').write_text(json.dumps(config))
    # No evaluator code is placed in the candidate mount namespace.
    start = '''set -eu
iii --no-update-check --config /runtime-state/config.json &
sleep 1
iii trigger configuration::register --address 127.0.0.1 --port 50179 --namespace default --json '{"id":"kanban","name":"Kanban","description":"Evaluation data","schema":{"type":"object","properties":{"data_dir":{"type":"string"}},"required":["data_dir"]},"initial_value":{"data_dir":"/data","preserve_me":true}}'
exec iii compose --up --engine ws://127.0.0.1:50179 --file /workspace/worker-compose.yaml
'''
    with (evidence / 'runtime.log').open('wb') as output:
        process = subprocess.Popen(candidate_command(['/bin/sh', '-c', start]), env=ENV,
                                   stdin=subprocess.DEVNULL, stdout=output,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        try:
            ready = False
            for _ in range(120):
                if process.poll() is not None:
                    break
                try:
                    with urllib.request.urlopen('http://127.0.0.1:3000/api/config', timeout=.3) as response:
                        ready = response.status == 200
                    if ready:
                        break
                except (OSError, ValueError):
                    pass
                time.sleep(.25)
            if not ready:
                build_checks.append({'id': 'application_startup', 'status': 'failed',
                                     'detail': 'Compose application did not become ready; see runtime.log.'})
                (evidence / 'result.json').write_text(json.dumps(result))
                return 0
            probe_env = {**ENV, 'III_SDK_MODULE': '/dependencies/iii-sdk/dist/index.mjs',
                         'PLAYWRIGHT_MODULE': os.environ['PLAYWRIGHT_MODULE'],
                         'PLAYWRIGHT_BROWSERS_PATH': '/browsers'}
            with (evidence / 'probe.log').open('wb') as log:
                probe = subprocess.run(['/runtime/node', '/trusted/probe.mjs', '--case', case_id,
                                '--base-url', 'http://127.0.0.1:3000', '--engine-url', 'ws://127.0.0.1:50179',
                                '--output', '/evidence'], env=probe_env, stdout=log,
                               stderr=subprocess.STDOUT, timeout=120, check=False)
            if probe.returncode or not all((evidence / name).is_file() for name in ('result.json', 'coverage.json')):
                raise RuntimeError('trusted probe failed or returned incomplete evidence')
            result = json.loads((evidence / 'result.json').read_text())
            coverage = json.loads((evidence / 'coverage.json').read_text())
            if result.get('case_id') != case_id or coverage.get('case_id') != case_id or coverage.get('criteria') != [check for check in result['checks'] if check['id'].startswith('criterion_')]:
                raise RuntimeError('trusted probe returned inconsistent coverage')
            result['build_checks'] = build_checks
            result['duration_ms'] = round((time.monotonic() - started) * 1000)
            (evidence / 'result.json').write_text(json.dumps(result, indent=2))
            return 0
        finally:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait(timeout=5)


def file_digest(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def tree_digest(root):
    digest = hashlib.sha256()
    for path in sorted(root.rglob('*')):
        digest.update(str(path.relative_to(root)).encode() + b'\0')
        if path.is_symlink():
            digest.update(b'link\0' + os.readlink(path).encode())
        elif path.is_file():
            digest.update(b'file\0' + file_digest(path).encode())
        else:
            digest.update(b'directory')
        digest.update(b'\0')
    return digest.hexdigest()


def main():
    if len(sys.argv) == 3 and sys.argv[1] == '--inside':
        try:
            return inside(sys.argv[2])
        except Exception as error:
            Path('/evidence/result.json').write_text(json.dumps({
                'schema': 'kanban-evaluation/v1', 'case_id': sys.argv[2],
                'status': 'infrastructure_failed' if isinstance(error, InfrastructureError) else 'evaluation_failed', 'functional_status': None,
                'error': str(error), 'checks': []}))
            return 2
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--fixture', type=Path, required=True)
    parser.add_argument('--catalog', type=Path, required=True)
    parser.add_argument('--case', required=True)
    parser.add_argument('--revision', choices=['base', 'reference'], required=True)
    parser.add_argument('--output', type=Path, required=True)
    for name in ('node', 'iii', 'pnpm', 'dependencies', 'browser-dependencies', 'browsers'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--playwright-module', default='.pnpm/playwright@1.61.1/node_modules/playwright/index.mjs')
    args = parser.parse_args()
    args.output = args.output.resolve()
    if args.output.exists():
        parser.error('--output must be new; never reuse or overwrite an execution')
    for name in ('fixture', 'catalog', 'node', 'iii', 'pnpm', 'dependencies', 'browser_dependencies', 'browsers'):
        setattr(args, name, getattr(args, name).resolve(strict=True))
    args.output.mkdir(parents=True, mode=0o700)
    evidence = args.output / 'evidence'
    evidence.mkdir(mode=0o700)
    try:
        # Import only controller-owned code, never modules from the snapshot.
        from snapshot import prepare
        metadata = prepare(args.fixture, args.catalog, args.case, args.revision, args.output / 'workspace')
        for name in ('data', 'runtime-state'):
            (args.output / name).mkdir()
        command = ['/usr/bin/bwrap', '--unshare-all', '--die-with-parent', '--new-session',
                   '--cap-drop', 'ALL', '--clearenv', *runtime_roots(),
                   '--proc', '/proc', '--dev', '/dev', '--tmpfs', '/tmp',
                   '--ro-bind', str(Path(__file__).resolve().parent), '/trusted',
                   '--bind', str(args.output / 'workspace'), '/workspace',
                   '--bind', str(evidence), '/evidence',
                   '--bind', str(args.output / 'data'), '/data',
                   '--bind', str(args.output / 'runtime-state'), '/runtime-state',
                   '--ro-bind', str(args.dependencies), '/dependencies',
                   '--ro-bind', str(args.browser_dependencies), '/browser-deps',
                   '--ro-bind', str(args.browsers), '/browsers', '--dir', '/runtime']
        for name in ('node', 'iii', 'pnpm'):
            command += ['--ro-bind', str(getattr(args, name)), '/runtime/' + name]
        for path in ('/etc/fonts', '/etc/ld.so.cache'):
            if Path(path).exists():
                command += ['--ro-bind', path, path]
        for key, value in {**ENV, 'PLAYWRIGHT_MODULE': '/browser-deps/' + args.playwright_module}.items():
            command += ['--setenv', key, value]
        metadata['runtime_sha256'] = {name: file_digest(getattr(args, name)) for name in ('node', 'iii', 'pnpm')}
        metadata['dependency_lock_sha256'] = file_digest(args.dependencies / '.pnpm/lock.yaml')
        metadata['runtime_trees_sha256'] = {name: tree_digest(getattr(args, name)) for name in ('dependencies', 'browser_dependencies', 'browsers')}
        metadata['controller_sha256'] = {path.name: file_digest(path) for path in Path(__file__).parent.glob('*') if path.is_file()}
        metadata['model_execution'] = False
        (evidence / 'provenance.json').write_text(json.dumps(metadata, indent=2))
        code = bounded(command + ['/usr/bin/python3', '-I', '/trusted/run.py', '--inside', args.case],
                       evidence / 'controller.log', 420)
        if not (evidence / 'result.json').exists():
            raise RuntimeError(f'isolated evaluator exited {code} without a verdict')
        result = json.loads((evidence / 'result.json').read_text())
        print(json.dumps({'output': str(args.output), 'result': result}))
        if result.get('status') in ('infrastructure_failed', 'evaluation_failed'):
            return 2
        return 0 if result.get('functional_status') == 'passed' else 1
    except Exception as error:
        result = {'schema': 'kanban-evaluation/v1', 'case_id': args.case,
                  'status': 'infrastructure_failed', 'functional_status': None,
                  'error': str(error), 'checks': []}
        (evidence / 'result.json').write_text(json.dumps(result, indent=2))
        print(json.dumps(result))
        return 2


if __name__ == '__main__':
    sys.exit(main())
