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

The Console routes live under `#/ext/harness-e2e`. The page exposes Tests,
Executions, Suites and Stacks and keeps entity detail inside the same extension
route.

**Run tests** executes a suite, or scenarios ticked by hand, against the
Harness already running at `III_URL`. Its first field is the suite (see
[Suites](#suites)); picking one ticks its scenarios, runs and retries, and
changing any of them makes the suite unnamed. **Run again**, on any execution
(local or imported), opens the same form with that execution's suite,
scenarios, runs, technical retries, model and agent profile copied and
editable; a suite this runner does not list is offered *as recorded*. Both call
`e2e::dashboard::execution-start`, which creates an execution with a `local`
origin on this worker's stack, and the Console follows it on its page. The
execution records its suite (name and snapshot digest) and shows it in its
header; an imported execution also shows the stack its contract names. The form discovers registered provider/model pairs from the stack and
scenario ids from the same E2E binary only when it opens, and still sends what
it holds when that catalog cannot be read; runs, retries and agent profile sit
under **Advanced**. Run tests starts from the model of the newest execution
that the catalog still lists, and picks none without one. There is no seed:
every execution the Console starts runs the canonical cases, so any two pair
by scenario and repetition when compared (a `seed` an older Console sends is
ignored). A scenario of a sequential
group (such as `registry_implementation` then `registry_verification`) brings
the whole group: the catalog lists the groups (`scenario_groups`), so the form
ticks and counts the group before running, and the execution notes it. One
execution runs at a time: a start while another runs names that execution
(`"<label>" (<id>) is still running`), and the form offers to open it. A finished execution can be
deleted with its native runs, previous attempts included.

**Run this scenario again**, on any scenario of a finished local execution,
calls `e2e::dashboard::execution-slot-rerun`. Every
check runs first and nothing changes: the recorded requests must still be
valid, and the stack's identity (Harness and engine versions, runner
revision, native contracts) must be the one the execution pinned, or the call
is refused with what differs and Run again (a new execution) is the way. The
scenario's slots then run again with the requests they ran (every round, the
canonical case, a sequential group whole) and the execution is `running`. A
slot's run is replaced only when its new run is admitted; the last attempt
counts, as a re-run job does on GitHub, even when it does worse. The replaced
run stays on the slot (`previous_attempts`) and in the detail
(`previous_reports`), listed under the scenario with its result, score,
reason and evidence record, and outside the score, totals, measurements,
comparison and test history. A rerun that is cancelled or stops (a restart, a
failed admission, a run that reports another identity, which is removed and
never counted) leaves the rounds it did not replace as they were and returns
the execution to its finished state with a warning; an execution with slots
that never ran stays cancelled or interrupted until they do. The scenario
reads `rerun ×N` on its execution and in a comparison and its summary. A
rerun keeps the stack the execution recorded and warns, once, when workers
differ. An imported execution is not run here, which would mix stacks:
re-run its job on GitHub and import the run again (the import takes the
highest attempt).

**Where** picks this harness (the above) or **Docker**, which asks for a
**Stack** (the repository's, this Console's, or, running an execution again,
the one it recorded, *as recorded*); the worker sends the stack's YAML to the
executor. The execution appears at once with its groups, each `queued`,
`running`, `done`, `failed`, `cancelled` or `interrupted`, and the phase it is
in; its results arrive when every group ended and the worker imported them.
Cancel stops its containers and keeps what finished. Running a scenario of it
again runs its groups in new containers as the execution's next attempt,
finalizes and imports again: the last attempt counts. See [Run in
Docker](../README.md#run-in-docker). Run again of a GitHub run that recorded
its stack starts in Docker on it; one that recorded none runs on this harness.

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
same record as one run here: the list, the report, evidence and
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
declares, inside that execution's directory, up to 10 MB. **Rerun selected**
opens Run again with B's parameters and only the ticked scenarios.

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

## Suites

A suite is only what to test: its scenarios, how many times each runs and how
many technical retries a crash gets. The model, the agent profile and the stack
are chosen when it runs. **Suites** lists the master plan's suites
(`config/test-plan.json`, read-only) and this Console's. **copy** makes a suite
of this Console from any suite and opens it to edit its name, tests, runs and
retries; **delete** removes one. Executions that ran a suite keep its name and
digest after it changes or goes.

An execution records its suite in its parameters: the id, the name and the
digest of the snapshot it materialized to (`profile_sha256`). A suite of the
master plan run as it is materializes as reviewed, so its digest is the one the
exact-stack workflow records for it; scenarios ticked by hand make an unnamed
suite. Comparing two executions lists the suite among the parameters that
differ.

## Stacks

A stack is where a suite runs: an iii Compose project plus the executor's keys
`iii` (the iii release) and optional `template`, written as
[`stacks/*.yaml`](../stacks/) writes it. **Stacks** lists the repository's
stacks (embedded in the binary; **view** opens their YAML read-only) and this
Console's, each with the iii release, template and containers (with the version
or commit each pins) it declares. **copy** makes a stack of this Console from
any stack and opens its YAML to edit; the text is kept exactly as written,
comments included, and **delete** removes one.

The stack is read with YAML 1.2 rules, as Compose reads it: `on`, `no` and dates
stay text. It is refused only when it is past 32 KiB or its aliases expand past
1 MiB (integrity), when it does not parse, or when it has no `containers`
mapping. Everything else is a warning next to the editor and on the stack,
never blocking: a container that is not a mapping or has no `worker`, a worker
that is neither `package://` nor `path://`, a `path://` worker (it exists only
on this machine), a `commit:` pin (it takes effect once the executor runs commit
pins), a top-level key neither the executor nor Compose reads, a tag the
executor's loader refuses (`!env`, `!!python/…`), and an `iii`, `template`,
`version` or `commit` that is not text to the executor (`1.10`, `0123456`:
quote it). Run tests in Docker runs on one of them; on this harness an
execution runs on this worker's own stack.

The shared Rust coordinator persists every child identity before dispatch and
reserves admission across the whole execution. It cancels active work before
releasing admission, retains finished evidence and marks remaining slots. Restart
reconciles retained children and interrupts the execution without resuming it.
The main execution list shows the parent; its detail links to native artifacts.
No synthetic results report is created. Missing telemetry stays unavailable.

Suites, stacks and composed executions live in the local SQL store, accessed
only through the database worker. The store carries no version and no migration
step: at start the worker recreates any table whose layout fingerprint moved,
keeping every suite, stack, execution and receipt the current binary can still
read.
The baseline/candidate plans suites replaced are dropped; the executions they
ran stay, without a suite.

`e2e::dashboard::suites-list`, `suite-create`, `suite-update` and
`suite-delete` read and change suites; `stacks-list`, `stack-create`,
`stack-update` and `stack-delete` read and change stacks; `credentials-list`,
`credential-set`, `credential-delete` and `credentials-import` read (names
only) and change the provider credentials Docker executions receive, shown
below the stacks; `execution-cancel` stops an execution.

Run deterministic browser acceptance after building the dashboard and Rust binary:

```bash
pnpm exec playwright install chromium
pnpm test:profiles
```

This test uses a local fixture server and never calls a model. Rust coordination
tests materialize every profile, including the Capability and Evolution slots.
