#!/usr/bin/env python3
"""Run trusted SWE probes against untrusted code behind an OS boundary.

Linux uses a new mount/PID/network namespace through bubblewrap; Docker with
an already cached Python image is the fallback, including on macOS. Python -I
only controls interpreter imports and is NOT the isolation boundary. Seatbelt
alone is intentionally unsupported: it cannot guarantee detached child cleanup.

The allowed probe script and the candidate execute inside the same boundary.
This protects controller state, private Git objects and other fixture snapshots;
it does not make same-interpreter assertions immune to malicious introspection.
Only the administrator/controller may choose this wrapper and probe path.
"""
import argparse
import json
import math
import os
from pathlib import Path
import platform
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import uuid


class IsolationError(RuntimeError):
    pass


# Deliberately do not inherit HOME, credentials, proxies, Python configuration,
# Docker remote endpoints, or application settings from the controller.
ENV = {'PATH': '/usr/local/bin:/usr/bin:/bin', 'LANG': 'C.UTF-8', 'LC_ALL': 'C.UTF-8'}
OUTPUT_LIMIT = 1024 * 1024

# Executed inside the selected boundary under the same identity as the probe.
# Read bytes, never import/execute candidate or verifier code during preflight.
PREFLIGHT_INPUTS = """
import os, pathlib, stat, sys
workspace, probes = map(pathlib.Path, sys.argv[1:3])
previous = pathlib.Path(sys.argv[3]) if len(sys.argv) > 3 else None
def input_error(error):
    raise error
list(workspace.iterdir())
for directory, _, files in os.walk(workspace, onerror=input_error, followlinks=False):
    for name in files:
        path = pathlib.Path(directory) / name
        if stat.S_ISREG(path.lstat().st_mode):
            with path.open('rb') as source:
                source.read(1)
with probes.open('rb') as source:
    source.read(1)
if previous:
    list(previous.iterdir())
    for directory, _, files in os.walk(previous, onerror=input_error, followlinks=False):
        for name in files:
            path = pathlib.Path(directory) / name
            if stat.S_ISREG(path.lstat().st_mode):
                with path.open('rb') as source:
                    source.read(1)
"""


def _probe(command):
    try:
        result = subprocess.run(command, env=ENV, stdin=subprocess.DEVNULL,
                                capture_output=True, timeout=10, text=True)
        return result.returncode == 0
    except (OSError, subprocess.TimeoutExpired):
        return False


def _bwrap_command(binary, workspace, probes, previous=None):
    command = [binary, '--unshare-all', '--die-with-parent', '--new-session',
               '--cap-drop', 'ALL', '--clearenv']
    # Runtime roots only. In particular, never bind /, /home, /root, /tmp,
    # controller directories, the Docker socket, or the fixture repository.
    for root in ('/usr', '/lib', '/lib64', '/bin'):
        if Path(root).exists():
            command += ['--ro-bind', root, root]
    command += ['--proc', '/proc', '--dev', '/dev', '--tmpfs', '/tmp',
                '--ro-bind', str(workspace), '/workspace',
                '--dir', '/trusted', '--ro-bind', str(probes), '/trusted/probes.py',
                '--chdir', '/workspace']
    if previous:
        command += ['--ro-bind', str(previous), '/previous']
    for key, value in {**ENV, 'TMPDIR': '/tmp', 'HOME': '/tmp',
                       'PYTHONDONTWRITEBYTECODE': '1'}.items():
        command += ['--setenv', key, value]
    return command


