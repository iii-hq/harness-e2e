import http from "node:http";
import { randomUUID } from "node:crypto";

export type Role = "system" | "user" | "assistant" | "tool";

export interface ChatMessage {
  role: Role;
  content: string;
  tool_call_id?: string;
  tool_calls?: ToolCall[];
}

export interface ToolCall {
  id: string;
  type: "function";
  function: { name: string; arguments: string };
}

export interface Usage {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
}

export interface Conversation {
  id: string;
  title: string | null;
  messages: ChatMessage[];
  usage: Usage;
}

export interface Config {
  port: number;
  baseUrl: string;
  apiKey: string;
  model: string;
  systemPrompt: string;
  maxHistoryTurns: number;
  tokenBudget: number;
}

const TOOLS = [
  {
    type: "function",
    function: {
      name: "server_time",
      description: "Current server time in ISO-8601.",
      parameters: { type: "object", properties: {}, required: [] },
    },
  },
  {
    type: "function",
    function: {
      name: "calculator",
      description: "Evaluate an arithmetic expression.",
      parameters: {
        type: "object",
        properties: { expression: { type: "string" } },
        required: ["expression"],
      },
    },
  },
] as const;

export function readConfig(env: NodeJS.ProcessEnv): Config {
  const apiKey = (env.LLM_API_KEY ?? "").trim();
  if (apiKey === "") {
    throw new Error("preflight failed: missing LLM_API_KEY");
  }
  const baseUrl = (env.LLM_BASE_URL ?? "").trim();
  if (baseUrl === "") {
    throw new Error("preflight failed: missing LLM_BASE_URL");
  }
  return {
    port: Number(env.PORT ?? 0),
    baseUrl: baseUrl.replace(/\/+$/, ""),
    apiKey,
    model: env.LLM_MODEL ?? "test-model",
    systemPrompt: env.SYSTEM_PROMPT ?? "You are a helpful assistant.",
    maxHistoryTurns: Number(env.MAX_HISTORY_TURNS ?? 8),
    tokenBudget: Number(env.TOKEN_BUDGET ?? 100000),
  };
}

export function evaluateExpression(expression: string): string {
  const tokens = expression.match(/\d+(?:\.\d+)?|[+\-*/()]/g) ?? [];
  if (tokens.join("") !== expression.replace(/\s+/g, "")) {
    throw new Error("unsupported expression");
  }
  let position = 0;
  const peek = (): string | undefined => tokens[position];
  const expr = (): number => {
    let value = term();
    while (peek() === "+" || peek() === "-") {
      const operator = tokens[position++];
      value = operator === "+" ? value + term() : value - term();
    }
    return value;
  };
  const term = (): number => {
    let value = factor();
    while (peek() === "*" || peek() === "/") {
      const operator = tokens[position++];
      const right = factor();
      if (operator === "/" && right === 0) throw new Error("division by zero");
      value = operator === "*" ? value * right : value / right;
    }
    return value;
  };
  const factor = (): number => {
    const token = tokens[position++];
    if (token === "(") {
      const value = expr();
      if (tokens[position++] !== ")") throw new Error("unbalanced parentheses");
      return value;
    }
    if (token === undefined || !/^\d/.test(token)) throw new Error("unexpected token");
    return Number(token);
  };
  const result = expr();
  if (position !== tokens.length) throw new Error("trailing input");
  return String(result);
}

export function runTool(name: string, args: string): string {
  const parsed: Record<string, unknown> = args.trim() === "" ? {} : JSON.parse(args);
  if (name === "server_time") return new Date().toISOString();
  if (name === "calculator") return evaluateExpression(String(parsed.expression ?? ""));
  throw new Error(`unknown tool ${name}`);
}

export function buildPayload(conversation: Conversation, config: Config, next: string): ChatMessage[] {
  const history = conversation.messages.filter((message) => message.role !== "system");
  const kept = config.maxHistoryTurns * 2;
  const bounded = kept <= 0 ? [] : history.slice(Math.max(0, history.length - kept));
  return [
    { role: "system", content: config.systemPrompt },
    ...bounded.map((message) => ({ role: message.role, content: message.content })),
    { role: "user", content: next },
  ];
}

interface StreamOutcome {
  content: string;
  toolCalls: ToolCall[];
  usage: Usage;
}

