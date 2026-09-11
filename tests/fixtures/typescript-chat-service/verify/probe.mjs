// Runner-owned hidden verification harness for the typescript_chat_service scenario.
// Usage: node probe.mjs <workspace-root>
// Prints exactly one JSON line: {"passed":bool,"checks":{...},"details":{...}}
import http from "node:http";
import { spawn } from "node:child_process";
import { setTimeout as delay } from "node:timers/promises";
import fs from "node:fs";
import path from "node:path";

const workspace = path.resolve(process.argv[2] ?? ".");
const checks = {};
const details = {};

function record(name, passed, detail) {
  checks[name] = Boolean(passed);
  if (detail !== undefined) details[name] = detail;
}

// ---------------------------------------------------------------- provider --

function streamBody(chunks, usage) {
  return chunks.map((chunk) => `data: ${JSON.stringify(chunk)}\n\n`).concat([
    `data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: "stop" }], usage })}\n\n`,
    "data: [DONE]\n\n",
  ]);
}

function textStream(parts, usage, gapMs = 0) {
  return {
    kind: "stream",
    gapMs,
    frames: streamBody(
      parts.map((text) => ({ choices: [{ delta: { content: text } }] })),
      usage,
    ),
  };
}

function toolStream(name, args, usage) {
  return {
    kind: "stream",
    gapMs: 0,
    frames: [
      `data: ${JSON.stringify({
        choices: [
          {
            delta: {
              tool_calls: [
                { index: 0, id: "call_1", type: "function", function: { name, arguments: args } },
              ],
            },
          },
        ],
      })}\n\n`,
      `data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: "tool_calls" }], usage })}\n\n`,
      "data: [DONE]\n\n",
    ],
  };
}

function jsonResponse(body) {
  return { kind: "json", body };
}

function failure(status) {
  return { kind: "failure", status };
}

async function startProvider(script) {
  const requests = [];
  const queue = [...script];
  const server = http.createServer((request, response) => {
    const chunks = [];
    request.on("data", (chunk) => chunks.push(chunk));
    request.on("end", async () => {
      let body = {};
      try {
        body = JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}");
      } catch {
        body = { parseError: true };
      }
      const entry = {
        path: request.url,
        authorization: request.headers.authorization ?? null,
        body,
        firstChunkAt: null,
        lastChunkAt: null,
      };
      requests.push(entry);
      const responder = queue.shift() ?? textStream(["ok"], {
        prompt_tokens: 1,
        completion_tokens: 1,
        total_tokens: 2,
      });
      if (responder.kind === "failure") {
        response.writeHead(responder.status, { "content-type": "application/json" });
        response.end(JSON.stringify({ error: { message: "provider is down" } }));
        return;
      }
      if (responder.kind === "json") {
        response.writeHead(200, { "content-type": "application/json" });
        response.end(JSON.stringify(responder.body));
        return;
      }
      response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
      for (const frame of responder.frames) {
        response.write(frame);
        entry.firstChunkAt ??= Date.now();
        entry.lastChunkAt = Date.now();
        if (responder.gapMs > 0) await delay(responder.gapMs);
      }
      response.end();
    });
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return { server, requests, url: `http://127.0.0.1:${server.address().port}/v1` };
}

// ----------------------------------------------------------------- subject --

async function startSubject(env) {
  const child = spawn(process.execPath, ["src/server.ts"], {
    cwd: workspace,
    env: { ...process.env, ...env, III_TELEMETRY_ENABLED: "false" },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk) => (stdout += chunk));
  child.stderr.on("data", (chunk) => (stderr += chunk));
  const exited = new Promise((resolve) => child.on("exit", (code) => resolve(code ?? -1)));
  const port = Number(env.PORT);
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) break;
    try {
      const response = await fetch(`http://127.0.0.1:${port}/healthz`);
      if (response.ok) {
        await response.arrayBuffer();
        return { child, exited, stdout: () => stdout, stderr: () => stderr, ready: true, port };
      }
    } catch {
      // not listening yet
    }
    await delay(150);
  }
  return { child, exited, stdout: () => stdout, stderr: () => stderr, ready: false, port };
}

