import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  DashboardPageActions,
  dashboardHeaderActionClassName,
} from '@/components/DashboardPageActions'
import {
  hashForNewPlan,
  hashForPlan,
  hashForWorkspace,
} from '@/hooks/use-hash-route'
import {
  type DashboardDataBridge,
  type DashboardExecutionSummary,
  getDashboardDataBridge,
  type LocalPlan,
} from '@/lib/dashboard-data-source'
import {
  buildPlanComparison,
  formatPlanMetricDelta,
  formatPlanMetricValue,
  loadExecutionSummaries,
  metricById,
  PLAN_CORE_METRICS,
  type PlanComparison,
  type PlanMetricComparison,
  type PlanMetricId,
  type PlanVerdict,
} from '@/lib/plan-comparison'

type PlanFilter = 'all' | 'needs_action' | 'running' | 'compared' | 'regressed'

function statePresentation(plan: LocalPlan) {
  if (
    plan.locked &&
    plan.state === 'draft' &&
    plan.incomplete_execution_ids.length
  ) {
    return {
      label: 'Retry available',
      tone: 'status-incomplete',
      detail: 'The last baseline attempt was incomplete.',
    }
  }
  switch (plan.state) {
    case 'baseline_running':
      return {
        label: 'Baseline running',
        tone: 'status-incomplete',
        detail: 'Capturing the official baseline.',
      }
    case 'baseline_ready':
      return {
        label: 'Ready for candidate',
        tone: 'status-neutral',
        detail: 'Baseline captured; run the same scope after your change.',
      }
    case 'candidate_running':
      return {
        label: 'Candidate running',
        tone: 'status-incomplete',
        detail: 'Comparing the locked scope against the baseline.',
      }
    case 'comparison_ready':
      return {
        label: 'Comparison available',
        tone: 'status-neutral',
        detail: 'Candidate results are available for review.',
      }
    default:
      return {
        label: 'Draft',
        tone: 'status-incomplete',
        detail: 'Select the scope, then capture a baseline.',
      }
  }
}

function matchesFilter(
  plan: LocalPlan,
  filter: PlanFilter,
  comparison: PlanComparison | null,
) {
  if (filter === 'all') return true
  if (filter === 'needs_action')
    return plan.state === 'draft' || plan.state === 'baseline_ready'
  if (filter === 'running')
    return (
      plan.state === 'baseline_running' || plan.state === 'candidate_running'
    )
  if (filter === 'regressed') return comparison?.verdict === 'regressed'
  return plan.candidate_execution_ids.length > 0
}

function formatDate(value: string) {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return 'Unknown date'
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: 'medium',
    timeStyle: 'short',
  }).format(date)
}

function modelLabel(plan: LocalPlan) {
  return (
    [plan.provider, plan.model].filter(Boolean).join(' / ') || 'Model not set'
  )
}

const verdictPresentation: Record<
  PlanVerdict,
  { label: string; tone: string }
> = {
  improved: { label: 'Improved', tone: 'status-pass' },
  stable: { label: 'Stable', tone: 'status-neutral' },
  regressed: { label: 'Regressed', tone: 'status-fail' },
  inconclusive: { label: 'Inconclusive', tone: 'status-incomplete' },
}

function ComparisonMetric({ metric }: { metric: PlanMetricComparison }) {
  return (
    <div className="plan-list-comparison-metric">
      <span>{metric.label}</span>
      <div>
        <strong>{formatPlanMetricValue(metric, 'baseline')}</strong>
        <i aria-hidden="true">→</i>
        <strong>{formatPlanMetricValue(metric, 'candidate')}</strong>
      </div>
      <small className={`metric-tone-${metric.tone}`}>
        {formatPlanMetricDelta(metric)}
      </small>
    </div>
  )
}

function comparisonRange(comparison: PlanComparison, id: PlanMetricId) {
  const metric = metricById(comparison, id)
  return metric
    ? `${formatPlanMetricValue(metric, 'baseline')} → ${formatPlanMetricValue(metric, 'candidate')}`
    : 'Not reported'
}