async function streamCompletion(
  config: Config,
  messages: ChatMessage[],
  onDelta: (text: string) => void,
): Promise<StreamOutcome> {
  const response = await fetch(`${config.baseUrl}/chat/completions`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${config.apiKey}`,
    },
    body: JSON.stringify({ model: config.model, messages, stream: true, tools: TOOLS }),
  });
  if (!response.ok || response.body === null) {
    throw new Error(`provider responded ${response.status}`);
  }
  const decoder = new TextDecoder();
  let buffer = "";
  let content = "";
  const toolCalls: ToolCall[] = [];
  let usage: Usage = { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 };
  for await (const chunk of response.body as unknown as AsyncIterable<Uint8Array>) {
    buffer += decoder.decode(chunk, { stream: true });
    let cut = buffer.indexOf("\n");
    while (cut !== -1) {
      const line = buffer.slice(0, cut).trim();
      buffer = buffer.slice(cut + 1);
      cut = buffer.indexOf("\n");
      if (!line.startsWith("data:")) continue;
      const payload = line.slice(5).trim();
      if (payload === "[DONE]") continue;
      const event = JSON.parse(payload);
      if (event.usage) usage = event.usage;
      const delta = event.choices?.[0]?.delta ?? {};
      if (typeof delta.content === "string" && delta.content !== "") {
        content += delta.content;
        onDelta(delta.content);
      }
      for (const call of delta.tool_calls ?? []) {
        const index: number = call.index ?? 0;
        const existing = toolCalls[index];
        if (existing === undefined) {
          toolCalls[index] = {
            id: call.id ?? `call_${index}`,
            type: "function",
            function: {
              name: call.function?.name ?? "",
              arguments: call.function?.arguments ?? "",
            },
          };
        } else {
          existing.function.name += call.function?.name ?? "";
          existing.function.arguments += call.function?.arguments ?? "";
        }
      }
    }
  }
  return { content, toolCalls: toolCalls.filter(Boolean), usage };
}

async function requestTitle(config: Config, question: string, answer: string): Promise<{ title: string; usage: Usage }> {
  const response = await fetch(`${config.baseUrl}/chat/completions`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${config.apiKey}`,
    },
    body: JSON.stringify({
      model: config.model,
      stream: false,
      messages: [
        { role: "system", content: "Title this conversation." },
        { role: "user", content: `${question}\n${answer}` },
      ],
      response_format: {
        type: "json_schema",
        json_schema: {
          name: "conversation_title",
          schema: {
            type: "object",
            properties: { title: { type: "string" } },
            required: ["title"],
            additionalProperties: false,
          },
        },
      },
    }),
  });
  if (!response.ok) throw new Error(`title provider responded ${response.status}`);
  const body = await response.json();
  const parsed = JSON.parse(body.choices?.[0]?.message?.content ?? "{}");
  const title = typeof parsed.title === "string" ? parsed.title : "";
  const usage: Usage = body.usage ?? { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 };
  return { title, usage };
}

function addUsage(target: Usage, delta: Usage): void {
  target.prompt_tokens += delta.prompt_tokens ?? 0;
  target.completion_tokens += delta.completion_tokens ?? 0;
  target.total_tokens += delta.total_tokens ?? 0;
}

