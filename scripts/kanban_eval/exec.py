"""Execute one command in a controller-selected candidate, with bounded output."""
import json
import os
import re
import resource
import subprocess
import sys
import tempfile


def execute(container, command):
    if not re.fullmatch(r'[0-9a-f]{64}', container) or len(command.encode()) > 65536:
        raise ValueError('invalid candidate or oversized command')
    limit = 256 * 1024

    def limits():
        resource.setrlimit(resource.RLIMIT_FSIZE, (limit, limit))

    with tempfile.TemporaryFile() as log:
        proc = subprocess.Popen(['docker', 'exec', '-w', '/workspace', container,
                                 '/bin/sh', '-c', command], stdin=subprocess.DEVNULL,
                                stdout=log, stderr=subprocess.STDOUT, preexec_fn=limits)
        try:
            code = proc.wait(timeout=120)
            if log.tell() >= limit:
                raise RuntimeError('command output exceeded 256 KiB')
            log.seek(0)
            return {'exit_code': code, 'stdout': log.read(limit).decode(errors='replace'), 'stderr': ''}
        except BaseException:
            proc.kill()
            proc.wait()
            subprocess.run(['docker', 'rm', '-f', container], stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL, check=False, timeout=15)
            raise


if __name__ == '__main__':
    print(json.dumps(execute(sys.argv[1], sys.stdin.read(65537))))
