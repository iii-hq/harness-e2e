#!/usr/bin/env python3
"""Run trusted Kanban probes in Docker-native isolation.

Supports controls, standalone model smoke tests, and an external Harness subject.
All runtime inputs are administrator-selected, never supplied by candidate code.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import resource
import signal
import subprocess
import sys
import time

ENV = {'PATH': '/runtime:/usr/bin:/bin', 'HOME': '/tmp', 'LANG': 'C.UTF-8',
       'LC_ALL': 'C.UTF-8', 'III_TELEMETRY_ENABLED': 'false', 'OTEL_ENABLED': 'false',
       'III_COMPOSE_STATE_DIR': '/runtime-state/compose',
       'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': '/dev/null',
       'GIT_TERMINAL_PROMPT': '0', 'CI': 'true'}

RUNTIME_ENGINE = '''set -eu
iii --no-update-check --config /runtime-state/config.json &
sleep 1
'''
RUNTIME_REGISTER = '''
iii trigger configuration::register --address 127.0.0.1 --port 50179 --namespace default --json '{"id":"kanban","name":"Kanban","description":"Evaluation data","schema":{"type":"object","properties":{"data_dir":{"type":"string"}},"required":["data_dir"]},"initial_value":{"data_dir":"/data","preserve_me":true}}'
'''
RUNTIME_COMPOSE = '''
exec iii compose --up --engine ws://127.0.0.1:50179 --file /workspace/worker-compose.yaml
'''

STOP_RUNTIME = '''import os,signal,sys,time
keeper=int(sys.argv[1])
try: assert open(f'/proc/{keeper}/cmdline','rb').read()==b'sleep\\0infinity\\0'
except (FileNotFoundError,ProcessLookupError): raise RuntimeError('container keeper process is unavailable')
keep={1,os.getpid(),keeper}
for _ in range(100):
 targets=[int(name) for name in os.listdir('/proc') if name.isdigit() and int(name) not in keep]
 if not targets: break
 for pid in targets:
  try: os.kill(pid,signal.SIGKILL)
  except ProcessLookupError: pass
 time.sleep(.01)
else: raise RuntimeError('candidate processes did not stop')
'''

READY_CHECK = "import urllib.request; assert urllib.request.urlopen('http://127.0.0.1:3000/api/config',timeout=.3).status == 200"

HOT_RELOAD_PREPARE = r'''import json,pathlib,sys
root=pathlib.Path(sys.argv[1]);backup=pathlib.Path(sys.argv[2]);marker=sys.argv[3]
excluded={'.git','dist','node_modules'};candidates=[]
for path in root.rglob('*'):
 try: relative=path.relative_to(root)
 except ValueError: continue
 if path.suffix not in ('.ts','.mts','.cts') or path.name.endswith('.d.ts'): continue
 if any(part in excluded for part in relative.parts): continue
 current=root
 if any((current:=current/part).is_symlink() for part in relative.parts): continue
 if path.is_file(): candidates.append((relative,path))
candidates.sort(key=lambda item:(item[0].parts[:1]!=('src',),str(item[0])))
selected=[];total=0
for relative,path in candidates:
 data=path.read_bytes()
 if len(selected)>=100 or total+len(data)>4*1024*1024: break
 selected.append((relative,path,data));total+=len(data)
backup.mkdir()
manifest=[]
for number,(relative,path,data) in enumerate(selected):
 name=str(number);(backup/name).write_bytes(data);manifest.append({'path':str(relative),'backup':name})
(backup/'manifest.json').write_text(json.dumps(manifest))
line=b'\nconsole.log('+json.dumps(marker).encode()+b');\n'
for _relative,path,data in selected:path.write_bytes(data+line)
print(json.dumps({'count':len(selected)}))
'''

HOT_RELOAD_RESTORE = r'''import json,pathlib,shutil,sys
root=pathlib.Path(sys.argv[1]);backup=pathlib.Path(sys.argv[2]);excluded={'.git','dist','node_modules'}
if backup.is_dir():
 manifest=json.loads((backup/'manifest.json').read_text())
 for item in manifest:
  relative=pathlib.PurePosixPath(item['path']);path=root.joinpath(*relative.parts)
  assert not relative.is_absolute() and '..' not in relative.parts
  assert isinstance(item['backup'],str) and item['backup'].isdigit()
  assert not any(part in excluded for part in relative.parts)
  current=root
  assert not any((current:=current/part).is_symlink() for part in relative.parts)
  assert path.is_file()
  path.write_bytes((backup/item['backup']).read_bytes())
 shutil.rmtree(backup)
'''


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
        'docker', 'run', '--detach', '--init', '--pull', 'never', '--network', network,
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
    for key, value in {**environment, 'III_TELEMETRY_ENABLED': 'false'}.items():
        command += ['--env', f'{key}={value}']
    for source, target, writable in mounts:
        command += mount(source, target, writable)
    return command + [image, '-c', 'exec sleep infinity']


def runtime_mounts(args):
    return [(str(getattr(args, name)), '/runtime/' + name, False)
            for name in ('node', 'iii', 'pnpm')]


def docker_exec(container, command):
    return ['docker', 'exec', '--env', 'III_TELEMETRY_ENABLED=false', container, *command]


def stage_changes_command(workspace):
    return ['/bin/sh', '-c', '''set -eu
git -C "$1" add --all --force -- . ':!kanban/node_modules' ':!kanban/dist' ':!data'
git -C "$1" reset --quiet HEAD -- kanban/node_modules kanban/dist data
''', 'stage-changes', str(workspace)]


def capture_candidate_diff(candidate, evidence, name, cancel=None):
    if bounded(docker_exec(candidate, stage_changes_command('/workspace')),
               evidence / f'{name}-stage.log', 10, candidate, cancel):
        raise EvaluationError('candidate change capture failed')
    if bounded(docker_exec(candidate, ['git', '-C', '/workspace', 'diff', '--cached',
                                       '--no-ext-diff', '--binary', 'HEAD']),
               evidence / f'{name}.diff', 10, candidate, cancel):
        raise EvaluationError('candidate diff capture failed')
    if bounded(docker_exec(candidate, ['git', '-C', '/workspace', 'write-tree']),
               evidence / f'{name}.tree', 10, candidate, cancel):
        raise EvaluationError('candidate source tree capture failed')


def verify_candidate_source(candidate, evidence, cancel=None):
    trees = [(evidence / f'{name}.tree').read_text().strip() for name in ('subject', 'evaluated')]
    if not all(re.fullmatch(r'(?:[0-9a-f]{40}|[0-9a-f]{64})', tree) for tree in trees):
        raise EvaluationError('candidate source tree capture is invalid')
    different_trees = trees[0] != trees[1]
    paths = []
    generated = []
    if different_trees:
        listing = evidence / 'source-changes.paths'
        if bounded(docker_exec(candidate, ['git', '-C', '/workspace', 'diff', '--name-only',
                                           '--no-renames', '-z', *trees]),
                   listing, 10, candidate, cancel):
            raise EvaluationError('candidate changed-path capture failed')
        paths = listing.read_bytes().decode(errors='replace').rstrip('\0').split('\0')
        # Compose writes this canonical empty lock even for path-only projects.
        # Existing locks and locks recording dependencies remain source inputs.
        if 'worker-compose.lock' in paths and new_empty_compose_lock(candidate, evidence, trees, cancel):
            paths.remove('worker-compose.lock')
            generated.append('worker-compose.lock')
    changed = bool(paths)
    atomic_json(evidence / 'source-integrity.json', {
        'subject_tree': trees[0], 'evaluated_tree': trees[1], 'changed': changed,
        'changed_paths': paths, 'generated_paths': generated,
        'subject_diff_empty': not (evidence / 'subject.diff').stat().st_size,
    })
    if changed:
        raise EvaluationError('candidate source changed during evaluation: ' + ', '.join(paths[:10]))


def new_empty_compose_lock(candidate, evidence, trees, cancel=None):
    entries = []
    for name, tree in zip(('subject', 'evaluated'), trees):
        listing = evidence / f'{name}-compose-lock.tree'
        if bounded(docker_exec(candidate, ['git', '-C', '/workspace', 'ls-tree', tree, '--', 'worker-compose.lock']),
                   listing, 10, candidate, cancel):
            raise EvaluationError('Compose lock provenance capture failed')
        entries.append(listing.read_text().strip())
    if entries[0] or not entries[1].startswith('100644 blob '):
        return False
    content = evidence / 'generated-compose-lock.txt'
    if bounded(docker_exec(candidate, ['git', '-C', '/workspace', 'show', f'{trees[1]}:worker-compose.lock']),
               content, 10, candidate, cancel):
        raise EvaluationError('Compose lock content capture failed')
    return content.read_bytes() == b'version: 1\ncontainers: {}\n'


def write_failure(evidence, case, error):
    result_path = evidence / 'result.json'
    if result_path.is_file():
        try:
            provisional = json.loads(result_path.read_text())
        except (ValueError, OSError):
            provisional = {}
        if isinstance(provisional, dict) and provisional.get('functional_status') in ('passed', 'failed'):
            atomic_json(evidence / 'functional-result.json', provisional)
    result = {'schema': 'kanban-evaluation/v1', 'case_id': case,
              'status': 'evaluation_failed' if isinstance(error, EvaluationError) else 'infrastructure_failed',
              'functional_status': None, 'error': str(error) or type(error).__name__, 'checks': []}
    atomic_json(result_path, result)
    return result


def remove_container(container):
    try:
        subprocess.run(['docker', 'rm', '-f', container], stdin=subprocess.DEVNULL,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                       check=False, timeout=15)
    except (OSError, subprocess.TimeoutExpired):
        pass


def bounded(command, log, timeout, container=None, cancel=None, poll=None):
    with open(log, 'wb') as output:
        proc = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=output,
                                stderr=subprocess.STDOUT, start_new_session=True,
                                env={**os.environ, 'III_TELEMETRY_ENABLED': 'false'},
                                preexec_fn=limit_log_size)
        deadline = time.monotonic() + timeout
        try:
            while True:
                if cancel and cancel.is_file():
                    raise KeyboardInterrupt('external subject cancelled')
                if poll:
                    poll()
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    if container:
                        remove_container(container)
                    raise subprocess.TimeoutExpired(command, timeout)
                try:
                    return proc.wait(timeout=min(remaining, .25 if cancel or poll else remaining))
                except subprocess.TimeoutExpired:
                    pass
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


def container_keeper(container):
    script = """import pathlib
