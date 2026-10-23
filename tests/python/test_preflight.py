"""The check preparation makes before it spends: the model's provider takes
its key, the Kanban fixture's catalog reads, the Registry answers; asked again
while they do not, and failed with a cause Release Control can say."""

import contextlib
import http.server
import importlib.util
import io
import json
import pathlib
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch


ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


preflight = load("preflight")
prepare_execution = sys.modules["prepare_execution"]

KANBAN = "49b505e46d2f99afba873197d28c79e257190082"


class Clock:
    """Time that passes only when the check sleeps."""

    def __init__(self):
        self.now, self.slept = 0.0, []

    def __call__(self):
        return self.now

    def sleep(self, seconds):
        self.slept.append(seconds)
        self.now += seconds


def flaky(failures, error=preflight.Unavailable):
    """A check that raises `error` its first `failures` calls."""
    calls = []

    def check():
        calls.append(None)
        if len(calls) <= failures:
            raise error(f"HTTP 503 (call {len(calls)})")
    check.calls = calls
    return check


class Server(http.server.ThreadingHTTPServer):
    """A local service answering each path with the statuses it is given, in turn."""

    def __init__(self, statuses):
        self.statuses, self.seen = {path: list(codes) for path, codes in statuses.items()}, []
        outer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def answer(self):
                length = int(self.headers.get("Content-Length") or 0)
                outer.seen.append((self.command, self.path, self.headers, self.rfile.read(length)))
                codes = outer.statuses[self.path]
                code = codes.pop(0) if len(codes) > 1 else codes[0]
                self.send_response(code)
                self.end_headers()
                self.wfile.write(b"{}")

            do_GET = do_POST = answer

            def log_message(self, *args):
                pass

        super().__init__(("127.0.0.1", 0), Handler)
        threading.Thread(target=self.serve_forever, daemon=True).start()

    def url(self, path):
        return f"http://127.0.0.1:{self.server_address[1]}{path}"


class UntilReadyTests(unittest.TestCase):
    def test_what_answers_at_once_is_asked_once(self):
        clock, check = Clock(), flaky(0)
        preflight.until_ready({"registry": check}, 300, clock=clock, sleep=clock.sleep)
        self.assertEqual((len(check.calls), clock.slept), (1, []))

    def test_what_did_not_answer_is_asked_again_waiting_longer_each_time(self):
        clock, failing, fine = Clock(), flaky(2), flaky(0)
        with contextlib.redirect_stderr(io.StringIO()) as stderr:
            preflight.until_ready({"registry": failing, "provider": fine}, 300, clock=clock, sleep=clock.sleep)
        self.assertEqual(clock.slept, [10, 20])
        self.assertEqual(len(failing.calls), 3)
        # Only what was not ready is asked again.
        self.assertEqual(len(fine.calls), 1)
        self.assertIn("::warning::not ready, asking again in 10s: HTTP 503 (call 1)", stderr.getvalue())

    def test_what_never_answers_fails_once_the_ceiling_has_gone_by(self):
        clock = Clock()
        with contextlib.redirect_stderr(io.StringIO()), \
                self.assertRaisesRegex(preflight.Unavailable, r"^HTTP 503 \(call 5\), for 100s$"):
            preflight.until_ready({"registry": flaky(99)}, 100, clock=clock, sleep=clock.sleep)
        # 10, 20, 40, then what is left of the 100 seconds.
        self.assertEqual(clock.slept, [10, 20, 40, 30])

    def test_a_refusal_fails_at_once(self):
        clock = Clock()
        with self.assertRaisesRegex(preflight.Refused, "call 1"):
            preflight.until_ready({"provider": flaky(1, preflight.Refused), "registry": flaky(99)}, 300,
                                  clock=clock, sleep=clock.sleep)
        self.assertEqual(clock.slept, [])