export function createServer(config: Config): http.Server {
  const conversations = new Map<string, Conversation>();
  const locks = new Map<string, Promise<unknown>>();

  const serialize = <T>(id: string, task: () => Promise<T>): Promise<T> => {
    const previous = locks.get(id) ?? Promise.resolve();
    const next = previous.then(task, task);
    locks.set(id, next.catch(() => undefined));
    return next;
  };

  const json = (response: http.ServerResponse, status: number, body: unknown): void => {
    const encoded = JSON.stringify(body);
    response.writeHead(status, {
      "content-type": "application/json",
      "content-length": Buffer.byteLength(encoded),
    });
    response.end(encoded);
  };

  const readBody = async (request: http.IncomingMessage): Promise<Record<string, unknown>> => {
    const chunks: Buffer[] = [];
    for await (const chunk of request) chunks.push(chunk as Buffer);
    const raw = Buffer.concat(chunks).toString("utf8");
    return raw.trim() === "" ? {} : JSON.parse(raw);
  };

  return http.createServer((request, response) => {
    void (async () => {
      const url = new URL(request.url ?? "/", "http://localhost");
      const path = url.pathname;
      try {
        if (request.method === "GET" && path === "/healthz") {
          json(response, 200, { status: "ok" });
          return;
        }
        if (request.method === "POST" && path === "/api/conversations") {
          const conversation: Conversation = {
            id: randomUUID(),
            title: null,
            messages: [],
            usage: { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 },
          };
          conversations.set(conversation.id, conversation);
          json(response, 201, { id: conversation.id, title: null });
          return;
        }
        const messageMatch = /^\/api\/conversations\/([^/]+)\/messages$/.exec(path);
        if (request.method === "POST" && messageMatch) {
          const conversation = conversations.get(messageMatch[1]);
          if (conversation === undefined) {
            json(response, 404, { error: { code: "conversation_not_found" } });
            return;
          }
          const body = await readBody(request);
          const content = typeof body.content === "string" ? body.content.trim() : "";
          if (content === "") {
            json(response, 400, { error: { code: "invalid_request" } });
            return;
          }
          if (conversation.usage.total_tokens >= config.tokenBudget) {
            json(response, 429, {
              error: { code: "token_budget_exceeded", used: conversation.usage.total_tokens, budget: config.tokenBudget },
            });
            return;
          }
          await serialize(conversation.id, () => handleTurn(conversation, content, response));
          return;
        }
        const detailMatch = /^\/api\/conversations\/([^/]+)$/.exec(path);
        if (request.method === "GET" && detailMatch) {
          const conversation = conversations.get(detailMatch[1]);
          if (conversation === undefined) {
            json(response, 404, { error: { code: "conversation_not_found" } });
            return;
          }
          json(response, 200, {
            id: conversation.id,
            title: conversation.title,
            usage: conversation.usage,
            messages: conversation.messages.map((message) => ({ role: message.role, content: message.content })),
          });
          return;
        }
        json(response, 404, { error: { code: "not_found" } });
      } catch (error) {
        if (!response.headersSent) {
          json(response, 500, { error: { code: "internal_error", message: String(error) } });
        } else {
          response.end();
        }
      }
    })();
  });

  async function handleTurn(
    conversation: Conversation,
    content: string,
    response: http.ServerResponse,
  ): Promise<void> {
    response.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-cache",
      connection: "keep-alive",
    });
    const send = (event: string, data: unknown): void => {
      response.write(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`);
    };
    const turnUsage: Usage = { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 };
    try {
      const payload = buildPayload(conversation, config, content);
      let outcome = await streamCompletion(config, payload, (text) => send("delta", { text }));
      addUsage(turnUsage, outcome.usage);
      const followUp = [...payload];
      let guard = 0;
      while (outcome.toolCalls.length > 0 && guard < 4) {
        guard += 1;
        followUp.push({ role: "assistant", content: outcome.content, tool_calls: outcome.toolCalls });
        for (const call of outcome.toolCalls) {
          let result: string;
          try {
            result = runTool(call.function.name, call.function.arguments);
          } catch (error) {
            result = `error: ${(error as Error).message}`;
          }
          send("tool", { name: call.function.name, result });
          followUp.push({ role: "tool", tool_call_id: call.id, content: result });
        }
        outcome = await streamCompletion(config, followUp, (text) => send("delta", { text }));
        addUsage(turnUsage, outcome.usage);
      }
      const answer = outcome.content;
      conversation.messages.push({ role: "user", content });
      conversation.messages.push({ role: "assistant", content: answer });
      if (conversation.title === null) {
        try {
          const titled = await requestTitle(config, content, answer);
          if (titled.title !== "") {
            conversation.title = titled.title;
            addUsage(turnUsage, titled.usage);
            send("title", { title: titled.title });
          }
        } catch {
          // A missing title never fails an otherwise complete turn.
        }
      }
      addUsage(conversation.usage, turnUsage);
      send("done", {
        messageId: randomUUID(),
        usage: {
          promptTokens: turnUsage.prompt_tokens,
          completionTokens: turnUsage.completion_tokens,
          totalTokens: turnUsage.total_tokens,
        },
      });
    } catch (error) {
      send("error", { code: "provider_error", message: (error as Error).message });
    } finally {
      response.end();
    }
  }
}

export function start(env: NodeJS.ProcessEnv = process.env): http.Server {
  const config = readConfig(env);
  const server = createServer(config);
  server.listen(config.port, "127.0.0.1", () => {
    const address = server.address();
    const port = typeof address === "object" && address !== null ? address.port : config.port;
    process.stdout.write(`listening ${port}\n`);
  });
  return server;
}

if (process.argv[1] !== undefined && import.meta.filename === process.argv[1]) {
  try {
    start();
  } catch (error) {
    process.stderr.write(`${(error as Error).message}\n`);
    process.exit(1);
  }
}