function freePort() {
  return new Promise((resolve, reject) => {
    const probe = http.createServer();
    probe.listen(0, "127.0.0.1", () => {
      const { port } = probe.address();
      probe.close(() => resolve(port));
    });
    probe.on("error", reject);
  });
}

async function stop(subject) {
  if (subject.child.exitCode === null) subject.child.kill("SIGKILL");
  await subject.exited;
}

// --------------------------------------------------------------- sse client --

async function readEvents(url, body) {
  const response = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  const events = [];
  if (!response.ok || response.body === null) {
    let payload = null;
    try {
      payload = await response.json();
    } catch {
      payload = null;
    }
    return { status: response.status, events, payload };
  }
  const decoder = new TextDecoder();
  let buffer = "";
  for await (const chunk of response.body) {
    buffer += decoder.decode(chunk, { stream: true });
    let cut = buffer.indexOf("\n\n");
    while (cut !== -1) {
      const raw = buffer.slice(0, cut);
      buffer = buffer.slice(cut + 2);
      cut = buffer.indexOf("\n\n");
      const event = { name: "message", data: null, at: Date.now() };
      for (const line of raw.split("\n")) {
        if (line.startsWith("event:")) event.name = line.slice(6).trim();
        if (line.startsWith("data:")) {
          try {
            event.data = JSON.parse(line.slice(5).trim());
          } catch {
            event.data = line.slice(5).trim();
          }
        }
      }
      events.push(event);
    }
  }
  return { status: response.status, events, payload: null };
}

const named = (events, name) => events.filter((event) => event.name === name);
const text = (events) => named(events, "delta").map((event) => event.data?.text ?? "").join("");

async function newConversation(port) {
  const response = await fetch(`http://127.0.0.1:${port}/api/conversations`, { method: "POST" });
  const body = await response.json();
  return { status: response.status, id: body.id, body };
}

async function getConversation(port, id) {
  const response = await fetch(`http://127.0.0.1:${port}/api/conversations/${id}`);
  return { status: response.status, body: await response.json() };
}

async function say(port, id, content) {
  return readEvents(`http://127.0.0.1:${port}/api/conversations/${id}/messages`, { content });
}

const usage = (prompt, completion) => ({
  prompt_tokens: prompt,
  completion_tokens: completion,
  total_tokens: prompt + completion,
});

const titleResponse = (title) =>
  jsonResponse({
    choices: [{ message: { role: "assistant", content: JSON.stringify({ title }) } }],
    usage: usage(3, 2),
  });

async function withStack(env, script, run) {
  const provider = await startProvider(script);
  const port = await freePort();
  const subject = await startSubject({
    PORT: String(port),
    LLM_BASE_URL: provider.url,
    LLM_API_KEY: "test-key",
    LLM_MODEL: "probe-model",
    SYSTEM_PROMPT: "SENTINEL-SYSTEM-PROMPT",
    MAX_HISTORY_TURNS: "8",
    TOKEN_BUDGET: "100000",
    ...env,
  });
  try {
    return await run({ subject, provider, port });
  } finally {
    await stop(subject);
    await new Promise((resolve) => provider.server.close(resolve));
  }
}

// ------------------------------------------------------------------ checks --