class ProbeTests(unittest.TestCase):
    def setUp(self):
        self.server = Server({"/models": [200], "/chat": [400], "/resolve": [200], "/catalog": [200]})
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)

    def provider(self, provider, statuses, key="sk-test", path="/models", body=None):
        self.server.statuses[path] = list(statuses)
        probes = {provider: (self.server.url(path), body)}
        variable = preflight.credential_catalog()[0][provider]
        with patch.dict(preflight.PROVIDER_PROBES, probes), contextlib.redirect_stderr(io.StringIO()):
            preflight.check_provider(provider, {variable: key} if key else {})

    def test_a_key_the_provider_takes_passes_and_one_it_refuses_fails(self):
        self.provider("deepseek", [200])
        method, _, headers, _ = self.server.seen[-1]
        self.assertEqual((method, headers["Authorization"]), ("GET", "Bearer sk-test"))
        self.provider("anthropic", [200])
        self.assertEqual(self.server.seen[-1][2]["x-api-key"], "sk-test")
        self.assertEqual(self.server.seen[-1][2]["anthropic-version"], "2023-06-01")
        for status in (401, 402, 403):
            with self.subTest(status=status), \
                    self.assertRaisesRegex(preflight.Refused, f"deepseek refused DEEPSEEK_API_KEY: HTTP {status}"):
                self.provider("deepseek", [status])
        # xAI answers 400 to a key it does not know.
        with self.assertRaisesRegex(preflight.Refused, "xai refused XAI_API_KEY: HTTP 400"):
            self.provider("xai", [400])

    def test_zai_is_asked_an_empty_chat_whose_400_means_the_key_passed(self):
        body = preflight.PROVIDER_PROBES["zai"][1]
        self.assertEqual(body["messages"], [])
        self.provider("zai", [400], path="/chat", body=body)
        method, _, headers, sent = self.server.seen[-1]
        self.assertEqual((method, json.loads(sent)), ("POST", body))
        with self.assertRaisesRegex(preflight.Refused, "zai refused ZAI_API_KEY: HTTP 401"):
            self.provider("zai", [401], path="/chat", body=body)

    def test_a_missing_key_is_refused_and_a_provider_without_a_probe_is_not_checked(self):
        with self.assertRaisesRegex(preflight.Refused, "DEEPSEEK_API_KEY is not set"):
            self.provider("deepseek", [200], key="")
        seen = len(self.server.seen)
        with contextlib.redirect_stderr(io.StringIO()) as stderr:
            preflight.check_provider("llamacpp", {})
            preflight.check_provider("claude-code", {})
        self.assertEqual(len(self.server.seen), seen)
        self.assertIn("provider llamacpp: no key to check", stderr.getvalue())

    def test_load_shedding_gateway_faults_and_silence_are_worth_asking_again(self):
        for status in (429, 500, 503):
            with self.subTest(status=status), self.assertRaisesRegex(
                    preflight.Unavailable, f"cannot reach deepseek at .*/models: HTTP {status}"):
                self.provider("deepseek", [status])
        with patch.dict(preflight.PROVIDER_PROBES, {"deepseek": ("http://127.0.0.1:9/models", None)}), \
                self.assertRaisesRegex(preflight.Unavailable, "cannot reach deepseek at .*: no answer"):
            preflight.check_provider("deepseek", {"DEEPSEEK_API_KEY": "sk-test"})

    def test_the_registry_answers_when_it_says_anything_but_load_or_fault(self):
        with patch.object(preflight, "REGISTRY_URL", self.server.url("")), contextlib.redirect_stderr(io.StringIO()):
            self.server.statuses["/resolve"] = [404]
            preflight.check_registry()
            method, _, _, sent = self.server.seen[-1]
            self.assertEqual((method, json.loads(sent)["worker"]), ("POST", "harness-e2e"))
            self.server.statuses["/resolve"] = [502]
            # Release Control reads this phrase as test infrastructure.
            with self.assertRaisesRegex(preflight.Unavailable, r"^cannot reach the registry at http://127\.0\.0\.1:\d+: HTTP 502$"):
                preflight.check_registry()

    def test_the_kanban_catalog_must_read_and_a_drifted_one_only_warns(self):
        embedded = (ROOT / "src/scenarios/kanban/catalog.json").read_bytes()

        def answer(url, *_, **__):
            self.assertEqual(url, f"https://raw.githubusercontent.com/iii-hq/kanban-e2e-fixture/{KANBAN}/scenarios/catalog.json")
            return next(responses)

        for response, warned in (((200, embedded), False), ((200, b"{}"), True), ((404, b""), True)):
            responses = iter([response])
            with self.subTest(response=response[0], warned=warned), patch.object(preflight, "answer", answer), \
                    contextlib.redirect_stderr(io.StringIO()) as stderr:
                preflight.check_kanban(KANBAN)
                self.assertEqual("Kanban catalog drift" in stderr.getvalue(), warned)
        with patch.object(preflight, "answer", side_effect=preflight.Unavailable("HTTP 503")), \
                self.assertRaisesRegex(preflight.Unavailable,
                                       "cannot read the Kanban fixture's catalog at iii-hq/kanban-e2e-fixture@49b505e46d2f: HTTP 503"):
            preflight.check_kanban(KANBAN)


