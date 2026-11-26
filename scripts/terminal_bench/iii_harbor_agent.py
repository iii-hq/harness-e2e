"""Harbor external agent that runs the iii harness on Terminal-Bench.

Harbor owns the task container and its verifier. Per trial this agent
installs the system packages Harbor's Codex agent installs, registers two
bridge functions on an isolated iii engine (a shell executor and view_image),
loads the task's skills into iii-directory and connects its MCP servers through
mcp-client, sends the task to `harness::send` with only those allowed, waits
for the session tree, and fills Harbor's AgentContext from `harness::metrics`.

    PYTHONPATH=scripts/terminal_bench harbor run -d terminal-bench@2.0 \
      -a iii_harbor_agent:IIIHarnessAgent -m anthropic/<router catalog id> \
      --ak engine_url=ws://127.0.0.1:49634 --ak namespace=tbench [--ak prompt=override]
"""
import asyncio
import base64
import hashlib
import json
import math
import os
import re
import shlex
import shutil
import threading
import time
import uuid
from urllib.parse import urlsplit

from harbor.agents.base import BaseAgent
from harbor.agents.capabilities import AgentCapabilities
from harbor.agents.installed.codex import Codex
from harbor.models.agent.context import ModelUsage
from iii import InitOptions, register_worker

MAX_T, DEFAULT_T, SLACK = 270, 120, 15  # harness dispatch_timeout_ms is 300000
# Codex CLI 0.170.0 cuts exec output at 10000 tokens, kept as the first and last ~20 KB.
CLIP = 40_000
POLL_S, SETTLE_S, CLEANUP_S = 5, 1, 45
CALL_TIMEOUT_MS, PROBE_CALL_MS, CLEANUP_CALL_MS = 30_000, 2_000, 10_000
TERMINAL = ("completed", "cancelled", "failed")
PROBE = "type mktemp head tail wc sleep >/dev/null 2>&1"
MARK = "@@iii-tb "
# The command runs detached with its output in a file, so a timeout neither
# kills it nor loses what it printed, and a background child cannot hold
# Harbor's exec pipe open. The status line is MARK + "done <rc>" or
# "running <pid> <log>"; docker's stderr (compose warnings) may come before it.
# set -m: without job control bash starts a background job with SIGINT and
# SIGQUIT ignored, and nothing the model runs could be interrupted. It also has
# bash print job notices ("[1]+ Done bash -c '<command>'") on its stderr, even
# after MARK, so the wrapper drops its own stderr once the job owns "$f".
WRAP = (
    'set -m; f=$(mktemp) || exit 125\n'
    'bash -c {q} >"$f" 2>&1 </dev/null &\n'
    'p=$!; exec 2>/dev/null; end=$((SECONDS+{t}))\n'
    'while kill -0 $p && [ $SECONDS -lt $end ]; do sleep 0.1 || sleep 1; done\n'
    'if kill -0 $p; then h="running $p $f"; else wait $p; h="done $?"; fi\n'
    'echo "' + MARK + '$h"; [ -f "$f" ] || {{ echo "[output lost: the command removed $f]"; exit 0; }}\n'
    's=$(wc -c <"$f")\n'
    'if [ "$s" -le {clip} ]; then cat "$f"; else head -c {half} "$f"; '
    "printf '\\n[... %d bytes omitted ...]\\n' $((s-{clip})); tail -c {half} \"$f\"; fi\n"
    'case $h in done*) rm -f "$f";; esac'
)
EXEC_SCHEMA = {"type": "object", "required": ["command"], "properties": {
    "command": {"type": "string", "description": "Bash script run with bash -c in the task container"},
    "timeout_sec": {"type": "integer", "minimum": 1, "maximum": MAX_T, "default": DEFAULT_T},
    "cwd": {"type": "string", "description": "Absolute directory in the container; default: the task working directory"}}}
EXEC_DESCRIPTION = (
    "Run a bash command inside the task's Linux container (the machine the task is about). "
    "Each call is a fresh non-interactive `bash -c`: cd, exports and variables do not persist; "
    "chain with && or pass cwd. Returns exit_code, output (stdout and stderr merged; first and "
    "last 20 KB when long) and timed_out. timeout_sec defaults to 120, max 270. A command still "
    "running at timeout_sec is not killed: it keeps running, and the result has timed_out true, "
    "its output so far, its pid and log_path; read the log or kill it in later calls (it leads its "
    "own process group, so kill -- -PID ends the whole tree). Start "
    "servers or long jobs in the background with output redirected (nohup CMD > /tmp/job.log "
    "2>&1 &). There is no TTY: interactive programs (vim, less, REPLs, password or y/n prompts) "
    "do not work, so use non-interactive flags, here-docs or scripts.")