def _docker_command(binary, image, workspace, probes, name, previous=None):
    # Bind mounts preserve mode0700 export ownership. Match the controller's
    # identity rather than chmod inputs or exposing their private parent paths.
    for path in (workspace, probes, *([previous] if previous else [])):
        if ',' in str(path) or '\n' in str(path):
            raise IsolationError('unsupported mount path')
    command = [binary, 'run', '--rm', '--pull=never', '--name', name, '--init',
            '--network', 'none', '--read-only', '--cap-drop=ALL',
            '--security-opt=no-new-privileges', '--pids-limit=128',
            '--memory=512m', '--cpus=2', f'--user={os.getuid()}:{os.getgid()}',
            '--tmpfs', '/tmp:rw,nosuid,nodev,size=256m,mode=1777',
            '--mount', f'type=bind,src={workspace},dst=/workspace,readonly',
            '--mount', f'type=bind,src={probes},dst=/trusted/probes.py,readonly']
    if previous:
        command += ['--mount', f'type=bind,src={previous},dst=/previous,readonly']
    return command + ['--workdir', '/workspace', '--entrypoint', '/usr/local/bin/python3',
            '--env', 'PATH=/usr/local/bin:/usr/bin:/bin', '--env', 'HOME=/tmp',
            '--env', 'TMPDIR=/tmp', '--env', 'LANG=C.UTF-8',
            '--env', 'PYTHONDONTWRITEBYTECODE=1', image]


def select_backend(workspace, probes, previous=None):
    """Preflight the actual boundary without loading any candidate code."""
    check = PREFLIGHT_INPUTS + '\nimport socket,tempfile; s=socket.socket(); s.bind(("127.0.0.1",0)); tempfile.TemporaryFile()'
    preflight_args = ['/workspace', '/trusted/probes.py'] + (['/previous'] if previous else [])
    bwrap = shutil.which('bwrap') if platform.system() == 'Linux' else None
    if bwrap:
        command = _bwrap_command(bwrap, workspace, probes, previous)
        if _probe(command + ['/usr/bin/python3', '-I', '-c', check, *preflight_args]):
            return ('bwrap', command + ['/usr/bin/python3'], None)
    docker = shutil.which('docker')
    if docker:
        # Resolve cached official Python images to immutable image IDs. No pulls,
        # builds, arbitrary environment-supplied images, or remote daemon config.
        try:
            result = subprocess.run([docker, 'image', 'ls', '--no-trunc', '--format', '{{.Repository}} {{.ID}}'],
                                    env=ENV, capture_output=True, text=True, timeout=10)
            images = sorted({line.split()[1] for line in result.stdout.splitlines()
                             if len(line.split()) == 2 and line.split()[0] == 'python'})
        except (OSError, subprocess.TimeoutExpired):
            images = []
        for image in images:
            name = 'swe-probe-preflight-' + uuid.uuid4().hex
            command = _docker_command(docker, image, workspace, probes, name, previous)
            try:
                if _probe(command + ['-I', '-c', check, *preflight_args]):
                    run_name = 'swe-probe-' + uuid.uuid4().hex
                    return ('docker', _docker_command(docker, image, workspace, probes, run_name, previous), (docker, run_name))
            finally:
                _remove_container(docker, name)
    raise IsolationError('no usable OS isolation backend; requires Linux bubblewrap or Docker with a cached Python image')


def _remove_container(binary, name):
    try:
        result = subprocess.run([binary, 'rm', '--force', name], env=ENV,
                       stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, timeout=10)
        if result.returncode == 0:
            return True
        # An already removed --rm container is also a successful cleanup.
        result = subprocess.run([binary, 'container', 'ls', '--all', '--format', '{{.Names}}'],
                                env=ENV, capture_output=True, text=True, timeout=10)
        return result.returncode == 0 and name not in result.stdout.splitlines()
    except (OSError, subprocess.TimeoutExpired):
        return False


