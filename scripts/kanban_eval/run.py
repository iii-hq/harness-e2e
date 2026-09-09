#!/usr/bin/env python3
"""Run trusted Kanban control probes in Docker-native isolation.

This is a local control runner, not a model campaign or a registered ScenarioId.
All runtime inputs are administrator-selected, never supplied by candidate code.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import signal
import subprocess
import sys
import time

ENV = {'PATH': '/runtime:/usr/bin:/bin', 'HOME': '/tmp', 'LANG': 'C.UTF-8',
       'LC_ALL': 'C.UTF-8', 'III_TELEMETRY_ENABLED': 'false', 'OTEL_ENABLED': 'false',
       'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': '/dev/null',
       'GIT_TERMINAL_PROMPT': '0', 'CI': 'true'}


class InfrastructureError(RuntimeError):
    pass


class EvaluationError(RuntimeError):
    pass


def limit_log_size():
    resource.setrlimit(resource.RLIMIT_FSIZE, (16 * 1024 ** 2, 16 * 1024 ** 2))


def mount(source, target, writable=False):
    value = f'type=bind,src={source},dst={target}'
    return ['--mount', value if writable else value + ',readonly']


def container_command(image, network, role, mounts, environment=ENV, bounded_workspace=False):
    command = [
        'docker', 'run', '--detach', '--pull', 'never', '--network', network,
        '--label', f'kanban-eval.role={role}',
        '--workdir', '/workspace' if role == 'candidate' else '/tmp',
        '--entrypoint', '/bin/sh',
        '--read-only', '--user', f'{os.getuid()}:{os.getgid()}',
        '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges=true',
        '--pids-limit', '256', '--memory', '2g', '--cpus', '2',
        '--tmpfs', '/tmp:rw,nosuid,nodev,size=256m',
    ]
    if bounded_workspace:
        for path, size in (('/workspace', '256m'), ('/data', '64m'), ('/runtime-state', '64m')):
            command += ['--tmpfs', f'{path}:rw,nosuid,nodev,size={size},uid={os.getuid()},gid={os.getgid()}']
    for key, value in environment.items():
        command += ['--env', f'{key}={value}']
    for source, target, writable in mounts:
        command += mount(source, target, writable)
    return command + [image, '-c', 'exec sleep infinity']


def docker_exec(container, command):
    return ['docker', 'exec', container, *command]


def remove_container(container):
    subprocess.run(['docker', 'rm', '-f', container], stdin=subprocess.DEVNULL,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False)


def bounded(command, log, timeout, container=None):
    with open(log, 'wb') as output:
        proc = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=output,
                                stderr=subprocess.STDOUT, start_new_session=True,
                                preexec_fn=limit_log_size)
        try:
            return proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            if container:
                remove_container(container)
            raise
        finally:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            proc.wait()


def start_container(command):
    completed = subprocess.run(command, check=False, stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    container = completed.stdout.strip()
    if completed.returncode or not container:
        raise InfrastructureError(completed.stderr.strip() or 'Docker did not return a container ID')
    return container


def probe_result(evidence, case_id, returncode):
    if returncode or not all((evidence / name).is_file() for name in ('result.json', 'coverage.json')):
        raise RuntimeError('trusted probe failed or returned incomplete evidence')
    result = json.loads((evidence / 'result.json').read_text())
    coverage = json.loads((evidence / 'coverage.json').read_text())
    checks = result.get('checks', [])
    criteria = [check for check in checks if check['id'].startswith('criterion_')]
    status = result.get('status')
    functional = result.get('functional_status')
    if (result.get('schema') != 'kanban-evaluation/v1'
            or coverage.get('schema') != 'kanban-evaluation-coverage/v1'
            or result.get('case_id') != case_id or coverage.get('case_id') != case_id
            or coverage.get('criteria') != criteria
            or any(check['status'] not in ('passed', 'failed', 'unverified') for check in checks)
            or len({check['id'] for check in checks}) != len(checks)):
        raise RuntimeError('trusted probe returned inconsistent coverage')
    if status == 'evaluation_failed':
        valid = functional is None and coverage.get('complete') is False
    else:
        expected_functional = 'failed' if any(check['status'] == 'failed' for check in checks) else 'passed'
        complete = bool(criteria) and not any(check['status'] == 'unverified' for check in checks)
        expected_status = 'failed' if expected_functional == 'failed' else 'passed' if complete else 'incomplete'
        valid = bool(checks) and functional == expected_functional and status == expected_status and coverage.get('complete') is complete
    if not valid:
        raise RuntimeError('trusted probe returned inconsistent verdict')
    return result


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
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--fixture', type=Path, required=True)
    parser.add_argument('--catalog', type=Path, required=True)
    parser.add_argument('--case', required=True)
    parser.add_argument('--revision', choices=['base', 'reference'], required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--image', required=True)
    parser.add_argument('--subject-model', choices=['deepseek-v4-flash'])
    parser.add_argument('--subject-url')
    parser.add_argument('--subject-namespace')
    for name in ('node', 'iii', 'pnpm', 'dependencies', 'browser-dependencies', 'browsers'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--playwright-module', default='.pnpm/playwright@1.61.1/node_modules/playwright/index.mjs')
    args = parser.parse_args()
    if args.subject_model and (args.revision != 'base' or not args.subject_url or not args.subject_namespace):
        parser.error('subject execution requires --revision base, --subject-url and --subject-namespace')
    args.output = args.output.resolve()
    if args.output.exists():
        parser.error('--output must be new; never reuse or overwrite an execution')
    for name in ('fixture', 'catalog', 'node', 'iii', 'pnpm', 'dependencies', 'browser_dependencies', 'browsers'):
        setattr(args, name, getattr(args, name).resolve(strict=True))
    args.output.mkdir(parents=True, mode=0o700)
    evidence = args.output / 'evidence'
    evidence.mkdir(mode=0o700)
    containers = []
    try:
        from snapshot import prepare
        metadata = prepare(args.fixture, args.catalog, args.case, args.revision, args.output / 'workspace')
        for name in ('data', 'runtime-state'):
            (args.output / name).mkdir()
        inspected = subprocess.run(['docker', 'image', 'inspect', '--format', '{{.Id}}', args.image],
                                   check=False, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, text=True)
        image = inspected.stdout.strip()
        if inspected.returncode or not image.startswith('sha256:') or '\n' in image:
            raise InfrastructureError(inspected.stderr.strip() or 'image must resolve to one existing local Docker image')

        runtime_mounts = [(root, root, False) for root in ('/usr', '/bin', '/lib', '/lib64')
                          if Path(root).exists()]
        runtime_mounts += [(str(getattr(args, name)), '/runtime/' + name, False)
                          for name in ('node', 'iii', 'pnpm')]
        candidate_mounts = ([] if args.subject_model else [
            (str(args.output / 'workspace'), '/workspace', True),
            (str(args.output / 'data'), '/data', True),
            (str(args.output / 'runtime-state'), '/runtime-state', True),
        ]) + [
            (str(args.dependencies), '/dependencies' if args.subject_model else '/workspace/kanban/node_modules', False),
            *runtime_mounts,
        ]
        candidate = start_container(container_command(image, 'none', 'candidate', candidate_mounts,
                                                      bounded_workspace=bool(args.subject_model)))
        containers.append(candidate)
        if args.subject_model:
            archive = subprocess.check_output(['git', '-C', str(args.output / 'workspace'), 'archive', 'HEAD'])
            subprocess.run(['docker', 'exec', '-i', candidate, 'tar', '--no-same-owner', '-xf', '-', '-C', '/workspace'],
                           input=archive, check=True, timeout=30)
            subprocess.run(docker_exec(candidate, ['/bin/sh', '-c',
                           'mkdir -p /workspace/kanban && ln -s /dependencies /workspace/kanban/node_modules']),
                           check=True, timeout=10)
            baseline = '''set -eu
git init --quiet --initial-branch=main --template= /workspace
git -C /workspace add --all -- . ':!kanban/node_modules'
GIT_AUTHOR_NAME='Harness Snapshot' GIT_AUTHOR_EMAIL=snapshot@harness.invalid GIT_COMMITTER_NAME='Harness Snapshot' GIT_COMMITTER_EMAIL=snapshot@harness.invalid GIT_AUTHOR_DATE='2000-01-01T00:00:00Z' GIT_COMMITTER_DATE='2000-01-01T00:00:00Z' git -C /workspace -c commit.gpgsign=false -c core.hooksPath=/dev/null commit --quiet -m 'Initialize scenario snapshot'
git -C /workspace rev-parse HEAD
'''
            head = subprocess.check_output(docker_exec(candidate, ['/bin/sh', '-c', baseline]), timeout=10).decode().strip()
            if head != metadata['snapshot_git_head']:
                raise InfrastructureError('candidate baseline commit differs from exported snapshot')

        probe_env = {**ENV, 'III_SDK_MODULE': '/dependencies/iii-sdk/dist/index.mjs',
                     'PLAYWRIGHT_MODULE': '/browser-deps/' + args.playwright_module,
                     'PLAYWRIGHT_BROWSERS_PATH': '/browsers'}
        evaluator_mounts = [
            (str(Path(__file__).resolve().parent), '/trusted', False),
            (str(evidence), '/evidence', True),
            (str(args.dependencies), '/dependencies', False),
            (str(args.browser_dependencies), '/browser-deps', False),
            (str(args.browsers), '/browsers', False),
            *runtime_mounts,
        ]
        for path in ('/etc/fonts', '/etc/ld.so.cache'):
            if Path(path).exists():
                evaluator_mounts.append((path, path, False))
        evaluator = start_container(container_command(image, f'container:{candidate}', 'evaluator', evaluator_mounts, probe_env))
        containers.append(evaluator)

        metadata['container_image_id'] = image
        metadata['runtime_sha256'] = {name: file_digest(getattr(args, name)) for name in ('node', 'iii', 'pnpm')}
        metadata['dependency_lock_sha256'] = file_digest(args.dependencies / '.pnpm/lock.yaml')
        metadata['runtime_trees_sha256'] = {name: tree_digest(getattr(args, name)) for name in ('dependencies', 'browser_dependencies', 'browsers')}
        metadata['controller_sha256'] = {path.name: file_digest(path) for path in Path(__file__).parent.glob('*') if path.is_file()}
        metadata['model_execution'] = False
        (evidence / 'provenance.json').write_text(json.dumps(metadata, indent=2))
        (evidence / 'private-canary').write_text('private control state')

        check = """import pathlib,socket