# Codex CLI 0.170.0's view_image takes only a path and sends the file at "original" detail:
# its own bytes unless a side passes 6000 px or the image passes 10000 32x32 patches.
# Harness 1.10 replaces a function result over 256 KiB (content plus details, and details
# repeats the image) with a marker, so a file over MAX_IMAGE_BYTES goes as WebP, downscaled
# only as far as it must be. The context manager shows a result image for one step only.
MAX_SIDE, MAX_PIXELS = 6000, 10_000 * 32 * 32
MAX_IMAGE_BYTES, MAX_FILE_BYTES, MAX_SOURCE_PIXELS = 90_000, 64 << 20, 40_000_000
MAGICK = shutil.which("magick")
# Decoded on the host with an explicit ImageMagick coder, so a file never picks its own decoder.
IMAGE_TYPES = ((b"\x89PNG\r\n\x1a\n", "PNG"), (b"\xff\xd8\xff", "JPEG"), (b"GIF87a", "GIF"),
               (b"GIF89a", "GIF"), (b"BM", "BMP"), (b"II*\0", "TIFF"), (b"MM\0*", "TIFF"),
               *((b"P%d" % i, "PNM") for i in range(1, 7)))
SENT_AS_IS = {"PNG": "image/png", "JPEG": "image/jpeg", "WEBP": "image/webp"}
READ_IMAGE = (
    'f={q}; [ -f "$f" ] && [ -r "$f" ] || {{ echo "' + MARK + 'no readable file at $f"; exit 0; }}\n'
    'command -v base64 >/dev/null || {{ echo "' + MARK + 'the container has no base64"; exit 0; }}\n'
    's=$(wc -c <"$f"); [ "$s" -le {max} ] || {{ echo "' + MARK + 'the file is $s bytes, over {max}"; exit 0; }}\n'
    'echo "' + MARK + 'ok"; base64 <"$f"')
VIEW_SCHEMA = {"type": "object", "required": ["path"], "properties": {
    "path": {"type": "string", "description": "Image file in the task container, absolute or relative "
                                              "to the task working directory"}}}
VIEW_DESCRIPTION = (
    "View an image file from the task's container when visual inspection is needed (PNG, JPEG, GIF, "
    "WebP, BMP, PNM or TIFF). The image stays in your context for a while; older images are dropped "
    "later, so write the values, labels and shapes you need from it in your reply text, which stays "
    "in your context. To compare regions, combine them "
    "into one image with a shell command. PNG, JPEG and WebP files up to 90 KB come as they are, "
    "anything else as WebP, downscaled only if it would not fit; the result text gives both sizes. "
    "For fine detail, crop the region into a new file with a shell command and view that.")
# Worded to hold on any stack: context-manager 1.2.7 drops a result image after one step, newer
# builds keep a window of them. Never tell the model to re-call: that is what fed the view loop.
VIEW_NOTE = "Write down what you need from this image: older images are dropped from your context."
TB_SYSTEM_PROMPT = """You are an autonomous software agent working inside a Linux container to complete one task.
- One tool runs bash commands in that container. Use it for everything: inspecting, editing files, building, running and testing. The other shows you an image file from the container when you need to look at one.
- No human is available. Never ask questions or wait for confirmation; make the most reasonable choice and continue.
- Each command runs in a fresh non-interactive shell with no TTY. Run long jobs in the background with output redirected to a file and poll it.
- Keep going until the task is completely done, then verify your work. Automated tests you cannot see will check the final state of the container, so leave files, services and outputs exactly as the task requires.
- When the task is done, stop calling the tool and reply with a short summary of what you did."""
STOCK_NOTE = (
    "\n\nThe task's machine is an isolated Linux container, not your host. Work only through "
    "the function {fn}, a shell executor, not delegation: call agent_call with "
    '{{"function": "{fn}", "description": "Inspect the task", "payload": {{"command": "pwd"}}}}. '
    "{desc}\n\nTo look at an image file in the container, call {view} the same way, with the "
    'payload {{"path": "<image file>"}}. {view_desc}')
# Harbor 0.27.0 Codex.install() runs ensure_system_dependencies with these before its first
# model call; the iii arm gets the same packages from the same helper. Still Codex-only: the
# nvm Node 22 it symlinks over /usr/local/bin/node (apt's node stays here) and its own CLI.
CODEX_SYSTEM_DEPS = ("curl", "bash", "nodejs", "npm", "ripgrep")
TOOLS = ("curl", "rg", "node", "npm")
TOOLS_PROBE = f"for t in {' '.join(TOOLS)}; do command -v $t >/dev/null 2>&1 && echo $t; done; true"
# Task skills: Codex copies the task's skills_dir into its own skills root in the container.
# The iii arm loads each SKILL.md into iii-directory under a per-trial id and lists only
# those ids for the session; the skill's other files stay in the container.
LIST_SKILLS = (
    'for f in {q}/*/SKILL.md; do [ -f "$f" ] || continue; '
    'printf "' + MARK + '%s\\n" "$(basename "$(dirname "$f")")"; base64 <"$f" | tr -d "\\n"; echo; done; true')
SKILL_NOTE = "\nThis skill's other files are in the task container under {dir}/; use the shell function to read or run them.\n"
# Task MCP servers are compose sidecars the host reaches only by IP; mcp-client takes plain
# http only on loopback, so each one goes through a loopback forwarder owned by this trial.
RESOLVE = (
    'h={q}; a=$( (getent ahostsv4 "$h" || getent hosts "$h") 2>/dev/null | awk \'{{print $1; exit}}\'); '
    '[ -n "$a" ] || a=$(python3 -c \'import socket,sys; print(socket.gethostbyname(sys.argv[1]))\' "$h" 2>/dev/null); '
    'echo "' + MARK + '$a"')
