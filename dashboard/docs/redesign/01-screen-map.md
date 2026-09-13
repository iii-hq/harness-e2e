# Screen map: as built

The Harness E2E Console extension today, route by route, before any redesign.
Paths are relative to `dashboard/src`. Line numbers refer to `main` at the
time of writing (September 2026). Nothing here proposes a change; the
redesign starts from this map and from `00-design-system.md`.

## Routes

| Route (`#/ext/harness-e2e/…`) | Page | Main files | Lines |
|---|---|---|---|
| `tests` | A · Tests catalog | `pages/TestsCatalogPage.tsx` | 1023 |
| `tests/<id>` | B · Test history | `pages/TestHistoryPage.tsx`, `components/AboutTestPanel.tsx` | 2007 + 302 |
| `plans` | C · Plans list | `pages/PlansPage.tsx` | 1112 |
| `plans/new` | D · Plan create | `pages/LocalPlanPage.tsx`, `components/ExecutionSetup.tsx`, `ProviderModelDropdown.tsx`, `LocalRunnerDialog.tsx` | 580 + 862 + 411 + 490 |
| `plans/<id>` | E · Plan detail | `pages/PlanDetailPage.tsx`, `ImportedPlanDetailPage.tsx`, `components/PlanCharts.tsx` | 2942 + 866 + 471 |
| `executions` (default) | F · Executions list | `pages/ExecutionsPage.tsx` | 840 |
| `execution/<id>[/run/<runId>]` | G · Execution detail | `pages/ExecutionPage.tsx` + 9 components | 6028 |
| `compare` | H · Compare | `pages/TestsPage.tsx` | 1916 |

Total: 28.6k lines of non-test TypeScript, 9k lines of tests, 2.3k lines of
design system, 1.2k lines of CSS outside it (`legacy.css` 533,
`PrimaryMetricsView.css` 431, `dashboard-shell.css` 186).

---

## Route A and B — Tests catalog and Test history

Shared chrome for both routes: `components/DashboardShell.tsx:1` uses
`@iii-dev/console-ui` (`PageShell`, `PageBody`, `PageMain`, its own `PageHeader`)
and renders the section bar tests/executions/plans (`DashboardShell.tsx:80-84`,
section resolved at `:48-68`). Pages push their header buttons into that bar via
`components/DashboardPageActions.tsx:34-67`, which renders `null` itself and
styles links with the page-local `dashboardHeaderActionClassName`
(`DashboardPageActions.tsx:10-24`), not with DS `Button`.

---

## Route A — Tests catalog · `#/ext/harness-e2e/tests`

Route: `page: 'workspace', view: 'tests'` (`App.tsx:38-40`,
`hooks/use-hash-route.ts:95`, `:143-148`). File: `pages/TestsCatalogPage.tsx`
(1023 lines).

### 1. Job
Find a test by name or lifecycle and jump to its evidence, or judge at a glance
which tests have any retained executions at all.

### 2. Layout, top to bottom
| # | Section | Code | Primitives vs page markup |
|---|---|---|---|
| 1 | Console section bar + header actions (`new plan`, `compare versions`) | `TestsCatalogPage.tsx:306-327` (`TestsCatalogActions`), mounted at `:729-733` | page-specific: `dashboardHeaderActionClassName`, plain `<a>` |
| 2 | Page title `tests` + one-line counts + `catalog <sha>` with copy | `:735-742`, `CatalogRevision` `:329-353` | DS `PageHeader`; the copy button is raw `<button>` + Tailwind (`:337-350`) |
| 3 | Filter toolbar: search input, sort `Select`, lifecycle chips, realism `Select`, `with executions` chip, live count `<output>` | `:743-860` | DS `Input`, `Select`, `FilterChip`, `FilterChipGroup`; search icon/clear-X and the `<output>` are page markup (`:745-769`, `:850-858`) |
| 4 | Lifecycle groups: `h2` with status dot + count, one table per group; retired collapses to a single button | `:899-989` | `h2` uses `ds-label` + `ds-status-dot`; the collapse buttons use `buttonClassName({variant:'quiet'})` |
| 5 | Catalog table: columns test, definition, human horizon, realism, evidence, runs, last execution, (history link) | `CatalogTable` `:509-548`, `COLUMNS` `:355-392`, `CatalogRows` `:394-507` | DS `DataTable`/`DataTableRow`, `numericCellClassName`; every cell body is bespoke Tailwind (`DimensionCell` `:220-241`, definition chip `:451-459`, history link `:493-500`) |
| 6 | `load more tests` + progress caption | `:1002-1017` | `buttonClassName({variant:'secondary'})` on a raw `<button>` |

Screenshot (`shots-full/tests-light-1440.png`, `tests-dark-1440.png`, identical
layout, colors inverted): 68 catalog rows, 50 loaded, single `NEVER RUN · 50`
group, `show 40 more never run`, `load more tests`. Every row reads
`—  / no samples / 0 / No retained execution`; six of seven columns are empty
markers in the default state.

### 3. Actions
- Header: `new plan` (primary, only when the bridge resolved, `:309-317`),
  `compare versions` (`:318-324`).
- Row: whole row navigates to history (`DataTableRow href` `:420-427`), plus the
  test-id link (`:431-436`) and the trailing `history`/`evidence` link
  (`:493-500`) — three targets to the same URL; the trailing label changes word
  by `local` (`:498`).
- Toolbar: clear search X, sort select, 3-4 lifecycle chips, realism select,
  `with executions` toggle.
- Body: `show N more <lifecycle>` / `show fewer`, retired expand, `load more
  tests`, `clear filters` inside the empty state, copy catalog revision.
- No primary action inside the page body; the only primary is `new plan` in the
  console bar.

### 4. States
- loading: `CatalogSkeleton` (`:550-564`), 1 + 8 pulsing bars, `aria-busy`;
  header summary swaps to `loading the catalog…` (`:737`).
- error: DS `EmptyState tone="error"` `Test catalog unavailable` (`:862-869`);
  errors from `loadMore` land in the same banner and replace the table (`:622`).
- empty (no rows at all) vs empty (filtered): one `EmptyState` with two copy
  variants and a conditional `clear filters` (`:871-896`).
- loading more: button label `loading…`, `disabled`, `aria-busy` (`:1004-1012`).
- no running/live state: the catalog never subscribes to run changes.
- highlight: `?highlight=<id>` scrolls the row into view and clears after 2 s
  (`:671-691`), class `is-highlighted` (`:425-427`).
