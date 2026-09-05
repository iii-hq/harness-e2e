import test from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import type { AddressInfo } from "node:net";

import {
  buildPayload,
  createServer,
  evaluateExpression,
  readConfig,
  runTool,
  type ChatMessage,
  type Config,
  type Conversation,
} from "../src/server.ts";

function providerStub(frames: string[]): Promise<{ url: string; close: () => Promise<void>; bodies: unknown[] }> {
  const bodies: unknown[] = [];
  const server = http.createServer((request, response) => {
    const chunks: Buffer[] = [];
    request.on("data", (chunk: Buffer) => chunks.push(chunk));
    request.on("end", () => {
      bodies.push(JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}"));
      response.writeHead(200, { "content-type": "text/event-stream" });
      for (const frame of frames) response.write(frame);
      response.end();
    });
  });
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address() as AddressInfo;
      resolve({
        url: `http://127.0.0.1:${port}/v1`,
        bodies,
        close: () => new Promise<void>((done) => server.close(() => done())),
      });
    });
  });
}

function testConfig(overrides: Partial<Config> = {}): Config {
  return {
    port: 0,
    baseUrl: "http://127.0.0.1:1/v1",
    apiKey: "public-test-key",
    model: "public-test-model",
    systemPrompt: "You are a helpful assistant.",
    maxHistoryTurns: 8,
    tokenBudget: 100000,
    ...overrides,
  };
}

test("readConfig rejects a missing API key", () => {
  assert.throws(() => readConfig({ LLM_BASE_URL: "http://127.0.0.1:1/v1" }), /LLM_API_KEY/);
});

test("readConfig reads the configured environment", () => {
  const config = readConfig({
    PORT: "8081",
    LLM_BASE_URL: "http://127.0.0.1:9/v1/",
    LLM_API_KEY: "key",
    MAX_HISTORY_TURNS: "3",
    TOKEN_BUDGET: "500",
  });
  assert.equal(config.port, 8081);
  assert.equal(config.baseUrl, "http://127.0.0.1:9/v1");
  assert.equal(config.maxHistoryTurns, 3);
  assert.equal(config.tokenBudget, 500);
});

test("evaluateExpression respects arithmetic precedence", () => {
  assert.equal(evaluateExpression("2+3*4"), "14");
  assert.equal(evaluateExpression("(2+3)*4"), "20");
  assert.throws(() => evaluateExpression("2 ** 3"));
});

test("runTool executes the two supported tools", () => {
  assert.equal(runTool("calculator", JSON.stringify({ expression: "10/4" })), "2.5");
  assert.ok(!Number.isNaN(Date.parse(runTool("server_time", "{}"))));
  assert.throws(() => runTool("shell", "{}"));
});

test("buildPayload keeps the system prompt, bounded history, and the new message", () => {
  const conversation: Conversation = {
    id: "c1",
    title: null,
    usage: { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 },
    messages: [
      { role: "user", content: "u1" },
      { role: "assistant", content: "a1" },
      { role: "user", content: "u2" },
      { role: "assistant", content: "a2" },
    ] satisfies ChatMessage[],
  };
  const payload = buildPayload(conversation, testConfig({ maxHistoryTurns: 1, systemPrompt: "SP" }), "u3");
  assert.deepEqual(payload, [
    { role: "system", content: "SP" },
    { role: "user", content: "u2" },
    { role: "assistant", content: "a2" },
    { role: "user", content: "u3" },
  ]);
});

test("the service streams a turn and stores the exchange", async (t) => {
  const provider = await providerStub([
    `data: ${JSON.stringify({ choices: [{ delta: { content: "pi" } }] })}\n\n`,
    `data: ${JSON.stringify({ choices: [{ delta: { content: "ng" } }] })}\n\n`,
    `data: ${JSON.stringify({
      choices: [{ delta: {}, finish_reason: "stop" }],
      usage: { prompt_tokens: 4, completion_tokens: 2, total_tokens: 6 },
    })}\n\n`,
    "data: [DONE]\n\n",
  ]);
  t.after(() => provider.close());
  const server = createServer(testConfig({ baseUrl: provider.url }));
  t.after(() => new Promise<void>((resolve) => server.close(() => resolve())));
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as AddressInfo;

  const health = await fetch(`http://127.0.0.1:${port}/healthz`);
  assert.equal(health.status, 200);
  assert.deepEqual(await health.json(), { status: "ok" });

  const created = await fetch(`http://127.0.0.1:${port}/api/conversations`, { method: "POST" });
  assert.equal(created.status, 201);
  const { id } = (await created.json()) as { id: string };

  const streamed = await fetch(`http://127.0.0.1:${port}/api/conversations/${id}/messages`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ content: "ping" }),
  });
  assert.equal(streamed.status, 200);
  assert.match(streamed.headers.get("content-type") ?? "", /text\/event-stream/);
  const body = await streamed.text();
  assert.match(body, /event: delta/);
  assert.match(body, /event: done/);
  assert.equal(
    [...body.matchAll(/event: delta\ndata: (.*)\n/g)]
      .map((match) => JSON.parse(match[1]).text)
      .join(""),
    "ping",
  );

  const detail = await fetch(`http://127.0.0.1:${port}/api/conversations/${id}`);
  const stored = (await detail.json()) as { messages: { role: string; content: string }[] };
  assert.deepEqual(stored.messages, [
    { role: "user", content: "ping" },
    { role: "assistant", content: "ping" },
  ]);
});

test("an unknown conversation is rejected", async (t) => {
  const server = createServer(testConfig());
  t.after(() => new Promise<void>((resolve) => server.close(() => resolve())));
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as AddressInfo;
  const response = await fetch(`http://127.0.0.1:${port}/api/conversations/missing`);
  assert.equal(response.status, 404);
});
