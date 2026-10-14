import io
import os
import subprocess
import tempfile
import threading
import unittest
import zipfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


SCRIPT = Path(__file__).parents[2] / "scripts" / "download_artifact.sh"


class DownloadTests(unittest.TestCase):
    def download(self, statuses):
        body = io.BytesIO()
        with zipfile.ZipFile(body, "w") as archive:
            archive.writestr("contract.json", "ok")
        requests = []

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                requests.append(self.path)
                status = statuses[min(len(requests) - 1, len(statuses) - 1)]
                self.send_response(status)
                self.end_headers()
                if status == 200:
                    self.wfile.write(body.getvalue())

            def log_message(self, *_):
                pass

        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as directory:
                environment = os.environ | {
                    "GITHUB_API_URL": f"http://127.0.0.1:{server.server_port}",
                    "GITHUB_REPOSITORY": "iii-hq/harness-e2e",
                    "GITHUB_TOKEN": "test",
                }
                result = subprocess.run(
                    ["bash", str(SCRIPT), "42", directory],
                    env=environment, capture_output=True, text=True,
                )
                extracted = (Path(directory) / "contract.json").read_text() if result.returncode == 0 else None
        finally:
            server.shutdown()
            thread.join()
            server.server_close()
        return result, requests, extracted

    def test_transient_failure_is_retried(self):
        result, requests, extracted = self.download([503, 200])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((len(requests), extracted), (2, "ok"))

    def test_transient_failure_stops_after_the_retry_limit(self):
        result, requests, _ = self.download([503])
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(requests), 4)

    def test_definitive_failure_is_not_retried(self):
        result, requests, _ = self.download([404])
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(requests), 1)


if __name__ == "__main__":
    unittest.main()