class PreflightCommandTests(unittest.TestCase):
    def contract(self, directory, groups):
        root = pathlib.Path(directory)
        (root / "execution.json").write_text(json.dumps({
            "model": "deepseek/deepseek-v4-flash", "fixtures": {"kanban-e2e-fixture": KANBAN}}))
        (root / "suite.json").write_text(json.dumps({"campaigns": [
            {"campaign_id": "r01", "groups": [{"id": group} for group in groups]}]}))
        return root

    def test_the_kanban_catalog_is_checked_only_when_a_group_runs_kanban(self):
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(list(preflight.checks_for(self.contract(directory, ["case-timer-wake"]), {})),
                             ["provider", "registry"])
            self.assertEqual(list(preflight.checks_for(self.contract(directory, ["case-kanban-c1"]), {})),
                             ["provider", "kanban", "registry"])

    def run_command(self, directory, routes, seconds="30"):
        """main() against `routes`, url -> the answers it gives in turn."""
        clock = Clock()

        def answer(url, *_, **__):
            reply = routes[url].pop(0) if len(routes[url]) > 1 else routes[url][0]
            if isinstance(reply, Exception):
                raise reply
            return reply

        embedded = (ROOT / "src/scenarios/kanban/catalog.json").read_bytes()
        routes.setdefault(f"https://raw.githubusercontent.com/iii-hq/kanban-e2e-fixture/{KANBAN}/scenarios/catalog.json",
                          [(200, embedded)])
        routes.setdefault("https://api.deepseek.com/models", [(200, b"{}")])
        routes.setdefault("https://api.workers.iii.dev/resolve", [(200, b"{}")])
        until_ready = preflight.until_ready
        with patch.object(preflight, "answer", answer), \
                patch.object(preflight, "until_ready",
                             lambda checks, s: until_ready(checks, s, clock=clock, sleep=clock.sleep)), \
                patch.dict("os.environ", {"DEEPSEEK_API_KEY": "sk-test", "HARNESS_E2E_PREFLIGHT_SECONDS": seconds}), \
                contextlib.redirect_stderr(io.StringIO()) as stderr:
            status = preflight.main(["--contract-dir", str(self.contract(directory, ["case-kanban-c1"]))])
        return status, stderr.getvalue(), clock.slept

    def test_ready_recovered_and_still_failing_executions(self):
        down = preflight.Unavailable("HTTP 503")
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(self.run_command(directory, {})[::2], (0, []))
            # The Registry shed load twice, then answered.
            status, _, slept = self.run_command(
                directory, {"https://api.workers.iii.dev/resolve": [down, down, (200, b"{}")]})
            self.assertEqual((status, slept), (0, [10, 20]))
            # It never did: preparation fails, saying so on its last line.
            status, stderr, slept = self.run_command(directory, {"https://api.workers.iii.dev/resolve": [down]})
            self.assertEqual((status, slept), (1, [10, 20]))
            last = stderr.strip().splitlines()[-1]
            self.assertEqual(last, "[FAIL] cannot reach the registry at https://api.workers.iii.dev: HTTP 503, for 30s")
            # A refused key fails at once.
            status, stderr, slept = self.run_command(directory, {"https://api.deepseek.com/models": [(401, b"")]})
            self.assertEqual((status, slept), (1, []))
            self.assertIn("[FAIL] deepseek refused DEEPSEEK_API_KEY: HTTP 401", stderr)

    def test_the_failure_reaches_release_control_as_the_preparation_record(self):
        """run_in_image.sh keeps the step's stderr and names it; the record is
        what report_preparation_failure posts as the summary's failure."""
        with tempfile.TemporaryDirectory() as directory:
            _, stderr, _ = self.run_command(directory, {"https://api.workers.iii.dev/resolve": [
                preflight.Unavailable("no answer (timed out)")]})
            prepared = pathlib.Path(directory) / "prepare"
            prepared.mkdir()
            (prepared / "failed").write_text("preflight\n")
            (prepared / "preflight.log").write_text(stderr)
            self.assertEqual(prepare_execution.failure_record(prepared), {
                "phase": "preflight", "code": None,
                "message": "cannot reach the registry at https://api.workers.iii.dev: no answer (timed out), for 30s"})


if __name__ == "__main__":
    unittest.main()
