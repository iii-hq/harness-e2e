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
const runner = read('src', 'components', 'LocalRunnerDialog.tsx')
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
  assert.match(overviewPage, /hashForWorkspace\('tests'\)/)
  assert.match(overviewPage, /hashForPlans\(\)/)
  assert.match(overviewPage, /View local plans/)
  assert.match(overviewPage, /overview-intelligence-grid/)
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
})

test('keeps the workspace navigation and versioned test flow', () => {
  for (const view of ['overview', 'tests', 'capability', 'executions']) {
    assert.match(sectionNav, new RegExp(`id: '${view}'`))
  }
  assert.match(testsPage, /Tests across system versions/)
  assert.match(testsPage, /loadVersionResult\(row\.test_id/)
  assert.match(testsPage, /prefetchedCatalog/)
  assert.match(catalogPage, /href=\{hashForWorkspace\(\)\}[\s\S]*Overview/)
  assert.match(historyPage, /<header className="topbar">/)
  assert.match(historyPage, /<span className="brand-copy">/)
  assert.match(historyPage, /Test catalog/)
  assert.doesNotMatch(historyPage, /tmh-topbar|tmh-brand|tmh-context/)
  assert.match(plansPage, /All local plans/)
  assert.match(plansPage, /hashForNewPlan\(\)/)
  assert.match(plansPage, /Comparison ready/)
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
  assert.match(planPage, /Create a focused local plan/)
  assert.match(planPage, /Find a test/)
  assert.match(planPage, /Search by name or id/)
  assert.match(planPage, /Select visible/)
  assert.match(planPage, /plan-test-option/)
  assert.match(planPage, /choose the smallest useful scope/)
  assert.match(planPage, /Sampling and retries/)
  assert.match(planPage, /Runs create logical evidence samples/)
  assert.match(planPage, /plan-advanced-control/)
  assert.doesNotMatch(planPage, /local-scenario-options/)
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
  assert.match(executionPage, /detail-index hidden sticky/)
  assert.match(executionPage, /max-\[840px\]:grid/)
  assert.match(
    executionPage,
    /anchor === 'evidence' \|\| anchor === 'raw-data'\) return 'technical'/,
  )
  assert.doesNotMatch(executionPage, /id="evidence"|Evidence register/)
  assert.match(executionPage, /AssessmentWorkspace detail/)
  assert.match(executionPage, /diagnostic runs/)
  assert.match(executionPage, /onTranscript/)
  assert.doesNotMatch(executionPage, /Open transcript/)
  assert.match(executionPage, /aggregateAssessmentMetrics/)
  assert.match(executionPage, /Execution indicators/)
  assert.match(executionPage, /Total tokens/)
  assert.doesNotMatch(executionPage, /buildHarnessRecommendation|Next run plan/)
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
  assert.match(transcript, /normalizeTranscript/)
  assert.match(transcript, /session-transcript-dialog/)
  assert.match(transcript, /conversation-shell/)
  assert.match(transcript, /conversation-tool/)
  assert.match(transcript, /formatTranscriptPayload/)
})

test('keeps an empty local execution label serializable and its action visible', () => {
  const styles = read('src', 'index.css')

  assert.match(runner, /label: form\.label\.trim\(\),/)
  assert.doesNotMatch(runner, /label: form\.label \|\| null/)
  assert.match(runner, /form="local-runner-form"/)
  assert.match(
    styles,
    /\.local-runner-dialog \{[\s\S]*grid-template-rows: auto minmax\(0, 1fr\) auto/,
  )
  assert.match(
    styles,
    /\.local-runner-dialog \.local-runner \{[\s\S]*overflow: auto/,
  )
})

test('groups execution and judge models under their providers', () => {
  for (const view of [runner, planPage, historyPage]) {
    assert.match(view, /ProviderModelDropdown/)
    assert.match(view, /Execution model/)
    assert.match(view, /Judge model/)
  }
  assert.match(modelDropdown, /expandedProviders/)
  assert.match(
    modelDropdown,
    /const collapsed = !expandedProviders\.has\(group\.provider\)/,
  )
  assert.match(modelDropdown, /aria-expanded=\{open\}/)
  assert.match(modelDropdown, /provider-model-group-toggle/)
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