- filters and highlight are mirrored into the hash (`:630-634`) and re-read on
  `hashchange` (`:638-650`); scroll is saved in `sessionStorage` (`:652-669`).

### 5. Data
- `getDashboardDataBridge()` → `bridge.listTests({limit:50, cursor})`
  (`:587-605`, `:610-627`); bridge at `lib/dashboard-data-source.ts:658`, calls
  worker `e2e::dashboard::tests-list` (`src/dashboard/bus.rs:32`, handler
  `src/dashboard/bus.rs:730-736`). Responses are memoized per payload
  (`dashboard-data-source.ts:629-642`).
- Types `TestCatalogRow`, `TestsListResponse` in `lib/test-catalog.ts`.
- Derived in-page: `filterCatalogRows` `:254-271`, `sortCatalogRows` `:273-297`,
  `groupCatalogRows` `:299-304`, `catalogCountLabels` `:102-126`, the three
  `catalog*Presentation` helpers `:165-217`. Shared helper
  `catalogExecutionSummary` (`lib/test-catalog-view.ts:98-118`),
  `shortDefinition`/`definitionTitle` (`lib/definition-digest.ts`).
- Sorting, filtering, grouping and paging are all client-side over the rows
  fetched so far: filters apply only to loaded pages (`:693-696`).

### 6. Size and debt
- `TestsCatalogPage.tsx` 1023 lines, of which ~460 are presentation helpers and
  cell renderers; test file `TestsCatalogPage.test.tsx` 220 lines.
- `legacy.css` classes used: `page-shell` only (`:734`, defined
  `legacy.css:79-83`, and overridden on the same element by Tailwind width and
  padding utilities — the legacy rule is nearly dead weight).
  `legacy.css:506` `.test-catalog-panel` has no user left in the codebase.
- Exported for reuse by the history page: `catalogRealismPresentation` is
  imported by `pages/TestHistoryPage.tsx:92` — presentation logic leaking across
  screens.
- Lifecycle dot + label is hand-rolled (`:128-148`, `:437-447`) while
  `TestHistoryPage.tsx:1289-1300` shows the same lifecycle through DS
  `StatusBadge`: two vocabularies for one fact.
- Three links per row to the same target; `local` changes only the word.
- Counts are three different denominators in one line (`:113-125`) and repeated
  in the toolbar `<output>` and again beside `load more`.

---

## Route B — Test history / detail · `#/ext/harness-e2e/tests/<test_id>`

Route: `page: 'test-history'` (`App.tsx:23-24`, `use-hash-route.ts:92`,
`:173-176`). File: `pages/TestHistoryPage.tsx` (2007 lines).

### 1. Job
Read the retained evidence for one test: did it pass, what the metrics are, and
put two executions side by side.

### 2. Layout, top to bottom
| # | Section | Code | Primitives vs page markup |
|---|---|---|---|
| 1 | Console bar actions `compare systems`, `run this test` (primary) | `:1255-1270`, `runThisTest` `:1243-1251` | `dashboardHeaderActionClassName` |
| 2 | Breadcrumb `tests / <id>`, title, identity line, lifecycle badge, `local` chip, `copy sha`, `prev`/`next` | `:1272-1366` | DS `PageHeader` + `StatusBadge`; the `local` chip `:1302-1304`, the copy button and the prev/next pair are `buttonClassName` on raw elements (`:1306-1363`); disabled nav is faked with `pointer-events-none opacity-50` (`:1333`, `:1350`) |
| 3 | Release Control reference strip: `Reference: Release Control` button, reference `Select`, `back to reference plan`, `clear reference`, loading line, warning `Callout`, reference-case and candidate-case `Select`s, an `ObservationComparisonPanel` when both sides resolve | `:1368-1485` | DS `Select`, `Callout`, `buttonClassName`; layout is bespoke |
| 4 | Metric tiles: successful executions, mean score, median duration/tokens/cost | `:1531-1589` | DS `MetricCard` in a Tailwind container-query grid (`:1540-1543`) |
| 5 | `<details>` "Score trend" wrapping a `SectionPanel` with a hand-written SVG chart | `:1591-1610`, `ScoreTrendChart` `:266-464`, `SectionPanel` from `components/ExecutionComparisonPanel.tsx:113-143` | raw `<svg>`, ~200 lines, with an SVG `<g role="button">` as hit target (`:384-399`) |
| 6 | Filters: definition `Select`, `ProviderModelDropdown`, system `Select`, result chips, live count | `:1612-1705` | DS `Select`, `FilterChip`; `ProviderModelDropdown` is a 410-line page-level widget (`components/ProviderModelDropdown.tsx`) |
| 7 | Selection notice `Callout` | `:1707-1709` | DS `Callout` |
| 8 | History table: a/b checkbox, execution, model · system, result, score, duration, tokens, cost, actions | `:1742-1892` | DS `DataTable`/`DataTableRow`/`StatusBadge`/`numericCellClassName`; the a/b checkbox, the two-line cells and the action cluster are bespoke (`:1794-1886`) |
| 9 | `ObservationComparisonPanel` (local a/b) | `:1894-1906`, component `:526-679` | wraps `ExecutionComparisonPanel` + DS `Callout`/`StatusBadge` |
| 10 | `AboutTestPanel` — prompt, criteria, budget/denied | `:1909-1915`, `components/AboutTestPanel.tsx:217-302` | DS `Panel`, `StatusBadge`, `buttonClassName`; prompt block and criteria list are bespoke (`AboutTestPanel.tsx:80-213`) |
| 11 | Sticky selection bar: count, comparability sentence, `swap`, `clear`, `compare a → b` (primary) | `:1918-1994` | raw `div` `sticky bottom-0 bg-panel` + `buttonClassName` |
| 12 | Execution details `Dialog` (overlay) | `ExecutionDetailsDialog` `:683-860` | DS `Dialog`, `StatusBadge`, `Callout`; embeds `AboutTestPanel` (stacked) and `AssessmentWorkspace` (955 lines) |

Screenshot (`shots-full/test-history-light-1440.png`, `browser_cross_site`,
never run): only four blocks are visible — breadcrumb/title/identity, the lone
`reference: release control` button, the empty state, and `about this test`
(prompt, 4 criteria, limits). The primary action appears three times on one
screen (`run this test` in the bar, `run this test` and `add to a new plan` in
the empty state).

### 3. Actions
- Primary: `run this test` (bar `:1243-1251`; empty state `:1504-1513`) and, once
  two rows are ticked, `compare a → b` in the sticky bar (`:1976-1990`) which only
  scrolls to `#test-comparison-title`.
