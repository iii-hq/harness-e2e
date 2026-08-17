const sampleRuns = [
  {
    id: 'run_08_16_1421',
    label: 'main · codex-luna',
    revision: '7cf1d8a',
    profile: 'stateless-core',
    contract: 'core-v4',
    subject: 'codex / gpt-5.6-luna',
    judge: 'codex / gpt-5.5',
    status: 'attention',
    completedAt: '2026-08-16T17:21:00Z',
    durationSeconds: 1122,
    counts: {
      passed: 12,
      concerns: 0,
      failed: 1,
      inconclusive: 2,
      skipped: 1,
    },
    blocking: 1,
    score: 84,
    evidence: 46,
    availability: 'full',
    scenarios: [
      { id: 'direct_answer', outcome: 'passed', summary: 'Required output and response contract satisfied.', dimension: 'Deliverable', evidence: 4 },
      { id: 'security_review', outcome: 'passed', summary: 'Findings were evidence-linked and correctly prioritized.', dimension: 'Robustness', evidence: 5 },
      { id: 'shell_coder_sandbox', outcome: 'failed', summary: 'One blocking filesystem integrity gate failed.', dimension: 'Structural integrity', evidence: 8 },
      { id: 'coordination.3', outcome: 'inconclusive', summary: 'Judge evidence was unavailable for the final turn.', dimension: 'Infrastructure', evidence: 3 },
      { id: 'reactive_automation', outcome: 'passed', summary: 'Lifecycle completion and cleanup gates passed.', dimension: 'Efficiency', evidence: 7 },
      { id: 'custom_validator', outcome: 'skipped', summary: 'Scenario was outside the selected profile.', dimension: 'Deliverable', evidence: 0 },
    ],
    isSample: true,
  },
  {
    id: 'run_08_16_1008',
    label: 'main · codex-sol',
    revision: '7cf1d8a',
    profile: 'stateless-core',
    contract: 'core-v4',
    subject: 'codex / gpt-5.6-sol',
    judge: 'codex / gpt-5.5',
    status: 'passed',
    completedAt: '2026-08-16T13:08:00Z',
    durationSeconds: 965,
    counts: {
      passed: 16,
      concerns: 0,
      failed: 0,
      inconclusive: 0,
      skipped: 0,
    },
    blocking: 0,
    score: 91,
    evidence: 58,
    availability: 'full',
    scenarios: [
      { id: 'direct_answer', outcome: 'passed', summary: 'Required output and response contract satisfied.', dimension: 'Deliverable', evidence: 4 },
      { id: 'security_review', outcome: 'passed', summary: 'Findings were evidence-linked and correctly prioritized.', dimension: 'Robustness', evidence: 5 },
      { id: 'shell_coder_sandbox', outcome: 'passed', summary: 'Filesystem operations and cleanup gates passed.', dimension: 'Structural integrity', evidence: 9 },
      { id: 'coordination.3', outcome: 'passed', summary: 'Delegation and completion evidence were retained.', dimension: 'Efficiency', evidence: 6 },
      { id: 'reactive_automation', outcome: 'passed', summary: 'Lifecycle completion and cleanup gates passed.', dimension: 'Efficiency', evidence: 7 },
      { id: 'custom_validator', outcome: 'passed', summary: 'Custom validation contract completed successfully.', dimension: 'Deliverable', evidence: 4 },
    ],
    isSample: true,
  },
  {
    id: 'run_08_15_1836',
    label: 'feature/lifecycle · codex-luna',
    revision: '2ab91e0',
    profile: 'coordination',
    contract: 'coordination-v2',
    subject: 'codex / gpt-5.6-luna',
    judge: 'codex / gpt-5.5',
    status: 'incomplete',
    completedAt: '2026-08-15T21:36:00Z',
    durationSeconds: 1339,
    counts: {
      passed: 9,
      concerns: 0,
      failed: 0,
      inconclusive: 4,
      skipped: 1,
    },
    blocking: 0,
    score: null,
    evidence: 27,
    availability: 'aggregate',
    scenarios: [
      { id: 'coordination.1', outcome: 'passed', summary: 'Single-agent task completed with retained evidence.', dimension: 'Deliverable', evidence: 4 },
      { id: 'coordination.2', outcome: 'passed', summary: 'Two-agent coordination completed successfully.', dimension: 'Efficiency', evidence: 5 },
      { id: 'coordination.3', outcome: 'inconclusive', summary: 'Final judge evidence was not retained.', dimension: 'Infrastructure', evidence: 2 },
      { id: 'coordination.4', outcome: 'inconclusive', summary: 'Execution stopped before a terminal assessment.', dimension: 'Infrastructure', evidence: 2 },
    ],
    isSample: true,
  },
  {
    id: 'run_08_15_1204',
    label: 'main · local-luna',
    revision: 'f41c620',
    profile: 'sandbox',
    contract: 'sandbox-v3',
    subject: 'codex / gpt-5.6-luna',
    judge: 'codex / gpt-5.5',
    status: 'passed',
    completedAt: '2026-08-15T15:04:00Z',
    durationSeconds: 713,
    counts: {
      passed: 8,
      concerns: 0,
      failed: 0,
      inconclusive: 0,
      skipped: 1,
    },
    blocking: 0,
    score: 87,
    evidence: 31,
    availability: 'full',
    scenarios: [
      { id: 'shell_coder_sandbox', outcome: 'passed', summary: 'Filesystem operations and cleanup gates passed.', dimension: 'Structural integrity', evidence: 9 },
      { id: 'terminal_tooling', outcome: 'passed', summary: 'Commands completed and outputs were retained.', dimension: 'Deliverable', evidence: 5 },
      { id: 'workspace_cleanup', outcome: 'skipped', summary: 'Cleanup probe was outside this local profile.', dimension: 'Robustness', evidence: 0 },
    ],
    isSample: true,
  },
]

