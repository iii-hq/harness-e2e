"""Fetching the group bundles the finalizer aggregates: which bundle each group
job gets, and that a bundle GitHub withholds for a while is waited for, while
one it keeps withholding fails the finalizer instead of being aggregated as a
group that never ran."""

import contextlib
import hashlib
import importlib.util
import io
import json
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest
import urllib.parse
import zipfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from unittest import mock


ROOT = pathlib.Path(__file__).resolve().parents[2]


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


group_artifacts = load("group_artifacts")

TOKEN = "token-for-the-api-only"
REPOSITORY = "/repos/iii-hq/harness-e2e"
# What GitHub answered on the 12th download of a 111-group campaign.
SECONDARY = (403, {"Retry-After": "5"},
             b'{"message":"You have exceeded a secondary rate limit. Please wait a few minutes before you try again."}')


def job(group, start, end, attempt=3, conclusion="success"):
    return {"name": f"smoke-r01 · {group}", "run_attempt": attempt, "status": "completed",
            "started_at": f"2026-09-21T{start}Z", "completed_at": f"2026-09-21T{end}Z",
            "conclusion": conclusion}


def artifact(group, attempt, identifier, created, expired=False):
    return {"id": identifier, "name": f"e2e-observation-execution-1-smoke-r01-{group}-gh-{attempt}",
            "created_at": f"2026-09-21T{created}Z", "expired": expired}


def ids(selected):
    return ",".join(str(item["id"]) for item in selected.values())


class SelectionTests(unittest.TestCase):
    def test_artifacts_belong_to_each_jobs_actual_execution(self):
        old_job = job("case-old", "05:00:00", "05:01:00")
        new_job = job("case-new", "11:00:00", "11:01:00", conclusion="failure")
        old = artifact("case-old", 1, 10, "05:00:50")
        stale = artifact("case-new", 1, 11, "05:00:50")
        diagnostic = artifact("case-new", 3, 30, "11:00:50")
        cases = [
            ("copied_success_and_new_failure", [old_job, new_job], [old, stale, diagnostic], 1, "10,30"),
            ("missing_new_artifact", [new_job], [stale], 1, ""),
            ("expired_new_artifact", [new_job], [stale, dict(diagnostic, expired=True)], 1, ""),
            ("different_contract", [old_job], [old], 2, ""),
            ("ambiguous_artifacts", [new_job], [diagnostic, dict(diagnostic, id=31)], 1, ""),
            ("ambiguous_jobs", [new_job, new_job], [diagnostic], 1, ""),
            ("malformed_job", [dict(new_job, completed_at=None)], [diagnostic], 1, ""),
            ("single_artifact", [old_job], [old, stale], 1, "10"),
            ("single_new_failure", [new_job], [diagnostic], 1, "30"),
            ("future_attempt", [new_job], [artifact("case-new", 4, 40, "11:00:50")], 1, ""),
        ]
        for name, jobs, artifacts, contract_attempt, expected in cases:
            with self.subTest(name=name):
                selected = group_artifacts.select(jobs, artifacts, "execution-1", contract_attempt, 3)
                self.assertEqual(ids(selected), expected)
                if name == "single_artifact":
                    self.assertEqual(selected["smoke-r01 · case-old"]["run_attempt"], 1)

    def test_an_aggregate_only_rerun_selects_each_groups_earlier_artifact(self):
        # Attempt 2 reran the finalizer alone: both groups are copies that
        # keep the times of attempt 1, and only attempt 1 uploaded bundles.
        jobs = [job("case-a", "05:00:00", "05:01:00", attempt=2), job("case-b", "05:00:00", "05:02:00", attempt=2)]
        artifacts = [artifact("case-a", 1, 10, "05:00:50"), artifact("case-b", 1, 11, "05:01:50")]
        selected = group_artifacts.select(jobs, artifacts, "execution-1", 1, 2)
        self.assertEqual(ids(selected), "10,11")
        self.assertEqual({item["run_attempt"] for item in selected.values()}, {1})

        # Attempt 3 reran the finalizer alone after attempt 2 reran case-b:
        # case-a keeps its attempt-1 bundle, case-b gets the one it ran again.
        jobs = [job("case-a", "05:00:00", "05:01:00"), job("case-b", "11:00:00", "11:02:00")]
        artifacts += [artifact("case-b", 2, 21, "11:01:50")]
        selected = group_artifacts.select(jobs, artifacts, "execution-1", 1, 3)
        self.assertEqual({name: item["run_attempt"] for name, item in selected.items()},
                         {"smoke-r01 · case-a": 1, "smoke-r01 · case-b": 2})
        self.assertEqual(ids(selected), "10,21")