children=pathlib.Path('/proc/1/task/1/children').read_text().split()
assert len(children)==1,children
assert pathlib.Path(f'/proc/{children[0]}/cmdline').read_bytes()==b'sleep\\0infinity\\0'
print(children[0])
"""
    completed = subprocess.run(docker_exec(container, ['/usr/bin/python3', '-I', '-c', script]),
                               stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, check=False, text=True, timeout=10)
    keeper = completed.stdout.strip()
    if completed.returncode or not keeper.isdigit() or int(keeper) <= 1:
        raise InfrastructureError('cannot identify container keeper process: '
                                  + completed.stderr.strip()[-4096:])
    return keeper


def atomic_json(path, value):
    temporary = path.with_name(path.name + '.tmp')
    temporary.write_text(json.dumps(value, indent=2))
    os.replace(temporary, path)


def wait_for_external_subject(output, ready, timeout=1800):
    atomic_json(output / 'ready.json', ready)
    deadline = time.monotonic() + timeout
    while True:
        if (output / 'cancel').is_file():
            raise KeyboardInterrupt('external subject cancelled')
        if (output / 'subject-complete').is_file():
            try:
                complete = json.loads((output / 'subject-complete').read_text())
            except (OSError, json.JSONDecodeError) as error:
                raise EvaluationError('external subject completion marker is invalid') from error
            if not isinstance(complete, dict) or type(complete.get('model_invoked')) is not bool:
                raise EvaluationError('external subject completion marker is invalid')
            return complete
        if time.monotonic() >= deadline:
            raise EvaluationError(f'external subject did not complete within {timeout:g} seconds')
        time.sleep(.25)


def start_runtime(candidate, runtime_log, register_configuration=True):
    command = RUNTIME_ENGINE + (RUNTIME_REGISTER if register_configuration else '') + RUNTIME_COMPOSE
    with runtime_log.open('ab') as output:
        return subprocess.Popen(docker_exec(candidate, ['/bin/sh', '-c', command]),
                                stdin=subprocess.DEVNULL, stdout=output,
                                stderr=subprocess.STDOUT, start_new_session=True,
                                preexec_fn=limit_log_size)


def wait_runtime_ready(process, evaluator, cancel=None, timeout=30):
    healthy = 0
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if cancel and cancel.is_file():
            raise KeyboardInterrupt('external subject cancelled')
        if process.poll() is not None:
            break
        ready = subprocess.run(docker_exec(evaluator, ['/usr/bin/python3', '-I', '-c', READY_CHECK]),
                               stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL, check=False, timeout=2).returncode == 0
        healthy = healthy + 1 if ready else 0
        if healthy >= 12:
            return True
        time.sleep(.25)
    return False


def restart_runtime(candidate, evaluator, evidence, runtime, cancel=None,
                    register_configuration=False, reset_configuration=False):
    number = runtime[1] + 1
    if bounded(docker_exec(candidate, ['/usr/bin/python3', '-I', '-c', STOP_RUNTIME, runtime[2]]),
               evidence / f'control-restart-{number}.log', 10, candidate, cancel):
        raise RuntimeError('runtime shutdown failed')
    try:
        runtime[0].wait(timeout=5)
    except subprocess.TimeoutExpired:
        runtime[0].kill()
        runtime[0].wait()
    if reset_configuration:
        reset = """import pathlib,shutil