export function PlanComparisonSummary({
  plan,
  baseline,
  candidate,
}: {
  plan: LocalPlan
  baseline: DashboardExecutionSummary | null
  candidate: DashboardExecutionSummary | null
}) {
  const candidateCount = plan.candidate_execution_ids.length
  if (candidateCount === 0) {
    const snapshot = baseline ? buildPlanComparison(baseline, baseline) : null
    return (
      <section className="plan-list-comparison plan-list-comparison-empty">
        <div>
          <span className="section-kicker">
            {baseline ? 'Baseline snapshot' : 'Comparison not started'}
          </span>
          <strong>
            {baseline
              ? 'Run a candidate to measure evolution'
              : 'Capture a baseline before comparing results'}
          </strong>
        </div>
        {snapshot && (
          <div className="plan-list-baseline-metrics">
            {PLAN_CORE_METRICS.map((id) => {
              const metric = metricById(snapshot, id)
              return metric ? (
                <span key={id}>
                  <small>{metric.label}</small>
                  <b>{formatPlanMetricValue(metric, 'baseline')}</b>
                </span>
              ) : null
            })}
          </div>
        )}
      </section>
    )
  }

  const comparison = buildPlanComparison(baseline, candidate)
  const verdict = verdictPresentation[comparison.verdict]
  return (
    <section
      className={`plan-list-comparison plan-list-comparison-${comparison.verdict}`}
      aria-label="Latest candidate compared with baseline"
    >
      <header>
        <div>
          <span className="section-kicker">Latest candidate vs baseline</span>
          <strong>
            Candidate #{candidateCount}
            {candidate?.completed_at
              ? ` · ${formatDate(candidate.completed_at)}`
              : ''}
          </strong>
        </div>
        <span className={`status-pill ${verdict.tone}`}>{verdict.label}</span>
      </header>
      {comparison.metrics.length > 0 ? (
        <div className="plan-list-comparison-grid">
          {PLAN_CORE_METRICS.map((id) => {
            const metric = metricById(comparison, id)
            return metric ? <ComparisonMetric key={id} metric={metric} /> : null
          })}
        </div>
      ) : (
        <p>{comparison.detail}</p>
      )}
      <footer>
        <span>
          Hard gates{' '}
          <strong>{comparisonRange(comparison, 'hard_gates')}</strong>
        </span>
        <span>
          Technical failures{' '}
          <strong>{comparisonRange(comparison, 'technical_failures')}</strong>
        </span>
        <span>
          Cost <strong>{comparisonRange(comparison, 'cost')}</strong>
        </span>
      </footer>
    </section>
  )
}

function PlanListHeader() {
  return (
    <DashboardPageActions
      active="plans"
      actionsLabel="Local plan actions"
      actions={
        <a
          className={dashboardHeaderActionClassName({ primary: true })}
          href={hashForNewPlan()}
          aria-label="New local plan"
        >
          New plan
        </a>
      }
    />
  )
}

function PlanCard({
  plan,
  executionSummaries,
}: {
  plan: LocalPlan
  executionSummaries: Record<string, DashboardExecutionSummary>
}) {
  const presentation = statePresentation(plan)
  const candidateCount = plan.candidate_execution_ids.length
  const scopeLabel = `${plan.scenarios.length} test${plan.scenarios.length === 1 ? '' : 's'} · ${plan.runs} run${plan.runs === 1 ? '' : 's'} each`
  const latestCandidateId = plan.candidate_execution_ids.at(-1) ?? ''
  const baseline = plan.baseline_execution_id
    ? (executionSummaries[plan.baseline_execution_id] ?? null)
    : null
  const candidate = latestCandidateId
    ? (executionSummaries[latestCandidateId] ?? null)
    : null
  const comparison = candidateCount
    ? buildPlanComparison(baseline, candidate)
    : null

  return (
    <article
      className={`plan-list-card${comparison ? ` plan-list-card-${comparison.verdict}` : ''}`}
    >
      <header className="plan-list-card-header">
        <div className="plan-list-card-title">
          <div className="plan-list-card-state">
            <span className={`status-pill ${presentation.tone}`}>
              {presentation.label}
            </span>
            {plan.locked && (
              <span className="plan-list-lock">Scope locked</span>
            )}
          </div>
          <h3>{plan.label || 'Untitled local plan'}</h3>
          <p>{plan.purpose || presentation.detail}</p>
        </div>
        <a className="button button-secondary" href={hashForPlan(plan.id)}>
          Open plan <span aria-hidden="true">→</span>
        </a>
      </header>
      <div className="plan-list-card-meta">
        <div>
          <span>Scope</span>
          <strong>{scopeLabel}</strong>
        </div>
        <div>
          <span>Model</span>
          <strong>{modelLabel(plan)}</strong>
        </div>
        <div>
          <span>Lifecycle</span>
          <strong>
            {plan.baseline_execution_id
              ? 'Baseline captured'
              : 'Baseline pending'}
            {candidateCount > 0
              ? ` · ${candidateCount} candidate${candidateCount === 1 ? '' : 's'}`
              : ''}
          </strong>
        </div>
      </div>
      <PlanComparisonSummary
        plan={plan}
        baseline={baseline}
        candidate={candidate}
      />
      <footer className="plan-list-card-footer">
        <span>Updated {formatDate(plan.updated_at)}</span>
        <code>{plan.id}</code>
      </footer>
    </article>
  )
}

