# Goals

Implement the service described in `PROTOCOL.md`. Every numbered goal is graded.

- **G1 — Runtime.** TypeScript executed directly by Node (`node src/server.ts`),
  no build step, no runtime or development dependency, no network access other
  than the configured provider. `package.json` keeps `dependencies` and
  `devDependencies` empty and `node_modules/` must not exist.
- **G2 — Typing.** Keep the exported contract of `src/server.ts` (names,
  signatures, exported types). Type every value explicitly: the sources must
  contain no `any` annotation and no `as any` cast, and must stay erasable
  (no enums or namespaces).
- **G3 — Service surface.** Implement `GET /healthz`, `POST /api/conversations`,
  `GET /api/conversations/:id` and `POST /api/conversations/:id/messages`,
  with the exact status codes and error codes of `PROTOCOL.md`.
- **G4 — Real streaming.** Forward every provider fragment as its own `delta`
  event while the provider is still sending. Buffering the full answer and
  flushing it at the end fails this goal.
- **G5 — Payload correctness.** Each provider payload is exactly the system
  prompt, the bounded history in order, and the new user message.
- **G6 — Bounded history.** Replay at most `MAX_HISTORY_TURNS` user/assistant
  pairs, keeping the most recent ones.
- **G7 — Tools.** Advertise `server_time` and `calculator`, execute tool calls
  locally, emit one `tool` event per call, return the results to the provider,
  and stream the continuation. `calculator` is a real parser with `* /`
  precedence and parentheses — never `eval`.
- **G8 — Structured title.** After the first successful exchange, request a
  title with a `json_schema` response format, store it, and emit one `title`
  event. A title failure must not fail the turn.
- **G9 — Token budget.** Track cumulative usage per conversation and refuse a
  turn past `TOKEN_BUDGET` with `429 token_budget_exceeded` without calling the
  provider.
- **G10 — Failure handling.** A provider failure ends the turn with a single
  `error` event carrying `provider_error`; the failed turn is not stored and is
  never replayed in a later payload.
- **G11 — Preflight.** Missing `LLM_API_KEY` or `LLM_BASE_URL` exits non-zero
  with the variable name on stderr, before binding the port.
- **G12 — Concurrency.** Conversations are isolated and turns within one
  conversation are serialized, so concurrent traffic never mixes histories.
- **G13 — Public suite.** `node --test` passes without modifying anything under
  `tests/`.
- **G14 — Documentation.** `README.md` documents how to run the service, every
  environment variable (including `LLM_BASE_URL`, `MAX_HISTORY_TURNS` and
  `TOKEN_BUDGET`), the endpoints, and the tools.