const statusLabels = {
  passed: 'Passed',
  attention: 'Needs attention',
  incomplete: 'Incomplete',
  running: 'Running',
  unknown: 'Not recorded',
}

const outcomeLabels = {
  passed: 'Passed',
  failed: 'Failed',
  inconclusive: 'Inconclusive',
  skipped: 'Skipped',
  unknown: 'Not recorded',
}

const state = {
  runs: sampleRuns,
  selectedId: sampleRuns[0].id,
  activeView: viewFromHash(),
  source: 'sample',
  search: '',
  statusFilter: 'all',
  scenarioRunId: sampleRuns[0].id,
  compareLeftId: sampleRuns[1].id,
  compareRightId: sampleRuns[0].id,
}

const $ = (selector) => document.querySelector(selector)
const $$ = (selector) => Array.from(document.querySelectorAll(selector))

function escapeHtml(value) {
  return String(value ?? '')
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#039;')
}

function finiteNumber(value) {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function objectValue(value) {
  return value && typeof value === 'object' && !Array.isArray(value) ? value : {}
}

function stringValue(value, fallback = '') {
  return typeof value === 'string' && value ? value : fallback
}

function count(source, key) {
  return finiteNumber(source[key]) ?? 0
}

function sum(values) {
  return values.reduce((total, value) => total + (finiteNumber(value) ?? 0), 0)
}

function viewFromHash() {
  const view = window.location.hash.replace(/^#/, '')
  return ['overview', 'runs', 'scenarios', 'compare'].includes(view)
    ? view
    : 'overview'
}

function normalizeScenario(scenario) {
  const summary = objectValue(scenario.assessment_summary)
  const statuses = objectValue(summary.system_statuses)
  const blocking = sum([
    count(statuses, 'hard_gate_failed'),
    count(statuses, 'infrastructure_error'),
    count(statuses, 'resource_limit'),
    count(statuses, 'subject_error'),
    count(statuses, 'judge_error'),
  ])
  const passed = count(statuses, 'passed') + count(statuses, 'passed_with_concerns')
  const unavailable = count(statuses, 'unavailable')
  const outcome = blocking > 0 ? 'failed' : passed > 0 ? 'passed' : unavailable > 0 ? 'inconclusive' : 'unknown'
  return {
    id: stringValue(scenario.id, 'unknown-scenario'),
    outcome,
    summary: outcome === 'unknown' ? 'Detailed assessment was not recorded for this scenario.' : 'Open the execution detail to inspect evidence and assessment results.',
    dimension: 'Recorded assessment',
    evidence: finiteNumber(summary.evidence_reference_count) ?? 0,
  }
}

function normalizeExecution(execution) {
  const totals = objectValue(execution.totals)
  const assessment = objectValue(execution.assessment_summary)
  const statuses = objectValue(assessment.system_statuses)
  const expected = finiteNumber(totals.expected_reports)
  const received = finiteNumber(totals.received_reports)
  const explicitStatuses = Object.keys(statuses).length > 0
  const passed = explicitStatuses
    ? count(statuses, 'passed')
    : (finiteNumber(totals.passed_scenarios) ?? 0)
  const concerns = explicitStatuses ? count(statuses, 'passed_with_concerns') : 0
  const failed = explicitStatuses
    ? sum([
        count(statuses, 'hard_gate_failed'),
        count(statuses, 'infrastructure_error'),
        count(statuses, 'resource_limit'),
        count(statuses, 'subject_error'),
        count(statuses, 'judge_error'),
      ])
    : sum([
        finiteNumber(totals.hard_gate_failures),
        finiteNumber(totals.technical_failures),
        finiteNumber(totals.infra_failures),
        finiteNumber(totals.resource_limit_failures),
      ])
  const inconclusive = explicitStatuses
    ? count(statuses, 'unavailable')
    : finiteNumber(totals.missing_reports) ?? 0
  const rawStatus = stringValue(execution.status)
  const status = rawStatus === 'running'
    ? 'running'
    : ['incomplete', 'cancelled', 'cancelling', 'unavailable'].includes(rawStatus)
      ? 'incomplete'
      : failed > 0
        ? 'attention'
        : passed + concerns > 0
          ? 'passed'
          : 'unknown'
  const subjects = Array.isArray(execution.subjects) ? execution.subjects : []
  const firstSubject = objectValue(subjects[0])
  const judge = objectValue(firstSubject.judge)
  const source = objectValue(execution.source)
  const scenarios = subjects
    .flatMap((subject) => Array.isArray(subject?.scenarios) ? subject.scenarios : [])
    .filter((scenario, index, all) => all.findIndex((candidate) => candidate?.id === scenario?.id) === index)
    .map(normalizeScenario)
  const totalStatuses = passed + concerns + failed + inconclusive
  const skipped = explicitStatuses && expected !== null && totalStatuses < expected
    ? Math.max(0, expected - totalStatuses)
    : 0
  const blocking = failed
  const revision = stringValue(source.sha) || stringValue(source.revision) || stringValue(source.commit) || 'Not reported'
  const profile = stringValue(execution.lane) || stringValue(source.profile) || 'Not reported'
  const provider = stringValue(firstSubject.provider)
  const model = stringValue(firstSubject.model)
  const judgeProvider = stringValue(judge.provider)
  const judgeModel = stringValue(judge.model)
  return {
    id: stringValue(execution.id, crypto.randomUUID()),
    label: stringValue(execution.label) || stringValue(execution.workflow_name, 'Harness E2E execution'),
    revision,
    profile,
    contract: stringValue(execution.assessment_contract_hash) || stringValue(source.profile_digest) || null,
    subject: model ? `${provider ? `${provider} / ` : ''}${model}` : 'Not reported',
    judge: judgeModel ? `${judgeProvider ? `${judgeProvider} / ` : ''}${judgeModel}` : 'Not reported',
    status,
    completedAt: stringValue(execution.completed_at) || stringValue(execution.generated_at),
    durationSeconds: finiteNumber(totals.wall_time_seconds) ?? finiteNumber(totals.workflow_duration_seconds),
    counts: { passed, concerns, failed, inconclusive, skipped },
    blocking,
    score: finiteNumber(assessment.median_quality_score),
    evidence: finiteNumber(assessment.evidence_reference_count) ?? 0,
    availability: stringValue(execution.availability, 'unavailable'),
    scenarios,
    isSample: false,
  }
}

function selectedRun() {
  return state.runs.find((run) => run.id === state.selectedId) ?? state.runs[0]
}

function scenarioRun() {
  return state.runs.find((run) => run.id === state.scenarioRunId) ?? selectedRun()
}

function totalCount(run) {
  return sum(Object.values(run.counts))
}

function formatDate(value) {
  if (!value) return 'Not reported'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  }).format(date)
}