- Secondary: `compare systems`, `add to a new plan`, `prev`/`next` test,
  `copy sha`, `Reference: Release Control`, `back to reference plan`,
  `clear reference`, `swap`, `clear`, `clear filters`.
- Per row: a/b checkbox (`:1796-1802`), row click opens the dialog
  (`:1789-1792`), `open` link to the execution page (`:1869-1879`),
  `ScenarioChatAction` menu (`:1880-1884`).
- In the trend chart: clicking a point selects it as a/b (`:392-398`).
- Dialog footer: `open full execution report`, `ScenarioChatAction`, `close`
  (primary) (`:762-784`).

### 4. States
- loading (history): skeleton of 4 pulsing bars (`:1711-1723`); header summary
  `loading the history…` (`:1275-1277`); count `<output>` says
  `loading executions…` (`:1700-1702`).
- error (history): `EmptyState tone="error"` replaces everything (`:1487-1494`).
- empty (never run, unfiltered): `EmptyState` + two actions (`:1496-1527`).
- empty (filtered): `EmptyState` + `clear filters` (`:1724-1740`).
- reference: `referenceLoading` line (`:1421-1423`), `referenceError ||
  candidateError` warning `Callout` (`:1424-1428`), reference-with-no-results
  message set at `:1008-1011`, candidate-with-no-results at `:1038-1042`.
- running/live: only the RC candidate execution is watched —
  `watchExecution(bridge, candidateExecutionId, refresh)` (`:1047`,
  `lib/watch-execution.ts:4-49`, events + 5 s poll). The history table itself
  never refreshes while a run is in flight.
- selection invalidation: rows that leave the filter clear the a/b selection and
  raise `selectionNotice` (`:1161-1173`).
- dialog: own loading/error/loaded triad (`:847-855`).
- All of filters, a/b keys, open dialog, reference and candidate ids live in the
  hash (`:880-910`, `:1125-1142`).

### 5. Data
- `bridge.getTestHistory({test_id, test_version, subject_provider,
  subject_model, system_version_id, limit:100})` (`:1071-1101`) →
  `dashboard-data-source.ts:661-665` → `e2e::dashboard::test-history-get`
  (`src/dashboard/bus.rs:34`, handler `:744-749`).
- `bridge.listTests({limit:100})` for identity, spec and prev/next
  (`:1105-1123`) → `e2e::dashboard::tests-list`.
- `bridge.getExecution(id)` in the dialog (`:705-707`) and for the RC candidate
  (`:1034`) → `e2e::dashboard::execution-get` (`bus.rs:27`).
- `getImportedReference` / `listImportedExecutions`
  (`lib/release-control-reference.ts:815-837`) sit on the same bridge:
  `listExecutions` paged at 200 filtered by `origin === 'remote'`
  (`e2e::dashboard::executions-list`, `bus.rs:26`).
- `referenceScenarioObservations` / `localScenarioObservations`
  (`release-control-reference.ts:708`, `:743`).
- Comparison logic: `compareTestObservations`, `sameScenarioDefinition`,
  `testObservationKey` (`lib/test-history-comparison.ts`), `COMPARISON_METRICS`
  and formatters (`components/ExecutionComparisonPanel.tsx`).
- Local math in-page: `mean`/`median` (`:139-158`), `knownMetricCount` (`:233`),
  `contractSummary` (`:204-215`), `definitionStatement` (`:920-930`),
  `comparisonVerdict` (`:475-524`).

### 6. Size and debt
- `TestHistoryPage.tsx` 2007 lines — the largest page. Inside it:
  `ScoreTrendChart` ~200, `ObservationComparisonPanel` ~155,
  `ExecutionDetailsDialog` ~180, helpers ~150. Test file 288 lines.
- `legacy.css` used: `page-shell` only (`:1271`). `legacy.css:531`
  `.history-table` (`min-width: 1180px`) is dead — the table uses
  `data-history-table` + `minWidth="56rem"` (`:1745-1747`).
- Two comparison surfaces on one screen: the RC reference panel (`:1473-1484`)
  and the local a/b panel (`:1894-1906`) render the same component with
  different semantics, plus the sticky bar restates the same selection
  (`:1925-1946`).
- Three places state comparability in different words: `:638-650` (callout),
  `:664-670` (callout) and `:1936-1945` (sticky bar).
- Two identity vocabularies for lifecycle, as noted in Route A.
- `ScenarioChatAction` (295 lines) appears in row actions, comparison actions
  and the dialog, and in 5 other files (`PrimaryMetricsView`,
  `AssessmentWorkspace`, `ScenarioMatrix`, `SemanticTestFlow`, `TestsPage`).
- Content that belongs elsewhere: the RC reference strip is plan/execution
  comparison work grafted onto a single-test page; `AssessmentWorkspace` inside
  the dialog is the execution page's body (`pages/ExecutionPage.tsx` owns the
  same view); `AboutTestPanel` renders twice per session (page `:1909` and
  dialog `:826-828`).
- `listTests({limit:100})` (`:1108`) truncates identity and prev/next: a test
  past row 100 loses its spec, lifecycle badge and neighbours, while the catalog
  pages at 50. The history query is also capped at 100 executions with no
  "load more" (`:1085`).
- `statusPresentation` (`:182-197`) and `catalogRealismPresentation` (imported
  from the catalog page) duplicate `lib/execution-verdict.ts` /
  `lib/execution-view.ts` vocabulary used by the execution screens.

---

---

## Route C — Plans list · `#/ext/harness-e2e/plans`

Route `page: 'plans'` (`hooks/use-hash-route.ts:17`, `:122-123`). File:
`pages/PlansPage.tsx` (1112 lines).

### 1. Job
See the plans that exist (local and imported from Release Control), their
baseline-versus-candidate verdict, and open one or create a new one.

