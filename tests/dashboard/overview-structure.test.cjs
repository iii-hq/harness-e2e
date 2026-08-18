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
const runner = read('src', 'components', 'LocalRunnerDialog.tsx')
const executionSetup = read('src', 'components', 'ExecutionSetup.tsx')
const scenarioMatrix = read('src', 'components', 'ScenarioMatrix.tsx')
const modelDropdown = read('src', 'components', 'ProviderModelDropdown.tsx')
const planPage = read('src', 'pages', 'LocalPlanPage.tsx')
const plansPage = read('src', 'pages', 'PlansPage.tsx')
const catalogPage = read('src', 'pages', 'TestsCatalogPage.tsx')
const historyPage = read('src', 'pages', 'TestHistoryPage.tsx')
const transcript = read('src', 'components', 'TranscriptDialog.tsx')
const dataSource = read('src', 'lib', 'dashboard-data-source.ts')
const viewModel = read('src', 'lib', 'execution-view.ts')
const sectionNav = read('src', 'components', 'SectionNav.tsx')
const testsPage = read('src', 'pages', 'TestsPage.tsx')
const publisher = fs.readFileSync(
  path.join(repositoryRoot, 'scripts', 'publish_harness_e2e_dashboard.py'),
  'utf8',
)

test('prioritizes the first-read execution signal', () => {
  assert.match(overviewPage, /Latest execution/)
  assert.match(overviewPage, /Scenario pass rate/)
  assert.match(overviewPage, /Reliability events/)
  assert.match(overviewPage, /Investigate execution/)
  assert.match(overviewPage, /Subject/)
  assert.match(overviewPage, /Judge/)
  assert.match(appHeader, /hashForWorkspace\('tests'\)/)
  assert.match(overviewPage, /hashForPlans\(\)/)
  assert.match(overviewPage, /View local plans/)
  assert.match(overviewPage, /overview-intelligence-grid/)
  assert.match(overviewPage, /className="overview-card-action[^"]*inline-flex/)
  assert.doesNotMatch(
    overviewPage,
    /className="button button-secondary" href=\{hashForPlans\(\)\}/,
  )
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
  assert.doesNotMatch(sectionNav, /capability/i)
})