async function checkBootAndStreaming() {
  await withStack(
    {},
    [
      textStream(["Hel", "lo ", "world"], usage(10, 6), 120),
      titleResponse("Greeting thread"),
    ],
    async ({ subject, provider, port }) => {
      record("service_boots", subject.ready, { stderr: subject.stderr().slice(-400) });
      if (!subject.ready) return;
      const created = await newConversation(port);
      record("conversation_created", created.status === 201 && typeof created.id === "string", created.body);
      const turn = await say(port, created.id, "hi there");
      const deltas = named(turn.events, "delta");
      const done = named(turn.events, "done");
      record("stream_incremental", deltas.length >= 3 && text(turn.events) === "Hello world", {
        deltas: deltas.length,
        text: text(turn.events),
      });
      const providerLast = provider.requests[0]?.lastChunkAt ?? Infinity;
      record(
        "stream_is_live",
        deltas.length > 0 && deltas[0].at < providerLast,
        { firstDeltaAt: deltas[0]?.at ?? null, providerLastChunkAt: providerLast },
      );
      record(
        "done_reports_usage",
        done.length === 1 && done[0].data?.usage?.totalTokens >= 16,
        done[0]?.data ?? null,
      );
      const first = provider.requests[0]?.body ?? {};
      record(
        "system_prompt_applied",
        Array.isArray(first.messages) &&
          first.messages[0]?.role === "system" &&
          first.messages[0]?.content === "SENTINEL-SYSTEM-PROMPT" &&
          first.messages.filter((message) => message.role === "system").length === 1,
        first.messages ?? null,
      );
      record(
        "authorized_provider_call",
        provider.requests[0]?.authorization === "Bearer test-key" && first.stream === true,
        { authorization: provider.requests[0]?.authorization ?? null, stream: first.stream ?? null },
      );
      const titled = named(turn.events, "title");
      const detail = await getConversation(port, created.id);
      record(
        "structured_title",
        titled.length === 1 &&
          titled[0].data?.title === "Greeting thread" &&
          detail.body.title === "Greeting thread",
        { event: titled[0]?.data ?? null, stored: detail.body.title ?? null },
      );
      const titleRequest = provider.requests[1]?.body ?? {};
      record(
        "title_uses_structured_output",
        titleRequest.response_format?.type === "json_schema" && titleRequest.stream !== true,
        titleRequest.response_format ?? null,
      );
      record(
        "conversation_persisted",
        detail.status === 200 &&
          detail.body.messages?.length === 2 &&
          detail.body.messages[0]?.role === "user" &&
          detail.body.messages[0]?.content === "hi there" &&
          detail.body.messages[1]?.role === "assistant" &&
          detail.body.messages[1]?.content === "Hello world",
        detail.body.messages ?? null,
      );
    },
  );
}

async function checkMultiTurnAndHistoryBound() {
  await withStack(
    { MAX_HISTORY_TURNS: "2" },
    [
      textStream(["one"], usage(5, 1)),
      titleResponse("Counting"),
      textStream(["two"], usage(5, 1)),
      textStream(["three"], usage(5, 1)),
    ],
    async ({ subject, provider, port }) => {
      if (!subject.ready) {
        record("multi_turn_payload", false, "subject did not boot");
        record("history_bounded", false, "subject did not boot");
        return;
      }
      const created = await newConversation(port);
      await say(port, created.id, "first");
      await say(port, created.id, "second");
      await say(port, created.id, "third");
      const turns = provider.requests.filter((entry) => entry.body.stream === true);
      const second = turns[1]?.body?.messages ?? [];
      record(
        "multi_turn_payload",
        JSON.stringify(second) ===
          JSON.stringify([
            { role: "system", content: "SENTINEL-SYSTEM-PROMPT" },
            { role: "user", content: "first" },
            { role: "assistant", content: "one" },
            { role: "user", content: "second" },
          ]),
        second,
      );
      const third = turns[2]?.body?.messages ?? [];
      record(
        "history_bounded",
        JSON.stringify(third) ===
          JSON.stringify([
            { role: "system", content: "SENTINEL-SYSTEM-PROMPT" },
            { role: "user", content: "first" },
            { role: "assistant", content: "one" },
            { role: "user", content: "second" },
            { role: "assistant", content: "two" },
            { role: "user", content: "third" },
          ]),
        third,
      );
    },
  );
}