MCP_READY_S, MCP_PROBE_S = 90, 5
MCP_NOTE = (
    "\n\nThe task's MCP server {name} is connected through these functions, called the same way: {ids}. "
    'engine::functions::info with {{"function_id": "<id>"}} gives a tool\'s input schema.')
_mcp_config_lock = None


# Harbor's error_policy.py classifies a trial by exception class name. All of
# these but ContextWindowExceededError give an invalid reward, which drops the
# trial from the mean, so they are kept for provider and transport outages.
class ApiError(RuntimeError): pass
class ApiRateLimitError(ApiError): pass
class ApiOverloadedError(ApiError): pass
class ApiConnectionError(ApiError): pass
class ContextWindowExceededError(ApiError): pass
class AgentAuthenticationError(ApiError): pass
class ModelNotFoundError(ApiError): pass
# A name Harbor does not know: graded with a valid reward, like a terminus-2
# crash, so a failure of the harness or the model counts as a 0. Harbor's default
# policy would also retry it; the h2h configs set max_retries 0.
class HarnessTurnError(RuntimeError): pass


EXACT_FAILURES = {
    "harness.unknown_model": ModelNotFoundError, "router/no_provider_for_model": ModelNotFoundError,
    "router/ambiguous_model": ModelNotFoundError,
    # llm-router's own cap of 64 in-flight provider calls per process, not the account
    # quota: our stack failed, so the trial is a 0 instead of tripping the quota stop rule.
    "router/capacity_exceeded": HarnessTurnError,
}
# Substrings of the other harness failure.code values (router/*, llm.*, harness.*), first match wins.
FAILURE_CLASSES = (
    ("context_overflow", ContextWindowExceededError), ("rate_limited", ApiRateLimitError),
    ("auth", AgentAuthenticationError), ("not_configured", AgentAuthenticationError),
    ("unknown_provider", ModelNotFoundError),
    ("provider_unavailable", ApiOverloadedError), ("stream_", ApiOverloadedError),
    ("provider_rejected", ApiError),
)


def turn_error(status):
    failure = status.get("failure") or {}
    code = failure.get("code") or ""
    cls = EXACT_FAILURES.get(code) or next(
        (c for key, c in FAILURE_CLASSES if key in code),
        ApiOverloadedError if failure.get("retryable") else HarnessTurnError)
    return cls(f"harness turn {status.get('status')}: {code or 'no failure code'}: "
               f"{status.get('result_error')}")


def model_response(message):
    # Harness 1.10 appends an empty assistant (stop_reason "end", requested model) before each
    # provider call and later fills in only content and usage, so a failed call leaves it empty.
    # Content counts alone: a reply cut by the safety filter has text but no usage.
    return message.get("role") == "assistant" and (
        bool(message.get("content")) or ((message.get("usage") or {}).get("output") or 0) > 0)


def transcript_stats(transcript, totals):
    """Model responses in the transcript. models_seen is the requested id: the store never rewrites it."""
    if transcript is None:  # cleanup cut short: output tokens still show whether the model answered
        return {"models_seen": None, "generations": None, "reasoning_generations": None,
                "first_model_response": (totals.get("output_tokens") or 0) > 0 if totals else None}
    gens = [g for g in (m.get("message") or {} for m in transcript["messages"]) if model_response(g)]
    return {"models_seen": sorted({g["model"] for g in gens if g.get("model")}),
            "generations": len(gens),
            "reasoning_generations": sum(((g.get("usage") or {}).get("reasoning") or 0) > 0 for g in gens),
            "first_model_response": bool(gens)}


def _clip(text):
    if len(text) <= CLIP:
        return text
    return f"{text[:CLIP // 2]}\n[... {len(text) - CLIP} chars omitted ...]\n{text[-CLIP // 2:]}"


def _image_type(raw):
    if raw[:4] == b"RIFF" and raw[8:12] == b"WEBP":
        return "WEBP"
    return next((coder for magic, coder in IMAGE_TYPES if raw.startswith(magic)), None)


async def _magick(raw, *args):
    proc = await asyncio.create_subprocess_exec(
        MAGICK, *args, stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE)
    try:
        out, err = await asyncio.wait_for(proc.communicate(raw), 60)
    except BaseException:
        if proc.returncode is None:
            proc.kill()
        raise
    if proc.returncode:
        raise ValueError(err.decode(errors="replace").strip()[-300:])
    return out


