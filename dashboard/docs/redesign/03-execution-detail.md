# Execution detail on the Console's components

The second screen rebuilt under the redesign, after the executions ledger.
Route `#/ext/harness-e2e/execution/<id>` with the evidence record on
`/run/<runId>`. Job: read the verdict, drill from the aggregate to each test
and each run's evidence, and act (cancel, re-run, delete, open in chat).

## What changed

The previous page composed twelve components and 6.2k lines: two parallel
result views (grouped assessments and a scenario matrix), two metrics views,
four disclosure layers with "scents", a section bar of anchors, a CSS island
for the metrics and 34 legacy `.scenario-*` selectors. The rebuilt page is one
column with one of each thing:

| Part | Built with | Notes |
|---|---|---|
| Actions in the section bar | host `Button` | `cancel execution` (primary while live, with `back to plan` beside it for plan-owned executions), else `re-run same scope` / `back to plan` (primary), `copy link` (pill), `delete` (ghost, not for plan-owned executions) |
| Header | `PageHeader` + host `Badge` | breadcrumb, title, one sentence (`N tests · M runs`, or the live progress), the status badge in the header actions slot |
| Identity | definition list on host tokens | subject, started, trigger, id |
| Notices | host `StatusPanel` (`warn`) | evidence bundle unavailable (with the retained totals as cards), refresh failed, live progress error, persistence errors |
| Live state | host `StatusPanel` (`info`) + `LiveProgressPanel` / `PlanProgress` | `running · 1 of 4 tests · 2m 00s elapsed` and what to expect |
| Verdict | host `StatusPanel` (`success` / `warn` / `alert` from the aggregate) | the headline and the next step, once, in the tone of the outcome |
| Numbers | six `MetricCard`s | tests passed/total, score mean, completion rate, runtime, tokens with reported cost, turns with function calls |
| Results | host `Table`, one row per test | test and definition digest, result badge with the reason, score, runs, runtime, tokens, turns (summed over the retained runs); the row opens onto its runs |
| Runs of a test | nested host `Table` | run, outcome badge, score, runtime, tokens, turns, `transcript`, `evidence` (the record route), chat action; the workflow steps of composite scenarios below |
| Evidence record | existing `AssessmentDetailDialog` on the `/run/<runId>` route | unchanged for now: criteria matrix, telemetry, recommendation |
| Provenance | host `CollapsibleCard` | results contracts (with a `warn` badge when written under another contract), the fact list, `copy json`, the raw JSON |
| Delete | host `ConfirmDialog` | |
| Loading / error | host `Skeleton`, host `EmptyState` with retry and a way back | |

Logic lives in `src/lib/execution-detail.ts` (summary sentence, status,
identity, metric cards, snapshot cards, live copy, provenance rows, result
filters) and `src/lib/status-badge.ts` (the one status → badge mapping, shared
with the ledger). The page renders.

## Retired

`ScenarioMatrix`, `ExecutionMetricsPanel` and their tests; the page-internal
`NarrativeSection`, `CountsSection`, `ProvenanceSection`, `SectionBar`,
`LiveState`, `DisclosureLayer` use and the four anchor layers; the
`.scenario-*` block of `legacy.css`. `PrimaryMetricsView` and
`DisclosureLayer` survive only because the plan pages still use them.

## Layering, again

`legacy.css` styled bare `table`, `th`, `td` and `tbody tr`, which reached
the host's `Table` inside the extension. Those rules are now scoped to
`.page-shell`, the wrapper only legacy pages carry.

## Margins and rhythm

The host's table cells drop their outer padding on the first and last
column, so a table sits flush with the content edge, as the Console's own
tables do. The opened runs of a test are inset by one gutter (`px-4`) inside
the highlighted row so they read as nested. Sections sit 32px apart
(`mt-8`); the verdict and the metric strip form one group 12px apart; the
provenance trigger and body take the host's card header and body paddings
(`px-3 py-2.5`, `p-3`). The metric strip is an auto-fit grid (`minmax(10rem,
1fr)`), so six, five or four cards fill the row without a trailing gap. The
disclosure caret leads the test row, so it stays in view when a narrow table
scrolls and clicking it never scrolls the row away.

## Open

- Narrow containers: the results table scrolls horizontally like the host's
  tables; the runs table nests inside it.
- The evidence record dialog still renders with the extension's `Dialog`;
  it moves to the host `Dialog` when the transcript dialog does.