class FetchTests(unittest.TestCase):
    """main() against a local GitHub: two listings, an artifact's zip endpoint
    that redirects to a blob store, and the blob. `faults(path, n)` may answer
    the n-th request of a path instead, as (status, headers, body)."""

    def fetch(self, faults=lambda path, n: None, groups=("case-a",), attempt=1):
        jobs = [job(group, "05:00:00", "05:01:00", attempt=attempt) for group in groups]
        zips, artifacts = {}, []
        for identifier, group in enumerate(groups, start=10):
            body = io.BytesIO()
            with zipfile.ZipFile(body, "w") as archive:
                archive.writestr("results.json", json.dumps({"group": group}))
            zips[identifier] = body.getvalue()
            artifacts.append(dict(artifact(group, 1, identifier, "05:00:50"),
                                  digest="sha256:" + hashlib.sha256(body.getvalue()).hexdigest()))
        requests = []

        def answer(server, path):
            url = urllib.parse.urlsplit(path)
            query = urllib.parse.parse_qs(url.query)
            items = {f"{REPOSITORY}/actions/runs/77/attempts/{attempt}/jobs": ("jobs", jobs),
                     f"{REPOSITORY}/actions/runs/77/artifacts": ("artifacts", artifacts)}
            if url.path in items:
                key, values = items[url.path]
                size, page = int(query["per_page"][0]), int(query.get("page", ["1"])[0])
                return 200, {}, json.dumps({"total_count": len(values),
                                            key: values[(page - 1) * size:page * size]}).encode()
            if match := re.fullmatch(f"{REPOSITORY}/actions/artifacts/([0-9]+)/zip", url.path):
                return 302, {"Location": f"http://127.0.0.1:{server.server_port}/blob/{match[1]}?sig=signed"}, b""
            if match := re.fullmatch("/blob/([0-9]+)", url.path):
                return 200, {}, zips[int(match[1])]
            return 404, {}, b""

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                requests.append((self.path, self.headers.get("Authorization")))
                count = sum(1 for path, _ in requests if path == self.path)
                status, headers, body = faults(self.path, count) or answer(self.server, self.path)
                self.send_response(status)
                for name, value in headers.items():
                    self.send_header(name, value)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *_):
                pass

        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        self.addCleanup(lambda: (server.shutdown(), thread.join(), server.server_close()))
        root = pathlib.Path(self.enterContext(tempfile.TemporaryDirectory()))
        sleeps, out, err = [], io.StringIO(), io.StringIO()
        with mock.patch.dict("os.environ", {
            "GITHUB_API_URL": f"http://127.0.0.1:{server.server_port}", "GITHUB_REPOSITORY": "iii-hq/harness-e2e",
            "GITHUB_RUN_ID": "77", "GITHUB_RUN_ATTEMPT": str(attempt), "GITHUB_TOKEN": TOKEN,
            "EXECUTION_KEY": "execution-1", "CONTRACT_ATTEMPT": "1",
        }), mock.patch.object(group_artifacts.time, "sleep", sleeps.append), contextlib.chdir(root), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = group_artifacts.main()
        return code, requests, sleeps, root, out.getvalue() + err.getvalue()

    @staticmethod
    def zips(requests):
        return [path for path, _ in requests if path.endswith("/zip")]

    def test_a_rate_limited_download_is_waited_out_and_the_group_restored(self):
        code, requests, sleeps, root, output = self.fetch(
            lambda path, n: SECONDARY if path.endswith("/zip") and n == 1 else None)
        self.assertEqual(code, 0, output)
        self.assertEqual(sleeps, [5])
        bundle = root / "target/downloaded-groups/e2e-observation-execution-1-smoke-r01-case-a-gh-1"
        self.assertEqual(json.loads((bundle / "results.json").read_text()), {"group": "case-a"})

        # Restored as an observed group: what the aggregate counts as passed.
        contracts = root / "target/harness-e2e-contract/contracts"
        contracts.mkdir(parents=True)
        (contracts / "smoke-r01.json").write_text('{"campaign":"smoke-r01"}')
        (root / "scripts").mkdir()
        shutil.copy(ROOT / "scripts/executor.sh", root / "scripts")
        (root / "scripts/exact_stack_campaign.py").write_text("print('case-a')\n")
        subprocess.run(["bash", "scripts/executor.sh", "finalize", "restore"], cwd=root, check=True)
        group = root / "target/harness-e2e-campaign/smoke-r01/groups/case-a"
        self.assertTrue((group / "results.json").is_file())
        self.assertFalse((group / "failure.json").exists())

    def test_a_download_that_keeps_failing_fails_the_finalizer(self):
        code, requests, sleeps, root, output = self.fetch(
            lambda path, n: SECONDARY if path.endswith("/zip") else None)
        self.assertEqual(code, 1)
        self.assertEqual(len(self.zips(requests)), group_artifacts.ATTEMPTS)
        self.assertEqual(len(sleeps), group_artifacts.ATTEMPTS - 1)
        summary_file = root / "target/harness-e2e-campaign/execution-summary.json"
        summary = json.loads(summary_file.read_text())
        self.assertEqual(summary["campaigns"], [])
        self.assertEqual((summary["outcome"], summary["phase"]), ("infra_failed", "workflow_artifact_download"))
        self.assertIn("e2e-observation-execution-1-smoke-r01-case-a-gh-1", summary["unavailable"])
        self.assertEqual((summary["failure"]["code"], summary["failure"]["selected"], summary["failure"]["downloaded"]),
                         ("group_artifact_download_failed", 1, 0))
        # Nothing is laid out to be aggregated without it.
        self.assertEqual(list((root / "target/harness-e2e-campaign").glob("*/")), [])
        for text in (output, summary_file.read_text()):
            self.assertNotIn(TOKEN, text)
            self.assertNotIn("/blob/", text)

    def test_transient_answers_are_retried_and_definitive_ones_are_not(self):
        garbage = (200, {}, b"not the bundle")
        # A rate limit that names no wait is waited out for a minute, as GitHub asks.
        cases = {
            "429": (lambda path, n: (429, {}, b"") if path.endswith("/zip") and n == 1 else None, 0, 2, [60]),
            "secondary without headers": (lambda path, n: (403, {}, SECONDARY[2])
                                          if path.endswith("/zip") and n <= 2 else None, 0, 3, [60, 60]),
            "502": (lambda path, n: (502, {}, b"") if path.endswith("/zip") and n == 1 else None, 0, 2, [2]),
            "digest": (lambda path, n: garbage if path.startswith("/blob/") and n == 1 else None, 0, 2, [2]),
            "404": (lambda path, n: (404, {}, b"") if path.endswith("/zip") else None, 1, 1, []),
            "plain 403": (lambda path, n: (403, {}, b'{"message":"Resource not accessible"}')
                          if path.endswith("/zip") else None, 1, 1, []),
        }
        for name, (faults, expected_code, expected_requests, expected_sleeps) in cases.items():
            with self.subTest(name=name):
                code, requests, sleeps, _, output = self.fetch(faults)
                self.assertEqual(code, expected_code, output)
                self.assertEqual(len(self.zips(requests)), expected_requests)
                self.assertEqual(sleeps, expected_sleeps)

    def test_a_retried_download_redirected_to_the_same_url_keeps_its_retries(self):
        # urllib counts redirects on the Request: reused, the 5th identical
        # redirect would read as a loop and end the retries as "HTTP 302".
        code, requests, sleeps, root, output = self.fetch(
            lambda path, n: (503, {}, b"") if path.startswith("/blob/") else None)
        self.assertEqual(code, 1)
        self.assertEqual(len(self.zips(requests)), group_artifacts.ATTEMPTS)
        self.assertEqual(len([path for path, _ in requests if path.startswith("/blob/")]), group_artifacts.ATTEMPTS)
        summary = json.loads((root / "target/harness-e2e-campaign/execution-summary.json").read_text())
        self.assertIn(f"answered HTTP 503, {group_artifacts.ATTEMPTS} times", summary["unavailable"])

    def test_retry_after_is_never_undercut_and_backs_off(self):
        limited = (403, {"Retry-After": "1"}, SECONDARY[2])
        code, _, sleeps, _, output = self.fetch(lambda path, n: limited if path.endswith("/zip") and n <= 2 else None)
        self.assertEqual(code, 0, output)
        self.assertEqual(sleeps, [2, 4])

    def test_a_wait_past_the_budget_fails_without_sleeping(self):
        limited = (403, {"Retry-After": "3600"}, SECONDARY[2])
        code, _, sleeps, _, _ = self.fetch(lambda path, n: limited if path.endswith("/zip") else None)
        self.assertEqual(code, 1)
        self.assertEqual(sleeps, [])

    def test_the_token_is_not_sent_to_the_blob_store(self):
        code, requests, _, _, output = self.fetch()
        self.assertEqual(code, 0, output)
        api = [token for path, token in requests if path.startswith(REPOSITORY)]
        blob = [token for path, token in requests if path.startswith("/blob/")]
        self.assertEqual(set(api), {f"Bearer {TOKEN}"})
        self.assertEqual(blob, [None])

    def test_listings_are_read_fifty_a_page_and_retried(self):
        # GitHub answered 502 to every 100-job page of a rerun that copied 114 jobs.
        groups = [f"case-{index:03}" for index in range(114)]
        code, requests, sleeps, root, output = self.fetch(
            lambda path, n: (502, {}, b"") if "per_page=100" in path or ("/jobs?" in path and path.endswith("&page=2")
                                                                          and n == 1) else None,
            groups=groups, attempt=2)
        self.assertEqual(code, 0, output)
        pages = [path.rsplit("?", 1)[1] for path, _ in requests if "/jobs?" in path]
        self.assertEqual(pages, ["per_page=50&page=1", "per_page=50&page=2", "per_page=50&page=2",
                                 "per_page=50&page=3"])
        self.assertEqual(sleeps, [2])
        selected = json.loads((root / "target/selected-group-artifacts.json").read_text())
        self.assertEqual(len(selected), 114)
        self.assertEqual(len(list((root / "target/downloaded-groups").iterdir())), 114)

    def test_a_listing_that_cannot_be_read_fails_the_finalizer(self):
        code, _, _, root, _ = self.fetch(lambda path, n: (502, {}, b"") if "/jobs?" in path else None)
        self.assertEqual(code, 1)
        summary = json.loads((root / "target/harness-e2e-campaign/execution-summary.json").read_text())
        self.assertEqual(summary["failure"]["code"], "group_artifact_listing_failed")
        self.assertIn("the jobs listing, page 1", summary["unavailable"])
        self.assertFalse((root / "target/selected-group-artifacts.json").exists())


if __name__ == "__main__":
    unittest.main()
