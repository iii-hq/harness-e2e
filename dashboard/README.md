# Harness E2E benchmark dashboard

This React application replaces the generic benchmark-action index. It uses the
same frontend stack as `workers/console`: React 19, strict TypeScript, Vite,
Tailwind CSS, Biome, Vitest, and pnpm. The workflow-generated `data.js` remains
the source of truth for metric trends. `executions.js` indexes workflow attempts,
and `runs/<execution-id>.json` supplies the retained execution report.

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
`http://localhost:4173/index.html` on the same machine, or replace `localhost`
with the machine's address when accessing it remotely.

The dashboard can now execute one or more scenarios against the Harness already
running at `III_URL`. It discovers registered provider/model pairs from that
stack and scenario ids from the same E2E binary. The primary form only asks for an
optional label, a subject model, and scenarios; URL, judge override, run count,
and technical retries remain under **Advanced options** with safe defaults. Use
**Refresh catalog** after restarting the Harness or changing its URL. The binary
runs only one experiment at a time, streams its log, indexes the resulting
`results.json`, and keeps run metadata and logs under
`target/harness-e2e-local-runs/`.

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

The React boot loader activates local execution controls only when
`executions.js` declares `mode: "local"`. The Pages publisher always emits
`mode: "published"`, so the published dashboard keeps using only its static
history and never calls the loopback execution APIs.

The execution label is optional and intentionally descriptive only. The local
dashboard does not inspect or record Harness code changes: restart or modify the
Harness however you want, run another experiment, then select any two execution
rows and open **Compare selected**. The comparison always remains available;
different subjects, run counts, scenario sets, and behavioral contracts are
shown as warnings instead of blocking the comparison.

Use `--listen 0.0.0.0:PORT` to select another port and `--runs-dir` to select
another local history. Local mode exposes controls that can start and cancel E2E
runs, so expose the port only on a trusted network. Use `--listen
127.0.0.1:4173` to restrict access to the local machine. The WebSocket URL is
accessed on the host by the runner and does not need to be reachable by the
browser.

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
Pages.

Each full execution summary also carries compact per-scenario averages for
tokens, wall time, cost, function calls, function-call errors, sessions, and
turns. Tokens mean input plus output; cache-read tokens are already represented
in input usage and are not added again. The execution table also exposes exact
total tokens and function calls for every retained diagnostic report.

Operational health is the primary overview. Efficiency appears after the latest
status, completeness, first actionable failure, KPIs, and scenario matrix. Its
cards show current suite totals, while deltas use only successful scenarios with the same subject,
scenario id, and behavioral contract fingerprint. New and changed scenarios
collect five comparable executions before receiving a trend verdict. Removed
scenarios remain visible as historical rows and never count as an efficiency
gain. Contract changes start a new baseline instead of joining incompatible data.
Select any scenario in the efficiency table to compare that scenario execution
by execution. The modal switches between cost, tokens, duration, function calls,
and function errors, marks contract boundaries, and links each point to its full
execution details.
