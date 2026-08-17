import { useCallback, useEffect, useMemo, useState } from 'react'
import { LocalRunnerDialog } from '@/components/LocalRunnerDialog'
import { ThemeToggle } from '@/components/ThemeToggle'
import {
  hashForCoverage,
  hashForExecution,
  hashForNewPlan,
  hashForPlans,
  hashForWorkspace,
  type WorkspaceView,
} from '@/hooks/use-hash-route'
import {
  type DashboardDataBridge,
  type DashboardExecutionSummary,
  getDashboardDataBridge,
  type JsonObject,
} from '@/lib/dashboard-data-source'
import {
  buildExecutionPresentation,
  categoryLabel,
  categoryMessage,
  type ExecutionPresentation,
  type FailureCategory,
  formatDate,
  formatDuration,
  formatPercent,
  isExecutionAttention,
} from '@/lib/execution-view'

function statusCopy(presentation: ExecutionPresentation) {
  if (presentation.attention === 'passed')
    return {
      label: 'Passed',
      title: 'Latest execution passed',
      tone: 'status-pass',
    }
  if (presentation.attention === 'running')
    return {
      label: 'Running',
      title: 'Execution is still running',
      tone: 'status-incomplete',
    }
  if (presentation.attention === 'cancelling')
    return {
      label: 'Cancelling',
      title: 'Cancellation is in progress',
      tone: 'status-incomplete',
    }
  if (presentation.attention === 'cancelled')
    return {
      label: 'Cancelled',
      title: 'Execution was cancelled',
      tone: 'status-incomplete',
    }
  if (presentation.attention === 'incomplete')
    return {
      label: 'Incomplete',
      title: 'Evidence is incomplete',
      tone: 'status-incomplete',
    }
  if (presentation.attention === 'unavailable')
    return {
      label: 'No report',
      title: 'No report evidence is available',
      tone: 'status-incomplete',
    }
  return {
    label: 'Needs attention',
    title: 'Latest execution needs attention',
    tone: 'status-fail',
  }
}

function modelNames(models: ExecutionPresentation['subjects']) {
  if (models.length === 0) return 'Not reported'
  return models.map((model) => `${model.provider}/${model.model}`).join(', ')
}

function SummaryKpi({
  label,
  value,
  caption,
}: {
  label: string
  value: string
  caption: string
}) {
  return (
    <article className="kpi-card min-h-0">
      <div className="kpi-label">{label}</div>
      <div className="kpi-value mt-4 text-[clamp(1.8rem,3vw,2.55rem)]">
        {value}
      </div>
      <div className="kpi-delta">{caption}</div>
    </article>
  )
}

function FailureBreakdown({
  presentation,
}: {
  presentation: ExecutionPresentation
}) {
  const allCategories: FailureCategory[] = [
    'infrastructure',
    'resource_limit',
    'subject',
    'judge',
    'hard_gate',
    'inconclusive',
  ]
  const categories = allCategories.filter(
    (key) => presentation.breakdown[key] > 0,
  )
  if (categories.length === 0) {
    return (
      <p className="m-0 text-sm text-ink-muted">
        No blocking events were reported.
      </p>
    )
  }
  return (
    <div className="grid gap-2 sm:grid-cols-2">
      {categories.map((category) => (
        <div
          key={category}
          className="rounded-lg border border-danger/25 bg-danger/5 px-3 py-2 text-sm text-ink-soft"
        >
          <strong>{categoryLabel(category)}</strong>
          <span className="ml-2 text-ink-muted">
            {categoryMessage(category, presentation.breakdown[category])}
          </span>
        </div>
      ))}
    </div>
  )
}

