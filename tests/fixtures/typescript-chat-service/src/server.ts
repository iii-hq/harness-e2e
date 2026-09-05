// Chat service skeleton. Every exported value below is part of the fixed public
// contract exercised by `tests/server.test.ts` and by the hidden verification
// suite. Keep the names, signatures, and file path; implement the behavior
// described in `PROTOCOL.md`.
import type http from "node:http";

export type Role = "system" | "user" | "assistant" | "tool";

export interface ToolCall {
  id: string;
  type: "function";
  function: { name: string; arguments: string };
}

export interface ChatMessage {
  role: Role;
  content: string;
  tool_call_id?: string;
  tool_calls?: ToolCall[];
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

/** Reads and validates the process environment. Throws when preflight fails. */
export function readConfig(env: NodeJS.ProcessEnv): Config {
  void env;
  throw new Error("readConfig is not implemented");
}

/** Evaluates `+ - * / ( )` over non-negative decimals and returns the result. */
export function evaluateExpression(expression: string): string {
  void expression;
  throw new Error("evaluateExpression is not implemented");
}

/** Runs one local tool by name with its JSON-encoded arguments. */
export function runTool(name: string, args: string): string {
  void name;
  void args;
  throw new Error("runTool is not implemented");
}

/** Builds the provider payload: system prompt, bounded history, new message. */
export function buildPayload(
  conversation: Conversation,
  config: Config,
  next: string,
): ChatMessage[] {
  void conversation;
  void config;
  void next;
  throw new Error("buildPayload is not implemented");
}

/** Creates the HTTP server described in PROTOCOL.md. Does not listen. */
export function createServer(config: Config): http.Server {
  void config;
  throw new Error("createServer is not implemented");
}

/** Reads the environment, creates the server, and starts listening. */
export function start(env: NodeJS.ProcessEnv = process.env): http.Server {
  void env;
  throw new Error("start is not implemented");
}

if (process.argv[1] !== undefined && import.meta.filename === process.argv[1]) {
  try {
    start();
  } catch (error) {
    process.stderr.write(`${(error as Error).message}\n`);
    process.exit(1);
  }
}