function formatDuration(seconds) {
  if (finiteNumber(seconds) === null) return 'Not reported'
  const minutes = Math.floor(seconds / 60)
  const remainder = Math.round(seconds % 60)
  return `${minutes}m ${String(remainder).padStart(2, '0')}s`
}

function scoreLabel(score) {
  return finiteNumber(score) === null ? '—' : score.toFixed(1)
}

function outcomeForRun(run) {
  const total = totalCount(run)
  return total > 0 ? `${run.counts.passed} / ${total}` : '—'
}

function statusMarkup(run) {
  return `<span class="status-pill status-${run.status}">${escapeHtml(statusLabels[run.status] ?? statusLabels.unknown)}</span>`
}

function runRowMarkup(run) {
  const selected = run.id === state.selectedId
  return `
    <button class="run-row" type="button" aria-pressed="${selected}" data-run-id="${escapeHtml(run.id)}">
      <span class="run-cell">
        <strong>${escapeHtml(run.label)}</strong>
        <small>${escapeHtml(formatDate(run.completedAt))} · ${escapeHtml(run.id)}</small>
      </span>
      <span class="run-cell">
        <strong>${escapeHtml(run.profile)}</strong>
        <small>${escapeHtml(run.subject)}</small>
      </span>
      <span>${statusMarkup(run)}</span>
      <span class="run-score">${escapeHtml(scoreLabel(run.score))}<small>${finiteNumber(run.score) === null ? 'Not recorded' : 'Advisory'}</small></span>
      <span class="run-duration">${escapeHtml(formatDuration(run.durationSeconds))}</span>
    </button>`
}

