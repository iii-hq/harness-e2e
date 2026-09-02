const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')

const repositoryRoot = path.join(__dirname, '..', '..')
const dashboardRoot = path.join(repositoryRoot, 'dashboard')
const read = (...segments) =>
  fs.readFileSync(path.join(dashboardRoot, ...segments), 'utf8')
const overviewPage = read('src', 'pages', 'OverviewPage.tsx')
const executionPage = read('src', 'pages', 'ExecutionPage.tsx')
const coveragePage = read('src', 'pages', 'CoveragePage.tsx')
const appHeader = read('src', 'components', 'AppHeader.tsx')
const dashboardShell = read('src', 'components', 'DashboardShell.tsx')
const runner = read('src', 'components', 'LocalRunnerDialog.tsx')
const executionSetup = read('src', 'components', 'ExecutionSetup.tsx')
const scenarioMatrix = read('src', 'components', 'ScenarioMatrix.tsx')
const modelDropdown = read('src', 'components', 'ProviderModelDropdown.tsx')
const planPage = read('src', 'pages', 'LocalPlanPage.tsx')
const planDetailPage = read('src', 'pages', 'PlanDetailPage.tsx')
const plansPage = read('src', 'pages', 'PlansPage.tsx')
const catalogPage = read('src', 'pages', 'TestsCatalogPage.tsx')
const historyPage = read('src', 'pages', 'TestHistoryPage.tsx')
const transcript = read('src', 'components', 'TranscriptDialog.tsx')
const dataSource = read('src', 'lib', 'dashboard-data-source.ts')
const viewModel = read('src', 'lib', 'execution-view.ts')
const testsPage = read('src', 'pages', 'TestsPage.tsx')
const publisher = fs.readFileSync(
  path.join(repositoryRoot, 'scripts', 'publish_harness_e2e_dashboard.py'),
  'utf8',
)

test('prioritizes the first-read execution signal', () => {
  assert.match(overviewPage, /Latest execution/)
  assert.match(overviewPage, /Scenario pass rate/)
  assert.match(overviewPage, /Recent executions/)
  assert.match(overviewPage, /Investigate execution/)
  assert.match(overviewPage, /Subject/)
  assert.match(overviewPage, /modelNames\(presentation\.judges\)/)
  assert.match(dashboardShell, /value: 'tests'/)
  assert.match(overviewPage, /hashForNewPlan\(\)/)
  assert.match(overviewPage, /New plan/)
  assert.match(overviewPage, /lg:grid-cols-\[minmax\(0,1fr\)_300px\]/)
  assert.match(overviewPage, /buttonClassName\(\{ variant: 'secondary' \}\)/)
  assert.doesNotMatch(overviewPage, /hashForPlans/)
  assert.doesNotMatch(overviewPage, /View Actions/)
  assert.doesNotMatch(
    overviewPage,
    /CurrentWork|current-work-panel|No local plan yet/,
  )
  assert.match(viewModel, /assessment_summary/)
  assert.match(viewModel, /infrastructure_error/)
  assert.match(viewModel, /resource_limit/)
  assert.doesNotMatch(
    overviewPage,
    /HARNESS_EXECUTIONS|Technical Failed|window\.Harness/,
  )
  assert.doesNotMatch(overviewPage, /Confidence grows in the background/)
  assert.doesNotMatch(overviewPage, /01 \/ Current signal/)
})

