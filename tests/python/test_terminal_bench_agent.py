import asyncio
import base64
import importlib.util
import json
import logging
import os
import shlex
import subprocess
import sys
import tempfile
import threading
import time
import types
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


def fake_modules():
    """Stand-ins for harbor and iii-sdk, which CI does not install."""
    names = ("harbor", "harbor.agents", "harbor.agents.base", "harbor.agents.capabilities",
             "harbor.agents.installed", "harbor.agents.installed.codex", "harbor.models",
             "harbor.models.agent", "harbor.models.agent.context", "iii")
    modules = {name: types.ModuleType(name) for name in names}

    class BaseAgent:
        def __init__(self, logs_dir, model_name=None, logger=None, mcp_servers=None, skills_dir=None, **kwargs):
            self.logs_dir, self.model_name = Path(logs_dir), model_name
            self.logger = logger or logging.getLogger("fake-harbor")
            self.session_id = None
            self.mcp_servers, self.skills_dir = mcp_servers or [], skills_dir

    class ModelUsage:
        def __init__(self, **fields):
            self.__dict__.update(fields)

    class Codex:
        installs = []

        def __init__(self, logs_dir, logger=None, **kwargs):
            self.logs_dir = logs_dir

        async def ensure_system_dependencies(self, environment, dependencies):
            self.installs.append((self.logs_dir, dependencies))

    modules["harbor.agents.base"].BaseAgent = BaseAgent
    modules["harbor.agents.capabilities"].AgentCapabilities = types.SimpleNamespace
    modules["harbor.agents.installed.codex"].Codex = Codex
    modules["harbor.models.agent.context"].ModelUsage = ModelUsage
    modules["iii"].InitOptions = dict
    modules["iii"].register_worker = None
    return modules


with mock.patch.dict(sys.modules, fake_modules()):
    SPEC = importlib.util.spec_from_file_location(
        "iii_harbor_agent", ROOT / "scripts/terminal_bench/iii_harbor_agent.py")
    AGENT = importlib.util.module_from_spec(SPEC)
    SPEC.loader.exec_module(AGENT)
logging.getLogger("fake-harbor").setLevel(logging.CRITICAL)
TRIAL = "regex-log__AbC1234"


def metrics(complete, function_calls=3):
    return {"root_session_id": "x", "complete": complete, "by_session": [], "totals": {
        "input_tokens": 100, "cache_read_tokens": 1000, "cache_write_tokens": 50, "output_tokens": 20,
        "reasoning_tokens": None, "cost_usd": 0.5, "function_calls": function_calls,
        "function_call_errors": 0, "sessions": 1, "turns": 4, "unreported_generations": 0,
        "by_model": [{"provider": "anthropic", "model": "claude-x", "input_tokens": 100,
                      "cache_read_tokens": 1000, "cache_write_tokens": 50, "output_tokens": 20,
                      "cost_usd": 0.5}]}}


def status(state, wake=False, failure=None):
    return {"session_id": "x", "turn_id": "t1", "status": state, "children": [],
            "expects_wake": wake, "open_asks": 0, "failure": failure, "result_error": None}


def assistant(reasoning=0, content=({"type": "text", "text": "ok"},), usage=True):
    # As harness 1.10 stores a reply: stop_reason "end" and the requested model, whatever was served.
    message = {"role": "assistant", "model": "codex/gpt-6.2-sol", "provider": "openai-codex",
               "stop_reason": "end", "content": list(content), "timestamp": 1}
    if usage:
        message["usage"] = {"input": 10, "output": 5, "reasoning": reasoning}
    return {"entry_id": "e", "message": message}


def placeholder():
    """The empty assistant harness 1.10 appends before a provider call that then failed."""
    return assistant(content=(), usage=False)


def context():
    return types.SimpleNamespace(n_input_tokens=None, n_cache_tokens=None, n_output_tokens=None,
                                 cost_usd=None, model_usage=None, metadata=None)


class FakeIII:
    """Answers call_async from a script: a value, a list consumed in order (the
    last item repeats) or a callable of the payload."""

    def __init__(self):
        self.script = {
            "engine::functions::info": {}, "harness::send": {"accepted": True, "session_id": "x", "turn_id": "t1"},
            "harness::system-prompt::get": {"parts": []}, "harness::stop": {"stopping": True},
            "session::messages": {"messages": [{"role": "user"}], "next_cursor": None}}
        self.calls, self.unregistered, self.functions, self.state = [], [], {}, "connected"

    def get_connection_state(self):
        return self.state

    def shutdown(self):
        pass

    def register_function(self, function_id, handler, *, description=None, request_format=None):
        self.functions[function_id] = (handler, description, request_format)
        return types.SimpleNamespace(unregister=lambda: self.unregistered.append(function_id))

    async def call_async(self, request):
        function_id, payload = request["function_id"], request["payload"]
        self.calls.append((function_id, payload))
        value = self.script[function_id]
        if callable(value):
            value = value(payload)
        elif isinstance(value, list):
            value = value.pop(0) if len(value) > 1 else value[0]
        if isinstance(value, Exception):
            raise value
        return value

    def payloads(self, function_id):
        return [payload for called, payload in self.calls if called == function_id]