function renderSelected() {
  const run = selectedRun()
  if (!run) return
  $('#selected-run-title').textContent = run.label
  $('#selected-run-id').textContent = run.id
  $('#selected-run-status').className = `status-pill status-${run.status}`
  $('#selected-run-status').textContent = statusLabels[run.status] ?? statusLabels.unknown
  $('#selected-pass-rate').textContent = outcomeForRun(run)
  $('#selected-pass-context').textContent = totalCount(run) > 0 ? 'Passed scenarios' : 'Not recorded'
  $('#selected-blocking').textContent = Number.isFinite(run.blocking) ? String(run.blocking) : '—'
  $('#selected-score').textContent = scoreLabel(run.score)
  $('#selected-revision').textContent = run.revision
  $('#selected-profile').textContent = run.profile
  $('#selected-subject').textContent = run.subject
  $('#selected-duration').textContent = formatDuration(run.durationSeconds)

  const parts = [
    ['passed', run.counts.passed, 'Passed'],
    ['concerns', run.counts.concerns, 'Passed with concerns'],
    ['failed', run.counts.failed, 'Failed'],
    ['inconclusive', run.counts.inconclusive + run.counts.skipped, 'Inconclusive / skipped'],
  ].filter(([, value]) => value > 0)
  const total = totalCount(run)
  $('#selected-total').textContent = total > 0 ? `${total} total` : 'Not recorded'
  $('#selected-distribution').innerHTML = total > 0
    ? parts.map(([key, value]) => `<span class="distribution-segment segment-${key}" style="width:${(value / total) * 100}%"></span>`).join('')
    : ''
  $('#selected-distribution').setAttribute(
    'aria-label',
    total > 0
      ? `${run.counts.passed} passed, ${run.counts.concerns} passed with concerns, ${run.counts.failed} failed, ${run.counts.inconclusive} inconclusive, ${run.counts.skipped} skipped`
      : 'Scenario distribution is not recorded',
  )
  $('#selected-legend').innerHTML = parts
    .map(([key, value, label]) => `<span><i class="legend-swatch segment-${key}"></i>${value} ${escapeHtml(label)}</span>`)
    .join('')
}

