# Harness E2E benchmark dashboard

This React application replaces the generic benchmark-action index. It uses the
same frontend stack as `workers/console`: React 19, strict TypeScript, Vite,
Tailwind CSS, Biome, Vitest, and pnpm. Published builds keep the static execution
index, split test catalog, lazy per-test evidence shards, and
`runs/<execution-id>.json` reports. Local mode uses a scoped iii data surface so
the browser requests only the execution page, test result, report, catalog, or
unread log suffix it currently needs.

Install, validate, and run the frontend with hot reload:

```bash
pnpm install --frozen-lockfile
pnpm typecheck
pnpm lint
pnpm test
pnpm dev
```

The dev server listens on `0.0.0.0:5173` and proxies dashboard APIs to
`http://127.0.0.1:4173` by default. Set `HARNESS_E2E_DASHBOARD_URL` when the Rust
server uses a different origin.

Start the local dashboard from the repository root:

```bash
cargo build --locked --bin harness-e2e
target/debug/harness-e2e dashboard
```

The server listens on `0.0.0.0:4173` by default. Open
`http://localhost:4173/#/overview` on the same machine, or replace `localhost`
with the machine's address when accessing it remotely.

The dashboard uses the same dependency-free hash-routing pattern as Console.
Canonical routes are `#/overview`, `#/tests`, `#/capability`, `#/executions`,
`#/execution/<id>`, and `#/coverage`. The old `#/scenarios` route redirects to
Tests, while `#/compare/<from>/<to>` remains a deep link into the same versioned
Tests view. All views use the single `index.html` entry point.

The dashboard can now execute one or more scenarios against the Harness already
running at `III_URL`. It discovers registered provider/model pairs from that
stack and scenario ids from the same E2E binary only when the execution dialog
opens. The primary form only asks for an optional label, a subject model, and
scenarios; URL, judge override, run count, and technical retries remain under
**Advanced options** with safe defaults. Use **Refresh catalog** after restarting
the Harness or changing its URL. The binary runs only one experiment at a time,
streams incremental log chunks, indexes the resulting `results.json`, and keeps
run metadata and logs under `target/harness-e2e-local-runs/`.

Local data follows the Console architecture: the React shell opens one lazy
WebSocket to the Rust server, which proxies only the dashboard's allow-listed iii
functions and change trigger. The initial overview receives at most 25 compact
summaries; filters, search, and subsequent pages execute on the server. An
execution page fetches one summary plus one report. Tests first loads immutable
system-version and cohort descriptors, then one compact row per test. Changing
a row's test version calls `e2e::dashboard::test-version-get`; retained
observations load only when that row is expanded. The backend builds one cached
read model from retained reports, pools raw run scores, and invalidates it on run
changes. Same-origin HTTP mirrors the iii functions and is used only when the
iii transport is unavailable.

To present runs submitted through the asynchronous `e2e::*` worker, point the
dashboard directly at that worker's output root. Canonical control-plane run
directories are discovered from their embedded execution identity and do not
need dashboard-specific metadata:

```bash
target/debug/harness-e2e dashboard \
  --view-only \
  --runs-dir target/e2e-demo/runs
```

`--view-only` labels the data as observed reports, hides the direct local
runner, and does not register its run, cancel, or catalog HTTP endpoints. This
is the presentation mode for executions submitted through `e2e::*`.

The dashboard executes itself as an isolated child process, so changing and
restarting the Harness never recompiles the E2E client. `serve` is an alias for
`dashboard`; neither command has a Cargo fallback.

The React boot loader activates local execution controls only when the runtime
descriptor reports `mode: "local"`. If the runtime surface is unavailable it
falls back to `executions.js`; the Pages publisher always emits
`mode: "published"`, so the published dashboard keeps using only its static
history and never calls the local execution APIs.

The execution label is optional and intentionally descriptive only. The local
dashboard does not infer a system version from that label: it uses the immutable
source revision or registry stack lock captured in results-v2. Tests compares
system version A with B inside one exact evaluation cohort. Each row keeps its
own contract-version selector. Changed case sets and contracts remain visible
side by side, but their numeric deltas are disabled.

Use `--listen 0.0.0.0:PORT` to select another port and `--runs-dir` to select
another local history. Local mode exposes controls that can start and cancel E2E
runs. Its `/ws` route is intentionally restricted to the dashboard read/run
functions and browser callbacks, but it is not an authentication boundary:
expose the port only on a trusted network. Use `--listen 127.0.0.1:4173` to
restrict access to the local machine. The Harness WebSocket URL is accessed on
the host by the runner and does not need to be reachable by the browser.

To preview the sample fixtures without a Rust backend, build and serve the Vite
bundle:

```bash
pnpm build
pnpm preview
```

When generated data is absent, the pages load their sample fixtures and label
the view as preview data. Test the React application and both data contracts
with:

```bash
pnpm test
node --test tests/dashboard/*.test.cjs
```

Metric names are stable identifiers:

```text
<quality|efficiency|reliability>::<subject>::<scenario|suite>::<metric>
```

The execution index retains 100 workflow attempts. The latest 30 also retain the
complete execution report: per-run prompts, transcripts, criteria, metrics,
costs, retries, hard gates, traces, and failure evidence. Each publish updates
the retained report metadata and removes unreferenced run files before deploying
Pages. It also emits `tests/index.json` for compact version/test metadata and one
`tests/data/<digest>.json` evidence shard per retained test version.

Each full execution summary also carries compact per-scenario averages for
tokens, wall time, cost, function calls, function-call errors, sessions, and
turns. Tokens mean input plus output; cache-read tokens are already represented
in input usage and are not added again. The execution table also exposes exact
total tokens and function calls for every retained diagnostic report.

Operational health remains the primary overview. Quality is never collapsed
into a suite-wide score. The Tests view is the comparison surface: it shows
pooled raw-run score, sample size, pass rate, outcome classes, cost, tokens, and
runtime for each test/version/system-version tuple. Technical and infrastructure
failures remain explicit outcomes and are never converted into zero scores.
