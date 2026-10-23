"""Registry install topology and outage handling."""
import contextlib
import http.server
import importlib.util
import io
import json
import pathlib
import socket
import tempfile
import threading
import unittest
import urllib.error
from unittest.mock import Mock, patch

import yaml

SCRIPT = pathlib.Path(__file__).resolve().parents[2] / "scripts/smoke_registry_install.py"
spec = importlib.util.spec_from_file_location("smoke_registry_install", SCRIPT)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

OUTAGE = """Error: {
  "code": "PACKAGE_DOWNLOAD_FAILED",
  "message": "container 'storage': could not download https://api.workers.iii.dev/download/storage/0.1.24/x86_64-unknown-linux-gnu: HTTP status server error (504 Gateway Timeout) for url (https://api.workers.iii.dev/download/storage/0.1.24/x86_64-unknown-linux-gnu)"
}"""


@contextlib.contextmanager
def registry_answering(status):
    class Handler(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            self.send_response(status)
            self.end_headers()

        def log_message(self, *_args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        yield f"http://127.0.0.1:{server.server_port}"
    finally:
        server.shutdown()
        server.server_close()


class RegistryOutageTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = pathlib.Path(temporary.name)
        self.binary = self.root / "harness-e2e"
        self.binary.write_text(
            "#!/bin/sh\necho '{\"version\": \"0.0.0\", \"supported_targets\": [\"x\"]}'\n"
        )
        self.binary.chmod(0o755)

    def smoke(self, registry):
        output, delays = io.StringIO(), []
        with patch.object(module.time, "sleep", delays.append), \
                contextlib.redirect_stdout(output):
            module.main(self.binary, self.root / "smoke", registry)
        return output.getvalue(), delays

    def test_unreachable_registry_warns_after_three_attempts(self):
        with socket.socket() as closed:
            closed.bind(("127.0.0.1", 0))
            registry = f"http://127.0.0.1:{closed.getsockname()[1]}"
        output, delays = self.smoke(registry)
        self.assertEqual(delays, [30, 60])
        self.assertIn("::warning title=Registry unreachable::", output)
        self.assertEqual(output.count("Registry unreachable (attempt"), 2)

    def test_registry_server_errors_warn(self):
        for status in (503, 429):
            with self.subTest(status=status), registry_answering(status) as registry:
                output, _ = self.smoke(registry)
                self.assertIn(f"answered {status}", output)
                self.assertIn("::warning", output)

    def test_registry_client_error_still_fails(self):
        with registry_answering(400) as registry, \
                self.assertRaises(urllib.error.HTTPError):
            self.smoke(registry)

    def test_only_registry_downloads_count_as_outages(self):
        self.assertTrue(module.registry_outage(OUTAGE))
        self.assertTrue(module.registry_outage(OUTAGE.replace(
            "server error (504 Gateway Timeout)", "client error (429 Too Many Requests)")))
        self.assertFalse(module.registry_outage(OUTAGE.replace(
            "server error (504 Gateway Timeout)", "client error (404 Not Found)")))
        self.assertFalse(module.registry_outage(OUTAGE.replace(
            "https://api.workers.iii.dev/download/storage/0.1.24/x86_64-unknown-linux-gnu",
            "http://127.0.0.1:41234/candidate.tar.gz")))
        self.assertTrue(module.registry_outage('Error: {"code": "REGISTRY_UNREACHABLE"}'))
        self.assertFalse(module.registry_outage('Error: {"code": "PACKAGE_DIGEST_MISMATCH"}'))


class RegistryTopologyTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = pathlib.Path(temporary.name)
        self.binary = self.root / "harness-e2e"
        self.binary.write_text("candidate")

    def test_engine_nodes_are_preserved_but_not_installed(self):
        resolved = {
            "graph": [
                {"name": "harness-e2e", "version": "9.9.9", "type": "binary",
                 "binaries": {"x": {"url": "https://example/harness"}}, "config": {}},
                {"name": "database", "version": "1.0.0", "type": "binary",
                 "binaries": {"x": {"url": "https://example/database"}}, "config": {}},
                {"name": "shared-engine", "version": "1.0.0", "type": "engine", "config": {}},
            ],
            "edges": [
                {"from": "harness-e2e", "to": "database"},
                {"from": "harness-e2e", "to": "shared-engine"},
            ],
        }
        with patch.object(module, "resolve", return_value=resolved), \
                patch.object(module.subprocess, "check_output", return_value='{"version":"2.0.0","supported_targets":["x"]}'), \
                patch.object(module.subprocess, "Popen", side_effect=RuntimeError("stop before engine start")):
            with self.assertRaisesRegex(RuntimeError, "stop before engine start"):
                module.run(self.binary, self.root / "smoke")

        directory = self.root / "smoke"
        self.assertEqual(json.loads((directory / "registry-resolution.json").read_text()), resolved)
        compose = yaml.safe_load((directory / "worker-compose.yaml").read_text())["containers"]
        lock = yaml.safe_load((directory / "worker-compose.lock").read_text())["containers"]
        self.assertEqual(set(compose), {"harness-e2e", "sql-service"})
        self.assertEqual(set(lock), {"harness-e2e", "sql-service"})
        self.assertEqual(compose["harness-e2e"]["start_after"], ["sql-service"])

    def test_binary_without_artifacts_still_fails(self):
        resolved = {"graph": [{"name": "harness-e2e", "version": "1.0.0", "type": "binary",
                                "config": {}}], "edges": []}
        server, thread = Mock(server_port=1), Mock()
        with patch.object(module, "resolve", return_value=resolved), \
                patch.object(module.subprocess, "check_output", return_value='{"version":"2.0.0","supported_targets":["x"]}'), \
                patch.object(module.http.server, "ThreadingHTTPServer", return_value=server), \
                patch.object(module.threading, "Thread", return_value=thread):
            with self.assertRaisesRegex(KeyError, "binaries"):
                module.run(self.binary, self.root / "invalid")


if __name__ == "__main__":
    unittest.main()