test('keeps the overview operational, dense, and Tailwind-based', () => {
  const styles = read('src', 'legacy.css')

  for (const primitive of [
    'Button',
    'MetricCard',
    'Panel',
    'StatusBadge',
  ]) {
    assert.match(overviewPage, new RegExp(`\\b${primitive}\\b`))
  }
  assert.match(overviewPage, /DashboardPageActions/)
  // Audit DS-12: utilities win by cascade layer, not by `important`.
  const entry = read('src', 'index.css')
  assert.match(entry, /@import "tailwindcss";/)
  assert.doesNotMatch(entry, /@import "tailwindcss" important/)
  assert.match(entry, /@layer theme, base, legacy, ds, components, utilities;/)
  assert.match(entry, /@import "\.\/legacy\.css" layer\(legacy\);/)
  assert.match(overviewPage, /lg:grid-cols-\[minmax\(0,1fr\)_300px\]/)
  assert.match(overviewPage, /xl:grid-cols-4/)
  assert.match(overviewPage, /Recent executions/)
  assert.match(overviewPage, /Total tokens/)
  assert.match(overviewPage, /Run the full suite or a subset/)
  assert.match(overviewPage, /@\/design-system\/styles\.css/)
  assert.doesNotMatch(overviewPage, /\.\/overview\.css/)
  assert.doesNotMatch(overviewPage, /Evidence that|earns trust/)
  assert.doesNotMatch(overviewPage, /overview-v2-hero|data-overview-visual/)
  assert.doesNotMatch(
    overviewPage,
    /useGSAP|ScrollTrigger|gsap\.registerPlugin/,
  )
  assert.doesNotMatch(overviewPage, /min-h-\[72rem\]/)
})

test('adapts overview metrics when persisted workflow evidence is available', () => {
  assert.match(overviewPage, /execution\.workflow_metrics/)
  assert.match(overviewPage, /Semantic steps/)
  assert.match(overviewPage, /Workflow runtime/)
  assert.match(overviewPage, /hard gates passed/)
  assert.match(overviewPage, /attentionWorkflowSteps/)
  assert.match(overviewPage, /activeWorkflowSteps/)
})