### 2. Layout, top to bottom
| # | Section | Code | Primitives vs page markup |
|---|---|---|---|
| 1 | Console bar actions `import history`, `new plan` (primary) | `PlansPage.tsx:563-` (header registration via `DashboardPageActions`) | `buttonClassName` on raw links |
| 2 | Title `plans` + one-line description | `:752-760` | DS `PageHeader` |
| 3 | Import result callouts (`History import failed`, `History imported with warnings`) | `:772-790` | DS `Callout` |
| 4 | Tabs `My plans` / `Templates` | `:795` (`TabsList` from `@iii-dev/console-ui`) | host tabs |
| 5 | Filter bar: search `Input`, plan-state chips (`all`, `needs action`, `running`, `compared`), count | `:853-880` | DS `Input`, `FilterChip`, `FilterChipGroup` |
| 6 | Plans table: label/purpose, baseline cell, comparison summary with `DeltaValue`, status badges, `open` | `PlanRow` `:466-561`, `PlanBaselineCell` `:326`, `PlanComparisonSummary` `:379`, `MetricDelta` `:294` | DS `DataTable`/`DataTableRow`, `StatusBadge`, `DeltaValue`; cell bodies are bespoke Tailwind |
| 7 | Release Control plans block (imported, `remote`) | `ReleaseControlPlans` `:79-285` | own `EmptyState`s (`Release Control history is unavailable`, `No Release Control history found`) |
| 8 | `How plans work` explainer (three steps: pick scope, capture baseline, run candidates) | `HowPlansWork` `:447-465` | bespoke markup, rendered inside the empty state |
| 9 | Import history `Dialog` | `:1060-1110` | DS `Dialog`, `Callout tone="warning"` `Import unavailable`, primary button |

On screen today (`plans-light-1440.png`): title, tabs, filter bar with four
chips at 0, `0 plans`, and the empty state `no local plans yet` with the
three-step explainer and a primary `new plan` button; nothing else, because
the worker restart dropped the old-format plans.

### 3. Actions
- Primary: `new plan` (console bar `:563-`, and inside the empty state `:943`).
- `import history` opens the dialog; the dialog's primary `import` (`:1081`).
- Per row: `open` (link to detail), whole-row navigation.
- Filters: search, four state chips, `clear filters` in the filtered empty
  state (`:951-960`); `try again` on the load error (`:903-912`).

### 4. States
- loading: `aria-busy` on the list (`:1` occurrence) with text placeholder.
- error: `EmptyState` `Plans could not be loaded` + `try again` (`:903`);
  `Comparison metrics unavailable` callout (`:892`) when metrics fail but plans
  load.
- empty: `No local plans yet` with explainer (`:930-949`); filtered:
  `No plans match these filters` (`:951`); templates tab: `No templates
  available` (`:835`).
- running: rows show a `running` badge; the word appears 30 times in the file,
  driven by `plan.state`.
- remote: Release Control block has its own loading/error/empty triad
  (`:92-120`).