function LatestExecution({
  presentation,
}: {
  presentation: ExecutionPresentation
}) {
  const status = statusCopy(presentation)
  const issue = presentation.primaryIssue
  const execution = presentation.execution
  return (
    <section
      className="latest-evidence"
      aria-labelledby="latest-health-heading"
    >
      <article className="panel latest-health">
        <div className="latest-health-heading">
          <div>
            <div className="section-kicker">01 / Current signal</div>
            <h2 id="latest-health-heading">Latest execution</h2>
          </div>
          <span className={`status-pill ${status.tone}`}>{status.label}</span>
        </div>
        <h3>{status.title}</h3>
        <p className="trend-description">
          {presentation.expectedReports !== null &&
          presentation.receivedReports !== null
            ? `${presentation.receivedReports} of ${presentation.expectedReports} expected reports received.`
            : 'Report completeness was not published for this execution.'}{' '}
          {presentation.available
            ? 'Open the detail to inspect the evidence and recommendation.'
            : 'Only aggregate metadata is retained.'}
        </p>
        <section
          className="latest-health-meta"
          aria-label="Latest execution identity"
        >
          <span>
            <small>Execution</small>
            <strong title={presentation.label}>{presentation.label}</strong>
          </span>
          <span>
            <small>Subject</small>
            <strong title={modelNames(presentation.subjects)}>
              {modelNames(presentation.subjects)}
            </strong>
          </span>
          <span>
            <small>Judge</small>
            <strong title={modelNames(presentation.judges)}>
              {modelNames(presentation.judges)}
            </strong>
          </span>
          <span>
            <small>Completed</small>
            <strong>{formatDate(presentation.completedAt)}</strong>
          </span>
        </section>
        <div
          className={`latest-first-failure ${isExecutionAttention(presentation) ? 'has-failure' : ''}`}
          aria-live="polite"
        >
          <span className="latest-signal-icon" aria-hidden="true">
            {issue ? '!' : '✓'}
          </span>
          <div>
            <strong>
              {issue
                ? `${categoryLabel(issue.category)} needs investigation`
                : 'No blocking failure in the latest execution'}
            </strong>
            <p>
              {issue
                ? categoryMessage(issue.category, issue.count)
                : 'The result is ready for deeper evidence review.'}
            </p>
          </div>
        </div>
        <div className="mt-4">
          <FailureBreakdown presentation={presentation} />
        </div>
        <div className="latest-health-actions">
          <a
            className="button button-primary"
            href={hashForExecution(execution.id)}
          >
            {issue ? 'Investigate execution' : 'Open execution'}
          </a>
          {execution.workflow_url && (
            <a className="button" href={execution.workflow_url}>
              Open workflow ↗
            </a>
          )}
        </div>
      </article>
      <section
        className="latest-kpi-grid"
        aria-label="Latest execution summary"
      >
        <SummaryKpi
          label="Scenario pass rate"
          value={formatPercent(presentation.passRate)}
          caption={`${formatPercent(presentation.coverage)} report coverage`}
        />
        <SummaryKpi
          label="Reliability events"
          value={String(presentation.breakdown.issues || '—')}
          caption="Affected scenarios, separated by category"
        />
        <SummaryKpi
          label="Model runtime"
          value={formatDuration(presentation.modelRuntimeSeconds)}
          caption={
            presentation.workflowRuntimeSeconds !== null
              ? `${formatDuration(presentation.workflowRuntimeSeconds)} total workflow`
              : 'Workflow duration not reported'
          }
        />
        <SummaryKpi
          label="Models"
          value={String(presentation.subjects.length)}
          caption={`${presentation.judges.length} judge model${presentation.judges.length === 1 ? '' : 's'}`}
        />
      </section>
    </section>
  )
}

