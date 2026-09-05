"""Real OS-boundary tests; run on a host with an isolation backend."""
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
import uuid
from unittest.mock import patch

ISOLATION = Path(__file__).resolve().parents[2] / 'src/scenarios/swe_service/isolation.py'


class IsolationTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.workspace = self.root / 'export'
        self.workspace.mkdir()
        (self.workspace / 'public.txt').write_text('candidate public data')
        self.secret = self.root / 'controller-private'
        self.secret.write_text('must never be readable')
        (self.workspace / 'escape').symlink_to(self.secret)
        self.probes = self.root / 'trusted-probes.py'

    def invoke(self, body, timeout=10, env=None):
        self.probes.write_text(body)
        result = subprocess.run([sys.executable, '-I', str(ISOLATION), '--workspace', str(self.workspace),
                               '--probes', str(self.probes), '--through', '0', '--timeout', str(timeout)],
                              capture_output=True, text=True, timeout=timeout + 20, env=env)
        if 'no usable OS isolation backend' in result.stdout and not os.environ.get('SWE_REQUIRE_OS_ISOLATION'):
            self.skipTest('OS integration requires Linux bubblewrap or cached Python Docker image')
        return result

    def test_real_private_reads_denied_public_reads_tmp_and_loopback_work(self):
        result = self.invoke('''import argparse,json,os,pathlib,socket,tempfile
p=argparse.ArgumentParser();p.add_argument('--workspace');p.add_argument('--through');a=p.parse_args()
w=pathlib.Path(a.workspace)
assert (w/'public.txt').read_text()=='candidate public data'
for path in [pathlib.Path(PRIVATE_PATH),w/'escape']:
 try: path.read_text()
 except (PermissionError,FileNotFoundError): pass
 else: raise AssertionError('private read succeeded')
try: (w/'mutated').write_text('bad')
except (PermissionError,OSError): pass
else: raise AssertionError('export was writable')
assert 'SWE_HOST_SECRET' not in os.environ
with tempfile.TemporaryDirectory() as t:
 pathlib.Path(t,'scratch').write_text('works')
with socket.socket() as server:
 server.bind(('127.0.0.1',0));server.listen()
 with socket.create_connection(server.getsockname()) as client:
  connection,_=server.accept()
  with connection: client.sendall(b'HTTP');assert connection.recv(4)==b'HTTP'
print(json.dumps({'passed':True,'checks':[]}))
'''.replace('PRIVATE_PATH', repr(str(self.secret))), env={**os.environ, 'SWE_HOST_SECRET': 'hidden'})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(json.loads(result.stdout)['passed'])
        self.assertFalse((self.workspace / 'mutated').exists())

    def test_preserves_failed_verdict_and_exit_semantics(self):
        result = self.invoke("print('{\"passed\":false,\"checks\":[]}')")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(json.loads(result.stdout)['passed'])
        result = self.invoke("print('{\"passed\":false,\"checks\":[],\"infrastructure_error\":\"probe failed\"}');raise SystemExit(3)")
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)

    def test_timeout_is_infrastructure_failure_and_stops_detached_child(self):
        marker = 'swe-detached-test-' + uuid.uuid4().hex
        started = time.monotonic()
        result = self.invoke(self.detached_probe(marker, hang=True), timeout=1)
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn('time limit', json.loads(result.stdout)['infrastructure_error'])
        self.assertLess(time.monotonic() - started, 10)
        self.assert_no_child(marker)

    def detached_probe(self, marker, hang):
        child = "import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);time.sleep(300)"
        return ("import json,subprocess,sys,time\n"
                f"subprocess.Popen([sys.executable,'-c',{child!r},{marker!r}],start_new_session=True)\n"
                "time.sleep(.2)\n" + ("time.sleep(300)\n" if hang else
                                       "print(json.dumps({'passed':True,'checks':[]}))\n"))

    def assert_no_child(self, marker):
        if sys.platform == 'linux':
            for cmdline in Path('/proc').glob('[0-9]*/cmdline'):
                try:
                    self.assertNotIn(marker.encode(), cmdline.read_bytes(), str(cmdline))
                except (FileNotFoundError, PermissionError, ProcessLookupError):
                    pass

    def test_success_also_cleans_detached_children(self):
        marker = 'swe-detached-test-' + uuid.uuid4().hex
        result = self.invoke(self.detached_probe(marker, hang=False))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assert_no_child(marker)

    def test_cancellation_cleans_detached_children(self):
        self.invoke("print('{\"passed\":true,\"checks\":[]}')")
        marker = 'swe-detached-test-' + uuid.uuid4().hex
        self.probes.write_text(self.detached_probe(marker, hang=True))
        process = subprocess.Popen([sys.executable, '-I', str(ISOLATION), '--workspace', str(self.workspace),
                                    '--probes', str(self.probes), '--through', '0'],
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            time.sleep(1)
            process.send_signal(signal.SIGTERM)
            stdout, stderr = process.communicate(timeout=15)
            self.assertEqual(process.returncode, 2, stdout + stderr)
            self.assertIn('cancelled', json.loads(stdout)['infrastructure_error'])
            self.assert_no_child(marker)
        finally:
            if process.poll() is None:
                process.kill()
                process.communicate(timeout=5)

    def load_module(self):
        spec = importlib.util.spec_from_file_location('swe_isolation', ISOLATION)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module

    def test_docker_identity_can_traverse_private_export_without_chmod(self):
        self.workspace.chmod(0o700)
        module = self.load_module()
        command = module._docker_command('docker', 'sha256:trusted', self.workspace, self.probes, 'test')
        self.assertIn(f'--user={os.getuid()}:{os.getgid()}', command)
        self.assertEqual(self.workspace.stat().st_mode & 0o777, 0o700)
        self.assertIn(f'type=bind,src={self.workspace},dst=/workspace,readonly', command)
        self.assertIn(f'type=bind,src={self.probes},dst=/trusted/probes.py,readonly', command)
        self.assertFalse(any(f'src={self.root},' in argument for argument in command))

    def test_preflight_reads_real_inputs_without_executing_them(self):
        module = self.load_module()
        self.workspace.chmod(0o700)
        self.probes.write_text("raise AssertionError('preflight must not execute probes')")
        command = [sys.executable, '-I', '-c', module.PREFLIGHT_INPUTS,
                   str(self.workspace), str(self.probes)]
        result = subprocess.run(command, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.probes.unlink()
        result = subprocess.run(command, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('FileNotFoundError', result.stderr)

    def test_real_docker_reads_controller_mode0700_export(self):
        module = self.load_module()
        self.workspace.chmod(0o700)
        self.probes.write_text("import pathlib,json;assert pathlib.Path('/workspace/public.txt').read_text()=='candidate public data';print(json.dumps({'passed':True,'checks':[]}))")
        actual_which = module.shutil.which
        with patch.object(module.shutil, 'which', side_effect=lambda name: None if name == 'bwrap' else actual_which(name)):
            try:
                code, output, errors = module.run(self.workspace, self.probes, 0, timeout=10)
            except module.IsolationError as error:
                if 'no usable OS isolation backend' in str(error) and not os.environ.get('SWE_REQUIRE_DOCKER_ISOLATION'):
                    self.skipTest('Docker integration requires a cached Python image')
                raise
        self.assertEqual(code, 0, errors.decode())
        self.assertTrue(json.loads(output)['passed'])
        self.assertEqual(self.workspace.stat().st_mode & 0o777, 0o700)

    def test_no_backend_fails_closed_without_running_probe(self):
        self.assertTrue(ISOLATION.is_file(), 'trusted OS isolation wrapper is missing')
        spec = importlib.util.spec_from_file_location('swe_isolation', ISOLATION)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        self.probes.write_text("raise AssertionError('must not execute')")
        with patch.object(module.shutil, 'which', return_value=None):
            with self.assertRaisesRegex(module.IsolationError, 'backend'):
                module.select_backend(self.workspace, self.probes)


if __name__ == '__main__':
    unittest.main()