async function checkTools() {
  await withStack(
    {},
    [
      toolStream("calculator", JSON.stringify({ expression: "2+3*4" }), usage(8, 4)),
      textStream(["The answer is 14"], usage(9, 5)),
      titleResponse("Arithmetic"),
      toolStream("server_time", "{}", usage(8, 4)),
      textStream(["Time reported"], usage(9, 5)),
    ],
    async ({ subject, provider, port }) => {
      if (!subject.ready) {
        record("tool_calculator", false, "subject did not boot");
        record("tool_server_time", false, "subject did not boot");
        record("tool_result_returned_to_provider", false, "subject did not boot");
        record("tools_advertised", false, "subject did not boot");
        return;
      }
      const created = await newConversation(port);
      const turn = await say(port, created.id, "what is 2+3*4?");
      const toolEvents = named(turn.events, "tool");
      record(
        "tool_calculator",
        toolEvents.length === 1 &&
          toolEvents[0].data?.name === "calculator" &&
          String(toolEvents[0].data?.result) === "14" &&
          text(turn.events).includes("The answer is 14"),
        { events: toolEvents.map((event) => event.data), text: text(turn.events) },
      );
      const followUp = provider.requests[1]?.body?.messages ?? [];
      const toolMessage = followUp.find((message) => message.role === "tool");
      record(
        "tool_result_returned_to_provider",
        toolMessage !== undefined && String(toolMessage.content) === "14",
        followUp,
      );
      const advertised = provider.requests[0]?.body?.tools ?? [];
      const names = advertised.map((tool) => tool.function?.name).sort();
      record(
        "tools_advertised",
        JSON.stringify(names) === JSON.stringify(["calculator", "server_time"]),
        names,
      );
      const second = await say(port, created.id, "what time is it?");
      const timeEvents = named(second.events, "tool");
      const value = timeEvents[0]?.data?.result ?? "";
      record(
        "tool_server_time",
        timeEvents.length === 1 &&
          timeEvents[0].data?.name === "server_time" &&
          !Number.isNaN(Date.parse(String(value))),
        { events: timeEvents.map((event) => event.data) },
      );
    },
  );
}

async function checkBudgetAndResilience() {
  await withStack(
    { TOKEN_BUDGET: "20" },
    [textStream(["spend"], usage(12, 8)), titleResponse("Budget")],
    async ({ subject, provider, port }) => {
      if (!subject.ready) {
        record("token_budget_enforced", false, "subject did not boot");
        return;
      }
      const created = await newConversation(port);
      await say(port, created.id, "burn the budget");
      const before = provider.requests.length;
      const refused = await say(port, created.id, "one more");
      record(
        "token_budget_enforced",
        refused.status === 429 &&
          refused.payload?.error?.code === "token_budget_exceeded" &&
          provider.requests.length === before,
        { status: refused.status, payload: refused.payload, calls: provider.requests.length - before },
      );
    },
  );
  await withStack(
    {},
    [
      textStream(["fine"], usage(5, 3)),
      titleResponse("Resilience"),
      failure(500),
      textStream(["recovered"], usage(5, 3)),
    ],
    async ({ subject, provider, port }) => {
      if (!subject.ready) {
        record("provider_error_surfaced", false, "subject did not boot");
        record("failed_turn_not_persisted", false, "subject did not boot");
        record("failed_turn_not_replayed", false, "subject did not boot");
        return;
      }
      const created = await newConversation(port);
      await say(port, created.id, "healthy turn");
      const broken = await say(port, created.id, "doomed turn");
      const errors = named(broken.events, "error");
      record(
        "provider_error_surfaced",
        broken.status === 200 &&
          errors.length === 1 &&
          errors[0].data?.code === "provider_error" &&
          named(broken.events, "done").length === 0,
        { status: broken.status, events: broken.events.map((event) => event.name) },
      );
      const detail = await getConversation(port, created.id);
      const contents = (detail.body.messages ?? []).map((message) => message.content);
      record(
        "failed_turn_not_persisted",
        contents.length === 2 && !contents.includes("doomed turn"),
        contents,
      );
      const before = provider.requests.length;
      await say(port, created.id, "after failure");
      const replay = provider.requests[before]?.body?.messages ?? [];
      record(
        "failed_turn_not_replayed",
        JSON.stringify(replay) ===
          JSON.stringify([
            { role: "system", content: "SENTINEL-SYSTEM-PROMPT" },
            { role: "user", content: "healthy turn" },
            { role: "assistant", content: "fine" },
            { role: "user", content: "after failure" },
          ]),
        replay,
      );
    },
  );
}