class HostEnv:
    """Harbor's docker exec contract on the host: bash -c, stderr merged, no stdin."""

    def __init__(self, tmp):
        self.tmp, self.calls = tmp, []

    async def exec(self, command, cwd=None, env=None, timeout_sec=None, user=None):
        self.calls.append({"command": command, "cwd": cwd, "timeout_sec": timeout_sec})
        proc = await asyncio.create_subprocess_exec(
            "bash", "-c", command, cwd=cwd, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT, env={**os.environ, "TMPDIR": self.tmp})
        out, _ = await asyncio.wait_for(proc.communicate(), timeout_sec)
        return types.SimpleNamespace(stdout=out.decode(), stderr=None, return_code=proc.returncode)


class FakeEnv:
    def __init__(self, *results):
        self.results, self.calls = list(results), []

    async def exec(self, command, cwd=None, env=None, timeout_sec=None, user=None):
        self.calls.append({"command": command, "cwd": cwd, "timeout_sec": timeout_sec})
        result = self.results.pop(0) if self.results else types.SimpleNamespace(stdout="", return_code=0)
        if isinstance(result, Exception):
            raise result
        return result


class TerminalBenchAgentTest(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(tmp.name)
        self.exec_tmp = self.tmp / "container-tmp"
        self.exec_tmp.mkdir()
        self.iii = FakeIII()
        AGENT.Codex.installs.clear()
        # CI has no ImageMagick; only the tests that convert an image need the real one.
        for name, value in (("_client", None), ("POLL_S", 0.01), ("SETTLE_S", 0),
                            ("MAGICK", AGENT.MAGICK or "magick")):
            patcher = mock.patch.object(AGENT, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)
        patcher = mock.patch.object(AGENT, "register_worker", return_value=self.iii)
        self.register_worker = patcher.start()
        self.addCleanup(patcher.stop)
        patcher = mock.patch.object(AGENT, "threading")  # the adapter's view only
        self.register_atexit = patcher.start()._register_atexit
        self.addCleanup(patcher.stop)

    def agent(self, **kwargs):
        agent = AGENT.IIIHarnessAgent(self.tmp / TRIAL / "agent", "anthropic/claude-x",
                                      engine_url="ws://127.0.0.1:49634", namespace="tbench", **kwargs)
        agent.session_id = f"{TRIAL}__agent"
        return agent

    async def ready(self, env=None, **kwargs):
        agent = self.agent(**kwargs)
        await agent.setup(env or FakeEnv())
        return agent

    async def test_bridge_delegates_to_environment_exec_from_the_sdk_loop(self):
        sdk_loop = asyncio.new_event_loop()
        thread = threading.Thread(target=sdk_loop.run_forever, daemon=True)
        thread.start()
        self.addCleanup(lambda: (sdk_loop.call_soon_threadsafe(sdk_loop.stop), thread.join(), sdk_loop.close()))
        env = HostEnv(str(self.exec_tmp))
        agent = await self.ready(env)
        self.assertEqual(list(self.iii.functions), [agent._fn_id, agent._view_id])
        self.assertRegex(agent._fn_id, r"^workspace::t[0-9a-f]{12}::exec$")
        self.assertEqual(agent._view_id, agent._fn_id.replace("::exec", "::view_image"))
        handler, description, schema = self.iii.functions[agent._fn_id]
        self.assertEqual((description, schema), (AGENT.EXEC_DESCRIPTION, AGENT.EXEC_SCHEMA))
        self.assertEqual(self.iii.functions[agent._view_id][1:], (AGENT.VIEW_DESCRIPTION, AGENT.VIEW_SCHEMA))
        self.register_worker.assert_called_once_with(
            "ws://127.0.0.1:49634", {"worker_name": f"tbench-harbor-{os.getpid()}", "namespace": "tbench"})
        # The SDK loop thread is non-daemon: without this hook harbor never exits.
        self.register_atexit.assert_called_once_with(self.iii.shutdown)
        self.assertTrue(agent._wrap)
        call = asyncio.run_coroutine_threadsafe(
            handler({"command": "echo out; echo err >&2; exit 3", "_caller_worker_id": "w"}), sdk_loop)
        result = await asyncio.wrap_future(call)
        self.assertEqual({k: result[k] for k in ("exit_code", "output", "timed_out")},
                         {"exit_code": 3, "output": "out\nerr\n", "timed_out": False})
        self.assertEqual(env.calls[-1]["timeout_sec"], AGENT.DEFAULT_T + AGENT.SLACK)
        logged = [json.loads(line) for line in (agent.logs_dir / "exec.jsonl").read_text().splitlines()]
        self.assertEqual((logged[0]["n"], logged[0]["exit_code"]), (1, 3))
        self.assertEqual(list(self.exec_tmp.iterdir()), [])

    async def test_output_is_clipped_to_first_and_last_20_kb_like_codex(self):
        agent = await self.ready(HostEnv(str(self.exec_tmp)))
        result = await agent._exec({"command": "head -c 100000 /dev/zero | tr '\\0' a"})
        marker = "\n[... 60000 bytes omitted ...]\n"
        self.assertEqual(result["output"], "a" * 20000 + marker + "a" * 20000)
        exact = await agent._exec({"command": "head -c 40000 /dev/zero | tr '\\0' a"})
        self.assertEqual(exact["output"], "a" * 40000)

    async def test_the_wrapper_shell_writes_nothing_but_the_status_line_and_the_output(self):
        # A job killed by a signal makes the wrapper bash report it on its own stderr,
        # like the "[1]+ Done ..." notices Harbor's merged streams put after MARK.
        env = HostEnv(str(self.exec_tmp))
        wrapped = AGENT.WRAP.format(q=shlex.quote("echo out; kill -KILL $$"), t=5,
                                    clip=AGENT.CLIP, half=AGENT.CLIP // 2)
        raw = await env.exec(wrapped, timeout_sec=20)
        self.assertEqual(raw.stdout, f"{AGENT.MARK}done 137\nout\n")

    async def test_a_command_that_deletes_its_own_log_says_so(self):
        agent = await self.ready(HostEnv(str(self.exec_tmp)))
        result = await agent._exec({"command": 'echo before; rm -f "$TMPDIR"/tmp.*; echo after'})
        self.assertEqual(result["exit_code"], 0)
        self.assertRegex(result["output"], r"^\[output lost: the command removed .*/tmp\.\w+\]\n$")

    async def test_a_malformed_timeout_falls_back_to_the_default(self):
        agent = await self.ready(HostEnv(str(self.exec_tmp)))
        for bad in ("120s", "1.5", [5]):
            result = await agent._exec({"command": "true", "timeout_sec": bad})
            self.assertEqual(result["exit_code"], 0)
        logged = [json.loads(line) for line in (agent.logs_dir / "exec.jsonl").read_text().splitlines()]
        self.assertEqual([entry["timeout_sec"] for entry in logged], [AGENT.DEFAULT_T] * 3)

    async def test_timeout_keeps_the_process_and_returns_its_partial_output(self):
        agent = await self.ready(HostEnv(str(self.exec_tmp)))
        result = await agent._exec({"command": "echo started; sleep 2; echo late", "timeout_sec": 1})
        self.addCleanup(lambda: Path(result["log_path"]).unlink(missing_ok=True))
        self.addCleanup(lambda: subprocess.run(["kill", "-9", str(result["pid"])], stderr=subprocess.DEVNULL))
        self.assertEqual({k: result[k] for k in ("exit_code", "output", "timed_out")},
                         {"exit_code": None, "output": "started\n", "timed_out": True})
        os.kill(result["pid"], 0)
        log = Path(result["log_path"])
        deadline = time.monotonic() + 10
        while "late" not in log.read_text() and time.monotonic() < deadline:
            await asyncio.sleep(0.1)
        self.assertEqual(log.read_text(), "started\nlate\n")

    async def test_exec_clamps_timeouts_rejects_empty_commands_and_survives_harbor_timeouts(self):
        env = FakeEnv(*[types.SimpleNamespace(stdout="", return_code=0)] * 3,  # probe, tools before and after
                      types.SimpleNamespace(stdout=f"WARN[0000] compose\n[1]+  Done  bash -c 'x' > \"$f\"\n{AGENT.MARK}done 3\nok\n", return_code=0),
                      types.SimpleNamespace(stdout="cwd missing\n", return_code=126),
                      RuntimeError("Command timed out after 120 seconds"), RuntimeError("docker died"))
        agent = await self.ready(env)
        noisy = await agent._exec({"command": "x", "timeout_sec": 999})
        self.assertEqual((noisy["exit_code"], noisy["output"]), (3, "ok\n"))
        self.assertEqual(env.calls[-1]["timeout_sec"], AGENT.MAX_T + AGENT.SLACK)
        self.assertEqual((await agent._exec({"command": "x", "cwd": "/nope"}))["exit_code"], 126)
        calls = len(env.calls)
        self.assertIn("non-empty", (await agent._exec({"command": "  "}))["output"])
        self.assertEqual(len(env.calls), calls)
        agent._wrap = False
        lost = await agent._exec({"command": "x"})
        self.assertEqual((lost["timed_out"], env.calls[-1]["command"]), (True, "x"))
        with self.assertRaisesRegex(RuntimeError, "docker died"):
            await agent._exec({"command": "x"})

    async def test_completion_waits_for_a_running_child_and_a_parked_root(self):
        agent = await self.ready()
        # Poll 1: root done, a child still running. Poll 2: the tree settled but the root
        # expects a wake (a queued reseed). Poll 3: done.
        self.iii.script["harness::metrics"] = [metrics(False), metrics(True), metrics(True)]
        sent_file = agent.logs_dir / "iii-send.json"
        polled_with_send_file = []

        polls = [status("completed"), status("completed", wake=True), status("completed")]

        def poll(payload):
            if payload.get("verbose"):
                return status("completed")
            polled_with_send_file.append(sent_file.exists())
            return polls.pop(0)
        self.iii.script["harness::status"] = poll
        self.iii.script["session::messages"] = {"next_cursor": None, "messages": [
            {"entry_id": "u", "message": {"role": "user"}}, assistant(reasoning=7), {"entry_id": "c", "message": {}},
            assistant(), placeholder(), assistant(reasoning=3)]}
        ctx = context()
        await agent.run("Fix the regex.", FakeEnv(), ctx)
        # The model can read logs_dir from the container: the send payload lands only at the end.
        self.assertEqual(polled_with_send_file, [False, False, False])
        self.assertEqual(json.loads(sent_file.read_text())["response"]["turn_id"], "t1")
        lean = [p for p in self.iii.payloads("harness::status") if not p.get("verbose")]
        self.assertEqual(len(lean), 3)
        self.assertEqual(self.iii.payloads("harness::stop"), [])
        self.assertEqual(self.iii.unregistered, [agent._fn_id, agent._view_id])
        self.assertTrue(all(p["omit_traces"] for p in self.iii.payloads("harness::metrics")))
        send = self.iii.payloads("harness::send")[0]
        self.assertEqual(send["session"]["metadata"]["e2e_run_id"], TRIAL)
        self.assertEqual(send["session_id"], agent._iii_session_id)
        for name in ("iii-send.json", "system-prompt.json", "metrics.json", "status.json", "transcript.json"):
            self.assertTrue((agent.logs_dir / name).exists(), name)
        self.assertEqual(ctx.metadata["status"], "completed")
        self.assertFalse(ctx.metadata["cancelled_by_harbor"])
        self.assertEqual(ctx.metadata["thinking_level"], "provider default")
        self.assertEqual({k: ctx.metadata[k] for k in (
            "models_seen", "generations", "reasoning_generations", "first_model_response")},
            {"models_seen": ["codex/gpt-6.2-sol"], "generations": 3, "reasoning_generations": 2,
             "first_model_response": True})

    async def test_setup_installs_codex_packages_before_the_bridge_and_records_the_tools(self):
        env = FakeEnv(types.SimpleNamespace(stdout="", return_code=0),
                      types.SimpleNamespace(stdout="WARN[0000] node noise\ncurl\n", return_code=0),
                      types.SimpleNamespace(stdout="curl\nrg\nnode\nnpm\n", return_code=0))
        agent = await self.ready(env)
        self.assertEqual(AGENT.Codex.installs, [(agent.logs_dir, ("curl", "bash", "nodejs", "npm", "ripgrep"))])
        self.assertEqual([c["command"] for c in env.calls], [AGENT.PROBE, AGENT.TOOLS_PROBE, AGENT.TOOLS_PROBE])
        self.assertEqual(agent._base_tools, {"curl": True, "rg": False, "node": False, "npm": False})
        self.assertTrue(agent._packages_installed)
        self.iii.script["harness::metrics"] = metrics(True)
        self.iii.script["harness::status"] = status("completed")
        ctx = context()
        await agent.run("x", FakeEnv(), ctx)
        self.assertEqual((ctx.metadata["base_tools"]["curl"], ctx.metadata["packages_installed"]), (True, True))
        # An install that fails fails setup the way it fails Codex's, with no bridge left behind.
        self.iii = FakeIII()
        self.register_worker.return_value = self.iii
        with mock.patch.object(AGENT, "_client", None), mock.patch.object(
                AGENT.Codex, "ensure_system_dependencies", side_effect=RuntimeError("apt-get failed")):
            with self.assertRaisesRegex(RuntimeError, "apt-get failed"):
                await self.agent().setup(FakeEnv())
        self.assertEqual(self.iii.functions, {})

    def test_transcript_stats_count_only_filled_replies(self):
        user = {"entry_id": "u", "message": {"role": "user"}}
        zero_output = assistant(content=())
        zero_output["message"]["usage"]["output"] = 0
        # The safety filter's cut reply has text but no usage: still a model response.
        stats = AGENT.transcript_stats({"messages": [
            user, placeholder(), assistant(reasoning=2), assistant(usage=False), zero_output, {"entry_id": "x"}]}, {})
        self.assertEqual(stats, {"models_seen": ["codex/gpt-6.2-sol"], "generations": 2,
                                 "reasoning_generations": 1, "first_model_response": True})
        # Every provider call failed: only the unfilled placeholders are stored.
        failed_only = AGENT.transcript_stats({"messages": [user, placeholder(), placeholder()]}, {"output_tokens": 0})
        self.assertEqual(failed_only, {"models_seen": [], "generations": 0, "reasoning_generations": 0,
                                       "first_model_response": False})
        self.assertEqual(AGENT.transcript_stats({"messages": []}, {})["models_seen"], [])
        self.assertEqual(AGENT.transcript_stats(None, {"output_tokens": 20}), {
            "models_seen": None, "generations": None, "reasoning_generations": None, "first_model_response": True})
        self.assertIsNone(AGENT.transcript_stats(None, {})["first_model_response"])

    async def test_harbor_cancellation_stops_the_tree_then_collects_and_unregisters(self):
        agent = await self.ready()
        self.iii.script["harness::metrics"] = lambda _: metrics(bool(self.iii.payloads("harness::stop")))
        self.iii.script["harness::status"] = status("running")
        ctx = context()
        with self.assertRaises(asyncio.TimeoutError):
            await asyncio.wait_for(agent.run("Fix the regex.", FakeEnv(), ctx), 0.2)
        self.assertEqual(self.iii.payloads("harness::stop"),
                         [{"session_id": agent._iii_session_id, "quiet_deadlines": True}])
        stop_at = next(i for i, (f, _) in enumerate(self.iii.calls) if f == "harness::stop")
        after = [p for f, p in self.iii.calls[stop_at:] if f == "harness::metrics"]
        self.assertEqual(after[-1], {"root_session_id": agent._iii_session_id,
                                     "omit_context": False, "omit_traces": True})
        self.assertTrue(all(p["omit_traces"] for p in self.iii.payloads("harness::metrics")))
        self.assertEqual(self.iii.unregistered, [agent._fn_id, agent._view_id])
        self.assertTrue(ctx.metadata["cancelled_by_harbor"])
        self.assertEqual(ctx.n_output_tokens, 20)
        self.assertTrue((agent.logs_dir / "transcript.json").exists())

    async def test_idempotency_key_differs_between_two_instances_of_one_trial(self):
        first, second = self.agent(), self.agent()
        keys = [a.send_payload("x", TRIAL, "regex-log")["idempotency_key"] for a in (first, second)]
        self.assertNotEqual(keys[0], keys[1])
        self.assertTrue(all(k.startswith(f"tbench:{TRIAL}:") for k in keys))
        self.assertNotEqual(first._iii_session_id, second._iii_session_id)
        self.assertRegex(first._iii_session_id, r"^s_[0-9a-f]{12}$")

    async def test_failure_codes_map_to_harbor_error_policy_names(self):
        cases = [
            ("router/provider_rate_limited", True, "ApiRateLimitError"),

            ("router/provider_transient", True, "ApiOverloadedError"),
            ("llm.context_overflow", False, "ContextWindowExceededError"),
            ("harness.context_overflow", False, "ContextWindowExceededError"),
            ("router/provider_auth_expired", False, "AgentAuthenticationError"),
            ("router/not_configured", False, "AgentAuthenticationError"),
            ("harness.unknown_model", False, "ModelNotFoundError"),
            ("router/no_provider_for_model", False, "ModelNotFoundError"),
            ("router/ambiguous_model", False, "ModelNotFoundError"),
            ("router/unknown_provider", False, "ModelNotFoundError"),
            # Only those exact codes: another code naming a model takes the retryable rule.
            ("provider/model_overloaded", True, "ApiOverloadedError"),
            ("router/stream_idle_timeout", False, "ApiOverloadedError"),
            ("router/provider_unavailable", False, "ApiOverloadedError"),
            ("router/provider_rejected", False, "ApiError"),
            # Harness and model failures count as a 0: Harbor keeps invalid trials out of the mean.
            ("harness.turn_internal", False, "HarnessTurnError"),
            # llm-router's own in-flight cap, retryable but not the account's quota.
            ("router/capacity_exceeded", True, "HarnessTurnError"),
            ("router/invalid_request", False, "HarnessTurnError"),
            ("llm.generation_failed", False, "HarnessTurnError")]
        for code, retryable, name in cases:
            error = AGENT.turn_error(status("failed", failure={"code": code, "retryable": retryable}))
            self.assertEqual(type(error).__name__, name, code)
        self.assertEqual(type(AGENT.turn_error(status("cancelled"))).__name__, "HarnessTurnError")
        agent = await self.ready()
        self.iii.script["harness::metrics"] = metrics(True)
        self.iii.script["harness::status"] = status(
            "failed", failure={"code": "router/provider_rate_limited", "retryable": True})
        with self.assertRaises(AGENT.ApiRateLimitError):
            await agent.run("x", FakeEnv(), context())
        self.assertEqual(self.iii.payloads("harness::stop"), [])
        # A send that timed out may have started the turn on the engine: stop it.
        self.iii.script["harness::send"] = RuntimeError("TIMEOUT")
        agent = await self.ready()
        with self.assertRaises(AGENT.ApiConnectionError):
            await agent.run("x", FakeEnv(), context())
        self.assertEqual(self.iii.payloads("harness::stop"),
                         [{"session_id": agent._iii_session_id, "quiet_deadlines": True}])
        self.iii.script["harness::send"] = {"accepted": True}
        self.iii.script["harness::status"] = None
        with self.assertRaisesRegex(AGENT.ApiError, "record is gone"):
            await (await self.ready()).run("x", FakeEnv(), context())

    async def test_zero_function_calls_and_open_asks_count_as_harness_failures(self):
        agent = await self.ready()
        self.iii.script["harness::metrics"] = metrics(True, function_calls=0)
        self.iii.script["harness::status"] = status("completed")
        with self.assertRaisesRegex(AGENT.HarnessTurnError, "without a single function call"):
            await agent.run("x", FakeEnv(), context())
        self.assertEqual(self.iii.payloads("harness::stop"), [])
        self.iii.script["harness::status"] = {**status("running"), "open_asks": 1}
        agent = await self.ready()
        with self.assertRaisesRegex(AGENT.HarnessTurnError, "human ask"):
            await agent.run("x", FakeEnv(), context())
        self.assertEqual(len(self.iii.payloads("harness::stop")), 1)

    async def test_setup_fails_fast_on_a_dead_engine_and_unregisters_when_cancelled(self):
        self.iii.state = "reconnecting"
        with self.assertRaises(AGENT.ApiConnectionError):
            await self.agent().setup(FakeEnv())
        self.assertEqual(self.iii.functions, {})
        self.iii.state = "connected"
        self.iii.script["engine::functions::info"] = RuntimeError("not found")
        agent = self.agent()
        with self.assertRaises(asyncio.TimeoutError):
            await asyncio.wait_for(agent.setup(FakeEnv()), 0.3)
        self.assertEqual(self.iii.unregistered, [agent._fn_id, agent._view_id])

    async def test_metadata_survives_a_cleanup_cut_short(self):
        agent = await self.ready()
        self.iii.script["harness::metrics"] = metrics(True)
        self.iii.script["harness::status"] = status("completed")

        async def hang():
            await asyncio.sleep(10)
        agent._transcript = hang
        ctx = context()
        with mock.patch.object(AGENT, "CLEANUP_S", 0.2):
            await agent.run("x", FakeEnv(), ctx)
        self.assertEqual((ctx.metadata["status"], ctx.metadata["function_calls"]), ("completed", 3))
        self.assertEqual((ctx.metadata["models_seen"], ctx.metadata["first_model_response"]), (None, True))
        self.assertTrue((agent.logs_dir / "status.json").exists())
        self.assertTrue((agent.logs_dir / "iii-send.json").exists())
        self.assertFalse((agent.logs_dir / "transcript.json").exists())
        self.assertEqual(self.iii.unregistered, [agent._fn_id, agent._view_id])

    async def test_wrapped_commands_keep_the_plain_shell_sigint_disposition(self):
        # bash ignores SIGINT in background jobs unless job control is on.
        agent = await self.ready(HostEnv(str(self.exec_tmp)))
        cmd = f"{shlex.quote(sys.executable)} -c 'import signal; print(signal.getsignal(signal.SIGINT))'"
        wrapped = (await agent._exec({"command": cmd}))["output"]
        agent._wrap = False
        self.assertEqual(wrapped, (await agent._exec({"command": cmd}))["output"])
        self.assertIn("default_int_handler", wrapped)

    async def test_view_image_reports_what_it_cannot_show_as_an_error_result(self):
        agent = await self.ready(HostEnv(str(self.exec_tmp)))
        handler = self.iii.functions[agent._view_id][0]
        (self.tmp / "notes.txt").write_text("hello")
        for payload, reason in (({"path": str(self.tmp / "missing.png")}, "no readable file at "),
                                ({"path": str(self.tmp / "notes.txt")}, "invalid or unsupported image data"),
                                ({"path": " "}, "path must be a non-empty string"), ("x", "path must be")):
            out = await handler(payload)
            self.assertTrue(out["is_error"])
            self.assertEqual(len(out["content"]), 1)
            self.assertIn(reason, out["content"][0]["text"])
        # Docker's warnings come before the status line, as for exec; a wrapper that never ran says so.
        agent._env = FakeEnv(types.SimpleNamespace(stdout="WARN[0000] noise\n@@iii-tb no readable file at /x\n",
                                                   return_code=0),
                             types.SimpleNamespace(stdout="OCI runtime exec failed", return_code=126))
        self.assertIn("no readable file at /x", (await agent._view_image({"path": "/x"}))["content"][0]["text"])
        self.assertIn("the read failed: OCI runtime", (await agent._view_image({"path": "/x"}))["content"][0]["text"])
        logged = [json.loads(line) for line in (agent.logs_dir / "exec.jsonl").read_text().splitlines()]
        self.assertEqual([entry["n"] for entry in logged], list(range(1, 7)))
        self.assertEqual(logged[0]["view_image"], str(self.tmp / "missing.png"))
        self.assertIn("no readable file", logged[0]["error"])

    @unittest.skipUnless(AGENT.MAGICK, "needs ImageMagick 7 (magick)")
    async def test_view_image_sends_small_files_as_they_are_and_fits_the_rest_under_the_result_cap(self):
        agent = await self.ready(HostEnv(str(self.exec_tmp)))
        handler = self.iii.functions[agent._view_id][0]

        def image(name, *spec):
            subprocess.run([AGENT.MAGICK, *spec, str(self.tmp / name)], check=True)
            return str(self.tmp / name)

        def size(data):
            return subprocess.run([AGENT.MAGICK, "identify", "-format", "%w %h", "-"], input=base64.b64decode(data),
                                  capture_output=True, check=True).stdout.decode()

        small = image("small.png", "-size", "40x20", "xc:red")
        out = await handler({"path": small})
        self.assertNotIn("is_error", out)
        self.assertEqual(out["content"], [
            {"type": "text", "text": f"{small}: 40x20. {AGENT.VIEW_NOTE}"},
            {"type": "image", "mime": "image/png", "data": base64.b64encode(Path(small).read_bytes()).decode()}])
        # A 1.4 MB noisy drawing goes as WebP; a 7000 px wide BMP also passes Codex's 6000 px side.
        big = await asyncio.to_thread(image, "big.png", "-seed", "1", "-size", "1400x1000", "plasma:")
        wide = image("wide.bmp", "-size", "7000x30", "xc:white")
        for path, source in ((big, "1400x1000"), (wide, "7000x30")):
            out = await handler({"path": path})
            text, picture = out["content"]
            self.assertEqual(picture["mime"], "image/webp")
            self.assertLessEqual(len(base64.b64decode(picture["data"])), AGENT.MAX_IMAGE_BYTES)
            shown = size(picture["data"]).replace(" ", "x")
            self.assertEqual(text["text"], f"{path}: {source}, shown downscaled to {shown}. {AGENT.VIEW_NOTE}")
            # Harness 1.10 caps content plus details (the whole return value) at 256 KiB.
            self.assertLess(len(json.dumps([out["content"], out])), 256 * 1024)
        self.assertEqual(shown, "6000x25")
        logged = json.loads((agent.logs_dir / "exec.jsonl").read_text().splitlines()[-1])
        self.assertEqual((logged["size"], logged["shown"], logged["mime"]), ([7000, 30], [6000, 25], "image/webp"))
        bad = self.tmp / "bad.png"
        bad.write_bytes(b"\x89PNG\r\n\x1a\njunk")
        out = await handler({"path": str(bad)})
        self.assertTrue(out["is_error"])
        self.assertIn("invalid or unsupported image data (", out["content"][0]["text"])

    def test_usage_fills_the_agent_context(self):
        ctx = context()
        m = metrics(True)
        m["totals"]["by_model"].append({"provider": "openai", "model": "gpt-x", "input_tokens": 7,
                                        "cache_read_tokens": 3, "output_tokens": None, "cost_usd": None})
        self.agent()._fill(ctx, m)
        self.assertEqual((ctx.n_input_tokens, ctx.n_cache_tokens, ctx.n_output_tokens, ctx.cost_usd),
                         (1150, 1000, 20, 0.5))
        self.assertEqual(sorted(ctx.model_usage), ["anthropic/claude-x", "openai/gpt-x"])
        self.assertEqual(ctx.model_usage["anthropic/claude-x"].n_input_tokens, 1150)
        self.assertEqual(vars(ctx.model_usage["openai/gpt-x"]),
                         {"n_input_tokens": 10, "n_cache_tokens": 3, "n_output_tokens": 0, "cost_usd": None})
        m["totals"].update(input_tokens=None, by_model=[])
        self.agent()._fill(ctx, m)
        self.assertEqual((ctx.n_input_tokens, ctx.model_usage), (None, None))

    def test_send_stamps_the_session_and_builds_both_prompt_modes(self):
        stock = self.agent()
        payload = stock.send_payload("Fix the regex.", TRIAL, "regex-log")
        self.assertEqual(payload["session"], {
            "kind": "e2e", "title": "Harness E2E: regex-log",
            "metadata": {"e2e_run_id": TRIAL, "e2e_scenario": "regex-log", "e2e_execution_kind": "harbor"}})
        self.assertEqual((payload["provider"], payload["model"]), ("anthropic", "claude-x"))
        self.assertEqual(payload["options"], {"functions": {"allow": [stock._fn_id, stock._view_id]},
                                              "max_turns": 1_000_000})
        self.assertTrue(payload["message"].startswith("Fix the regex.\n\n"))
        self.assertIn(f'agent_call with {{"function": "{stock._fn_id}"', payload["message"])
        self.assertIn(f'call {stock._view_id} the same way, with the payload {{"path": ', payload["message"])
        override = self.agent(prompt="override", thinking_level="high", max_turns=200)
        payload = override.send_payload("Fix the regex.", TRIAL, "regex-log")
        self.assertEqual(payload["message"], "Fix the regex.")
        self.assertEqual(payload["options"], {
            "functions": {"allow": [override._fn_id, override._view_id], "expose": "native"}, "max_turns": 200,
            "system_prompt": AGENT.TB_SYSTEM_PROMPT, "system_prompt_strategy": "override",
            "thinking_level": "high"})
        for kwargs in ({"prompt": "terminus"}, {"engine_url": None}, {"namespace": ""}, {"thinking_level": "max"}):
            with self.assertRaises(ValueError):
                AGENT.IIIHarnessAgent(self.tmp, "anthropic/claude-x", **{
                    "engine_url": "ws://127.0.0.1:49634", "namespace": "tbench", **kwargs})
        with self.assertRaises(ValueError):
            AGENT.IIIHarnessAgent(self.tmp, "claude-x", engine_url="ws://x", namespace="tbench")
        with mock.patch.object(AGENT, "MAGICK", None), self.assertRaisesRegex(ValueError, "ImageMagick"):
            self.agent()

    def test_skill_note_goes_after_the_frontmatter(self):
        note = "\nNOTE\n"
        self.assertEqual(AGENT.with_note("---\nname: a\n---\nBody\n", note), "---\nname: a\n---\n\nNOTE\nBody\n")
        self.assertEqual(AGENT.with_note("---\nname: a\n---", note), "---\nname: a\n---\nNOTE\n")
        self.assertEqual(AGENT.with_note("# Title\n", note), "NOTE\n# Title\n")

    async def test_task_skills_load_under_trial_ids_and_only_this_session_lists_them(self):
        root = self.exec_tmp / "app/.agents/skills"
        (root / "Perf Audit").mkdir(parents=True)
        (root / "Perf Audit/SKILL.md").write_text("---\nname: perf\ndescription: Measure CLS\n---\nRun scripts/cls.mjs\n")
        (root / "no-skill").mkdir()  # a folder without SKILL.md is not a skill
        self.iii.script.update({"directory::skills::create": {}, "directory::skills::delete": {}})
        agent = await self.ready(HostEnv(str(self.exec_tmp)), skills_dir=f"{root}/")
        skill_id = f"t{agent._hex}-perf-audit"
        self.assertEqual(self.iii.payloads("directory::skills::create"), [{"id": skill_id, "content": (
            "---\nname: perf\ndescription: Measure CLS\n---\n"
            + AGENT.SKILL_NOTE.format(dir=f"{root}/Perf Audit") + "Run scripts/cls.mjs\n")}])
        options = agent.send_payload("x", TRIAL, "t")["options"]
        self.assertEqual(options["skills"], [skill_id])
        self.assertEqual(options["functions"]["allow"], [agent._fn_id, agent._view_id, "directory::skills::get"])
        self.iii.script.update({"harness::metrics": metrics(True), "harness::status": status("completed")})
        ctx = context()
        await agent.run("x", FakeEnv(), ctx)
        self.assertEqual(ctx.metadata["skills"], [skill_id])
        self.assertEqual(self.iii.payloads("directory::skills::delete"), [{"id": skill_id}])
        # A task without skills gets no index: the get function is not allowed.
        self.assertEqual(self.agent().send_payload("x", TRIAL, "t")["options"]["functions"]["allow"][2:], [])

    def mcp_stack(self, states):
        """mcp-client and configuration as the adapter sees them; states feeds servers::list."""
        config = {"id": "tbench-mcp-client", "value": {"mcpServers": {"kept": {"type": "http", "url": "https://x"}}}}

        def validate(payload):
            (name, entry), = payload["mcpServers"].items()
            return {"servers": {name: {"ok": True, "errors": [], "warnings": [], "fingerprint": "f" * 64}}}

        def put(payload):
            config["value"] = payload["value"]
            return {}

        def listed(payload):
            state = states.pop(0) if len(states) > 1 else states[0]
            return {"servers": [{"name": name, "state": state, "tools_listed": state == "ready",
                                 "tools": [{"name": "browser_click", "function_id": f"mcp::{name}::browser_click",
                                            "registered": True},
                                           {"name": "skipped", "function_id": f"mcp::{name}::skipped",
                                            "registered": False}]}
                                for name in config["value"]["mcpServers"] if name != "kept"]}
        self.iii.script.update({
            "mcp-client::config::validate": validate, "mcp-client::servers::approve": {},
            "configuration::get": lambda _: json.loads(json.dumps(config)), "configuration::set": put,
            "mcp-client::servers::list": listed})
        return config

    async def test_task_mcp_servers_reach_the_session_through_a_loopback_forwarder(self):
        async def sidecar(reader, writer):
            writer.write(b"pong " + await reader.readline())
            await writer.drain()
            writer.close()
        upstream = await asyncio.start_server(sidecar, "127.0.0.1", 0)
        self.addAsyncCleanup(upstream.wait_closed)
        self.addCleanup(upstream.close)
        port = upstream.sockets[0].getsockname()[1]
        config = self.mcp_stack(["connecting", "ready"])
        server = types.SimpleNamespace(name="playwright", transport="sse", url=f"http://localhost:{port}/sse",
                                       headers={})
        agent = await self.ready(HostEnv(str(self.exec_tmp)), mcp_servers=[server])
        name = f"t{agent._hex}-playwright"
        entry = config["value"]["mcpServers"][name]
        self.assertEqual(len(name), 24)
        self.assertEqual(self.iii.payloads("mcp-client::servers::approve"), [{"server": name, "fingerprint": "f" * 64}])
        self.assertRegex(entry["url"], r"^http://127\.0\.0\.1:\d+/mcp$")
        self.assertIn("kept", config["value"]["mcpServers"])
        reader, writer = await asyncio.open_connection("127.0.0.1", int(entry["url"].split(":")[2].split("/")[0]))
        writer.write(b"ping\n")
        await writer.drain()
        self.assertEqual(await reader.read(), b"pong ping\n")
        writer.close()
        payload = agent.send_payload("x", TRIAL, "t")
        self.assertEqual(payload["options"]["functions"]["allow"],
                         [agent._fn_id, agent._view_id, f"mcp::{name}::*", "engine::functions::info"])
        self.assertIn(f"MCP server playwright is connected through these functions, called the same way: "
                      f"mcp::{name}::browser_click.", payload["message"])
        self.iii.script.update({"harness::metrics": metrics(True), "harness::status": status("completed")})
        ctx = context()
        await agent.run("x", FakeEnv(), ctx)
        self.assertEqual(ctx.metadata["mcp_servers"], [{"name": "playwright", "server": name, "tools": 1}])
        self.assertEqual(list(config["value"]["mcpServers"]), ["kept"])
        with self.assertRaises(OSError):
            await asyncio.open_connection("127.0.0.1", int(entry["url"].split(":")[2].split("/")[0]))

    async def test_an_mcp_server_that_never_gets_ready_fails_setup_and_leaves_nothing_behind(self):
        upstream = await asyncio.start_server(lambda r, w: w.close(), "127.0.0.1", 0)
        self.addAsyncCleanup(upstream.wait_closed)
        self.addCleanup(upstream.close)
        config = self.mcp_stack(["connecting"])
        url = f"http://localhost:{upstream.sockets[0].getsockname()[1]}/mcp"
        agent = self.agent(mcp_servers=[types.SimpleNamespace(name="pw", transport="streamable-http", url=url,
                                                              headers={})])
        with mock.patch.object(AGENT, "MCP_READY_S", 0.2), self.assertRaisesRegex(AGENT.ApiConnectionError, "connecting"):
            await agent.setup(HostEnv(str(self.exec_tmp)))
        self.assertEqual(list(config["value"]["mcpServers"]), ["kept"])
        self.assertEqual(self.iii.unregistered, [agent._fn_id, agent._view_id])
        # Unreachable, stdio and header-carrying servers are refused before mcp-client hears of them.
        for server, error in ((types.SimpleNamespace(name="a", transport="sse", url="http://localhost:9/sse",
                                                     headers={}), AGENT.ApiConnectionError),
                              (types.SimpleNamespace(name="b", transport="stdio", url=None, headers={}), ValueError),
                              (types.SimpleNamespace(name="c", transport="sse", url=url, headers={"X": "1"}),
                               ValueError)):
            with self.assertRaises(error):
                await self.agent(mcp_servers=[server]).setup(HostEnv(str(self.exec_tmp)))
        self.assertEqual(len(self.iii.payloads("mcp-client::servers::approve")), 1)


if __name__ == "__main__":
    unittest.main()