function renderReadiness() {
  const run = selectedRun()
  if (!run) return
  const items = [
    {
      icon: 'CP',
      title: 'Control plane',
      detail: state.source === 'live' ? 'Execution manifest available' : 'Using explicit prototype data',
      value: state.source === 'live' ? 'Connected' : 'Prototype',
    },
    {
      icon: 'AI',
      title: 'Subject & judge',
      detail: run.subject === 'Not reported' ? 'Model identity unavailable' : run.judge,
      value: run.subject === 'Not reported' ? 'Partial' : 'Ready',
    },
    {
      icon: 'EV',
      title: 'Evidence store',
      detail: run.evidence > 0 ? `${run.evidence} evidence references` : 'Evidence count not recorded',
      value: run.availability === 'full' ? 'Ready' : 'Partial',
    },
  ]
  $('#readiness-list').innerHTML = items
    .map((item) => `
      <li class="readiness-item">
        <span class="readiness-icon">${escapeHtml(item.icon)}</span>
        <span class="readiness-copy"><strong>${escapeHtml(item.title)}</strong><small>${escapeHtml(item.detail)}</small></span>
        <span class="readiness-value">${escapeHtml(item.value)}</span>
      </li>`)
    .join('')
}

function renderOverviewRuns() {
  $('#overview-run-list').innerHTML = state.runs.slice(0, 5).map(runRowMarkup).join('')
}

function matchesStatus(run) {
  return state.statusFilter === 'all' || run.status === state.statusFilter
}

function filteredRuns() {
  const query = state.search.trim().toLowerCase()
  return state.runs.filter((run) => {
    if (!matchesStatus(run)) return false
    if (!query) return true
    return [run.label, run.id, run.subject, run.judge, run.profile, run.revision]
      .join(' ')
      .toLowerCase()
      .includes(query)
  })
}

function renderLedger() {
  const runs = filteredRuns()
  $('#run-filter-result').textContent = `${runs.length} of ${state.runs.length} runs`
  $('#ledger-list').innerHTML = runs.length
    ? runs.map((run) => `
      <article class="ledger-row">
        <div class="run-cell">
          <button type="button" data-run-id="${escapeHtml(run.id)}">
            <strong>${escapeHtml(run.label)}</strong>
            <small>${escapeHtml(run.id)} · ${escapeHtml(formatDate(run.completedAt))}</small>
          </button>
        </div>
        <div>${statusMarkup(run)}</div>
        <div class="ledger-metric"><span>Subject</span><strong>${escapeHtml(run.subject)}</strong></div>
        <div class="ledger-metric"><span>Outcome</span><strong>${escapeHtml(outcomeForRun(run))}</strong></div>
        <div class="ledger-metric"><span>Evidence</span><strong>${escapeHtml(run.availability)}</strong></div>
      </article>`).join('')
    : '<div class="empty-state">No runs match these filters.</div>'
}

function runOptions(selectedId) {
  return state.runs
    .map((run) => `<option value="${escapeHtml(run.id)}" ${run.id === selectedId ? 'selected' : ''}>${escapeHtml(run.label)} · ${escapeHtml(formatDate(run.completedAt))}</option>`)
    .join('')
}

function renderScenarioOptions() {
  $('#scenario-run-select').innerHTML = runOptions(state.scenarioRunId)
}