function ExecutionHistory({
  executions,
}: {
  executions: DashboardExecutionSummary[]
}) {
  const [query, setQuery] = useState('')
  const [status, setStatus] = useState('all')
  const [event, setEvent] = useState('all')
  const filtered = useMemo(() => {
    const normalized = query.trim().toLowerCase()
    return executions.filter((execution) => {
      if (status !== 'all' && execution.status !== status) return false
      if (event !== 'all' && execution.event !== event) return false
      if (!normalized) return true
      return [
        execution.label,
        execution.id,
        execution.run_id,
        execution.completed_at,
        execution.source?.sha,
      ]
        .filter(Boolean)
        .join(' ')
        .toLowerCase()
        .includes(normalized)
    })
  }, [event, executions, query, status])
  return (
    <section
      className="panel executions-panel"
      aria-labelledby="executions-heading"
    >
      <div className="panel-heading executions-heading">
        <div>
          <div className="section-kicker">Recent executions</div>
          <h2 id="executions-heading">Recent executions</h2>
        </div>
        <span className="coverage-note">
          {filtered.length} of {executions.length} executions
        </span>
      </div>
      <section className="table-filters" aria-label="Execution filters">
        <label className="search-field">
          <span className="visually-hidden">Search executions</span>
          <input
            type="search"
            placeholder="Search label, run, or date"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
        <label>
          <span className="visually-hidden">Filter by status</span>
          <select
            value={status}
            onChange={(event) => setStatus(event.target.value)}
          >
            <option value="all">All statuses</option>
            <option value="passed">Passed</option>
            <option value="hard_gate_failed">Hard gate failed</option>
            <option value="technical_failed">Technical failure</option>
            <option value="infra_failed">Infrastructure failure</option>
            <option value="incomplete">Incomplete</option>
            <option value="cancelled">Cancelled</option>
            <option value="running">Running</option>
          </select>
        </label>
        <label>
          <span className="visually-hidden">Filter by trigger</span>
          <select
            value={event}
            onChange={(event) => setEvent(event.target.value)}
          >
            <option value="all">All triggers</option>
            <option value="schedule">Scheduled</option>
            <option value="workflow_dispatch">Manual</option>
            <option value="local">Local</option>
          </select>
        </label>
      </section>
      <div className="table-wrap">
        <table className="execution-table">
          <thead>
            <tr>
              <th scope="col">Execution</th>
              <th scope="col">Result</th>
              <th scope="col">Subject</th>
              <th scope="col">Scope</th>
              <th scope="col">Outcome</th>
              <th scope="col">Efficiency</th>
              <th scope="col">Evidence</th>
            </tr>
          </thead>
          <tbody>
            {filtered.map((execution) => {
              const presentation = buildExecutionPresentation(execution)
              const status = statusCopy(presentation)
              return (
                <tr key={execution.id}>
                  <td data-label="Execution">
                    <a href={hashForExecution(execution.id)}>
                      {presentation.label}
                    </a>
                    <small className="block text-ink-muted">
                      {formatDate(presentation.completedAt)}
                    </small>
                  </td>
                  <td data-label="Result">
                    <span
                      className={`table-status status-${status.tone.replace('status-', '')}`}
                    >
                      {status.label}
                    </span>
                  </td>
                  <td
                    data-label="Subject"
                    title={modelNames(presentation.subjects)}
                  >
                    {modelNames(presentation.subjects)}
                  </td>
                  <td data-label="Scope">
                    <div className="execution-table-stack">
                      <strong>
                        {presentation.receivedReports ?? '—'}/
                        {presentation.expectedReports ?? '—'}
                      </strong>
                      <small>
                        {formatPercent(presentation.coverage)} coverage
                      </small>
                    </div>
                  </td>
                  <td data-label="Outcome">
                    <div className="execution-table-stack">
                      <strong>{formatPercent(presentation.passRate)}</strong>
                      <small>
                        {presentation.primaryIssue
                          ? categoryMessage(
                              presentation.primaryIssue.category,
                              presentation.primaryIssue.count,
                            )
                          : 'No blocking events'}
                      </small>
                    </div>
                  </td>
                  <td data-label="Efficiency">
                    <div className="execution-table-stack">
                      <strong>
                        {formatDuration(presentation.modelRuntimeSeconds)}
                      </strong>
                      <small>
                        {presentation.execution.totals?.total_tokens
                          ? `${presentation.execution.totals.total_tokens.toLocaleString()} tokens`
                          : 'Tokens not reported'}
                      </small>
                    </div>
                  </td>
                  <td data-label="Evidence">
                    <span
                      className={`data-badge data-${execution.availability ?? 'unavailable'}`}
                    >
                      {execution.availability === 'full'
                        ? 'Diagnostic detail'
                        : execution.availability === 'aggregate'
                          ? 'Aggregate'
                          : 'No report'}
                    </span>
                  </td>
                </tr>
              )
            })}
          </tbody>
        </table>
        {filtered.length === 0 && (
          <p className="table-empty">No executions match these filters.</p>
        )}
      </div>
    </section>
  )
}

