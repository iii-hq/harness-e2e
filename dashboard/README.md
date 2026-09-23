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

**Run tests** executes one or more scenarios against the Harness already
running at `III_URL`. **Run again**, on any execution (local or imported), opens
the same form with that execution's scenarios, runs, technical retries, model
and agent profile copied and editable. Both call
`e2e::dashboard::execution-start`, which creates an execution with a `local`
origin and no plan on this worker's stack, and the Console follows it on its
page. The form discovers registered provider/model pairs from the stack and
scenario ids from the same E2E binary only when it opens, and still sends what
it holds when that catalog cannot be read; runs, retries and agent profile sit
under **Advanced**. Run tests starts from the model of the newest execution
that the catalog still lists, and picks none without one. There is no seed:
every execution the Console starts runs the canonical cases, so any two pair
by scenario and repetition when compared (a `seed` an older Console sends is
ignored). New plans run the canonical cases too; a saved plan keeps its seed
when edited or duplicated. A scenario of a sequential
group (such as `registry_implementation` then `registry_verification`) brings
the whole group: the catalog lists the groups (`scenario_groups`), so the form
ticks and counts the group before running, and the execution notes it. One
execution runs at a time: a start while another runs names that execution
(`"<label>" (<id>) is still running`), and the form offers to open it. A finished execution without a plan can be
deleted with its native runs; a plan's executions go with the plan.

Before its first slot every local execution records its stack: the containers
of the compose project that runs this worker (`package://` or `path://`, the
requested version, and the commit and dirty state of each path checkout) and
the versions `engine::workers::list` reports in the worker's namespace. What
cannot be read becomes a warning shown with the execution, never an error. A
scenario this runner does not know, or a run that fails on this stack, fails
only its own slot; the others run.

The React page uses the Console host's iii client and change trigger. The initial
overview receives at most 25 compact
summaries; filters, search, and subsequent pages execute on the server. An
execution page fetches one summary plus one report. Tests first loads immutable
system-version and cohort descriptors, then one compact row per test. Changing
a row's scenario definition calls `e2e::dashboard::test-version-get` with the
definition digest; retained observations load only when that row is expanded. The backend builds one cached
read model from retained reports, pools raw run scores, and invalidates it on run
changes. There is no alternate HTTP or static-data transport.

Executions offers **Import from GitHub**: completed exact-stack workflow runs of
the worker's `github_repository`, listed at once through
`e2e::dashboard::github-runs-list` (one `gh api` call), newest creation first,
with the latest attempt's date and the Release Control execution beside it.
Each run's suite, model, profile and runner version then fill in per row from
its contract artifact through `github-run-contracts`, read once and cached; the
Harness version is known only once the run is imported. `github-run-import` answers with
an `importing` execution at once; the worker downloads the run's bundle and
installs its native runs like finished local runs. An imported execution is the
same record as one planned here: the list, the report, evidence and
`e2e::dashboard::execution-rename` treat both alike, and its origin is shown as
text (`local` or `GitHub #<run>` with a link). `gh` errors are shown as they come.

Tick two executions and **compare** (`#/compare/<a>/<b>`): the first is A, the
base. Both are read with `e2e::dashboard::execution-get` and compared the way
Release Control compares them, so the figures are the same: runs pair by
scenario and slot (scenario, case seed, repetition), and a scenario either side
could not measure (missing, redefined `case.inputs_sha256`, technically
invalid, undetermined, unscored) leaves both totals until the reader counts it
again. Screenshots a run's deliverables declare are read on demand through
`e2e::dashboard::evidence-read`, which serves only files the run's report
declares, inside that execution's directory, up to 10 MB.

The execution label is optional and intentionally descriptive only. The local
page does not infer a system version from that label: it uses the immutable
source revision or registry stack lock captured in `results.json`. Tests compares
system version A with B inside one exact evaluation cohort. Each row keeps its
own scenario-definition selector, which lists the `behavior_sha256` digest of
every retained definition shortened to its first eight hex characters. Changed
case sets and contracts remain visible side by side, but their numeric deltas
are disabled.

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
Pages. It also emits `tests/index.json` for compact definition/test metadata and
one `tests/data/<digest>.json` evidence shard per retained scenario definition,
named after that definition's digest without the `sha256:` prefix.

Each full execution summary also carries compact per-scenario averages for
tokens, wall time, cost, function calls, function-call errors, sessions, and
turns. Tokens mean input plus output; cache-read tokens are already represented
in input usage and are not added again. The execution table also exposes exact
total tokens and function calls for every retained diagnostic report.

Operational health remains the primary overview. Quality is never collapsed
into a suite-wide score. The Tests view is the comparison surface: it shows
pooled raw-run score, sample size, pass rate, outcome classes, cost, tokens, and
runtime for each test/definition/system-version tuple. Technical and infrastructure
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
retry policy. Users may edit the scope and explicitly select the execution
model. The saved plan owns that configuration; later template changes do not
change it or prevent execution.

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
No synthetic results report is created. Missing telemetry stays unavailable.

Saved plans and composed receipts live in the local SQL store, accessed only
through the database worker. The store carries no version and no migration
step: at start the worker recreates any table whose layout fingerprint moved,
keeping every execution, plan and receipt the current binary can still read.
Plans written by another binary are deleted, never migrated.

Creation, reading, updates and starts use the `plan-*` iii functions. Starting a
plan requires a caller idempotency key. `e2e::dashboard::plan-control` provide requirements, export, execution lookup and
cancellation. The former profile-plan endpoint, duplicate creation/start actions,
native plan-context tracking and manual-route alias have been removed.

Run deterministic browser acceptance after building the dashboard and Rust binary:

```bash
pnpm exec playwright install chromium
pnpm test:profiles
```

This test uses a local fixture server and never calls a model. Rust coordination
tests materialize every profile, including the Capability and Evolution slots.