p=pathlib.Path('/runtime-state/configuration')
if p.is_symlink() or p.is_file(): p.unlink()
elif p.exists(): shutil.rmtree(p)
p.mkdir(parents=True)
"""
        if bounded(docker_exec(candidate, ['/usr/bin/python3', '-I', '-c', reset]),
                   evidence / f'control-reset-{number}.log', 10, candidate, cancel):
            raise RuntimeError('configuration reset failed')
    runtime[0] = start_runtime(candidate, evidence / 'runtime.log', register_configuration)
    runtime[1] = number
    if not wait_runtime_ready(runtime[0], evaluator, cancel, timeout=60):
        raise RuntimeError('runtime did not become ready after restart')
    return {'ready': True}


def hot_reload(candidate, evaluator, evidence, runtime, cancel=None):
    token = os.urandom(16).hex()
    marker = f'KANBAN_HOT_RELOAD_{token}'
    root = '/workspace/kanban'
    backup = f'/tmp/kanban-hot-reload-{token}'
    logs = docker_exec(candidate, ['/runtime/iii', 'trigger', 'compose::logs',
                       '--address', '127.0.0.1', '--port', '50179', '--namespace', 'default',
                       '--json', json.dumps({'file': '/workspace/worker-compose.yaml', 'tail': 1000})])
    logs_path = evidence / 'hot-reload-compose.log'
    successful_log_queries = 0
    observed = False
    try:
        try:
            completed = subprocess.run(docker_exec(candidate, ['/usr/bin/python3', '-I', '-c',
                                                               HOT_RELOAD_PREPARE, root, backup, marker]),
                                       stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                       stderr=subprocess.PIPE, check=False, timeout=10)
        except subprocess.TimeoutExpired as error:
            raise InfrastructureError('hot reload instrumentation timed out') from error
        if completed.returncode:
            raise InfrastructureError('hot reload instrumentation failed: '
                                      + completed.stderr.decode(errors='replace')[-4096:])
        try:
            prepared = json.loads(completed.stdout)
        except (TypeError, ValueError, json.JSONDecodeError) as error:
            raise InfrastructureError('hot reload instrumentation returned invalid output') from error
        if prepared == {'count': 0}:
            return {'observed': False, 'source_restored': True}
        if not isinstance(prepared, dict) or not isinstance(prepared.get('count'), int):
            raise InfrastructureError('hot reload instrumentation returned invalid output')
        deadline = time.monotonic() + 55
        while time.monotonic() < deadline:
            if cancel and cancel.is_file():
                raise KeyboardInterrupt('external subject cancelled')
            if runtime[0].poll() is not None:
                break
            try:
                code = bounded(logs, logs_path, 5, cancel=cancel)
            except subprocess.TimeoutExpired:
                code = None
            if code == 0:
                successful_log_queries += 1
                if marker.encode() in logs_path.read_bytes():
                    observed = True
                    break
            time.sleep(.25)
    finally:
        try:
            restored = subprocess.run(docker_exec(candidate, ['/usr/bin/python3', '-I', '-c',
                                                              HOT_RELOAD_RESTORE, root, backup]),
                                      stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                      stderr=subprocess.PIPE, check=False, timeout=10)
        except subprocess.TimeoutExpired as error:
            raise InfrastructureError('hot reload source restoration timed out') from error
        if restored.returncode:
            raise InfrastructureError('hot reload source restoration failed: '
                                      + restored.stderr.decode(errors='replace')[-4096:])
    if not successful_log_queries:
        raise InfrastructureError('Compose worker logs were unavailable during hot reload')
    recovered = wait_runtime_ready(runtime[0], evaluator, cancel, timeout=10)
    return {'observed': observed and recovered, 'source_restored': True}


def inspect_runtime(candidate, evaluator, cancel=None):
    signal_server = """import os,pathlib,signal