assert pathlib.Path('/workspace/README.md').is_file()
for p in ['/trusted','/evidence','/browser-deps','/browsers','/var/run/docker.sock']:
 assert not pathlib.Path(p).exists(),p
assert not pathlib.Path('/proc/1/root/evidence').exists()
s=socket.socket();s.settimeout(.2)
assert s.connect_ex(('1.1.1.1',443)) != 0
print('workspace readable; trusted files, evaluator process and external network inaccessible')
"""
        if bounded(docker_exec(candidate, ['/usr/bin/python3', '-I', '-c', check]),
                   evidence / 'isolation.log', 10, candidate):
            raise InfrastructureError('candidate isolation preflight failed; see isolation.log')

        if args.subject_model:
            prompt_file = evidence / 'subject-prompt.txt'
            prompt_file.write_text(metadata['prompt'])
            subject_env = {'III_SDK_MODULE': str(args.dependencies / 'iii-sdk/dist/index.mjs')}
            command = ['/usr/bin/env', *(f'{key}={value}' for key, value in subject_env.items()),
                       str(args.node), str(Path(__file__).with_name('subject.mjs')),
                       '--container', candidate, '--prompt-file', str(prompt_file), '--output', str(evidence),
                       '--engine-url', args.subject_url, '--namespace', args.subject_namespace,
                       '--provider', 'deepseek', '--model', args.subject_model]
            code = bounded(command, evidence / 'subject.log', 2050, candidate)
            if (evidence / 'subject.json').is_file():
                subject = json.loads((evidence / 'subject.json').read_text())
                metadata['model_execution'] = subject.get('model_invoked', False)
                (evidence / 'provenance.json').write_text(json.dumps(metadata, indent=2))
            if code or not (evidence / 'subject.json').is_file():
                raise EvaluationError('subject execution did not complete; see subject.log and subject.json')
            stop_children = """import os,signal