test('keeps the overview operational, dense, and Tailwind-based', () => {
  const styles = read('src', 'index.css')

  for (const primitive of [
    'Button',
    'MetricCard',
    'PageHeader',
    'Panel',
    'StatusBadge',
  ]) {
    assert.match(overviewPage, new RegExp(`\\b${primitive}\\b`))
  }
  assert.match(styles, /@import "tailwindcss" important/)
  assert.match(overviewPage, /lg:grid-cols-12 lg:grid-rows-2/)
  assert.match(overviewPage, /grid-flow-dense/)
  assert.match(overviewPage, /Performance overview/)
  assert.match(overviewPage, /Total tokens/)
  assert.match(overviewPage, /Measure improvement against a fixed comparison/)
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
  const styles = read('src', 'index.css')
  assert.match(styles, /\.overview-card-action\s*\{[\s\S]*display: inline-flex/)
  assert.match(styles, /\.overview-card-action:hover[\s\S]*border-bottom-color/)
  assert.match(
    styles,
    /\.overview-card-action:focus-visible[\s\S]*outline-offset/,
  )
})

test('keeps the workspace navigation and versioned test flow', () => {
  for (const view of ['overview', 'tests', 'executions']) {
    assert.match(sectionNav, new RegExp(`id: '${view}'`))
  }
  assert.match(testsPage, /Tests across system versions/)
  assert.match(testsPage, /loadVersionResult\(row\.test_id/)
  assert.match(testsPage, /prefetchedCatalog/)
  assert.match(catalogPage, /<AppHeader[\s\S]*active="tests"/)
  assert.match(catalogPage, /catalog-search-label">Search tests/)
  assert.match(catalogPage, /placeholder="Test ID"/)
  assert.match(
    read('src', 'index.css'),
    /\.catalog-search-field input[\s\S]*border: 1px solid var\(--line-strong\)/,
  )
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
    assert.match(page, /<AppHeader/)
  }
  assert.match(appHeader, /aria-current=\{current \? 'page' : undefined\}/)
  assert.match(appHeader, /Overview/)
  assert.match(appHeader, /Tests/)
  assert.match(appHeader, /Executions/)
  assert.match(appHeader, /Plans/)
  assert.match(appHeader, /Coverage/)
  assert.doesNotMatch(read('src', 'index.css'), /\.topbar(?:\s|,|\{)/)
  assert.doesNotMatch(historyPage, /tmh-topbar|tmh-brand|tmh-context/)
  assert.match(plansPage, /All local plans/)
  assert.match(plansPage, /hashForNewPlan\(\)/)
  assert.match(plansPage, /Comparison available/)
})

test('distinguishes active tests from tests with no execution history', () => {
  const styles = read('src', 'index.css')
  assert.match(catalogPage, /lifecycleStatusClass/)
  assert.match(catalogPage, /status-catalog-active/)
  assert.match(catalogPage, /status-catalog-never-run/)
  assert.match(styles, /\.status-catalog-active[\s\S]*color: var\(--success\)/)
  assert.match(
    styles,
    /\.status-catalog-never-run[\s\S]*color: var\(--text-muted\)/,
  )
})

test('keeps metric history rows readable and opens details on demand', () => {
  const styles = read('src', 'index.css')
  assert.match(historyPage, /Median tokens/)
  assert.match(historyPage, /Descriptive median/)
  assert.match(historyPage, /metricCaption/)
  assert.match(historyPage, /ExecutionDetailsDialog/)
  assert.match(
    historyPage,
    /reports: detail\.reports\.filter\([\s\S]*scenario_id === testId/,
  )
  assert.match(historyPage, /Assessment details for this test/)
  assert.match(historyPage, /Compare two runs/)
  assert.match(historyPage, /Set baseline/)
  assert.match(historyPage, /Set candidate/)
  assert.match(historyPage, /compareTestObservations/)
  assert.match(historyPage, /getExecution\(observation\.execution_id\)/)
  assert.match(historyPage, /View details/)
  assert.match(historyPage, /Open full execution report/)
  assert.match(historyPage, /systemSummary/)
  assert.doesNotMatch(historyPage, /item\.execution_id\.slice/)
  assert.doesNotMatch(historyPage, /systemLabel/)
  assert.doesNotMatch(historyPage, /See series/)
  assert.doesNotMatch(historyPage, /Metrics by compatible series/)
  assert.match(
    styles,
    /#test-metrics-history-proposal th,[\s\S]*white-space: normal/,
  )
  assert.match(styles, /overflow-wrap: anywhere/)
  assert.match(
    styles,
    /\.test-catalog-panel > \.panel-heading[\s\S]*padding: 24px 28px/,
  )
})

test('makes local plan scope selection focused and readable', () => {
  assert.match(planPage, /Create a benchmark plan/)
  assert.match(planPage, /ExecutionSetup/)
  assert.match(planPage, /ExecutionSetupReview/)
  assert.match(planPage, /Quick execution/)
  assert.match(planPage, /requestQuickExecution/)
  assert.match(executionSetup, /Find a test/)
  assert.match(executionSetup, /Search by name or id/)
  assert.match(executionSetup, /Select visible/)
  assert.match(executionSetup, /Select the benchmark scope/)
  assert.match(executionSetup, /Sampling, retries and seed/)
  assert.match(executionSetup, /Logical runs/)
  assert.doesNotMatch(planPage, /plan-test-option|plan-advanced-control/)
})

test('keeps plan panels explicitly padded and comparison content full bleed', () => {
  const styles = read('src', 'index.css')
  assert.match(styles, /--plan-panel-space-y: 24px/)
  assert.match(styles, /--plan-panel-space-x: 28px/)
  assert.match(styles, /--plan-card-space: 16px/)
  assert.match(
    styles,
    /\.plan-panel-heading,[\s\S]*\.plan-panel-section[\s\S]*padding: var\(--plan-panel-space-y\) var\(--plan-panel-space-x\)/,
  )
  assert.match(
    styles,
    /\.plans-list-heading[\s\S]*padding: var\(--plan-panel-space-y\) var\(--plan-panel-space-x\)/,
  )
  assert.match(
    styles,
    /@media \(max-width: 560px\)[\s\S]*--plan-panel-space-y: 18px[\s\S]*--plan-panel-space-x: 18px/,
  )
  assert.match(planPage, /panel-heading plan-panel-heading/g)
  assert.doesNotMatch(planPage, /className="panel-heading"/)
  assert.match(plansPage, /panel-heading plans-list-heading/)
  assert.match(planPage, /plan-execution-table-wrap/)
  assert.match(planPage, /plan-scenario-table-wrap/)
})

test('exposes baseline and arbitrary candidate comparison controls', () => {
  const styles = read('src', 'index.css')
  assert.match(plansPage, /Latest candidate vs baseline/)
  assert.match(plansPage, /Objective regressions/)
  assert.match(plansPage, /Not reported/)
  assert.match(planPage, /Baseline and candidates/)
  assert.match(
    planPage,
    /Choose a visual baseline and any number of candidates/,
  )
  assert.match(planPage, /PLAN_COMPARISON_TABLE_METRICS/)
  assert.match(planPage, /plan-execution-metric/)
  assert.match(planPage, /planMetricWinnerIds/)
  assert.match(planPage, /plan-winner-badge/)
  assert.match(planPage, /--plan-execution-column-count/)
  assert.match(styles, /table-layout: fixed/)
  assert.match(planPage, /Best values are highlighted/)
  assert.match(planPage, /Visual baseline/)
  assert.match(planPage, /Execution history/)
  assert.match(planPage, /plan-run-history-list/)
  assert.match(planPage, /ExecutionNameControl/)
  assert.match(planPage, /official plan baseline remains unchanged/)
  assert.match(
    planPage,
    /!plan\.baseline_execution_id[\s\S]*plan\.candidate_execution_ids\.length === 0/,
  )
  assert.doesNotMatch(planPage, /onSelectCandidate/)
  assert.doesNotMatch(planPage, /plan-execution-report-link/)
  assert.doesNotMatch(planPage, /scrollIntoView\(\{ behavior: 'smooth'/)
  assert.match(planPage, /aria-live="polite"/)
  assert.match(planPage, /baselineLabel.*resolvedCandidateLabel/s)
  assert.doesNotMatch(planPage, /plan-comparison-metrics/)
  assert.doesNotMatch(planPage, /ComparisonMetricCard/)
  assert.match(planPage, /Test breakdown/)
  assert.match(planPage, /Run another candidate/)
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
  assert.match(executionPage, /hashForExecution\(executionId, item\.id\)/)
  assert.match(executionPage, /detail-index sticky top-16/)
  assert.doesNotMatch(executionPage, /detail-index hidden/)
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
  assert.match(transcript, /max-\[560px\]:w-screen/)
  assert.match(transcript, /max-\[560px\]:h-dvh/)
})

test('migrates runner and transcript behavior to React components', () => {
  assert.match(runner, /getCatalog/)
  assert.match(runner, /startRun/)
  assert.match(runner, /cancelRun/)
  assert.match(runner, /subscribeRunChanges/)
  assert.match(runner, /aria-live/)
  assert.match(runner, /function trapDialogFocus/)
  assert.match(runner, /getClientRects\(\)\.length > 0/)
  assert.match(runner, /onKeyDownCapture=\{trapDialogFocus\}/)
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
  assert.match(runner, /hidden max-h-\[94dvh\]/)
  assert.match(runner, /open:grid open:grid-rows/)
  assert.match(runner, /lg:grid-cols-12/)
  assert.match(runner, /Create a reusable plan instead/)
  assert.match(executionSetup, /mode === 'plan'/)
})

test('groups execution and judge models under their providers', () => {
  for (const view of [runner, planPage]) assert.match(view, /ExecutionSetup/)
  for (const view of [executionSetup, historyPage]) {
    assert.match(view, /ProviderModelDropdown/)
  }
  assert.match(executionSetup, /Execution model/)
  assert.match(executionSetup, /Judge model/)
  assert.match(modelDropdown, /expandedProviders/)
  assert.match(
    modelDropdown,
    /const collapsed = !expandedProviders\.has\(group\.provider\)/,
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