inodes=set()
for table in ('/proc/net/tcp','/proc/net/tcp6'):
 for line in pathlib.Path(table).read_text().splitlines()[1:]:
  fields=line.split()
  if len(fields)>9 and fields[3]=='0A' and int(fields[1].split(':')[1],16)==3000:
   inodes.add(fields[9])
owners=[]
for process in pathlib.Path('/proc').iterdir():
 if not process.name.isdigit(): continue
 try:
  if process.stat().st_uid!=os.getuid() or pathlib.Path(os.readlink(process/'exe')).name!='node': continue
  sockets={os.readlink(fd) for fd in (process/'fd').iterdir()}
 except (FileNotFoundError,PermissionError): continue
 if any(f'socket:[{inode}]' in sockets for inode in inodes): owners.append(int(process.name))
assert len(owners)==1,owners
os.kill(owners[0],signal.SIGUSR1)
"""
    signalled = subprocess.run(docker_exec(candidate, ['/usr/bin/python3', '-I', '-c', signal_server]),
                               stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               check=False, timeout=10)
    if signalled.returncode:
        raise InfrastructureError('cannot identify the Node process listening on port 3000: '
                                  + signalled.stderr.decode(errors='replace')[-4096:])
    read_targets = """import sys,urllib.request
r=urllib.request.urlopen('http://127.0.0.1:9229/json/list',timeout=1)
b=r.read(65537);assert len(b)<=65536;sys.stdout.buffer.write(b)
"""
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if cancel and cancel.is_file():
            raise KeyboardInterrupt('external subject cancelled')
        targets = subprocess.run(docker_exec(evaluator, ['/usr/bin/python3', '-I', '-c', read_targets]),
                                 stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 check=False, timeout=3)
        if targets.returncode == 0:
            try:
                values = json.loads(targets.stdout)
                urls = [value['webSocketDebuggerUrl'] for value in values
                        if isinstance(value, dict) and isinstance(value.get('webSocketDebuggerUrl'), str)]
            except (TypeError, ValueError, json.JSONDecodeError) as error:
                raise InfrastructureError('Node inspector returned invalid JSON') from error
            if len(urls) != 1 or not urls[0].startswith('ws://127.0.0.1:9229/') or '\n' in urls[0]:
                raise InfrastructureError('Node inspector returned an invalid debugger target')
            return {'websocket_url': urls[0]}
        time.sleep(.25)
    raise InfrastructureError('Node inspector did not become ready')


def control_callback(evidence, candidate, evaluator, runtime, cancel=None):
    responses = {}

    def poll():
        request_path = evidence / 'control-request.json'
        if not request_path.is_file():
            return
        try:
            raw = request_path.read_text()
            if len(raw.encode()) > 1024 * 1024:
                raise ValueError('request exceeds 1 MiB')
            request = json.loads(raw)
            if not isinstance(request, dict):
                raise ValueError('control request must be an object')
            request_id = request.get('id')
            if not isinstance(request_id, str) or not request_id or len(request_id) > 128:
                raise ValueError('invalid control id')
        except (OSError, ValueError, json.JSONDecodeError) as error:
            atomic_json(evidence / 'control-response.json',
                        {'id': None, 'ok': False, 'error': str(error)})
            return
        if request_id in responses:
            atomic_json(evidence / 'control-response.json', responses[request_id])
            return
        try:
            operation = request.get('operation')
            payload = request.get('payload')
            if not isinstance(payload, dict):
                raise ValueError('control payload must be an object')
            if operation == 'read_store' and not payload:
                script = "import pathlib,sys;p=pathlib.Path('/data/tickets.json');b=p.read_bytes();assert len(b)<=1048576;sys.stdout.buffer.write(b)"
                completed = subprocess.run(docker_exec(candidate, ['/usr/bin/python3', '-I', '-c', script]),
                                           stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                           stderr=subprocess.PIPE, check=False, timeout=90)
                if completed.returncode:
                    raise RuntimeError(completed.stderr.decode(errors='replace')[-4096:])
                value = completed.stdout.decode(errors='replace')
            elif operation == 'write_store' and set(payload) == {'value'} and isinstance(payload['value'], str):
                value_bytes = payload['value'].encode()
                if len(value_bytes) > 1024 * 1024:
                    raise ValueError('store value exceeds 1 MiB')
                script = "import pathlib,sys;pathlib.Path('/data/tickets.json').write_bytes(sys.stdin.buffer.read(1048577))"
                completed = subprocess.run(['docker', 'exec', '-i', candidate, '/usr/bin/python3', '-I', '-c', script],
                                           input=value_bytes, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                           check=False, timeout=90)
                if completed.returncode:
                    raise RuntimeError(completed.stderr.decode(errors='replace')[-4096:])
                value = {'bytes': len(value_bytes)}
            elif operation == 'restart' and not payload:
                value = restart_runtime(candidate, evaluator, evidence, runtime, cancel)
            elif operation == 'restart' and payload == {
                    'register_configuration': False, 'reset_configuration': True}:
                value = restart_runtime(candidate, evaluator, evidence, runtime, cancel,
                                        register_configuration=False, reset_configuration=True)
            elif operation == 'hot_reload' and not payload:
                value = hot_reload(candidate, evaluator, evidence, runtime, cancel)
            elif operation == 'inspect_runtime' and not payload:
                value = inspect_runtime(candidate, evaluator, cancel)
            else:
                raise ValueError(f'unsupported control operation: {operation}')
            response = {'id': request_id, 'ok': True, 'value': value}
        except InfrastructureError:
            raise
        except subprocess.TimeoutExpired as error:
            if operation == 'inspect_runtime':
                raise InfrastructureError('Node inspector control timed out') from error
            response = {'id': request_id, 'ok': False, 'error': str(error)}
        except Exception as error:
            response = {'id': request_id, 'ok': False,
                        'error': str(error) or type(error).__name__}
        responses[request_id] = response
        atomic_json(evidence / 'control-response.json', response)

    return poll


def interrupted(_signum, _frame):
    raise KeyboardInterrupt('SIGTERM')


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
    subject = parser.add_mutually_exclusive_group()
    subject.add_argument('--subject-model', choices=['deepseek-v4-flash'])
    subject.add_argument('--external-subject', action='store_true')
    parser.add_argument('--subject-url')
    parser.add_argument('--subject-namespace')
    for name in ('node', 'iii', 'pnpm', 'dependencies', 'browser-dependencies'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--browsers', required=True)
    parser.add_argument('--playwright-module', default='.pnpm/playwright@1.61.1/node_modules/playwright/index.mjs')
    args = parser.parse_args()
    if args.subject_model and (args.revision != 'base' or not args.subject_url or not args.subject_namespace):
        parser.error('subject execution requires --revision base, --subject-url and --subject-namespace')
    if args.external_subject and args.revision != 'base':
        parser.error('external subject execution requires --revision base')
    if not args.browsers.startswith('/'):
        parser.error('--browsers must be an absolute path inside the pinned image')
    isolated_subject = bool(args.subject_model or args.external_subject)
    args.output = args.output.resolve()
    if args.output.exists():
        parser.error('--output must be new; never reuse or overwrite an execution')
    for name in ('fixture', 'catalog', 'node', 'iii', 'pnpm', 'dependencies', 'browser_dependencies'):
        setattr(args, name, getattr(args, name).resolve(strict=True))
    args.output.mkdir(parents=True, mode=0o700)
    evidence = args.output / 'evidence'
    evidence.mkdir(mode=0o700)
    containers = []
    candidate = None
    delivered_diff_captured = False
    evaluated_diff_captured = False
    cancel = args.output / 'cancel' if args.external_subject else None
    previous_sigterm = signal.signal(signal.SIGTERM, interrupted)
    try:
        if os.getuid() == 0:
            raise InfrastructureError('Kanban isolation requires a non-root host user')
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

        runtime_binary_mounts = runtime_mounts(args)
        candidate_mounts = ([] if isolated_subject else [
            (str(args.output / 'workspace'), '/workspace', True),
            (str(args.output / 'data'), '/data', True),
            (str(args.output / 'runtime-state'), '/runtime-state', True),
        ]) + [
            (str(args.dependencies), '/dependencies' if isolated_subject else '/workspace/kanban/node_modules', False),
            *runtime_binary_mounts,
        ]
        candidate = start_container(container_command(image, 'none', 'candidate', candidate_mounts,
                                                      bounded_workspace=isolated_subject))
        containers.append(candidate)
        keeper = container_keeper(candidate)
        atomic_json(args.output / 'containers.json', {'containers': containers})
        if isolated_subject:
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
                     'PLAYWRIGHT_MODULE': '/browser-deps/node_modules/' + args.playwright_module,
                     'PLAYWRIGHT_BROWSERS_PATH': args.browsers}
        evaluator_mounts = [
            (str(Path(__file__).resolve().parent), '/trusted', False),
            (str(evidence), '/evidence', True),
            (str(args.dependencies), '/dependencies', False),
            (str(args.browser_dependencies), '/browser-deps/node_modules', False),
            *runtime_binary_mounts,
        ]
        evaluator = start_container(container_command(image, f'container:{candidate}', 'evaluator', evaluator_mounts, probe_env))
        containers.append(evaluator)
        atomic_json(args.output / 'containers.json', {'containers': containers})

        metadata['container_image_id'] = image
        metadata['runtime_sha256'] = {name: file_digest(getattr(args, name)) for name in ('node', 'iii', 'pnpm')}
        metadata['dependency_lock_sha256'] = file_digest(args.dependencies / '.pnpm/lock.yaml')
        metadata['runtime_trees_sha256'] = {name: tree_digest(getattr(args, name)) for name in ('dependencies', 'browser_dependencies')}
        metadata['browser_path'] = args.browsers
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
                   evidence / 'isolation.log', 10, candidate, cancel):
            raise InfrastructureError('candidate isolation preflight failed; see isolation.log')

        if args.subject_model:
            prompt_file = evidence / 'subject-prompt.txt'
            prompt_file.write_text(metadata['prompt'])
            subject_env = {'III_SDK_MODULE': str(args.dependencies / 'iii-sdk/dist/index.mjs')}
            command = ['/usr/bin/env', *(f'{key}={value}' for key, value in subject_env.items()),
                       str(args.node), str(Path(__file__).with_name('subject.mjs')),
                       '--container', candidate, '--keeper', keeper,
                       '--prompt-file', str(prompt_file), '--output', str(evidence),
                       '--engine-url', args.subject_url, '--namespace', args.subject_namespace,
                       '--provider', 'deepseek', '--model', args.subject_model]
            code = bounded(command, evidence / 'subject.log', 2050, candidate, cancel)
            if (evidence / 'subject.json').is_file():
                subject = json.loads((evidence / 'subject.json').read_text())
                metadata['model_execution'] = subject.get('model_invoked', False)
                (evidence / 'provenance.json').write_text(json.dumps(metadata, indent=2))
            if code or not (evidence / 'subject.json').is_file():
                raise EvaluationError('subject execution did not complete; see subject.log and subject.json')
        elif args.external_subject:
            complete = wait_for_external_subject(args.output, {
                'candidate': candidate,
                'evaluator': evaluator,
                'keeper': keeper,
                'prompt': metadata['prompt'],
                'snapshot_git_head': metadata['snapshot_git_head'],
            })
            metadata['model_execution'] = complete['model_invoked']
            (evidence / 'provenance.json').write_text(json.dumps(metadata, indent=2))

        if isolated_subject:
            if bounded(docker_exec(candidate, ['/usr/bin/python3', '-I', '-c', STOP_RUNTIME, keeper]),
                       evidence / 'subject-cleanup.log', 10, candidate, cancel):
                raise EvaluationError('candidate background process cleanup failed')
        capture_candidate_diff(candidate, evidence, 'subject', cancel)
        delivered_diff_captured = True

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
                               evidence / f'{step}.log', 90, candidate, cancel)
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
                if isolated_subject:
                    subprocess.run(['docker', 'exec', '-i', candidate, '/usr/bin/tee', '/runtime-state/config.json'],
                                   input=json.dumps(config).encode(), stdout=subprocess.DEVNULL, check=True, timeout=10)
                process = start_runtime(candidate, evidence / 'runtime.log')
                runtime = [process, 0, keeper]
                started = time.monotonic()
                if not wait_runtime_ready(process, evaluator, cancel):
                    build_checks.append({'id': 'application_startup', 'status': 'failed',
                                         'detail': 'Compose application did not become ready; see runtime.log.'})
                    (evidence / 'result.json').write_text(json.dumps(result))
                else:
                    probe = docker_exec(evaluator, ['/runtime/node', '/trusted/probe.mjs', '--case', args.case,
                                        '--base-url', 'http://127.0.0.1:3000', '--engine-url', 'ws://127.0.0.1:50179',
                                        '--output', '/evidence'])
                    try:
                        code = bounded(probe, evidence / 'probe.log', 600, evaluator, cancel,
                                       control_callback(evidence, candidate, evaluator, runtime, cancel))
                        result = probe_result(evidence, args.case, code)
                    except InfrastructureError:
                        raise
                    except (RuntimeError, subprocess.TimeoutExpired) as error:
                        raise EvaluationError(str(error)) from error
                    result['build_checks'] = build_checks
                    result['duration_ms'] = round((time.monotonic() - started) * 1000)
                    (evidence / 'result.json').write_text(json.dumps(result, indent=2))

        capture_candidate_diff(candidate, evidence, 'evaluated', cancel)
        evaluated_diff_captured = True
        verify_candidate_source(candidate, evidence, cancel)

        result = json.loads((evidence / 'result.json').read_text())
        print(json.dumps({'output': str(args.output), 'result': result}))
        if result.get('status') in ('infrastructure_failed', 'evaluation_failed'):
            return 2
        return 0 if result.get('functional_status') == 'passed' else 1
    except (Exception, KeyboardInterrupt) as error:
        if (delivered_diff_captured and not evaluated_diff_captured
                and not isinstance(error, KeyboardInterrupt)):
            try:
                capture_candidate_diff(candidate, evidence, 'evaluated', cancel)
                evaluated_diff_captured = True
            except (Exception, KeyboardInterrupt):
                pass
            else:
                try:
                    verify_candidate_source(candidate, evidence, cancel)
                except Exception as integrity_error:
                    error = EvaluationError(f'{integrity_error}; prior error: {error}')
        result = write_failure(evidence, args.case, error)
        print(json.dumps(result))
        return 2
    finally:
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        for container in reversed(containers):
            remove_container(container)
        signal.signal(signal.SIGTERM, previous_sigterm)


if __name__ == '__main__':
    sys.exit(main())
