# Wire protocol

This file is frozen. The public suite and the hidden verification suite both
depend on it exactly as written.

## Configuration (environment)

| Variable | Required | Default | Meaning |
| --- | --- | --- | --- |
| `PORT` | yes | — | TCP port bound on `127.0.0.1`. |
| `LLM_BASE_URL` | yes | — | Provider root, e.g. `http://host:port/v1`. Trailing slashes are trimmed. |
| `LLM_API_KEY` | yes | — | Sent as `authorization: Bearer <key>`. |
| `LLM_MODEL` | no | `test-model` | Value of `model` in every provider request. |
| `SYSTEM_PROMPT` | no | `You are a helpful assistant.` | First message of every provider payload. |
| `MAX_HISTORY_TURNS` | no | `8` | Number of prior user/assistant pairs replayed to the provider. |
| `TOKEN_BUDGET` | no | `100000` | Cumulative `total_tokens` allowed per conversation. |

`LLM_API_KEY` and `LLM_BASE_URL` are checked before the socket is bound. When
either is missing the process writes the offending variable name to stderr and
exits with a non-zero code.

## Service API

### `GET /healthz`
`200` with `{"status":"ok"}`.

### `POST /api/conversations`
`201` with `{"id":"<string>","title":null}`.

### `GET /api/conversations/:id`
`200` with `{"id","title","usage":{"prompt_tokens","completion_tokens","total_tokens"},
"messages":[{"role","content"}]}`; `404` with `{"error":{"code":"conversation_not_found"}}`.

### `POST /api/conversations/:id/messages`
Request body `{"content":"<non-empty string>"}`.

- `404` `{"error":{"code":"conversation_not_found"}}` for an unknown conversation.
- `400` `{"error":{"code":"invalid_request"}}` for empty content.
- `429` `{"error":{"code":"token_budget_exceeded"}}` when the conversation already
  reached `TOKEN_BUDGET`. No provider request may be made in this case.
- Otherwise `200` with `content-type: text/event-stream` and this event grammar,
  each event written as `event: <name>\ndata: <json>\n\n`:

| Event | Data | When |
| --- | --- | --- |
| `delta` | `{"text":"<fragment>"}` | Once per provider content fragment, written before the provider finishes. |
| `tool` | `{"name":"<tool>","result":"<string>"}` | Once per executed tool call. |
| `title` | `{"title":"<string>"}` | Once, after the first successful exchange. |
| `done` | `{"messageId":"<string>","usage":{"promptTokens","completionTokens","totalTokens"}}` | Last event of a successful turn. |
| `error` | `{"code":"provider_error","message":"<string>"}` | Terminal event of a failed turn; no `done` is emitted. |

## Provider API (OpenAI-compatible subset)

`POST {LLM_BASE_URL}/chat/completions` with `authorization: Bearer {LLM_API_KEY}`.

Turn request: `{"model","messages","stream":true,"tools":[server_time, calculator]}`.
The response is SSE; every `data:` line carries a chunk and the stream ends with
`data: [DONE]`:

```
data: {"choices":[{"delta":{"content":"Hel"}}]}
data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"calculator","arguments":"{\"expression\":\"2+3*4\"}"}}]}}]}
data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":6,"total_tokens":16}}
data: [DONE]
```

When a chunk carries `tool_calls`, execute every call locally, emit one `tool`
event per call, then send a follow-up request whose `messages` end with the
assistant tool-call message and one `{"role":"tool","tool_call_id","content"}`
message per result, and stream the continuation.

Title request (after the first successful exchange, non-streaming):
`{"model","messages","stream":false,"response_format":{"type":"json_schema",
"json_schema":{"name":"conversation_title","schema":{...}}}}`. The response is
`{"choices":[{"message":{"content":"{\"title\":\"...\"}"}}],"usage":{...}}`.
A failed title request must not fail the turn.

## Tools

| Name | Arguments | Result |
| --- | --- | --- |
| `server_time` | `{}` | Current time as an ISO-8601 string. |
| `calculator` | `{"expression":"<string>"}` | Decimal result of `+ - * / ( )`, as a string. Never `eval`. |

## Accounting

`usage` from every provider response of a turn (including the title request) is
summed into the turn's `done` event and into the conversation total. A turn that
ends in `error` stores nothing: neither the user message nor a partial answer,
and the next turn's payload must not contain it.
