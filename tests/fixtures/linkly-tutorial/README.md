# Linkly tutorial scenario

`linkly_tutorial` runs the [agentic Linkly tutorial](https://iii.dev/docs/next/tutorials/linkly/agentic)
the way a user does: one Harness session, one prompt per chapter, the agent builds a URL shortener
chapter by chapter on the `linkly-agentic` scaffold. The scenario adds an eighth exchange, the
project-restart guard, then validates the finished project with twenty-two deterministic checks
worth 100 points (`metrics.json`). No judge model is involved.

| exchange | prompt | what the agent builds |
| --- | --- | --- |
| 1 | `ch-1-foundations.md` | `link` worker: `link::create` / `link::resolve` over `state`, `POST /links`, `GET /s/:code` |
| 2 | `ch-2-observe.md` | reads the engine traces for the redirects |
| 3 | `ch-3-persist.md` | SQLite `primary` database, `links` / `clicks` tables, state as read cache |
| 4 | `ch-4-durable.md` | `clicks` queue, `pubsub`, `analytics` database + Python worker, `PUT /links/:code` |
| 5 | `ch-5-stream.md` | `click-streamer` writing every click into the `clicks` stream |
| 6 | `ch-6-channels.md` | `bulk-importer` over a channel + `channel-client/import-links.js` |
| 7 | `ch-7-browser.md` | `rbac-proxy` on :3110, `auth` worker, `link::delete` / `link::request_delete`, Vite frontend |
| 8 | `guard.md` | asks for a project-wide `compose::restart`; the Harness must refuse it |

## The scaffold is the stack

The Harness scopes every `compose::*` call to the Compose project it was started from
(`III_COMPOSE_FILE`), so `compose::add worker=database` from the agent lands in the scaffold's own
`worker-compose.yaml`. The scaffold therefore *is* the stack under test and the runner attaches to it:

```bash
# 1. scaffold, enable a provider (key read from the environment, never printed), patch the
#    template gaps tracked in MOT-4739 (env_file + harness.start_after)
python3 scripts/linkly_stack.py scaffold --dir /tmp/linkly-run7 --provider deepseek
# 2. start it in its own terminal or tmux pane; it must stay running for the whole run
python3 scripts/linkly_stack.py up --dir /tmp/linkly-run7/linkly
# 3. once `python3 scripts/linkly_stack.py status --dir /tmp/linkly-run7/linkly` shows the
#    baseline ready, run the scenario against that engine
HARNESS_E2E_RUN_DIR=/tmp/linkly-run7/e2e cargo run --locked -- run \
  --url ws://127.0.0.1:49134 --model deepseek-flash --provider deepseek --scenario linkly_tutorial
# 4. stop the stack gracefully (SIGINT to the compose daemon; a pane kill orphans the workers)
python3 scripts/linkly_stack.py down --dir /tmp/linkly-run7/linkly
```

`setup` preflights and never mutates: `compose::status` names the project (or
`HARNESS_E2E_LINKLY_PROJECT` must match it), `.iii/project.ini` must say `source=linkly-agentic`,
the template's `link/src/index.ts` stub must be present and none of the agent-created directories
(`analytics/`, `click-streamer/`, `bulk-importer/`, `channel-client/`, `auth/`, `frontend/`) may
exist, the baseline containers (`http state cron queue shell harness llm-router session-manager
iii-directory`) and at least one `provider-*` must be `ready`, no `link::*` function may be
registered yet, and the http worker must answer on `127.0.0.1:3111`. A run consumes the scaffold:
use one repetition per scaffold and `down` + a new `scaffold` between runs.

Requirements on the executor: the `iii` CLI (for the helper), `curl`, Node 22+ and `npm` (the
Ch. 6 client), network access to the workers registry when the stack starts.

### Software engineering plan

The `software-engineering` profile includes `linkly_tutorial` once, in its own
`case-linkly-tutorial` group, without technical retries. All eight exchanges stay
in the same Harness session. The exact-stack workflow checks out `iii-hq/templates`
at `ba1dfd95d4f4120705c8b0cc95d9a2ef86a0290d` and scaffolds `linkly-agentic` with
the campaign's exact CLI. The scaffold hosts both Harness and the E2E runner.

Baseline package versions are replaced with the campaign's resolved versions;
`http` joins the runtime graph before it is frozen. The template's `shell` and
`console` containers use the exact `ide` and `ade` packages from the target graph,
so mutable legacy aliases cannot introduce a second package version. Template
configuration and engine workers are retained. Provider secrets stay in private
executor files outside the project and uploaded evidence. The ordinary Compose
startup, status collection and cleanup apply to this group too. The artifact's
`stack/template.json` records the template revision, and `stack/worker-compose.yaml`
and `stack/worker-compose-final.yaml` preserve the assembled and delivered stacks.
Mixed groups, repeated runs and technical retries are rejected because they
would reuse a consumed scaffold.

## Checks

All checks run in `capture`, after the eighth exchange, against the finished project. Codes created
by the checks carry an attempt-specific suffix so they never collide with the agent's own tests.

| chapter | metric | check |
| --- | --- | --- |
| 1 (20) | `foundations.create` | `POST /links {url, code}` → 201, body carries the code |
| | `foundations.redirect` | `GET /s/<code>` → 302, `Location` is the url |
| | `foundations.conflict` | same POST again → 409 |
| | `foundations.unknown` | `GET /s/nope-<tag>` → 404 |
| 2 (10) | `observe.traces_listed` | `engine::traces::list name="GET /s/:code"` → ≥ 1 trace |
| | `observe.tree_resolves` | `engine::traces::tree` on it → ≥ 1 root span |
| 3 (15) | `persist.links_row` | three creates, then `database::query db=primary SELECT * FROM links` names all three |
| | `persist.clicks_counted` | one redirect each, then `SELECT * FROM clicks` names all three (≤ 15 s) |
| | `persist.cache_warm` | `state::get scope=links key=<code>` → `{url}` |
| 4 (15) | `durable.daily_counts` | five creates grow `SUM(count)` over `analytics.daily_link_counts` by ≥ 5 (≤ 15 s) |
| | `durable.update_refreshes_cache` | `PUT /links/<code>` → 2xx and `link::resolve` returns the new url (≤ 10 s) |
| | `durable.clicks_queue` | `engine::queue::list_topics` lists `clicks` |
| 5 (10) | `stream.items_listed` | three clicks, then `stream::list clicks/all` has an item per code (≤ 15 s) |
| | `stream.item_shape` | those items carry `clicked_at` |
| 6 (10) | `channels.import_runs` | `node channel-client/import-links.js` exits 0 |
| | `channels.idempotent` | second run reports imported 0, skipped 2 |
| | `channels.rows_resolve` | `link::resolve` answers for `mylink` and `mydocslink` |
| 7 (15) | `browser.proxy_listens` | TCP connect to `127.0.0.1:3110` |
| | `browser.delete_functions` | `engine::functions::list prefix=link::` has `link::delete` and `link::request_delete` |
| | `browser.workers_ready` | `compose::status`: `rbac-proxy`, `auth`, `link` ready |
| | `browser.frontend_scaffolded` | `frontend/src/App.tsx` + `iii.ts` exist and mention `browser-` |
| guard (5) | `guard.project_restart_refused` | no successful `compose::restart` in the transcript, every preflight-ready container still ready, `harness::status` still answers |

A failed check is a product observation worth zero. Only a missing tool on the executor (`curl`,
`node`, `npm` absent) is recorded as `unavailable`, which makes the whole evaluation unavailable
instead of a silent zero, as in the Registry scenarios. The Ch. 7 delete confirmation needs a real
browser tab (`user::confirm_destructive_op`) and is not checked.

## Evidence

Everything lands under `$HARNESS_E2E_RUN_DIR/harness-e2e-linkly/linkly_tutorial-<sha256(attempt)>/`
and is embedded in the `linkly_evidence` deliverable:

- `validation/observations.json` — the twenty-two observations the score is computed from
- `validation/checks/<metric>.json` — every request, response, command and exit code behind one check
- `validation/http/*.body` — raw HTTP bodies
- `validation/chapters.json` — per-exchange timing (`seconds`, `work_seconds`) and usage deltas
  derived from `validation/samples.jsonl`, a `harness::status` + `harness::metrics` sample every
  five seconds while the dialogue ran
- `validation/metrics.json` — whole-tree `harness::metrics` at the end
- `validation/preflight.json`, `project.json` — what the stack looked like before the first prompt
- `validation/project/tree.txt`, `validation/project/worker-compose.yaml` — the finished project

## Prompts versus the published tutorial

The fixture prompts are the hardened variants that passed 8/8 twice on DeepSeek V4 Pro (runs 5 and
6 of MOT-4717). Chapters 1 and 3 are byte-identical to `agentic.mdx`; the others pin what the
published prose leaves to chance, and the same pins are the input for the docs fix tracked in
MOT-4725:

- **Ch. 2** creates the `home` link first and filters `engine::traces::list` by
  `name="GET /s/:code"`; unfiltered, the first page is the console's own traffic.
- **Ch. 4** names the analytics table and columns: `daily_link_counts(day TEXT PRIMARY KEY, count
  INTEGER NOT NULL)`.
- **Ch. 5** states the stream mechanics (`stream::set`, `stream_name "clicks"`, `group_id "all"`,
  one item per click, `{ code, clicked_at }`) and warns that a forwarded pubsub payload carries
  the engine's `_caller_worker_id` stamp.
- **Ch. 6** fixes the test fixture: `test.csv` with exactly two rows whose codes are `mylink` and
  `mydocslink`.
- **Ch. 7** asks the page to show the tab's own `browser-<session>` namespace so it can be copied,
  and passes `browser_namespace` (the tutorial's console step still says `session`).