async def encode_image(raw):
    """(mime, bytes, source size, shown size) for the model, or ValueError with the reason."""
    coder = _image_type(raw)
    if coder is None:
        raise ValueError("invalid or unsupported image data")
    src = f"{coder}:-[0]"  # the first frame of a GIF or TIFF
    try:
        w, h = map(int, (await _magick(raw, "identify", "-ping", "-format", "%w %h", src)).split())
    except ValueError as e:
        raise ValueError(f"invalid or unsupported image data ({e})") from e
    if w * h > MAX_SOURCE_PIXELS:
        raise ValueError(f"the image is {w}x{h}, over {MAX_SOURCE_PIXELS} pixels: downscale or crop it first")
    scale = min(1.0, MAX_SIDE / max(w, h), math.sqrt(MAX_PIXELS / (w * h)))
    if scale == 1 and coder in SENT_AS_IS and len(raw) <= MAX_IMAGE_BYTES:
        return SENT_AS_IS[coder], raw, (w, h), (w, h)
    for _ in range(6):
        shown = max(int(w * scale), 1), max(int(h * scale), 1)
        out = await _magick(raw, src, "-resize", "%dx%d!" % shown, "-quality", "90", "webp:-")
        if len(out) <= MAX_IMAGE_BYTES:
            return "image/webp", out, (w, h), shown
        scale *= 0.95 * math.sqrt(MAX_IMAGE_BYTES / len(out))
    raise ValueError(f"could not fit the image in {MAX_IMAGE_BYTES} bytes")


def _input_tokens(usage):
    # Harbor counts cached input; router Usage.input is the cache-miss slice for every provider.
    n = usage.get("input_tokens")
    if n is None:
        return n
    return n + (usage.get("cache_read_tokens") or 0) + (usage.get("cache_write_tokens") or 0)


_client = None
_lock = threading.Lock()


def _connect(url, namespace):
    """One client per harbor process; trials share it, interpreter exit shuts it down."""
    global _client
    with _lock:
        if _client is None:
            _client = register_worker(url, InitOptions(
                worker_name=f"tbench-harbor-{os.getpid()}", namespace=namespace))
            # The SDK loop thread is non-daemon and atexit runs only after Python joins
            # it, so harbor would never exit; these hooks run before that join.
            # shortcut: private threading hook, swap it if the SDK starts a daemon thread.
            threading._register_atexit(_client.shutdown)
        return _client


def _slug(text):
    return re.sub(r"[^a-z0-9-]+", "-", text.lower()).strip("-") or "x"


def with_note(content, note):
    """Put note right after the SKILL.md frontmatter, which must still open the file."""
    if content.startswith("---\n") and (end := content.find("\n---", 3)) != -1:
        cut = content.find("\n", end + 4)
        cut = len(content) if cut == -1 else cut + 1
        return content[:cut] + note + content[cut:]
    return note.lstrip("\n") + content


async def _pipe(reader, writer):
    try:
        while data := await reader.read(65536):
            writer.write(data)
            await writer.drain()
        if writer.can_write_eof():
            writer.write_eof()
    except OSError:
        pass


async def forward(host, port):
    """A loopback listener that relays each connection to host:port."""
    async def relay(down_r, down_w):
        try:
            up_r, up_w = await asyncio.open_connection(host, port)
        except OSError:
            down_w.close()
            return
        try:
            await asyncio.gather(_pipe(down_r, up_w), _pipe(up_r, down_w))
        finally:
            up_w.close()
            down_w.close()
    return await asyncio.start_server(relay, "127.0.0.1", 0)