function LocalComparisonCard() {
  return (
    <article
      className="overview-intelligence-card overview-comparison-card"
      aria-labelledby="local-comparison-heading"
    >
      <div className="overview-card-main">
        <div className="overview-card-heading">
          <div>
            <div className="section-kicker">Local comparison</div>
            <h3 id="local-comparison-heading">Focused and explicit</h3>
          </div>
          <span className="overview-card-state overview-card-state-neutral">
            Ready
          </span>
        </div>
        <p className="overview-card-copy">
          Choose only the tests relevant to the current change. Capture one
          baseline run, then rerun the same cases and seeds after editing the
          Harness.
        </p>
        <section
          className="overview-comparison-status"
          aria-label="Local comparison strategy"
        >
          <div className="overview-comparison-dimension">
            <span>Selected scope</span>
            <strong>Small and explicit</strong>
            <small>
              The comparison is bound to the developer&apos;s chosen tests, not
              the full catalog.
            </small>
          </div>
          <div className="overview-comparison-dimension">
            <span>Functional decision</span>
            <strong>After 1 paired run</strong>
            <small>
              New deliverable, structural, or technical failures are immediately
              actionable.
            </small>
          </div>
          <div className="overview-comparison-dimension">
            <span>Efficiency</span>
            <strong>Directional at n=1</strong>
            <small>
              Cost and wall-time changes inform the developer but do not block
              the local result.
            </small>
          </div>
          <div className="overview-comparison-dimension">
            <span>More confidence</span>
            <strong>Targeted rerun</strong>
            <small>
              Repeat only a suspicious test instead of rerunning the complete
              selection.
            </small>
          </div>
        </section>
        <section className="overview-versions" aria-label="Comparison sequence">
          <div className="overview-version">
            <span>Baseline</span>
            <strong>1 run per selected test</strong>
          </div>
          <span aria-hidden="true">→</span>
          <div className="overview-version">
            <span>Candidate</span>
            <strong>Same comparison plan</strong>
          </div>
        </section>
      </div>
      <div className="overview-card-foot">
        <span>One focused pair is enough to start</span>
        <a className="button button-secondary" href={hashForPlans()}>
          View local plans <span aria-hidden="true">→</span>
        </a>
      </div>
    </article>
  )
}

function CapabilityView({
  latest,
  compact = false,
}: {
  latest: ExecutionPresentation | null
  compact?: boolean
}) {
  const capability = capabilityData(latest?.execution)
  const qualifiedCases = capability.tiers.flatMap((tier) =>
    tier.qualified_case_ids.map((caseId) => ({ tier: tier.tier, caseId })),
  )
  if (compact) {
    const tierLookup = new Map(
      capability.tiers.map((tier) => [tier.tier.toLowerCase(), tier]),
    )
    return (
      <article
        className="overview-intelligence-card overview-capability-card"
        aria-labelledby="capability-heading"
      >
        <div className="overview-card-main">
          <div className="overview-card-heading">
            <div>
              <div className="section-kicker">Accumulated evidence</div>
              <h3 id="capability-heading">
                Confidence grows in the background
              </h3>
            </div>
            <span className="overview-card-state overview-card-state-neutral">
              Historical
            </span>
          </div>
          <p className="overview-card-copy">
            Compatible local reruns strengthen history without becoming a
            prerequisite. Five samples support local repeatability; p95 appears
            only after twenty complete samples.
          </p>
          <div className="overview-capability-highlight">
            <span>Highest repeatable tier</span>
            <strong>
              {capabilityTierLabel(capability.highest_repeatable_tier)}
            </strong>
          </div>
          <section
            className="overview-tier-list"
            aria-label="Complexity tier evidence"
          >
            {capabilityTierDefinitions.map(({ key, label }) => {
              const tier = tierLookup.get(key)
              const qualified = tier?.qualified_case_ids.length ?? 0
              const state =
                qualified > 0 ? 'qualified' : tier ? 'observed' : 'empty'
              const detail =
                qualified > 0
                  ? `${qualified} qualified case${qualified === 1 ? '' : 's'}`
                  : tier
                    ? 'Observed, not qualified'
                    : 'No evidence'
              return (
                <div
                  className={`overview-tier overview-tier-${state}`}
                  key={key}
                >
                  <strong>{label}</strong>
                  <span>{detail}</span>
                </div>
              )
            })}
          </section>
        </div>
        <div className="overview-card-foot">
          <span>Secondary to the local decision</span>
          <a href={hashForWorkspace('capability')}>View evidence →</a>
        </div>
      </article>
    )
  }
  return (
    <section
      className="panel capability-panel"
      aria-labelledby="capability-heading"
    >
      <div className="panel-heading">
        <div>
          <div className="section-kicker">Evidence frontier</div>
          <h2 id="capability-heading">Accumulated evidence</h2>
          <p className="trend-description">
            Capability is established from versioned tests and repeated
            execution evidence.
          </p>
        </div>
      </div>
      <div className="capability-summary">
        <article className="capability-card capability-primary">
          <div className="kpi-label">Highest repeatable tier</div>
          <strong>
            {capabilityTierLabel(capability.highest_repeatable_tier)}
          </strong>
          <small>
            Qualified per case; an unqualified case does not prove its tier.
          </small>
        </article>
        <article className="capability-card">
          <div className="kpi-label">Qualified cases</div>
          <strong>{qualifiedCases.length || '—'}</strong>
          <small>Cases with repeatable local evidence</small>
        </article>
        <article className="capability-card">
          <div className="kpi-label">Coverage</div>
          <strong>{latest ? formatPercent(latest.coverage) : '—'}</strong>
          <small>Report completeness</small>
        </article>
        <article className="capability-card">
          <div className="kpi-label">Observed scenarios</div>
          <strong>{latest?.expectedReports ?? '—'}</strong>
          <small>Expected reports in the latest execution</small>
        </article>
      </div>
      <div className="capability-qualifications">
        <div className="section-kicker">Qualified cases by tier</div>
        {qualifiedCases.length > 0 ? (
          <ul>
            {qualifiedCases.map(({ tier, caseId }) => (
              <li key={`${tier}:${caseId}`}>
                <span>{tier}</span>
                <strong>{caseId}</strong>
              </li>
            ))}
          </ul>
        ) : (
          <p>No case has repeatable evidence in the latest execution.</p>
        )}
      </div>
      <div className="comparison-bar">
        <div>
          <strong>Compare results by test and scenario version</strong>
          <span>
            Scores remain attached to one scenario contract and evaluated
            cohort.
          </span>
        </div>
        <a className="button" href={hashForWorkspace('tests')}>
          Open versioned tests
        </a>
      </div>
    </section>
  )
}

