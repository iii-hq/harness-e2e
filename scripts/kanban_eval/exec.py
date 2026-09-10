"""Execute one command in a controller-selected candidate, with bounded output."""
import json
import os
import re
import resource
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(__file__))
from run import STOP_RUNTIME
sys.path.pop(0)


COMMAND_TIMEOUT = 120


def execute(container, keeper, command):
    if (not re.fullmatch(r'[0-9a-f]{64}', container)
            or not keeper.isdigit() or int(keeper) <= 1
            or len(command.encode()) > 65536):
        raise ValueError('invalid candidate, keeper or oversized command')
    limit = 256 * 1024

    def limits():
        resource.setrlimit(resource.RLIMIT_FSIZE, (limit, limit))

    with tempfile.TemporaryFile() as log:
        proc = subprocess.Popen(['docker', 'exec', '-w', '/workspace', container,
                                 '/bin/sh', '-c', command], stdin=subprocess.DEVNULL,
                                stdout=log, stderr=subprocess.STDOUT, preexec_fn=limits)
        try:
            code = proc.wait(timeout=COMMAND_TIMEOUT)
            failure = (125, 'command output exceeded 256 KiB') if log.tell() >= limit else None
        except subprocess.TimeoutExpired:
            failure = (124, 'command timed out after 120 seconds')
        except BaseException:
            proc.kill()
            proc.wait()
            subprocess.run(['docker', 'rm', '-f', container], stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL, check=False, timeout=15)
            raise
        if failure:
            proc.kill()
            proc.wait()
            try:
                cleaned = subprocess.run(
                    ['docker', 'exec', container, '/usr/bin/python3', '-I', '-c',
                     STOP_RUNTIME, keeper], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                    stderr=subprocess.PIPE, check=False, timeout=15, text=True)
            except (OSError, subprocess.TimeoutExpired) as error:
                cleaned = None
                cleanup_error = str(error)
            else:
                cleanup_error = cleaned.stderr.strip()
            if not cleaned or cleaned.returncode:
                subprocess.run(['docker', 'rm', '-f', container], stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL, check=False, timeout=15)
                raise RuntimeError('candidate cleanup failed after command boundary: '
                                   + cleanup_error[-4096:])
            log.seek(0)
            return {'exit_code': failure[0], 'stdout': log.read(limit).decode(errors='replace'),
                    'stderr': failure[1] + '; candidate processes were stopped'}
        log.seek(0)
        return {'exit_code': code, 'stdout': log.read(limit).decode(errors='replace'), 'stderr': ''}


if __name__ == '__main__':
    print(json.dumps(execute(sys.argv[1], sys.argv[2], sys.stdin.read(65537))))