class IIIHarnessAgent(BaseAgent):
    capabilities = AgentCapabilities(skills=True, mcp_servers=True)

    def __init__(self, logs_dir, model_name=None, *, engine_url=None, namespace=None,
                 prompt="stock", max_turns=1_000_000, thinking_level=None, **kwargs):
        super().__init__(logs_dir, model_name, **kwargs)
        # Never fall back to III_URL: that is the live stack.
        if not engine_url or not namespace:
            raise ValueError("--ak engine_url=... and --ak namespace=... are required")
        if prompt not in ("stock", "override"):
            raise ValueError("--ak prompt must be stock or override")
        if thinking_level not in (None, "off", "minimal", "low", "medium", "high", "xhigh"):
            raise ValueError("--ak thinking_level must be off|minimal|low|medium|high|xhigh "
                             "(xhigh is the ceiling; there is no max)")
        self._provider, _, self._model = (model_name or "").partition("/")
        if not self._provider or not self._model:
            raise ValueError("-m must be <provider>/<router catalog id>")
        if not MAGICK:
            raise ValueError("view_image needs ImageMagick 7 (magick) on the host")
        self._engine_url, self._namespace = engine_url, namespace
        self._prompt, self._max_turns, self._thinking_level = prompt, max_turns, thinking_level
        self._hex = uuid.uuid4().hex[:12]
        # Neutral ids: both reach the model, and the Codex arm sees no benchmark name either.
        self._iii_session_id = f"s_{self._hex}"
        self._fn_id = f"workspace::t{self._hex}::exec"
        self._view_id = f"workspace::t{self._hex}::view_image"
        self._refs, self._skill_ids, self._mcp, self._forwarders = [], [], [], []
        self._iii = self._loop = self._env = self._send_log = None
        self._base_tools = self._packages_installed = None
        self._wrap = True
        self._inflight = set()
        self._n = 0
        self._sent = self._done = self._cancelled = False

    @staticmethod
    def name():
        return "iii-harness"

    def version(self):
        return "0.2"

    async def setup(self, environment):
        self._loop, self._env = asyncio.get_running_loop(), environment
        self.logs_dir.mkdir(parents=True, exist_ok=True)
        try:
            # register_worker blocks up to 30 s while it connects.
            self._iii = await asyncio.to_thread(_connect, self._engine_url, self._namespace)
        except Exception as e:
            raise ApiConnectionError(f"cannot reach the iii engine at {self._engine_url}: {e}") from e
        # register_worker returns a client that keeps retrying when the engine is down.
        if self._iii.get_connection_state() != "connected":
            raise ApiConnectionError(f"cannot reach the iii engine at {self._engine_url}")
        self._wrap = (await environment.exec(PROBE, timeout_sec=30)).return_code == 0
        self._base_tools = await self._tools(environment)
        # Harbor's own helper: same commands as the Codex arm, and the same exception on failure.
        await Codex(self.logs_dir, logger=self.logger).ensure_system_dependencies(environment, CODEX_SYSTEM_DEPS)
        self._packages_installed = all((await self._tools(environment)).values())
        bridges = ((self._fn_id, self._exec, EXEC_DESCRIPTION, EXEC_SCHEMA),
                   (self._view_id, self._view_image, VIEW_DESCRIPTION, VIEW_SCHEMA))
        try:  # Harbor cancels setup at its timeout: never leave the bridge registered
            for fn_id, work, description, schema in bridges:
                self._refs.append(self._iii.register_function(
                    fn_id, self._bridged(work), description=description, request_format=schema))
            for fn_id, *_ in bridges:
                for _ in range(40):
                    try:
                        await self._call("engine::functions::info", {"function_id": fn_id}, PROBE_CALL_MS)
                        break
                    except Exception:
                        await asyncio.sleep(0.25)
                else:
                    raise ApiConnectionError(f"{fn_id} never appeared on the engine")
            await self._load_skills()
            await self._connect_mcp()
            # shortcut: a fixed wait for the harness's event-driven function snapshot, and run()
            # raises HarnessTurnError on a turn with zero function calls; probe the harness if that fires.
            await asyncio.sleep(SETTLE_S)
        except BaseException:
            await self._release()
            raise

    async def _load_skills(self):
        if not self.skills_dir:
            return
        root = self.skills_dir.rstrip("/")
        r = await self._env.exec(LIST_SKILLS.format(q=shlex.quote(root)), timeout_sec=60)
        # Text before the first MARK is docker noise; base64 never holds MARK.
        for chunk in (r.stdout or "").split(MARK)[1:]:
            name, _, encoded = chunk.partition("\n")
            content = base64.b64decode(encoded.strip()).decode("utf-8", "replace")
            skill_id = f"t{self._hex}-{_slug(name)}"[:64]
            await self._call("directory::skills::create", {
                "id": skill_id, "content": with_note(content, SKILL_NOTE.format(dir=f"{root}/{name}"))})
            self._skill_ids.append(skill_id)

    async def _connect_mcp(self):
        for server in self.mcp_servers:
            u = urlsplit(server.url or "")
            if server.transport == "stdio" or u.scheme != "http" or not u.hostname:
                raise ValueError(f"MCP server {server.name}: only http:// sse and streamable-http servers "
                                 f"are supported, got {server.transport} {server.url}")
            if server.headers:
                raise ValueError(f"MCP server {server.name}: HTTP headers are not supported")
            r = await self._env.exec(RESOLVE.format(q=shlex.quote(u.hostname)), timeout_sec=30)
            ip, port = (r.stdout or "").rpartition(MARK)[2].strip(), u.port or 80
            if not ip:
                raise ApiConnectionError(f"MCP server {server.name}: {u.hostname} does not resolve in the container")
            try:
                _, w = await asyncio.wait_for(asyncio.open_connection(ip, port), MCP_PROBE_S)
                w.close()
            except (OSError, TimeoutError) as e:
                raise ApiConnectionError(f"MCP server {server.name}: the host cannot reach {ip}:{port}: {e}") from e
            self._forwarders.append(fwd := await forward(ip, port))
            # shortcut: mcp-client has no SSE client; @playwright/mcp (TB 4.1's only MCP task)
            # serves Streamable HTTP at /mcp beside /sse. An SSE-only server needs SSE in mcp-client.
            path = u.path.removesuffix("/sse") + "/mcp" if server.transport == "sse" else u.path
            entry = {"type": "http", "url": f"http://127.0.0.1:{fwd.sockets[0].getsockname()[1]}{path}"
                                            + (f"?{u.query}" if u.query else "")}
            name = f"t{self._hex}-{_slug(server.name)}"[:24].rstrip("-")
            check = (await self._call("mcp-client::config::validate", {"mcpServers": {name: entry}}))["servers"][name]
            if not check["ok"]:
                raise ValueError(f"MCP server {server.name}: mcp-client refused {entry}: {check['errors']}")
            # An approval given before the entry exists admits it when it arrives.
            await self._call("mcp-client::servers::approve", {"server": name, "fingerprint": check["fingerprint"]})
            self._mcp.append({"name": server.name, "server": name, "tools": []})
            await self._edit_mcp_config(lambda servers: servers.__setitem__(name, entry))
        if self._mcp:
            await self._wait_mcp()

    async def _edit_mcp_config(self, edit):
        global _mcp_config_lock
        # shortcut: configuration::set has no compare-and-set, so concurrent trials take turns
        # through one lock per Harbor process; two harbor runs on one stack could still drop an entry.
        _mcp_config_lock = _mcp_config_lock or asyncio.Lock()
        config_id = f"{self._namespace}-mcp-client"  # the III_CONFIG_NAME Compose gives mcp-client
        async with _mcp_config_lock:
            value = (await self._call("configuration::get", {"id": config_id})).get("value") or {}
            servers = dict(value.get("mcpServers") or {})
            edit(servers)
            await self._call("configuration::set", {"id": config_id, "value": {**value, "mcpServers": servers}})

    async def _wait_mcp(self):
        deadline = time.monotonic() + MCP_READY_S
        while True:
            listed = {s.get("name"): s for s in
                      (await self._call("mcp-client::servers::list", {})).get("servers") or []}
            pending = [m["server"] for m in self._mcp if (listed.get(m["server"]) or {}).get("state") != "ready"
                       or not listed[m["server"]].get("tools_listed")]
            if not pending:
                for m in self._mcp:
                    m["tools"] = [t["function_id"] for t in listed[m["server"]].get("tools") or []
                                  if t.get("registered")]
                return
            if time.monotonic() > deadline:
                raise ApiConnectionError("MCP servers not ready: " + "; ".join(
                    f"{n} {(listed.get(n) or {}).get('state')} {(listed.get(n) or {}).get('error')}" for n in pending))
            await asyncio.sleep(0.5)

    async def _release(self):
        self._unregister()
        for skill_id in self._skill_ids:
            await self._quietly("skill delete", self._call("directory::skills::delete", {"id": skill_id}, CLEANUP_CALL_MS))
        if names := [m["server"] for m in self._mcp]:
            await self._quietly("mcp-client config", self._edit_mcp_config(
                lambda servers: [servers.pop(n, None) for n in names]))
        for fwd in self._forwarders:
            fwd.close()
            fwd.close_clients()
        self._skill_ids, self._mcp, self._forwarders = [], [], []

    def _unregister(self):
        for ref in self._refs:
            ref.unregister()
        self._refs = []

    async def _tools(self, environment):
        found = set(((await environment.exec(TOOLS_PROBE, timeout_sec=30)).stdout or "").splitlines())
        return {t: t in found for t in TOOLS}

    async def _call(self, function_id, payload, timeout_ms=CALL_TIMEOUT_MS):
        return await self._iii.call_async(
            {"function_id": function_id, "payload": payload, "timeout_ms": timeout_ms})

    def _bridged(self, work):
        async def handler(data):
            # Runs on the SDK's loop thread; Harbor's environment lives on Harbor's loop.
            fut = asyncio.run_coroutine_threadsafe(work(data), self._loop)
            self._inflight.add(fut)
            try:
                return await asyncio.wrap_future(fut)
            finally:
                self._inflight.discard(fut)
        return handler

    def _log(self, **fields):
        self._n += 1
        with open(self.logs_dir / "exec.jsonl", "a") as log:
            log.write(json.dumps({"ts": time.time(), "n": self._n, **fields}) + "\n")

    async def _exec(self, data):
        data = data if isinstance(data, dict) else {}
        command, cwd = data.get("command"), data.get("cwd") or None
        try:  # the model may send "120s" or "1.5" despite the schema
            t = min(max(int(data.get("timeout_sec") or DEFAULT_T), 1), MAX_T)
        except (TypeError, ValueError):
            t = DEFAULT_T
        started = time.monotonic()
        if not isinstance(command, str) or not command.strip():
            out = {"exit_code": None, "output": "error: command must be a non-empty string", "timed_out": False}
        else:
            try:
                out = await (self._run_wrapped if self._wrap else self._run_plain)(command, cwd, t)
            except RuntimeError as e:  # Harbor's own exec timeout; anything else is a bridge failure
                if not str(e).startswith("Command timed out"):
                    raise
                out = {"exit_code": None, "output": "[timeout: output lost]", "timed_out": True}
        out["duration_ms"] = int((time.monotonic() - started) * 1000)
        self._log(command=command, cwd=cwd, timeout_sec=t, **out)
        return out

    async def _view_image(self, data):
        path = data.get("path") if isinstance(data, dict) else None
        started, log = time.monotonic(), {}
        try:
            if not isinstance(path, str) or not path.strip():
                raise ValueError("path must be a non-empty string")
            try:
                r = await self._env.exec(READ_IMAGE.format(q=shlex.quote(path), max=MAX_FILE_BYTES), timeout_sec=60)
            except RuntimeError as e:  # Harbor's own exec timeout; anything else is a bridge failure
                if not str(e).startswith("Command timed out"):
                    raise
                raise ValueError("reading the file timed out") from e
            # Text before MARK is docker noise, as for exec.
            _, found, rest = (r.stdout or "").partition(MARK)
            state, _, encoded = rest.partition("\n")
            if state != "ok":
                raise ValueError(state if found else f"the read failed: {_clip(r.stdout or '')}")
            raw = base64.b64decode(encoded)
            mime, image, source, shown = await encode_image(raw)
            log = {"bytes": len(raw), "size": source, "shown": shown, "sent_bytes": len(image), "mime": mime}
            text = f"{path}: {source[0]}x{source[1]}" + (
                f", shown downscaled to {shown[0]}x{shown[1]}" if shown != source else "") + ". " + VIEW_NOTE
            out = {"content": [{"type": "text", "text": text},
                               {"type": "image", "mime": mime, "data": base64.b64encode(image).decode()}]}
        except (ValueError, TimeoutError) as e:
            log = {"error": str(e)}
            out = {"content": [{"type": "text", "text": f"error: unable to view image {path!r}: {e}"}],
                   "is_error": True}
        self._log(view_image=path, **log, duration_ms=int((time.monotonic() - started) * 1000))
        return out

    async def _run_wrapped(self, command, cwd, t):
        r = await self._env.exec(WRAP.format(q=shlex.quote(command), t=t, clip=CLIP, half=CLIP // 2),
                                 cwd=cwd, timeout_sec=t + SLACK)
        # Text before MARK is docker noise or bash's stderr from before the job started.
        _, found, rest = (r.stdout or "").partition(MARK)
        head, _, output = rest.partition("\n")
        state = head.split() if found else []
        if len(state) == 2 and state[0] == "done":
            return {"exit_code": int(state[1]), "output": output, "timed_out": False}
        if len(state) == 3 and state[0] == "running":
            return {"exit_code": None, "output": output, "timed_out": True,
                    "pid": int(state[1]), "log_path": state[2]}
        # The wrapper never ran, e.g. a cwd that does not exist.
        return {"exit_code": r.return_code, "output": _clip(r.stdout or ""), "timed_out": False}

    async def _run_plain(self, command, cwd, t):
        r = await self._env.exec(command, cwd=cwd, timeout_sec=t)
        return {"exit_code": r.return_code, "output": _clip(r.stdout or ""), "timed_out": False}

    def send_payload(self, instruction, trial, task):
        allow = [self._fn_id, self._view_id, *(f"mcp::{m['server']}::*" for m in self._mcp)]
        functions = {"allow": allow}
        options = {"functions": functions, "max_turns": self._max_turns}
        if self._skill_ids:
            # Lists only this trial's skills; without the get function the harness drops the index.
            allow.append("directory::skills::get")
            options["skills"] = list(self._skill_ids)
        if self._prompt == "override":
            functions["expose"] = "native"
            options.update(system_prompt=TB_SYSTEM_PROMPT, system_prompt_strategy="override")
            message = instruction
        else:
            message = instruction + STOCK_NOTE.format(
                fn=self._fn_id, desc=EXEC_DESCRIPTION, view=self._view_id, view_desc=VIEW_DESCRIPTION)
            if self._mcp:
                allow.append("engine::functions::info")
                message += "".join(MCP_NOTE.format(name=m["name"], ids=", ".join(m["tools"])) for m in self._mcp)
        if self._thinking_level:
            options["thinking_level"] = self._thinking_level
        return {
            "session_id": self._iii_session_id, "message": message,
            "model": self._model, "provider": self._provider,
            # The hex differs per agent instance: Harbor retries reuse the trial name.
            "idempotency_key": f"tbench:{trial}:{self._hex}:send:1",
            "session": {"kind": "e2e", "title": f"Harness E2E: {task}",
                        "metadata": {"e2e_run_id": trial, "e2e_scenario": task, "e2e_execution_kind": "harbor"}},
            "options": options,
        }

    async def run(self, instruction, environment, context):
        trial = self.session_id.removesuffix("__agent")
        send = self.send_payload(instruction, trial, trial.rsplit("__", 1)[0])
        # Written at trial end: logs_dir is readable from the task container while the model works.
        self._send_log = {"request": send, "response": None}
        try:
            self._sent = True  # a send that timed out here may still have started the turn
            try:
                resp = self._send_log["response"] = await self._call("harness::send", send)
            except Exception as e:
                raise ApiConnectionError(f"harness::send failed: {e}") from e
            if resp.get("accepted") is not True or resp.get("merged") or resp.get("queued"):
                raise RuntimeError(f"harness::send did not start a new turn: {resp}")
            self._write("system-prompt.json", await self._quietly(
                "system prompt", self._call("harness::system-prompt::get", {"session_id": self._iii_session_id})))
            metrics, status = await self._wait(context)
            if status.get("status") != "completed":
                raise turn_error(status)
            if metrics["totals"].get("function_calls") == 0:
                raise HarnessTurnError("the turn completed without a single function call")
        except asyncio.CancelledError:
            self._cancelled = True
            raise
        finally:
            try:
                await asyncio.wait_for(self._collect(context), CLEANUP_S)
            except Exception as e:
                self.logger.warning("iii cleanup incomplete: %s", e)
            finally:
                await self._release()

    def _metrics(self, omit_context=True, timeout_ms=CALL_TIMEOUT_MS):
        return self._call("harness::metrics", {"root_session_id": self._iii_session_id,
                                               "omit_context": omit_context, "omit_traces": True}, timeout_ms)

    async def _wait(self, context):
        failures = 0
        while True:
            try:
                metrics = await self._metrics()
                status = await self._call("harness::status", {"session_id": self._iii_session_id})
                failures = 0
            except Exception as e:
                failures += 1
                if failures >= 12:
                    raise ApiConnectionError(f"harness polls keep failing: {e}") from e
                self.logger.warning("harness poll failed: %s", e)
                await asyncio.sleep(POLL_S)
                continue
            if status is None:
                raise ApiError("the harness turn record is gone")
            self._fill(context, metrics)
            if status.get("open_asks"):
                raise HarnessTurnError("the harness is waiting on a human ask")
            # metrics.complete covers children and armed wakes; expects_wake is a parked root.
            if metrics.get("complete") and status.get("status") in TERMINAL and not status.get("expects_wake"):
                self._done = True
                return metrics, status
            await asyncio.sleep(POLL_S)

    async def _collect(self, context):
        sid, metrics, status, transcript = self._iii_session_id, None, None, None
        try:
            if not self._sent:
                return
            if not self._done:
                await self._quietly("stop", self._call(
                    "harness::stop", {"session_id": sid, "quiet_deadlines": True}, CLEANUP_CALL_MS))
                for fut in list(self._inflight):
                    fut.cancel()
                for _ in range(10):
                    m = await self._quietly("metrics", self._metrics(timeout_ms=CLEANUP_CALL_MS))
                    if m and m.get("complete"):
                        break
                    await asyncio.sleep(2)
            metrics = await self._quietly("metrics", self._metrics(False, CLEANUP_CALL_MS))
            self._write("metrics.json", metrics)
            self._fill(context, metrics)
            status = await self._quietly("status", self._call(
                "harness::status", {"session_id": sid, "verbose": True}, CLEANUP_CALL_MS))
            self._write("status.json", status)
            transcript = await self._quietly("transcript", self._transcript())
            self._write("transcript.json", transcript)
        finally:  # CLEANUP_S may cut the collection short; keep what was collected
            self._write("iii-send.json", self._send_log)
            t, s = (metrics or {}).get("totals") or {}, status or {}
            context.metadata = {
                "session_id": sid, "turn_id": s.get("turn_id"), "status": s.get("status"),
                "stop_reason": s.get("stop_reason"), "failure": s.get("failure"), "result_error": s.get("result_error"),
                **{k: t.get(k) for k in ("turns", "sessions", "function_calls", "function_call_errors",
                                         "reasoning_tokens", "cache_write_tokens", "unreported_generations")},
                "bridge_calls": self._n, "cancelled_by_harbor": self._cancelled, "prompt": self._prompt,
                "provider": self._provider, "model": self._model, "max_turns": self._max_turns,
                "thinking_level": self._thinking_level or "provider default",
                "system_prompt_sha256": hashlib.sha256(TB_SYSTEM_PROMPT.encode()).hexdigest()
                if self._prompt == "override" else None,
                "engine_url": self._engine_url, "namespace": self._namespace,
                **transcript_stats(transcript, t),
                "base_tools": self._base_tools, "packages_installed": self._packages_installed,
                "skills": list(self._skill_ids),
                "mcp_servers": [{"name": m["name"], "server": m["server"], "tools": len(m["tools"])} for m in self._mcp],
            }

    async def _transcript(self):
        messages, cursor = [], None
        while True:
            page = await self._call("session::messages", {"session_id": self._iii_session_id, "limit": 500,
                                                          "cursor": cursor, "include_custom": True}, CLEANUP_CALL_MS)
            messages += page.get("messages") or []
            if not page.get("next_cursor") or page["next_cursor"] == cursor:
                return {"messages": messages}
            cursor = page["next_cursor"]

    def _fill(self, context, metrics):
        if not metrics:
            return
        t = metrics.get("totals") or {}
        context.n_input_tokens = _input_tokens(t)
        context.n_cache_tokens = t.get("cache_read_tokens")
        context.n_output_tokens = t.get("output_tokens")
        context.cost_usd = t.get("cost_usd")
        context.model_usage = {
            f"{b.get('provider')}/{b.get('model')}": ModelUsage(
                n_input_tokens=_input_tokens(b) or 0,
                n_cache_tokens=b.get("cache_read_tokens") or 0,
                n_output_tokens=b.get("output_tokens") or 0, cost_usd=b.get("cost_usd"))
            for b in t.get("by_model") or []} or None

    async def _quietly(self, what, awaitable):
        try:
            return await awaitable
        except Exception as e:
            self.logger.warning("iii %s failed: %s", what, e)
            return None

    def _write(self, name, value):
        # logs_dir is /logs/agent inside the task container: nothing secret goes here.
        if value is not None:
            (self.logs_dir / name).write_text(json.dumps(value, indent=1, default=str))
