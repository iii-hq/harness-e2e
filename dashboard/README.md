# Harness E2E Console page

This React application is an injectable Console page. Its build emits only
`dist-console/page.js` and `dist-console/styles.css`; the worker serves both
through `e2e::ui-content` and announces them with Console asset triggers.
Runtime data and actions use the `e2e::dashboard::*` functions registered by the
same worker. There is no standalone HTTP server, WebSocket proxy, static SPA or
`dashboard`/`serve` CLI command.

Execution details present the assessment contract as one objective layer: the
system outcome of each run and the deterministic conclusions that produced it.
The assessment matrix can be filtered by failures, availability, or asset
involvement. Every conclusion retains its criterion identity and links to the
immutable evidence register; missing assessment data is shown as unavailable
rather than inferred.

Install, validate, and build the frontend:

```bash
pnpm install --frozen-lockfile
pnpm typecheck
pnpm lint
pnpm test
pnpm build
```

The Console routes live under `#/ext/harness-e2e`. The page exposes Overview,
Tests, Executions and Plans and keeps entity detail inside the same extension
route.

The Console page can execute one or more scenarios against the Harness already
running at `III_URL`. It discovers registered provider/model pairs from that
stack and scenario ids from the same E2E binary only when the execution dialog
opens. The primary form only asks for an optional label, a subject model, and
scenarios; URL, the judge model Markdown tests need, run count, and technical
retries remain under **Advanced options** with safe defaults. Use **Refresh
catalog** after restarting the Harness or changing its URL. The binary runs only
one experiment at a time, streams incremental log chunks, indexes the resulting
`results.json`, and keeps run metadata and logs under the worker's configured
evidence root.

The React page uses the Console host's iii client and change trigger. The initial
overview receives at most 25 compact
summaries; filters, search, and subsequent pages execute on the server. An
execution page fetches one summary plus one report. Tests first loads immutable
system-version and cohort descriptors, then one compact row per test. Changing
a row's test version calls `e2e::dashboard::test-version-get`; retained
observations load only when that row is expanded. The backend builds one cached
read model from retained reports, pools raw run scores, and invalidates it on run
changes. There is no alternate HTTP or static-data transport.

In the Console, Plans offers **Reference: Release Control** through the RC browser
bridge. Each reference plan combines remote and local execution history and uses
the same comparison cards as scenario history. Scenario links preserve the selected
reference and candidate in the existing A → B comparison. Keep the authenticated
RC tab connected to the same personal Engine. Remote results are fetched on
demand; they are not installed into the native runs directory and no GitHub
token is needed. Origin labels distinguish Release Control results from local experiments.

Selecting **run locally** imports the selected execution's materialized test
parameters only when requested and starts a new local plan using the current
scenario contracts. Earlier plans and results are preserved. The local Harness
and scenario implementations may differ from the remote
reference; the comparison is descriptive. No local execution is posted to RC.
The comparison offers the same opt-in filter as RC: **Exclude tests with a zero
or missing result in A or B**. It removes matching test slots from both sides
and recalculates metrics; original executions remain unchanged.
A disconnected RC bridge leaves the existing local execution tools available.

The execution label is optional and intentionally descriptive only. The local
page does not infer a system version from that label: it uses the immutable
source revision or registry stack lock captured in `results.json`. Tests compares
system version A with B inside one exact evaluation cohort. Each row keeps its
own scenario-version selector. Changed case sets and contracts remain visible
side by side, but their numeric deltas are disabled.

Test the React page and its data contracts with:

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
costs, retries, runtime checks, traces, and failure evidence. Each publish updates
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

## UI guard-rails

Three checks keep the dashboard from accumulating new visual debt while the
design-system migration runs; all of them are part of `pnpm test` and
`node --test tests/dashboard/*.test.cjs`:

- `tests/dashboard/css-debt.test.cjs` counts 1px borders, radii other than the
  6px token, text below 11px, shadows, `!important` and arbitrary Tailwind
  sizes. The counts in `css-debt.baseline.json` can only go down. After a
  migration removes debt, lock the lower numbers in with
  `CSS_DEBT_UPDATE=1 node --test tests/dashboard/css-debt.test.cjs`.