export function PlansPage() {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [plans, setPlans] = useState<LocalPlan[]>([])
  const [executionSummaries, setExecutionSummaries] = useState<
    Record<string, DashboardExecutionSummary>
  >({})
  const [query, setQuery] = useState('')
  const [filter, setFilter] = useState<PlanFilter>('all')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [comparisonError, setComparisonError] = useState<string | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    setError(null)
    setComparisonError(null)
    try {
      const next = await getDashboardDataBridge()
      setBridge(next)
      if (next.mode !== 'local') {
        setPlans([])
        return
      }
      const response = await next.listPlans()
      const orderedPlans = [...response.plans].sort((left, right) =>
        right.updated_at.localeCompare(left.updated_at),
      )
      setPlans(orderedPlans)
      const executionIds = orderedPlans.flatMap((plan) => [
        plan.baseline_execution_id ?? '',
        plan.candidate_execution_ids.at(-1) ?? '',
      ])
      try {
        setExecutionSummaries(
          await loadExecutionSummaries(next.listExecutions, executionIds),
        )
      } catch (cause) {
        setExecutionSummaries({})
        setComparisonError(
          cause instanceof Error ? cause.message : String(cause),
        )
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  const filteredPlans = useMemo(() => {
    const normalized = query.trim().toLowerCase()
    return plans.filter((plan) => {
      const candidateId = plan.candidate_execution_ids.at(-1) ?? ''
      const comparison = candidateId
        ? buildPlanComparison(
            plan.baseline_execution_id
              ? executionSummaries[plan.baseline_execution_id]
              : null,
            executionSummaries[candidateId],
          )
        : null
      if (!matchesFilter(plan, filter, comparison)) return false
      if (!normalized) return true
      return [plan.label, plan.purpose, plan.id, ...plan.scenario_ids]
        .join(' ')
        .toLowerCase()
        .includes(normalized)
    })
  }, [executionSummaries, filter, plans, query])

  const activeCount = plans.filter((plan) =>
    ['baseline_running', 'candidate_running'].includes(plan.state),
  ).length
  const comparedCount = plans.filter(
    (plan) => plan.candidate_execution_ids.length > 0,
  ).length
  const needsActionCount = plans.filter(
    (plan) => plan.state === 'draft' || plan.state === 'baseline_ready',
  ).length
  const regressionCount = plans.filter((plan) => {
    const candidateId = plan.candidate_execution_ids.at(-1) ?? ''
    if (!candidateId || !plan.baseline_execution_id) return false
    return (
      buildPlanComparison(
        executionSummaries[plan.baseline_execution_id],
        executionSummaries[candidateId],
      ).verdict === 'regressed'
    )
  }).length

  return (
    <>
      <PlanListHeader />
      <div className="page-shell overview-shell plans-shell">
        <section className="page-heading" aria-labelledby="plans-title">
          <div>
            <div className="eyebrow">
              <span className="live-dot" aria-hidden="true" />
              Local comparison workspace
            </div>
            <h1 id="plans-title">Choose a plan to continue</h1>
            <p>
              Every plan is a small, explicit contract: capture one baseline,
              then rerun the same frozen scope after your Harness change.
            </p>
          </div>
          <div className="sync-block">
            <span>Plans in this dashboard</span>
            <strong>
              {bridge?.mode === 'local' ? plans.length : 'Local only'}
            </strong>
          </div>
        </section>

        {bridge?.mode !== 'local' && !loading ? (
          <section className="panel plans-view-only" role="status">
            <div>
              <div className="section-kicker">Local plans</div>
              <h2>Available only in the local dashboard</h2>
              <p>
                Published and view-only reports keep historical evidence, but do
                not expose local plan state or controls.
              </p>
            </div>
            <a className="button button-secondary" href={hashForWorkspace()}>
              Back to overview
            </a>
          </section>
        ) : (
          <>
            <section className="plans-how-it-works" aria-label="Plan workflow">
              <div>
                <span>01</span>
                <strong>Pick the change scope</strong>
                <small>Only the tests that matter for this edit.</small>
              </div>
              <div>
                <span>02</span>
                <strong>Capture baseline</strong>
                <small>The plan freezes cases, seeds and policy.</small>
              </div>
              <div>
                <span>03</span>
                <strong>Run candidates</strong>
                <small>
                  Review objective gates and directional efficiency.
                </small>
              </div>
              <a className="button button-primary" href={hashForNewPlan()}>
                ＋ New local plan
              </a>
            </section>

            <section className="plans-summary-grid" aria-label="Plan summary">
              <article className="panel plans-summary-card">
                <span>Needs action</span>
                <strong>{needsActionCount}</strong>
                <small>Drafts or plans ready for a candidate</small>
              </article>
              <article className="panel plans-summary-card">
                <span>Running</span>
                <strong>{activeCount}</strong>
                <small>Baseline or candidate runs active now</small>
              </article>
              <article className="panel plans-summary-card">
                <span>Compared</span>
                <strong>{comparedCount}</strong>
                <small>Plans with completed candidate evidence</small>
              </article>
              <article className="panel plans-summary-card plans-summary-regressions">
                <span>Objective regressions</span>
                <strong>{regressionCount}</strong>
                <small>Latest candidates worse than their baselines</small>
              </article>
            </section>

            <section
              className="panel plans-list-panel"
              aria-labelledby="plans-list-title"
            >
              <div className="panel-heading plans-list-heading">
                <div>
                  <div className="section-kicker">Plan history</div>
                  <h2 id="plans-list-title">All local plans</h2>
                </div>
                <span className="coverage-note">
                  {loading ? 'Loading…' : `${filteredPlans.length} shown`}
                </span>
              </div>
              <div className="plans-list-toolbar">
                <label className="search-field">
                  <span className="visually-hidden">Search plans</span>
                  <input
                    type="search"
                    value={query}
                    placeholder="Search label, purpose or test"
                    onChange={(event) => setQuery(event.target.value)}
                  />
                </label>
                <label className="plans-filter-field">
                  <span className="visually-hidden">Filter plans</span>
                  <select
                    value={filter}
                    onChange={(event) =>
                      setFilter(event.target.value as PlanFilter)
                    }
                  >
                    <option value="all">All plans</option>
                    <option value="needs_action">Needs action</option>
                    <option value="running">In progress</option>
                    <option value="compared">Compared</option>
                    <option value="regressed">Objective regressions</option>
                  </select>
                </label>
                <button
                  className="button button-secondary"
                  type="button"
                  onClick={() => void load()}
                >
                  Refresh
                </button>
              </div>
              {comparisonError && !error && (
                <div className="plans-comparison-notice" role="status">
                  Execution summaries could not be loaded. Plans remain
                  available, but comparison metrics are marked unavailable.{' '}
                  <span>{comparisonError}</span>
                </div>
              )}
              {error ? (
                <div className="plans-empty" role="alert">
                  <strong>Plans could not be loaded</strong>
                  <p>{error}</p>
                  <button
                    className="button button-secondary"
                    type="button"
                    onClick={() => void load()}
                  >
                    Try again
                  </button>
                </div>
              ) : loading ? (
                <p className="table-empty plans-loading">
                  Loading local plans…
                </p>
              ) : filteredPlans.length > 0 ? (
                <div className="plans-list">
                  {filteredPlans.map((plan) => (
                    <PlanCard
                      key={plan.id}
                      plan={plan}
                      executionSummaries={executionSummaries}
                    />
                  ))}
                </div>
              ) : (
                <div className="plans-empty">
                  <strong>
                    {plans.length === 0
                      ? 'No local plans yet'
                      : 'No plans match this filter'}
                  </strong>
                  <p>
                    {plans.length === 0
                      ? 'Start with only the tests relevant to the Harness change in front of you.'
                      : 'Try another status or search term.'}
                  </p>
                  {plans.length === 0 && (
                    <a
                      className="button button-primary"
                      href={hashForNewPlan()}
                    >
                      Create the first plan
                    </a>
                  )}
                </div>
              )}
            </section>
          </>
        )}
      </div>
    </>
  )
}
