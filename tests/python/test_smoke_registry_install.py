"""A Registry outage turns the packaged install smoke into a warning."""
import contextlib
import http.server
import importlib.util
import io
import pathlib
import socket
import tempfile
import threading
import unittest
import urllib.error
from unittest.mock import patch

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


class ResolutionTest(unittest.TestCase):
    def test_engine_workers_are_left_to_the_engine(self):
        resolved = {"graph": [
            {"name": "configuration", "type": "engine", "version": "0.25.0",
             "config": {"adapter": {"name": "fs"}}},
            {"name": "database", "type": "binary", "version": "0.5.23",
             "binaries": {"x": {"url": "u", "sha256": "s"}}, "config": None},
        ]}
        self.assertEqual([worker["name"] for worker in module.binary_workers(resolved)],
                         ["database"])


if __name__ == "__main__":
    unittest.main()