function OverviewDecisionModel({
  latest,
}: {
  latest: ExecutionPresentation | null
}) {
  return (
    <section
      className="overview-decision-model"
      aria-labelledby="decision-model-heading"
    >
      <header className="overview-decision-heading">
        <div>
          <div className="section-kicker">Decision model</div>
          <h2 id="decision-model-heading">Fast locally, confident over time</h2>
        </div>
        <p>
          A local comparison starts with one run per selected test. Repetition
          is targeted and historical validation never delays the engineering
          loop.
        </p>
      </header>
      <div className="overview-intelligence-grid">
        <LocalComparisonCard />
        <CapabilityView latest={latest} compact />
      </div>
    </section>
  )
}

type CapabilityTierView = {
  tier: string
  qualified_case_ids: string[]
}

const capabilityTierDefinitions = [
  { key: 'l0_atomic', label: 'L0' },
  { key: 'l1_sequential', label: 'L1' },
  { key: 'l2_stateful', label: 'L2' },
  { key: 'l3_concurrent', label: 'L3' },
  { key: 'l4_coordinated', label: 'L4' },
  { key: 'l5_adaptive', label: 'L5' },
]

function capabilityTierLabel(value: string | null) {
  if (!value) return 'Not repeatable yet'
  const normalized = value.toLowerCase()
  return (
    capabilityTierDefinitions.find(
      (definition) => definition.key === normalized,
    )?.label ?? value
  )
}

function capabilityData(execution: DashboardExecutionSummary | undefined): {
  highest_repeatable_tier: string | null
  tiers: CapabilityTierView[]
} {
  const value = execution?.capability
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    return { highest_repeatable_tier: null, tiers: [] }
  }
  const source = value as JsonObject
  const highest =
    typeof source.highest_repeatable_tier === 'string'
      ? source.highest_repeatable_tier
      : null
  const tiers = Array.isArray(source.tiers)
    ? source.tiers.flatMap((item) => {
        if (!item || typeof item !== 'object' || Array.isArray(item)) return []
        const tier = item as JsonObject
        if (typeof tier.tier !== 'string') return []
        const qualified_case_ids = Array.isArray(tier.qualified_case_ids)
          ? tier.qualified_case_ids.filter(
              (caseId): caseId is string => typeof caseId === 'string',
            )
          : []
        return [{ tier: tier.tier, qualified_case_ids }]
      })
    : []
  return { highest_repeatable_tier: highest, tiers }
}