async function checkConcurrencyIsolation() {
  const script = [];
  for (let index = 0; index < 12; index += 1) script.push(textStream([`reply-${index}`], usage(4, 2)));
  await withStack({}, script, async ({ subject, provider, port }) => {
    if (!subject.ready) {
      record("concurrent_conversations_isolated", false, "subject did not boot");
      return;
    }
    const ids = await Promise.all([0, 1, 2, 3].map(() => newConversation(port)));
    await Promise.all(ids.map((created, index) => say(port, created.id, `question-${index}`)));
    await Promise.all(ids.map((created, index) => say(port, created.id, `follow-${index}`)));
    const details = await Promise.all(ids.map((created) => getConversation(port, created.id)));
    const isolated = details.every((detail, index) => {
      const contents = (detail.body.messages ?? []).map((message) => message.content);
      return (
        contents.length === 4 &&
        contents[0] === `question-${index}` &&
        contents[2] === `follow-${index}`
      );
    });
    record("concurrent_conversations_isolated", isolated, details.map((detail) => detail.body.messages));
  });
}

async function checkPreflight() {
  const provider = await startProvider([]);
  const port = await freePort();
  const subject = await startSubject({
    PORT: String(port),
    LLM_BASE_URL: provider.url,
    LLM_API_KEY: "",
    SYSTEM_PROMPT: "SENTINEL-SYSTEM-PROMPT",
  });
  const code = await Promise.race([subject.exited, delay(8000).then(() => "timeout")]);
  await stop(subject);
  await new Promise((resolve) => provider.server.close(resolve));
  record(
    "preflight_rejects_missing_key",
    code !== "timeout" && code !== 0 && /LLM_API_KEY/i.test(subject.stderr()),
    { exitCode: code, stderr: subject.stderr().slice(-300) },
  );
}

function checkRepositoryShape() {
  let manifest = {};
  try {
    manifest = JSON.parse(fs.readFileSync(path.join(workspace, "package.json"), "utf8"));
  } catch (error) {
    details.zero_dependencies = String(error);
  }
  const dependencyCount =
    Object.keys(manifest.dependencies ?? {}).length +
    Object.keys(manifest.devDependencies ?? {}).length;
  record(
    "zero_dependencies",
    dependencyCount === 0 && !fs.existsSync(path.join(workspace, "node_modules")),
    { dependencyCount, nodeModules: fs.existsSync(path.join(workspace, "node_modules")) },
  );
  const readme = path.join(workspace, "README.md");
  const readmeText = fs.existsSync(readme) ? fs.readFileSync(readme, "utf8") : "";
  record(
    "readme_documents_service",
    readmeText.length >= 400 &&
      /LLM_BASE_URL/.test(readmeText) &&
      /TOKEN_BUDGET/.test(readmeText) &&
      /MAX_HISTORY_TURNS/.test(readmeText),
    { bytes: readmeText.length },
  );
  const sources = [];
  const walk = (directory) => {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      const full = path.join(directory, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (entry.name.endsWith(".ts")) sources.push(full);
    }
  };
  const sourceRoot = path.join(workspace, "src");
  if (fs.existsSync(sourceRoot)) walk(sourceRoot);
  const typed = sources.every((file) => {
    const body = fs.readFileSync(file, "utf8");
    return !/\bas any\b/.test(body) && !/:\s*any\b/.test(body);
  });
  record("typescript_without_any", sources.length > 0 && typed, { files: sources.length });
}

// -------------------------------------------------------------------- main --

try {
  await checkBootAndStreaming();
  await checkMultiTurnAndHistoryBound();
  await checkTools();
  await checkBudgetAndResilience();
  await checkConcurrencyIsolation();
  await checkPreflight();
  checkRepositoryShape();
} catch (error) {
  details.harness_error = String(error?.stack ?? error);
  checks.harness_completed = false;
}
checks.harness_completed ??= true;
const passed = Object.values(checks).every(Boolean);
process.stdout.write(`${JSON.stringify({ passed, checks, details })}\n`);