for name in os.listdir('/proc'):
 if name.isdigit() and int(name) not in (1,os.getpid()):
  try: os.kill(int(name),signal.SIGKILL)
  except ProcessLookupError: pass
"""
            if bounded(docker_exec(candidate, ['/usr/bin/python3', '-I', '-c', stop_children]),
                       evidence / 'subject-cleanup.log', 10, candidate):
                raise EvaluationError('candidate background process cleanup failed')
            if bounded(docker_exec(candidate, ['git', '-C', '/workspace', 'add', '--all', '--', '.',
                                            ':!kanban/node_modules', ':!kanban/dist', ':!data']),
                       evidence / 'subject-stage.log', 10, candidate):
                raise EvaluationError('candidate change capture failed')
            if bounded(docker_exec(candidate, ['git', '-C', '/workspace', 'diff', '--cached', '--no-ext-diff', 'HEAD']),
                       evidence / 'subject.diff', 10, candidate):
                raise EvaluationError('candidate diff capture failed')

        build_checks = []
        result = {'schema': 'kanban-evaluation/v1', 'case_id': args.case,
                  'status': 'failed', 'functional_status': 'failed', 'checks': build_checks}
        if subprocess.run(docker_exec(candidate, ['test', '-f', '/workspace/kanban/package.json']), check=False).returncode:
            build_checks.append({'id': 'application_present', 'status': 'failed',
                                 'detail': 'The selected snapshot contains no application.'})
            (evidence / 'result.json').write_text(json.dumps(result))
        else:
            for step in ('typecheck', 'test', 'build'):
                code = bounded(docker_exec(candidate, ['pnpm', '--dir', 'kanban', step]),
                               evidence / f'{step}.log', 90, candidate)
                build_checks.append({'id': step, 'status': 'passed' if code == 0 else 'failed',
                                     'detail': f'isolated command exit {code}'})
                if code:
                    (evidence / 'result.json').write_text(json.dumps(result))
                    break
            else:
                config = {'workers': [
                    {'name': 'iii-worker-manager', 'config': {'host': '127.0.0.1', 'port': 50179}},
                    {'name': 'configuration', 'config': {'adapter': {'name': 'fs', 'config': {'directory': '/runtime-state/configuration'}}}},
                ]}
                (args.output / 'runtime-state/config.json').write_text(json.dumps(config))
                if args.subject_model:
                    subprocess.run(['docker', 'exec', '-i', candidate, '/usr/bin/tee', '/runtime-state/config.json'],
                                   input=json.dumps(config).encode(), stdout=subprocess.DEVNULL, check=True, timeout=10)
                start = '''set -eu
iii --no-update-check --config /runtime-state/config.json &
sleep 1
iii trigger configuration::register --address 127.0.0.1 --port 50179 --namespace default --json '{"id":"kanban","name":"Kanban","description":"Evaluation data","schema":{"type":"object","properties":{"data_dir":{"type":"string"}},"required":["data_dir"]},"initial_value":{"data_dir":"/data","preserve_me":true}}'
exec iii compose --up --engine ws://127.0.0.1:50179 --file /workspace/worker-compose.yaml
'''
                with (evidence / 'runtime.log').open('wb') as runtime_log:
                    process = subprocess.Popen(docker_exec(candidate, ['/bin/sh', '-c', start]),
                                               stdin=subprocess.DEVNULL, stdout=runtime_log,
                                               stderr=subprocess.STDOUT, start_new_session=True,
                                               preexec_fn=limit_log_size)
                    started = time.monotonic()
                    ready_check = "import urllib.request; assert urllib.request.urlopen('http://127.0.0.1:3000/api/config',timeout=.3).status == 200"
                    ready = False
                    healthy_samples = 0
                    for _ in range(120):
                        if process.poll() is not None:
                            break
                        ready = subprocess.run(docker_exec(evaluator, ['/usr/bin/python3', '-I', '-c', ready_check]),
                                               stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                               stderr=subprocess.DEVNULL, check=False).returncode == 0
                        healthy_samples = healthy_samples + 1 if ready else 0
                        if healthy_samples >= 12:
                            break
                        time.sleep(.25)
                    ready = healthy_samples >= 12
                    if not ready:
                        build_checks.append({'id': 'application_startup', 'status': 'failed',
                                             'detail': 'Compose application did not become ready; see runtime.log.'})
                        (evidence / 'result.json').write_text(json.dumps(result))
                    else:
                        probe = docker_exec(evaluator, ['/runtime/node', '/trusted/probe.mjs', '--case', args.case,
                                            '--base-url', 'http://127.0.0.1:3000', '--engine-url', 'ws://127.0.0.1:50179',
                                            '--output', '/evidence'])
                        try:
                            code = bounded(probe, evidence / 'probe.log', 120, evaluator)
                            result = probe_result(evidence, args.case, code)
                        except (RuntimeError, subprocess.TimeoutExpired) as error:
                            raise EvaluationError(str(error)) from error
                        result['build_checks'] = build_checks
                        result['duration_ms'] = round((time.monotonic() - started) * 1000)
                        (evidence / 'result.json').write_text(json.dumps(result, indent=2))

        result = json.loads((evidence / 'result.json').read_text())
        print(json.dumps({'output': str(args.output), 'result': result}))
        if result.get('status') in ('infrastructure_failed', 'evaluation_failed'):
            return 2
        return 0 if result.get('functional_status') == 'passed' else 1
    except (Exception, KeyboardInterrupt) as error:
        result = {'schema': 'kanban-evaluation/v1', 'case_id': args.case,
                  'status': 'evaluation_failed' if isinstance(error, EvaluationError) else 'infrastructure_failed',
                  'functional_status': None,
                  'error': str(error) or type(error).__name__, 'checks': []}
        (evidence / 'result.json').write_text(json.dumps(result, indent=2))
        print(json.dumps(result))
        return 2
    finally:
        for container in reversed(containers):
            remove_container(container)


if __name__ == '__main__':
    sys.exit(main())