- `tests/dashboard/theme-contrast.test.cjs` resolves the shell's text tokens
  (`--text`, `--text-soft`, `--text-muted`, `--accent`, `--success`,
  `--warning`, `--danger`) through their `var()` chains and requires 4.5:1 on
  the panel, the raised panel and the fill in both themes, plus 3:1 for
  `--control-edge`. New colours must be channel lists (`--he-*-rgb`) or
  `color-mix()` of host tokens: the console build rewrites unknown hex
  literals to `var(--color-ink)`.
- `tests/dashboard/shell-narrow-nav.test.cjs` and
  `src/components/DashboardShell.test.tsx` describe the CSS-only toggle for the
  narrow section select (`todo` / `it.fails` until it lands).

Pull requests that touch the UI attach before/after captures from the Console
fixture:

```bash
pnpm screenshots
```

Captures and a typography census (`census.json`) land in
`dashboard/.screenshots/`, which is ignored by git.

## Executable profile plans

The dashboard has one kind of plan and one baseline/candidate lifecycle. Plan executions use the shared execution detail page, with aggregate metrics, scenario results and native evidence. **My
plans** uses the existing plan table and detail visualization for every plan.
**New plan** opens the same form for a blank scope, a starting profile or a copy.
The profiles are templates: they populate coverage, purpose, repetitions and
retry policy. Users may edit the scope and explicitly select the execution model,
plus the judge model when the scope includes a Markdown test. The saved plan owns
that configuration; later template changes do not change it or prevent execution.

**Save plan** keeps the configuration editable. **Save and run** saves it,
checks requirements and starts the baseline. Busy admission preserves the draft
and links to active work. **Duplicate plan** preserves scope, policy and evaluator,
asks for a new execution model and starts without baseline, candidates or history.
All plans lock configuration at first admission, capture a baseline only after
complete technically valid evidence, and run candidates through the same controls,
charts and history. There is no Evolution-specific lifecycle.

The shared Rust coordinator persists every child identity before dispatch and
reserves admission across the whole execution. It cancels active work before
releasing admission, retains finished evidence and marks remaining slots. Restart
reconciles retained children and interrupts the execution without resuming it.
The main execution list shows the parent; its detail links to native artifacts.
No synthetic Results v4 report is created. Missing telemetry stays unavailable.

Saved plans (`schema_version: 3`) and composed receipts live in the local SQL
store (storage schema 3), accessed only through the database worker. Run the
explicit `migrate-storage` dry-run and apply before switching from storage schema
1 or 2. Existing PlanStore files are migration inputs, never runtime authority.
The migration preserves IDs, baseline/candidate relationships, slots and native
child references; corrupt or active records block the cutover.

The plans list combines local plans and imported RC plans, marked `remote`.
Import history accepts the versioned JSON transport or explicitly discovers and
exports a plan through the RC bridge. Updating an imported plan is explicit.
Lists, execution detail, test history and comparisons read the local copy, even
when RC is disconnected. Imported history has no local cancel/edit/admit action.
Reproduction creates a separate local configuration and checks local capability.

GitHub evidence is fetched on demand through `e2e::dashboard::evidence-open`
using local `gh` authentication and Python 3. Paths stay relative to the bundle.
The reader verifies the workflow/attempt identity and manifest hashes and returns
availability independently of the retained results. File retrieval does not
create or select personal conversations.

Creation, reading, updates and starts use the `plan-*` iii functions. Starting a
plan requires a caller idempotency key. `e2e::dashboard::plan-control` provide requirements, historical import, explicit reproduction, export, execution lookup and
cancellation. The former profile-plan endpoint, duplicate creation/start actions,
native plan-context tracking and manual-route alias have been removed.

Run deterministic browser acceptance after building the dashboard and Rust binary:

```bash
pnpm exec playwright install chromium
pnpm test:profiles
```

This test uses a local fixture server and never calls a model. Rust coordination
tests materialize every profile, including the 47 Capability and 90 Evolution
slots, and pass Resilience exports through the existing Python suite validator.