### 5. Data
- `bridge.planControl(...)` only (`PlansPage.tsx`, via
  `lib/dashboard-data-source.ts`) → worker `e2e::dashboard::plan-control`
  (`src/dashboard/bus.rs:36`, registered `:437`: "Configure, export and execute
  saved plans, and inspect or cancel their composed executions").
- Comparison numbers: `lib/plan-comparison.ts` (score, tokens, duration, cost,
  coverage metrics and formatters).

### 6. Size and debt
- 1112 lines; `legacy.css` class used: `page-shell` only.
- `ReleaseControlPlans` (200 lines) is a second list with its own states inside
  the same page.
- `HowPlansWork` is onboarding copy living in a data page.
- `MetricDelta`/`PlanComparisonSummary` restate what `PlanDetailPage` renders
  in `PlanMetricsTable` and `PlanComparisonLayers`.

---

## Route D — Plan create · `#/ext/harness-e2e/plans/new` (also `.../new/profile/<id>`, `.../new/duplicate/<id>`, `.../new/edit/<id>`)

Route `page: 'plan-create'` (`use-hash-route.ts:19`, `:124-131`). Files:
`pages/LocalPlanPage.tsx` (580 lines; `LocalPlanCreatePage` `:121-540`),
`components/ExecutionSetup.tsx` (862 lines), `components/ProviderModelDropdown.tsx`
(411 lines), `components/LocalRunnerDialog.tsx` (490 lines).

### 1. Job
Name a plan, pick the model and the tests, and save it or save-and-run it.

### 2. Layout, top to bottom
| # | Section | Code | Primitives vs page markup |
|---|---|---|---|
| 1 | Console bar action `quick execution instead` | `LocalPlanPage.tsx` (label at `:` "quick execution instead") | `buttonClassName` |
| 2 | Breadcrumb `plans / new plan`, title, description | `:395-` | DS `PageHeader` |
| 3 | `Start from a template` `Select` + catalog readiness line (`catalog ready · 43 models · 68 tests` + refresh) | `:395-455`; `ExecutionSetup.tsx:324` (`Refresh catalog`) | DS `Select`; readiness line bespoke |
| 4 | `Name the plan`: label `Input` (required), purpose `Textarea` (optional) | `ExecutionSetup.tsx:336-387` (`SetupSection`) | DS `Field`, `Input`, `Textarea` |
| 5 | `Choose the model`: `ProviderModelDropdown` + `Advanced · sampling, retries and seed` disclosure | `:389-519`; `ProviderModelDropdown.tsx:68-411` | dropdown is a 411-line page-level widget, not a primitive |
| 6 | `Pick the tests`: search, `select visible (68)` / `clear`, chips `all 68` / `selected 0`, grouped checkbox list (chess, cross, engineering, kanban, registry, subagent, swe, todo, validation, other tests · 33) with `select group` per group | `:521-704`; `CatalogEmptyState` `:714` | DS `FilterChip`; checkbox groups are bespoke |
| 7 | Sticky footer: sentence (`0 tests · 0 runs · no model`, `1 run per test · 0 retries · canonical seed · ws://…`), `cancel`, `save plan`, `save and run` (primary) | `ExecutionSetupFooter` `:814-862`; `LocalPlanPage.tsx:498-533` | `buttonClassName`; footer bespoke |

On screen today (`plan-new-light-1440.png`): a single long column, 4213px
tall at 1440 wide, every test group expanded by default; the footer sits at
the very bottom, so the primary action is off-screen until the list is
scrolled through.

### 3. Actions
- Primary: `save and run` (`LocalPlanPage.tsx:526`); `save plan` (`:518`) is
  a second primary-styled button next to it.
- `quick execution instead` (switches to `LocalRunnerDialog`, whose own
  actions are `create a reusable plan instead`, `cancel execution`, `cancel`).
- `refresh catalog`, template select, `select visible`, `clear`, `select
  group` × 10, `show all tests` (`ExecutionSetup.tsx` label), advanced
  disclosure, `cancel`.

### 4. States
- Validation on submit: `validateExecutionSetup` (`ExecutionSetup.tsx`,
  called from `LocalPlanPage.tsx:277`, `:308`) sets per-field errors (the word
  `error` appears 30 times in `ExecutionSetup.tsx`); a `Callout` summarises
  (`LocalPlanPage.tsx`).
- catalog loading / unavailable: `CatalogEmptyState` (`ExecutionSetup.tsx:714`)
  with `refresh catalog`.
- saving: `aria-busy` on the form; `cancel` returns to the list.
- `LocalRunnerDialog`: loading, error, running with `live runner output` and
  `last output from the job`, cancel (`LocalRunnerDialog.tsx:121-490`).

### 5. Data
- `bridge.createPlan`, `getPlan`, `updatePlan`, `startPlan`
  (`LocalPlanPage.tsx`) → `e2e::dashboard::plan-create`, `plan-get`,
  `plan-update`, `plan-run-start` (`bus.rs:38-42`).
- `LocalRunnerDialog`: `bridge.getCatalog`, `startRun`, `getRunSnapshot`,
  `cancelRun` → `catalog-get`, `run-start`, `run-status`, `run-cancel`
  (`bus.rs:35`, `:43-45`).
- Templates and models come from the catalog payload (`catalog-get`).

### 6. Size and debt
- 580 + 862 + 411 + 490 = 2343 lines for one form and one dialog.
- `ExecutionSetup` serves two modes (`QUICK`, `PLAN`, `ExecutionSetup.tsx:17-18`)
  with branches throughout; `LocalRunnerDialog` re-implements the run flow the
  plan detail page also has.
- `ProviderModelDropdown` is the only custom dropdown in the app; every other
  choice uses the native `Select`.
- No `legacy.css` class used.

---

## Route E — Plan detail · `#/ext/harness-e2e/plans/<id>`

Route `page: 'plan-detail'` (`use-hash-route.ts:24`, `:133`). Files:
`pages/PlanDetailPage.tsx` (2942 lines, `LocalPlanDetailPage` `:2101-2942`),
`pages/LocalPlanPage.tsx:541-580` (thin wrapper), `pages/ImportedPlanDetailPage.tsx`
(866 lines, imported Release Control plans), `components/PlanCharts.tsx` (471 lines).

### 1. Job
Follow one plan: run the baseline, run candidates, compare them, and read the
verdict.

### 2. Layout, top to bottom (`PlanDetailPage.tsx`)
| # | Section | Code | Primitives vs page markup |
|---|---|---|---|
| 1 | Console bar actions `back to plans`, `new plan`, `rename` | `:2101-` | `buttonClassName` |
| 2 | Header: breadcrumb, label, purpose, status badge | `:2101-` | DS `PageHeader`, `StatusBadge` |
| 3 | `PlanLifecycle`: baseline → candidates stepper with `run another candidate`, `open active execution`, `cancel` | `:253-449` | DS `Callout` (×7 across the page), `buttonClassName`; stepper bespoke |
| 4 | `PlanScope`: tests, model, policy of the frozen scope | `:450-645` | DS `Panel`, bespoke lists |
| 5 | `ExecutionNameControl`: rename inline | `:646-773` | DS `Input`, `buttonClassName` |
| 6 | `PlanRunHistory` / `PlanExecutionHistory`: table of baseline and candidate executions, `open`, `report`, `view baseline execution`, `open the baseline report` | `:814-988`, `:1367-1588` | DS `DataTable`, `StatusBadge` |
| 7 | `PlanMetricsTable`: `exact values` toggle, metric rows with `DeltaValue` | `:1589-1712` | DS `DataTable`, `DeltaValue`, `Select` |
| 8 | `PlanComparisonLayers`: `reference` / `candidates` layers with `PlanCharts` (`Sparkline`, `DivergingBars`, `Dumbbell`) | `:1713-1813`; `PlanCharts.tsx:33`, `:165`, `:305` | hand-written SVG charts, own color constants (`PlanCharts.tsx:22-25`) |
| 9 | `PlanProvenance`: runner, revision, contract digests | `:1814-1844` | bespoke definition list |
| 10 | `PlanScenarioComparisonTable`: per-test baseline vs candidate rows | `:1845-2100` | DS `DataTable`, `StatusBadge` |
| 11 | Delete `Dialog` | `:2101-` | DS `Dialog` |

`ImportedPlanDetailPage.tsx` repeats 2, 3 (as `run reference locally` /
`local run in progress` / `open local execution`), 6 and 10 for a Release
Control plan with its own header, `EmptyState` (`:468`) and dialog.

No screenshot exists: no plan is stored locally right now.

### 3. Actions
- Primary: `run another candidate` / run baseline (`PlanLifecycle`), `save`
  in rename; imported: `run reference locally` (`ImportedPlanDetailPage.tsx:494`,
  `:800`).
- `cancel` (running execution), `open active execution`, `open`, `report`,
  `view baseline execution`, `open the baseline report`, `rename`, `delete`
  (dialog), `back to plans`, `new plan`, `exact values`, `reference` /
  `candidates` layer toggles.

### 4. States
- loading plan (`aria-busy`), plan not found / load error (`Callout`,
  `EmptyState` `:1`), no runs yet, baseline running, candidate running
  (`running` appears 54 times, `cancel` 20), cancelled, compared.
- Live: the active execution is watched (`bridge.getExecution` polling through
  `lib/watch-execution.ts`).

### 5. Data
- `bridge.getPlan`, `updatePlan`, `deletePlan`, `startPlan`, `getExecution`
  → `plan-get`, `plan-update`, `plan-delete`, `plan-run-start`,
  `execution-get`; imported plans add `planControl`, `getCatalog`.
- `lib/plan-comparison.ts` for every metric and delta; `lib/primary-metrics.ts`
  for the headline numbers.

### 6. Size and debt
- 2942 lines in one file: ten internal components, none exported, none tested
  in isolation; `ImportedPlanDetailPage` (866) duplicates four of them for the
  remote case.
- `PlanCharts.tsx` carries its own palette (`SURFACE`, `HAIRLINE`, `MUTED`,
  `ACCENT` at `:22-25`) instead of tokens.
- Duplicated widgets: run history table (also in `PlansPage` summary and
  `TestHistoryPage`), metrics table with `DeltaValue` (also
  `ExecutionComparisonPanel`), provenance list (also execution detail).
- `legacy.css` class used: `page-shell` only.

---

---

## Route F — Executions list · `#/ext/harness-e2e/executions` (default route)

Route `page: 'workspace', view: 'executions'` (`hooks/use-hash-route.ts:7`,
default `:27`). File: `pages/ExecutionsPage.tsx` (840 lines).

### 1. Job
Find a recent execution (running or retained) and open it.

### 2. Layout, top to bottom
| # | Section | Code | Primitives vs page markup |
|---|---|---|---|
| 1 | Console bar actions `new plan`, `run tests` (primary) | `ExecutionsPage.tsx:478-` | `buttonClassName` |
| 2 | Kicker `RECENT ACTIVITY AND RETAINED EVIDENCE`, title `executions`, count line (`1 executions · 1 loaded · 1 running`) | `:643-660` | DS `PageHeader` |
| 3 | Filters: search, trigger `Select`, result chips (`all`, `running`, `passed`…), sort `Select` (`newest first`, `oldest first`, `result`, `longest runtime`, `most tokens`), `showing N of M loaded` | `:674-735`, `SORTS :78`, `RESULT :161` | DS `Input`, `Select`, `FilterChip`, `FilterChipGroup` |
| 4 | Ledger: one table, day groups (`today · …`, `yesterday · …`) as separator rows, plan groups nested (`data-ledger-plan`), columns execution (label, trigger · lane, date), result (badge + `no report retained`), subject, scope, pass rate, runtime, tokens, `open` | `LedgerTable :417-477`, `LedgerRowCells :324-416`, `dayLabel :213` | DS `DataTable`, `DataTableRow`, `StatusBadge`; the day/plan separator rows and the two-line cells are bespoke `<tr>` markup |
| 5 | Cursor pagination (`load more`) | `:774-` | `buttonClassName` |

On screen today (`executions-light-1440.png`): one row (`e2e::* control-plane
run · my Harness · local`, `running`, dashes in every metric column), the
`RUNNING · 1` group label, `all 1` / `running 1` chips.

### 3. Actions
- Primary: `run tests` (console bar). `new plan` secondary.
- Row: whole row and `open` go to the execution; the plan separator links to
  the plan.
- Filters: search + clear, trigger select, result chips, sort select, `load
  more`, `try again` on error.

### 4. States
- loading: `aria-busy` (×2) with skeleton rows; `error`: `EmptyState`
  `Executions could not be loaded` (`:654`); empty / filtered empty:
  `EmptyState` (`:774`).
- running rows: badge `running`, metrics as `—`; the list refreshes on the
  `e2e::dashboard::changed` trigger (`lib/dashboard-data-source.ts`), no
  per-row polling.

### 5. Data
- `bridge.listExecutions({cursor, limit})` → `e2e::dashboard::executions-list`
  (`src/dashboard/bus.rs:26`, handler `:288`); presentation through
  `lib/execution-view.ts` (`presentation.completedAt`, verdict labels).

### 6. Size and debt
- 840 lines; `legacy.css`: `page-shell` only.
- The ledger's grouping rows break the `DataTable` abstraction (raw `<tr
  data-ledger-day>` `:457`).
- The result vocabulary (`passed`, `running`, `cancelling`, `cancelled`,
  `incomplete`, `unavailable`, `inconclusive`, `failed`) is restated in
  `ExecutionPage.tsx:121-137` and in `lib/execution-verdict.ts`.

---

## Route G — Execution detail · `#/ext/harness-e2e/execution/<id>` (+ `/run/<runId>` evidence record)

Route `page: 'execution'` (`use-hash-route.ts:9-14`, `:97-113`). Files:
`pages/ExecutionPage.tsx` (1300 lines) and the components it composes:
`AssessmentWorkspace.tsx` (956), `ScenarioMatrix.tsx` (970),
`PrimaryMetricsView.tsx` (492) + `PrimaryMetricsView.css` (431),
`ExecutionMetricsPanel.tsx` (224), `SemanticTestFlow.tsx` (711),
`TranscriptDialog.tsx` (426), `ScenarioChatAction.tsx` (296),
`ExecutionComparisonPanel.tsx` (222), `DisclosureLayer.tsx` (62).

### 1. Job
Read one execution's verdict, drill from the aggregate to each test and each
run's evidence, and act on it (cancel, re-run, delete, open in chat).

### 2. Layout, top to bottom
| # | Section | Code | Primitives vs page markup |
|---|---|---|---|
| 1 | Console bar: `copy link`, `re-run same scope` (primary while retained), `cancel execution` (while running) | `ExecutionPage.tsx:503-570` (`LiveState`), `:556` | `buttonClassName` |
| 2 | Breadcrumb (`executions /` or `plans / Plan /`), title, verdict badge, subtitle (`Execution in progress · results are provisional`) | `:628-` , `:816-820` | DS `PageHeader`, `StatusBadge` |
| 3 | Identity band: `SUBJECT`, `STARTED`, `TRIGGER`, `ID` | `:628-` | bespoke definition list (`ds-label`) |
| 4 | Live state box: `running · 1m 25s elapsed`, `cancel execution`, explanation | `LiveState :503-570` | bespoke `Panel`-like box |
| 5 | Section bar `next step` / `results` / `counts` / `provenance` | `SECTIONS :460`, `SectionBar :467-502` | bespoke anchor bar |
| 6 | `next step`: `NarrativeSection` (verdict sentence, what to do) | `:245-259` | bespoke |
| 7 | `results`: `Execution summary · N tests`, tabs `Grouped` / `By test`, then `AssessmentWorkspace` (`PrimaryMetricBoard`, `AssessmentMatrix`, per-run `AssessmentCard`/`AssessmentPanel`, `RunAssessment`, `AssessmentDetailDialog`) and `ScenarioMatrix` (`ResultContractStrip`, `ScenarioRow`, `MatrixCell`, `ScenarioExpansion`, `ScenarioReliabilityBand`, `RunOutcomeLedger`, `WorkflowDurationProfile`) | `AssessmentWorkspace.tsx:934`, `:234`, `:334`, `:817`, `:722`, `:641`; `ScenarioMatrix.tsx:33`, `:135`, `:275`, `:373`, `:390`, `:491`, `:628`, `:791` | DS `Callout`, `Dialog`, `StatusBadge`, `Panel`; the matrix is `legacy.css` `.scenario-*` (34 selectors, its only remaining users) |
| 8 | `counts`: `CountsSection` → `PrimaryMetricsView` (`MetricNumber`, `Readout`, `Tokens` / `Activity` bands, `Test results` with metric-group tabs) and `ExecutionMetricsPanel` (`MetricCard` ×4, table) | `:260-410`; `PrimaryMetricsView.tsx:125`, `:237`, `:308`, `:336`, `:363`; `ExecutionMetricsPanel.tsx:47` | `PrimaryMetricsView.css` (66 `.primary-*` / `pm-*` rules, 431 lines); DS `MetricCard`, `DataTable`, `Button`, `EmptyState` |
| 9 | `provenance`: `ProvenanceSection` (runner, revision, contracts, digests), `Retained runs` `Panel` (`DataTable`), `Retained artifacts` `Panel`, `EvidenceBundleUnavailable` callout | `:411-459`, `:829`, `:855`, `:571-627` | DS `Panel`, `DataTable`, `Callout`, `MetricCard` ×7 across the page |
| 10 | Workflow executions: `SemanticTestFlow` (`WorkflowMetricsOverview`, `SemanticTestCard`, evidence groups `Evaluations` / `Failures` / `Assets`) | `SemanticTestFlow.tsx:25`, `:115`, `:265`, `:381-427` | DS `StatusBadge`; cards bespoke |
| 11 | Overlays: `TranscriptDialog` (search, role chips, event cards, copy), `AssessmentDetailDialog`, delete `Dialog` (`Delete execution?`) | `TranscriptDialog.tsx:112`; `AssessmentWorkspace.tsx:641`; `ExecutionPage.tsx:1255` | DS `Dialog`, `FilterChip`, `Input` |
| 12 | `ScenarioChatAction` menu (open the run in the Console chat) | `ScenarioChatAction.tsx:44` | `buttonClassName`; appears in 7 files |

On screen today (`execution-light-1440.png`, the running execution): items
1–5 and an empty results area (`no test results yet · Metrics will appear as
test results become available`); the identity band shows `SUBJECT not
reported`.

### 3. Actions
- Primary: `re-run same scope` (retained) or `cancel execution` (running).
- `copy link`, `delete` (dialog `:1255`, `bridge.deleteExecution` `:993`),
  `open evidence` (`bridge.openEvidence`), section anchors, `Grouped` /
  `By test`, per-run `transcript`, `open` evidence record
  (`/run/<runId>`), `ScenarioChatAction`, transcript search/filters/copy,
  metric-group tabs, `exact values` style toggles.

### 4. States
- Verdict vocabulary `ExecutionPage.tsx:121-137`: passed, running,
  cancelling, cancelled, incomplete, unavailable, inconclusive, failed.
- Live: `watchExecution` (`lib/watch-execution.ts`, trigger + 5 s poll)
  while not terminal; `cancelling…` label (`:556`).
- `EmptyState` ×2 (no results yet `:746`; evidence bundle unavailable
  `:571-627` as `Callout`); `error` ×15 branches across load, cancel, delete
  and evidence open.

### 5. Data
- `bridge.cancelRun`, `deleteExecution`, `openEvidence` → `run-cancel`,
  `execution-delete`, `evidence-open` (`bus.rs:45`, `:29`, `:28`); the detail
  itself through `execution-get` / `attempt-get` (`bus.rs:27`, `:30`) via
  `lib/dashboard-data-source.ts`.
- Eleven lib modules: `assessment-view`, `execution-metrics`,
  `execution-verdict`, `execution-view`, `plan-execution`, `primary-metrics`,
  `release-control-reference`, `scenario-matrix`, `watch-execution`,
  `definition-digest`, `dashboard-data-source`.

### 6. Size and debt
- 1300 + 956 + 970 + 492 + 431 (CSS) + 224 + 711 + 426 + 296 + 222 = 6028
  lines for one screen and its overlays; the largest surface in the app.
- Two parallel result views of the same report: `AssessmentWorkspace`
  (grouped) and `ScenarioMatrix` (by test), each with its own row, cell,
  badge and dialog components; `PrimaryMetricsView` and
  `ExecutionMetricsPanel` both render token/activity counts.
- `PrimaryMetricsView.css` is the only remaining component stylesheet;
  `legacy.css` survives almost only for `.scenario-*`.
- `AssessmentWorkspace` is also embedded in the test-history dialog
  (`TestHistoryPage.tsx:683-860`); `ExecutionComparisonPanel` is shared with
  test history and compare.
- `ScenarioChatAction` (296 lines) is used from 7 files.

---

## Route H — Compare · `#/ext/harness-e2e/compare[?left&right]`

Route `page: 'compare'` (`use-hash-route.ts:15`, `:115-121`). File:
`pages/TestsPage.tsx` (1916 lines), despite its name.

### 1. Job
Put two system versions of the same cohort (model × lane) side by side and see
which tests regressed or improved.

### 2. Layout, top to bottom
| # | Section | Code | Primitives vs page markup |
|---|---|---|---|
| 1 | Console bar: `share link`, `new run on b` (primary) | `TestsPage.tsx:805-` | `buttonClassName` |
| 2 | Breadcrumb `tests / compare`, kicker `RETAINED COHORTS · PER-TEST MEAN SCORE AND RUN MEDIANS`, title `compare system versions`, `catalog <sha>` | `:1360-1382` | DS `PageHeader` |
| 3 | Builder panel: cohort `Select` (`no evaluated cohort yet`), explanation line, `A · BASELINE` `Select`, swap button, `B · CANDIDATE` `Select`, `Deltas only between runs of the same model and test contract.` | `:1383-1499`, `TestDefinitionSelect :558` | DS `Panel`, `Select`; swap is a raw button (`:1450`) |
| 4 | Filter chips `with evidence`, `one side`, `all 68` (+ `comparable`, `regressed`, `improved` when data exists) and search; count sentence (`0 of 68 tests · 0 comparable · 0 regressed · 0 improved · 0 on one side · 68 without evidence`) | `:1640-1730` | DS `FilterChip` ×6, `Input` |
| 5 | Verdict panel and per-test rows: `CompareRow` with `SideResult` a/b, `b − a` deltas (`DeltaValue`), `EvidenceRow` / `RowDetails` expansion | `:599`, `:152`, `:341`, `:407`, `:1601` | DS `DataTable`, `DeltaValue`, `StatusBadge`, `Callout` ×5 |
| 6 | Empty state `no evidence in this cohort for these two versions` + `run tests` | `:1544` | DS `EmptyState` |

On screen today (`compare-light-1440.png`): builder with no cohort, three
chips, the count sentence with zeros, and the empty state.

### 3. Actions
- Primary: `new run on b` (bar) and `run tests` (empty state).
- `share link`, cohort/version selects, swap a↔b, chips, search, row
  expansion, `ScenarioChatAction` per row.

### 4. States
- no cohort / waiting for history / no retained version (`:1517`
  `Versioned evidence could not be loaded`, `:1544` no common test), loading
  (`aria-busy`), `error` ×13, per-row `not comparable · values shown, deltas
  not interpreted` (`:484`).

### 5. Data
- `bridge.listTests`, `listEvaluatedVersions`, `getTestVersion` →
  `tests-list`, `evaluated-versions-list`, `test-version-get` (`bus.rs:32`,
  `:31`, `:33`); `lib/test-catalog-view.ts`, `lib/definition-digest.ts`.

### 6. Size and debt
- 1916 lines; the file name (`TestsPage`) does not match the route; its
  breadcrumb claims the `tests` section.
- Restates the history table (`EvidenceRow`) and the comparison panel of
  `TestHistoryPage`; three comparison surfaces exist across tests, history and
  compare.
- `legacy.css`: `page-shell` only.

---

## Shell and navigation

- Mount: `console-entry.tsx:55-62` registers one page with the host
  (`host.pages.register({ id: 'harness-e2e', render })`); the host renders it
  inside `[data-iii-ui="harness-e2e"]`, hands `tabId`, `panelSide`, its theme
  (`host.useTheme()` `:41`) and the `iii` client
  (`installDashboardIiiClient(host.iii)` `:53`); the changed trigger
  `e2e::dashboard::changed` (`:31`) drives live refresh.
- `components/DashboardShell.tsx:139-267`: `PageShell` / `PageHeader` /
  `PageBody` / `PageMain` from `@iii-dev/console-ui` (the host's own chrome:
  icon, title `harness e2e`, context description, close), then a sticky
  `<nav>` (`:215-259`) with the three section links (`Tests`, `Executions`,
  `Plans`, `:84-88`) as `<a aria-current>` (`:223-233`), a narrow `<select>`
  fallback (`:239-254`) toggled by `data-narrow` from `useContainerNarrow(720)`
  (`:147`), and the `PageActionsBar` (`:118-128`) where pages push their
  header buttons through `DashboardChromeContext.setHeader`
  (`:33-39`, `components/DashboardPageActions.tsx:34-67`).
- Skip link (`:191-200`) moves focus to `#harness-e2e-main` without changing
  the hash, because the host owns the router.
- Routes (`hooks/use-hash-route.ts:7-24`): `workspace` (`tests` |
  `executions`, default `executions` `:27`), `execution/<id>[/run/<runId>]`,
  `compare[?left&right]`, `tests/<testId>`, `plans`, `plans/new[...]`,
  `plans/<planId>`. `App.tsx:11-43` dispatches them; `lib/dashboard-runtime.ts`
  holds the hash prefix.
- Container queries: `.harness-e2e-dashboard` is `container-type: inline-size;
  container-name: harness` (`dashboard-shell.css:61-62`); narrow ≤ 720px,
  compact ≤ 480px.
- `dashboard-shell.css` (186 lines, layer `legacy`): the alias tokens
  (`--surface*`, `--text*`, `--line*`, `--accent`, `--success/--warning/--danger`,
  `--info`, `--glass`, `--backdrop`, `:8-38`), heading face, nav link and
  select styles (`:105-134`), focus ring and field outline (`:136-152`), skip
  link (`:156-177`), reduced motion.
- `index.css` (46 lines): the cascade layers (`theme, base, legacy, ds,
  components, utilities`), Tailwind with `source(none)`, `legacy.css` in layer
  `legacy`, the `@theme inline` aliases that turn the tokens above into
  utilities (`text-ink-muted`, `bg-panel-subtle`, `text-warning`…, `:13-37`),
  and `@theme reference` for the host's own variables (`:40-46`).
- `legacy.css` (533 lines) is now almost entirely `.scenario-*` (34 selectors,
  the scenario matrix on the execution page), plus `pagination`, `page-shell`,
  `history-table`, `test-catalog-panel`, `empty-state`, `badge`, `mono`.

---

## Cross-cutting

### Widgets built more than once
| Widget | Where |
|---|---|
| Result / lifecycle vocabulary and badge | `ExecutionPage.tsx:121-137`, `lib/execution-verdict.ts`, `TestsCatalogPage.tsx:128-148` (dot), `TestHistoryPage.tsx:1289-1300` (badge), `TestHistoryPage.tsx:182-197` |
| Execution/run history table | `TestHistoryPage.tsx:1742-1892`, `PlanDetailPage.tsx:814-988` and `:1367-1588`, `ExecutionPage.tsx:829` (retained runs), `TestsPage.tsx:341` (evidence rows), `ExecutionsPage.tsx:417` (ledger) |
| a/b comparison surface | `TestHistoryPage.tsx:1368-1485` (RC reference), `:1894-1906` (local a/b), `TestsPage.tsx:1383-1499` (builder), `PlanDetailPage.tsx:1713` (layers), all over `ExecutionComparisonPanel` |
| Metric tiles and delta tables | `TestHistoryPage.tsx:1531-1589`, `PlanDetailPage.tsx:1589`, `PlansPage.tsx:294-445`, `PrimaryMetricsView`, `ExecutionMetricsPanel` |
| Hand-written SVG charts | `TestHistoryPage.tsx:266-464` (score trend), `components/PlanCharts.tsx` (sparkline, diverging bars, dumbbell) |
| Provenance / identity lists | `ExecutionPage.tsx:411-459`, `PlanDetailPage.tsx:1814`, `TestHistoryPage.tsx:1272-1366` |
| Header action clusters | every page pushes raw `<a>`/`<button>` with `buttonClassName` into `PageActionsBar` (`components/DashboardPageActions.tsx`) |
| `ScenarioChatAction` | 7 files |
| Empty-state onboarding copy | `PlansPage.tsx:447` (`HowPlansWork`), `TestHistoryPage.tsx:1496-1527`, `TestsPage.tsx:1544` |

### Primary action placement
The primary action lives in the Console bar on every page (`new plan`, `run
tests`, `run this test`, `re-run same scope`, `new run on b`, `save and
run`), and is repeated inside empty states on B, C, H and inside the sticky
footer on D; on D it is off-screen until the whole test list is scrolled.

### Data path
All pages read through one bridge (`lib/dashboard-data-source.ts`) onto the
`e2e::dashboard::*` functions in `src/dashboard/bus.rs:26-46`, memoised per
payload, refreshed by the `e2e::dashboard::changed` trigger plus 5 s polling
for a watched execution (`lib/watch-execution.ts`). Lists page by cursor at
50 (catalog), 100 (history, hard cap) and 200 (executions for the RC
reference).