function renderScenarios() {
  const run = scenarioRun()
  if (!run) return
  $('#scenario-view-context').textContent = `${run.label} · ${run.id}`
  $('#scenario-grid').innerHTML = run.scenarios.length
    ? run.scenarios.map((scenario) => `
      <article class="scenario-card">
        <div class="scenario-card-heading">
          <h2>${escapeHtml(scenario.id)}</h2>
          <span class="outcome-chip outcome-${escapeHtml(scenario.outcome)}">${escapeHtml(outcomeLabels[scenario.outcome] ?? outcomeLabels.unknown)}</span>
        </div>
        <p>${escapeHtml(scenario.summary)}</p>
        <div class="scenario-meta">
          <span>${escapeHtml(scenario.dimension)}</span>
          <span>${scenario.evidence > 0 ? `${scenario.evidence} evidence refs` : 'Evidence not recorded'}</span>
        </div>
      </article>`).join('')
    : '<div class="panel empty-state">Scenario detail is not available in this compact execution record.</div>'
}

function renderCompareOptions() {
  $('#compare-left').innerHTML = runOptions(state.compareLeftId)
  $('#compare-right').innerHTML = runOptions(state.compareRightId)
}

function comparisonCard(run, role) {
  const total = totalCount(run)
  return `
    <article class="panel comparison-card">
      <span class="panel-kicker">${escapeHtml(role)}</span>
      <h2>${escapeHtml(run.label)}</h2>
      <p>${escapeHtml(run.id)} · ${escapeHtml(run.revision)}</p>
      <dl class="comparison-metrics">
        <div><dt>Operational result</dt><dd>${escapeHtml(statusLabels[run.status] ?? statusLabels.unknown)}</dd></div>
        <div><dt>Passed scenarios</dt><dd>${total > 0 ? `${run.counts.passed} of ${total}` : 'Not recorded'}</dd></div>
        <div><dt>Blocking events</dt><dd>${escapeHtml(run.blocking)}</dd></div>
        <div><dt>Quality score</dt><dd>${escapeHtml(scoreLabel(run.score))} <small>advisory</small></dd></div>
        <div><dt>Duration</dt><dd>${escapeHtml(formatDuration(run.durationSeconds))}</dd></div>
        <div><dt>Contract</dt><dd><code>${escapeHtml(run.contract ?? 'Not recorded')}</code></dd></div>
      </dl>
    </article>`
}

function renderComparison() {
  const left = state.runs.find((run) => run.id === state.compareLeftId) ?? state.runs[0]
  const right = state.runs.find((run) => run.id === state.compareRightId) ?? state.runs[1] ?? state.runs[0]
  if (!left || !right) return
  const comparable = Boolean(left.contract && right.contract && left.contract === right.contract)
  const known = Boolean(left.contract && right.contract)
  const note = $('#compatibility-note')
  note.className = `compatibility-note ${comparable ? 'is-compatible' : 'is-incompatible'}`
  note.textContent = comparable
    ? `Compatible contract (${left.contract}). Side-by-side values describe the same evaluation scope.`
    : known
      ? `Contracts differ (${left.contract} vs ${right.contract}). Numeric deltas are intentionally disabled.`
      : 'Contract identity is not recorded for both runs. Numeric deltas are intentionally disabled.'
  $('#comparison-grid').innerHTML = comparisonCard(left, 'Baseline') + comparisonCard(right, 'Candidate')
}

function renderSource() {
  const label = state.source === 'live' ? 'Live dashboard data' : 'Prototype data'
  $('#source-label').textContent = label
  $('#environment-status').textContent = state.source === 'live'
    ? 'Connected · manifest loaded'
    : 'Prototype · sample manifest'
}

function renderAll() {
  renderSource()
  renderSelected()
  renderReadiness()
  renderOverviewRuns()
  renderLedger()
  renderScenarioOptions()
  renderScenarios()
  renderCompareOptions()
  renderComparison()
}

function switchView(view, updateHash = true) {
  if (!['overview', 'runs', 'scenarios', 'compare'].includes(view)) return
  state.activeView = view
  $$('[data-view-panel]').forEach((panel) => {
    const active = panel.dataset.viewPanel === view
    panel.hidden = !active
    panel.classList.toggle('is-visible', active)
  })
  $$('[data-view]').forEach((control) => {
    control.classList.toggle('is-active', control.dataset.view === view)
    if (control.dataset.view === view) control.setAttribute('aria-current', 'page')
    else control.removeAttribute('aria-current')
  })
  $('#breadcrumb-view').textContent = view
  if (updateHash && window.location.hash !== `#${view}`) {
    window.history.replaceState(null, '', `#${view}`)
  }
  $('#main-content').focus({ preventScroll: true })
}