test('uses one low-emphasis action treatment in overview card footers', () => {
  const styles = read('src', 'legacy.css')
  assert.match(styles, /\.overview-card-action\s*\{[\s\S]*display: inline-flex/)
  assert.match(styles, /\.overview-card-action:hover[\s\S]*border-bottom-color/)
  assert.match(
    styles,
    /\.overview-card-action:focus-visible[\s\S]*outline-offset/,
  )
})

test('keeps the workspace navigation and versioned test flow', () => {
  for (const view of ['overview', 'tests', 'executions', 'plans']) {
    assert.match(dashboardShell, new RegExp(`value: '${view}'`))
  }
  assert.match(testsPage, /Tests across system versions/)
  assert.match(testsPage, /loadVersionResult\(row\.test_id/)
  assert.match(testsPage, /prefetchedCatalog/)
  assert.match(catalogPage, /<DashboardPageActions[\s\S]*active="tests"/)
  // Audit T-03 / T-07 / T-08 / T-12: DS table with a sticky first column,
  // lifecycle groups with retired collapsed, cursor pagination, filters in
  // the hash, and a text search with its own clear control.
  assert.match(catalogPage, /aria-label="Search tests"/)
  assert.match(catalogPage, /placeholder="Filter by name, id or title…"/)
  assert.doesNotMatch(catalogPage, /type="search"|min-w-\[82rem\]|Local dashboard only/)
  assert.match(catalogPage, /<DataTable\s+caption=\{caption\}\s+collapse\s+collapseInline\s+minWidth="64rem"\s+sticky\s*>/)
  assert.match(catalogPage, /ds-table-sticky-col/)
  assert.match(catalogPage, /data-catalog-group=\{group\.lifecycle\}/)
  assert.match(catalogPage, /retired test/)
  assert.match(catalogPage, /cursor: data\.next_cursor/)
  assert.match(catalogPage, /replaceRouteParams\(params\)/)
  assert.match(catalogPage, /catalogFiltersFromParams\(routeParams\(window\.location\.hash\)\)/)
  for (const page of [
    overviewPage,
    executionPage,
    coveragePage,
    catalogPage,
    historyPage,
    testsPage,
    planPage,
    plansPage,
  ]) {
    assert.match(page, /<DashboardPageActions/)
  }
  assert.match(dashboardShell, /<nav[\s\S]*aria-label="Harness E2E sections"/)
  assert.match(dashboardShell, /aria-current=\{\s*item\.value === section \? 'page' : undefined\s*\}/)
  assert.match(dashboardShell, /<select/)
  assert.match(dashboardShell, /<PageActionsBar/)
  assert.match(appHeader, /return null/)
  assert.doesNotMatch(read('src', 'legacy.css'), /\.topbar(?:\s|,|\{)/)
  assert.doesNotMatch(historyPage, /tmh-topbar|tmh-brand|tmh-context/)
  assert.match(plansPage, /title="plans"/)
  assert.match(plansPage, /hashForNewPlan\(\)/)
  assert.match(plansPage, /comparison available/)
})

test('distinguishes active tests from tests with no execution history', () => {
  assert.match(catalogPage, /lifecyclePresentation/)
  assert.match(catalogPage, /dotClassName: 'bg-\[var\(--success\)\]'/)
  assert.match(
    catalogPage,
    /never_run:[\s\S]*dotClassName: 'bg-\[var\(--color-ink-ghost\)\]'/,
  )
  assert.doesNotMatch(catalogPage, /status-catalog-/)
})

test('keeps metric history rows readable and opens details on demand', () => {
  const styles = read('src', 'legacy.css')
  // Audit TH-01/03/05/08/09/10/12/19: DS page, tiles only with data, trend
  // always visible, checkbox a/b with a selection bar, opaque DS dialog, no
  // impact table, state in the hash, no legacy .tmh-* CSS.
  assert.match(historyPage, /median tokens/)
  assert.match(historyPage, /data-history-tiles/)
  assert.match(historyPage, /knownCosts > 0 \?/)
  assert.match(historyPage, /data-score-trend/)
  assert.match(historyPage, /ExecutionDetailsDialog/)
  assert.match(
    historyPage,
    /reports: detail\.reports\.filter\([\s\S]*scenario_id === testId/,
  )
  assert.match(historyPage, /Assessment details for this test/)
  assert.match(historyPage, /data-selection-bar/)
  assert.match(historyPage, /type="checkbox"/)
  assert.match(historyPage, /compareTestObservations/)
  assert.match(historyPage, /getExecution\(observation\.execution_id\)/)
  assert.match(historyPage, /open full execution report/)
  assert.match(historyPage, /systemSummary/)
  assert.match(historyPage, /versionStatement/)
  assert.match(historyPage, /historyStateToParams\(filters, comparisonKeys, openKey\)/)
  assert.match(historyPage, /no retained executions yet/)
  assert.match(historyPage, /requestQuickExecution\(\[testId\]\)/)
  assert.doesNotMatch(historyPage, /Impact by scenario|View score history across|tmh-|Set A|View details/)
  assert.doesNotMatch(historyPage, /systemLabel|See series|Metrics by compatible series/)
  assert.doesNotMatch(styles, /tmh-|test-metrics-history-proposal/)
  assert.match(styles, /overflow-wrap: anywhere/)
  assert.match(
    styles,
    /\.test-catalog-panel > \.panel-heading[\s\S]*padding: 24px 28px/,
  )
})

test('makes local plan scope selection focused and readable', () => {
  // Audit PN-18 / PN-25 / PN-02: DS header with a breadcrumb, one column,
  // and the sticky footer instead of the review aside.
  assert.match(planPage, /title="new plan"/)
  assert.match(planPage, /breadcrumb=\{\[/)
  assert.match(planPage, /ExecutionSetup/)
  assert.match(planPage, /ExecutionSetupFooter/)
  assert.doesNotMatch(planPage, /ExecutionSetupReview|lg:grid-cols-12/)
  assert.match(planPage, /sticky bottom-0/)
  assert.match(planPage, /quick execution instead/)
  assert.match(planPage, /requestQuickExecution/)
  assert.match(planPage, /validateExecutionSetup/)
  assert.match(planPage, /unavailableScenarios/)
  assert.match(executionSetup, /Find a test/)
  assert.match(executionSetup, /Search by name or id/)
  assert.match(executionSetup, /select visible/i)
  assert.match(executionSetup, /select group/)
  assert.match(executionSetup, /Pick the tests/)
  assert.match(executionSetup, /Advanced · sampling, retries and seed/)
  assert.match(executionSetup, /Runs per test/)
  assert.doesNotMatch(executionSetup, /max-h-\[25rem\]|type="search"/)
  assert.doesNotMatch(planPage, /plan-test-option|plan-advanced-control/)
})

test('renders plan pages on the design system with no legacy plan CSS', () => {
  const styles = read('src', 'legacy.css')
  // PR B: every .plan-* block left legacy.css with the pages that used it.
  assert.doesNotMatch(styles, /^ *\.plan-/m)
  assert.doesNotMatch(styles, /--plan-panel-space/)
  assert.doesNotMatch(planPage, /className="(?:panel|page-heading|plan-)/)
  assert.doesNotMatch(planDetailPage, /className="(?:panel|page-heading|plan-)/)
  assert.match(planDetailPage, /<PageHeader[\s\S]*breadcrumb=/)
  assert.match(planDetailPage, /<PlanLifecycle[\s\S]*<PlanScope[\s\S]*<PlanRunHistory[\s\S]*<PlanExecutionHistory/)
})

test('exposes baseline and arbitrary candidate comparison controls', () => {
  assert.match(plansPage, /Latest candidate vs baseline/)
  assert.match(plansPage, /regressed/)
  assert.match(plansPage, /DeltaValue/)
  assert.match(planDetailPage, /baseline and candidates/)
  assert.match(planDetailPage, /Choose a visual baseline and any number of candidates/)
  assert.match(planDetailPage, /PLAN_COMPARISON_TABLE_METRICS/)
  assert.match(planDetailPage, /planMetricWinnerIds/)
  assert.match(planDetailPage, /Best values are highlighted/)
  assert.match(planDetailPage, /Visual baseline/)
  // Audit PD-12: a metric that no column reports is not a row.
  assert.match(planDetailPage, /entries\.some\(\(\{ value \}\) => value !== null\)/)
})

test('uses a typed bridge for local, observed and static execution sources', () => {
  assert.match(dataSource, /getExecution\(executionId/)
  assert.match(dataSource, /mode: 'local' \| 'observed' \| 'published'/)
  assert.match(dataSource, /executions\.json/)
  assert.match(dataSource, /runtime\.functions\.execution_get/)
  assert.match(dataSource, /runtime\.functions\.changed_trigger/)
  assert.doesNotMatch(dataSource, /window\.HARNESS_EXECUTIONS/)
})

test('organizes detail into progressive disclosure sections', () => {
  for (const section of ['summary', 'results', 'technical']) {
    assert.match(executionPage, new RegExp(`id=\\"${section}\\"`))
  }
  assert.match(executionPage, /hashForExecution\(detail\.id, 'results'\)/)
  // One skip-link and one main landmark, both owned by the shell (A11Y-06).
  assert.match(dashboardShell, /className="skip-link"/)
  assert.doesNotMatch(executionPage, /className="skip-link"/)
  assert.doesNotMatch(executionPage, /<main\b/)
  assert.doesNotMatch(executionPage, /detail-index/)
  assert.match(
    executionPage,
    /anchor === 'evidence' \|\| anchor === 'raw-data'\) return 'technical'/,
  )
  assert.match(executionPage, /if \(!anchor \|\| !detail\) return/)
  assert.match(executionPage, /\[anchor, detail\]/)
  assert.doesNotMatch(executionPage, /id="evidence"|Evidence register/)
  assert.match(executionPage, /ScenarioMatrix detail/)
  assert.match(scenarioMatrix, /AssessmentWorkspace/)
  assert.match(scenarioMatrix, /SemanticTestFlow/)
  assert.match(scenarioMatrix, /WorkflowDurationProfile/)
  assert.match(scenarioMatrix, /Inspect scenario evidence/)
  assert.match(executionPage, /onTranscript/)
  assert.doesNotMatch(executionPage, /Open transcript/)
  assert.match(executionPage, /aggregateAssessmentMetrics/)
  assert.match(executionPage, /buildHarnessRecommendation/)
  assert.match(executionPage, /Passed objectively; advisory review found gaps/)
  assert.match(executionPage, /Recommended next step/)
  assert.match(executionPage, /Primary concern/)
  assert.match(executionPage, /Benchmark results/)
  assert.match(executionPage, /Scenario results/)
  assert.match(scenarioMatrix, /Objective result/)
  assert.match(scenarioMatrix, /Advisory/)
  assert.match(scenarioMatrix, /Runtime/)
  assert.match(scenarioMatrix, /Structure/)
  assert.match(executionPage, /buildScenarioMatrix\(detail\)/)
  assert.match(executionPage, /@\/design-system\/styles\.css/)
  assert.doesNotMatch(executionPage, /StatusPill|01 · Summary|02 · Results/)
  assert.match(executionPage, /Preview raw JSON/)
  // The sheet behaviour below 560px lives in the design-system Dialog.
  assert.match(transcript, /<Dialog/)
  assert.match(read('src', 'design-system', 'primitives.css'), /@container harness \(max-width: 560px\) \{\s*\.ds-dialog,/)
})

test('migrates runner and transcript behavior to React components', () => {
  assert.match(runner, /getCatalog/)
  assert.match(runner, /startRun/)
  assert.match(runner, /cancelRun/)
  assert.match(runner, /subscribeRunChanges/)
  assert.match(runner, /aria-live/)
  const primitives = read('src', 'design-system', 'primitives.tsx')
  assert.match(primitives, /export function trapDialogFocus/)
  assert.match(primitives, /getClientRects\(\)\.length > 0/)
  assert.match(runner, /<Dialog/)
  assert.match(transcript, /normalizeTranscript/)
  assert.match(transcript, /session-transcript-dialog/)
  assert.match(transcript, /conversation-shell/)
  assert.match(transcript, /conversation-tool/)
  assert.match(transcript, /formatTranscriptPayload/)
})

test('keeps an empty local execution label serializable and its action visible', () => {
  assert.match(runner, /label: form\.label\.trim\(\),/)
  assert.doesNotMatch(runner, /label: form\.label \|\| null/)
  assert.match(runner, /form="local-runner-form"/)
  assert.match(read('src', 'design-system', 'primitives.css'), /\.ds-dialog\[open\] \{\s*display: grid;/)
  // Audit RS-03 / RS-07 / RS-13: one column, the summary and actions in the
  // dialog footer, the selection handed over to plans/new.
  assert.doesNotMatch(runner, /lg:grid-cols-12|ExecutionSetupReview/)
  assert.match(runner, /footer=\{/)
  assert.match(runner, /ExecutionSetupFooter/)
  assert.match(runner, /requestPlanFromSelection\(form\.scenarios\)/)
  assert.match(runner, /create a reusable plan instead/)
  assert.match(runner, /validateExecutionSetup/)
  assert.match(executionSetup, /mode === 'plan'/)
})

test('groups execution and judge models under their providers', () => {
  for (const view of [runner, planPage]) assert.match(view, /ExecutionSetup/)
  for (const view of [executionSetup, historyPage]) {
    assert.match(view, /ProviderModelDropdown/)
  }
  assert.match(executionSetup, /Execution model/)
  assert.match(executionSetup, /Judge model/)
  assert.match(modelDropdown, /collapsedProviders/)
  assert.match(
    modelDropdown,
    /const collapsed = collapsedProviders\.has\(group\.provider\)/,
  )
  assert.match(modelDropdown, /aria-expanded=\{open\}/)
  assert.match(modelDropdown, /role="option"/)
  assert.doesNotMatch(modelDropdown, /provider-model-group-toggle/)
})

test('publisher writes only the JSON manifest', () => {
  assert.match(publisher, /MANIFEST_FILENAME = "executions\.json"/)
  assert.match(publisher, /write_json_atomic\(manifest_path/)
  assert.match(publisher, /"mode": "published"/)
  assert.match(publisher, /legacy_manifest_path\.unlink/)
  assert.doesNotMatch(publisher, /HARNESS_EXECUTIONS/)
})

test('does not load removed DOM renderers', () => {
  const legacyLoader = read('src', 'hooks', 'useLegacyPage.ts')
  assert.doesNotMatch(
    legacyLoader,
    /overview\.js|execution\.js|local-runner\.js|execution-transcript\.js/,
  )
  for (const filename of [
    'overview.js',
    'execution.js',
    'local-runner.js',
    'execution-transcript.js',
    'dashboard-data.js',
    'execution-data.js',
  ]) {
    assert.equal(
      fs.existsSync(path.join(dashboardRoot, 'public', filename)),
      false,
      `${filename} should be removed`,
    )
  }
})