export function OverviewPage({ activeView }: { activeView: WorkspaceView }) {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [executions, setExecutions] = useState<DashboardExecutionSummary[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [runnerOpen, setRunnerOpen] = useState(false)

  const load = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const nextBridge = bridge ?? (await getDashboardDataBridge())
      setBridge(nextBridge)
      const manifest = await nextBridge.listExecutions({ limit: 100 })
      setExecutions(manifest.executions ?? [])
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setLoading(false)
    }
  }, [bridge])

  useEffect(() => {
    void load()
  }, [load])

  const latest = executions[0]
    ? buildExecutionPresentation(executions[0])
    : null
  return (
    <>
      <a className="skip-link" href="#main">
        Skip to execution dashboard
      </a>
      <div className="ambient ambient-one" aria-hidden="true" />
      <div className="ambient ambient-two" aria-hidden="true" />
      <header className="topbar">
        <a
          className="brand"
          href="https://github.com/iii-hq/harness-e2e"
          aria-label="iii Harness E2E"
        >
          <span className="brand-copy">
            <strong>iii</strong>
            <span>Harness benchmarks</span>
          </span>
        </a>
        <nav className="topbar-actions" aria-label="Dashboard actions">
          {bridge?.mode === 'local' && (
            <a className="button button-primary" href={hashForNewPlan()}>
              ＋ New local plan
            </a>
          )}
          {bridge?.mode === 'local' && (
            <button
              className="button button-secondary"
              type="button"
              onClick={() => setRunnerOpen(true)}
            >
              Quick execution
            </button>
          )}
          <a
            className="button button-secondary"
            href={hashForCoverage()}
            data-mobile-label="Coverage"
          >
            Coverage
          </a>
          <a
            className="button button-secondary"
            href={hashForWorkspace('tests')}
            data-mobile-label="Tests"
          >
            Tests
          </a>
          <ThemeToggle />
        </nav>
      </header>
      <main id="main" className="page-shell overview-shell">
        <section className="page-heading" aria-labelledby="page-title">
          <div>
            <div className="eyebrow">
              <span className="live-dot" aria-hidden="true" />
              Harness E2E
            </div>
            <h1 id="page-title">Harness evidence</h1>
            <p>
              Know what ran, what passed, and what requires attention before
              trusting a benchmark.
            </p>
          </div>
          <div className="sync-block">
            <span>Last published</span>
            <time dateTime={latest?.completedAt}>
              {latest
                ? formatDate(latest.completedAt)
                : loading
                  ? 'Loading…'
                  : 'Waiting for data'}
            </time>
          </div>
        </section>
        {error && (
          <section className="empty-state" role="alert">
            <div className="empty-icon" aria-hidden="true">
              !
            </div>
            <h2>Dashboard data unavailable</h2>
            <p>{error}</p>
            <button
              className="button"
              type="button"
              onClick={() => void load()}
            >
              Retry
            </button>
          </section>
        )}
        {!error && loading && (
          <section className="latest-evidence" aria-busy="true">
            <article className="panel latest-health">
              <div className="h-8 w-48 animate-pulse rounded bg-panel-soft" />
              <div className="mt-8 h-12 max-w-xl animate-pulse rounded bg-panel-soft" />
            </article>
          </section>
        )}
        {!error && !loading && (
          <div id="overview-content">
            {executions.length === 0 ? (
              <section className="empty-state">
                <div className="empty-icon" aria-hidden="true">
                  ⌁
                </div>
                <h2>No executions published</h2>
                <p>The next Harness E2E workflow will appear here.</p>
              </section>
            ) : (
              <>
                {activeView === 'overview' && latest && (
                  <>
                    <LatestExecution presentation={latest} />
                    <OverviewDecisionModel latest={latest} />
                    <ExecutionHistory executions={executions} />
                  </>
                )}
                {activeView === 'capability' && (
                  <CapabilityView latest={latest} />
                )}
                {activeView === 'executions' && (
                  <ExecutionHistory executions={executions} />
                )}
              </>
            )}
          </div>
        )}
      </main>
      <LocalRunnerDialog
        bridge={bridge}
        open={runnerOpen}
        onClose={() => setRunnerOpen(false)}
        onCompleted={() => void load()}
      />
      <footer>
        <span>Harness E2E · execution evidence</span>
        <a href="https://github.com/iii-hq/harness-e2e">
          Suite documentation <span aria-hidden="true">↗</span>
        </a>
      </footer>
    </>
  )
}