function selectRun(id) {
  if (!state.runs.some((run) => run.id === id)) return
  state.selectedId = id
  state.scenarioRunId = id
  renderSelected()
  renderReadiness()
  renderOverviewRuns()
  renderScenarioOptions()
  renderScenarios()
}

let toastTimer = null
function showToast(message) {
  const toast = $('#toast')
  toast.textContent = message
  toast.hidden = false
  window.clearTimeout(toastTimer)
  toastTimer = window.setTimeout(() => {
    toast.hidden = true
  }, 4200)
}

function openRunDialog() {
  const dialog = $('#run-dialog')
  if (typeof dialog.showModal === 'function') dialog.showModal()
  else dialog.setAttribute('open', '')
}

function initializeTheme() {
  const stored = localStorage.getItem('harness-e2e-prototype-theme')
  const preferred = window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark'
  document.documentElement.dataset.theme = stored === 'light' || stored === 'dark' ? stored : preferred
}

function toggleTheme() {
  const next = document.documentElement.dataset.theme === 'light' ? 'dark' : 'light'
  document.documentElement.dataset.theme = next
  localStorage.setItem('harness-e2e-prototype-theme', next)
}

async function loadLiveData() {
  try {
    const response = await fetch('../executions.json', { cache: 'no-store' })
    if (!response.ok) throw new Error(`manifest request failed (${response.status})`)
    const payload = await response.json()
    const executions = Array.isArray(payload.executions) ? payload.executions : []
    if (executions.length === 0) throw new Error('manifest contains no executions')
    const runs = executions.map(normalizeExecution)
    state.runs = runs
    state.source = 'live'
    state.selectedId = runs[0].id
    state.scenarioRunId = runs[0].id
    state.compareRightId = runs[0].id
    state.compareLeftId = runs[1]?.id ?? runs[0].id
    renderAll()
  } catch {
    state.source = 'sample'
    renderSource()
  }
}

document.addEventListener('click', (event) => {
  const viewControl = event.target.closest('[data-view]')
  if (viewControl) {
    switchView(viewControl.dataset.view)
    return
  }
  const runControl = event.target.closest('[data-run-id]')
  if (runControl) {
    selectRun(runControl.dataset.runId)
    switchView('overview')
    return
  }
  const action = event.target.closest('[data-action]')?.dataset.action
  if (action === 'new-run') openRunDialog()
  if (action === 'settings') showToast('Environment settings stay unchanged in this isolated prototype.')
  if (action === 'inspect-selected') {
    const run = selectedRun()
    if (run?.isSample) {
      state.scenarioRunId = run.id
      renderScenarioOptions()
      renderScenarios()
      switchView('scenarios')
    } else if (run) {
      window.location.assign(`../#/execution/${encodeURIComponent(run.id)}`)
    }
  }
})

$('#theme-toggle').addEventListener('click', toggleTheme)
$('#run-search').addEventListener('input', (event) => {
  state.search = event.target.value
  renderLedger()
})
$('#run-status-filter').addEventListener('change', (event) => {
  state.statusFilter = event.target.value
  renderLedger()
})
$('#scenario-run-select').addEventListener('change', (event) => {
  state.scenarioRunId = event.target.value
  renderScenarios()
})
$('#compare-left').addEventListener('change', (event) => {
  state.compareLeftId = event.target.value
  renderComparison()
})
$('#compare-right').addEventListener('change', (event) => {
  state.compareRightId = event.target.value
  renderComparison()
})
$('#run-form').addEventListener('submit', (event) => {
  if (event.submitter?.value === 'cancel') return
  event.preventDefault()
  $('#run-dialog').close()
  showToast('Evaluation plan previewed. No run was started by the prototype.')
})
window.addEventListener('hashchange', () => switchView(viewFromHash(), false))

initializeTheme()
renderAll()
switchView(state.activeView, false)
void loadLiveData()