def run(workspace, probes, through, canary=False, timeout=120, lifecycle=False, previous=None):
    backend, command, container = select_backend(workspace, probes, previous)
    probe_args = ['-I', '/trusted/probes.py', '--workspace', '/workspace', '--through', str(through)]
    if lifecycle:
        probe_args.append('--lifecycle')
    if previous:
        probe_args += ['--previous-workspace', '/previous']
    if backend == 'docker':
        # Independent in-container deadline also bounds lifetime if the host
        # wrapper is forcibly killed before its finally block can run.
        watchdog = ('import subprocess,sys; '
                    'p=subprocess.Popen([sys.executable,*sys.argv[2:]]); '
                    'sys.exit(p.wait(timeout=float(sys.argv[1])))')
        command += ['-I', '-c', watchdog, str(timeout), *probe_args]
    else:
        command += probe_args
    if canary:
        command.append('--canary')
    process = None
    old_handlers = {}

    def cancelled(signum, frame):
        raise IsolationError('probe execution cancelled')

    try:
        for signum in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
            old_handlers[signum] = signal.signal(signum, cancelled)
        with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
            process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=stdout,
                                       stderr=stderr, env=ENV, cwd='/', start_new_session=True,
                                       close_fds=True)
            deadline = time.monotonic() + timeout
            while process.poll() is None:
                if time.monotonic() >= deadline:
                    raise IsolationError('probe execution exceeded its time limit')
                if os.fstat(stdout.fileno()).st_size + os.fstat(stderr.fileno()).st_size > OUTPUT_LIMIT:
                    raise IsolationError('probe output exceeded its size limit')
                time.sleep(0.025)
            stdout.seek(0)
            stderr.seek(0)
            output = stdout.read(OUTPUT_LIMIT + 1)
            errors = stderr.read(OUTPUT_LIMIT + 1)
            if len(output) + len(errors) > OUTPUT_LIMIT:
                raise IsolationError('probe output exceeded its size limit')
            # Forward successful/failed probe JSON and its exit code unchanged.
            # Backend launch failures have no probe JSON and remain infrastructure.
            try:
                result = json.loads(output)
                if not isinstance(result, dict) or not isinstance(result.get('passed'), bool):
                    raise ValueError('invalid verdict')
            except (ValueError, UnicodeDecodeError):
                raise IsolationError('isolated probe did not return a valid verdict') from None
            return process.returncode, output, errors
    finally:
        # PID namespace termination kills even descendants that fork, setsid,
        # ignore SIGTERM, or keep output descriptors open. Docker rm --force
        # covers its daemon-owned namespace independently of the CLI process.
        for signum in old_handlers:
            signal.signal(signum, signal.SIG_IGN)
        cleaned = not container or _remove_container(*container)
        if process is not None:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                raise IsolationError('isolated process cleanup did not complete') from None
        for signum, handler in old_handlers.items():
            signal.signal(signum, handler)
        if not cleaned:
            raise IsolationError('isolated container cleanup did not complete')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--workspace', type=Path, required=True)
    parser.add_argument('--probes', type=Path, required=True)
    parser.add_argument('--through', type=int, choices=range(9), required=True)
    parser.add_argument('--canary', action='store_true')
    parser.add_argument('--lifecycle', action='store_true')
    parser.add_argument('--previous-workspace', type=Path)
    parser.add_argument('--timeout', type=float, default=120)
    args = parser.parse_args()
    try:
        if not args.workspace.is_absolute() or not args.probes.is_absolute():
            raise IsolationError('workspace and probe paths must be absolute')
        workspace = args.workspace.resolve(strict=True)
        probes = args.probes.resolve(strict=True)
        previous = args.previous_workspace.resolve(strict=True) if args.previous_workspace else None
        if not workspace.is_dir() or not probes.is_file() or probes.is_relative_to(workspace):
            raise IsolationError('expected an export directory and an external trusted probe file')
        if previous and (not args.previous_workspace.is_absolute() or not previous.is_dir()
                         or previous == workspace or previous.is_relative_to(workspace)
                         or workspace.is_relative_to(previous) or probes.is_relative_to(previous)):
            raise IsolationError('previous workspace must be a separate absolute export directory')
        if not math.isfinite(args.timeout) or args.timeout <= 0 or args.timeout > 120:
            raise IsolationError('probe timeout must be greater than zero and at most 120 seconds')
        code, output, errors = run(workspace, probes, args.through, args.canary, args.timeout,
                                   args.lifecycle, previous)
        sys.stdout.buffer.write(output)
        sys.stderr.buffer.write(errors)
        return code
    except (IsolationError, OSError) as error:
        print(json.dumps({'passed': False, 'checks': [], 'infrastructure_error': str(error)}))
        return 2


if __name__ == '__main__':
    sys.exit(main())
