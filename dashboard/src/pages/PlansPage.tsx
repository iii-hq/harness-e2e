import { useCallback, useEffect, useMemo, useState } from 'react'
import { ThemeToggle } from '@/components/ThemeToggle'
import {
  hashForNewPlan,
  hashForPlan,
  hashForWorkspace,
} from '@/hooks/use-hash-route'
import {
  type DashboardDataBridge,
  getDashboardDataBridge,
  type LocalPlan,
} from '@/lib/dashboard-data-source'

type PlanFilter = 'all' | 'draft' | 'running' | 'ready'

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
        tone: 'status-pass',
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
        label: 'Comparison ready',
        tone: 'status-pass',
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

function matchesFilter(plan: LocalPlan, filter: PlanFilter) {
  if (filter === 'all') return true
  if (filter === 'draft') return plan.state === 'draft'
  if (filter === 'running')
    return (
      plan.state === 'baseline_running' || plan.state === 'candidate_running'
    )
  return plan.state === 'baseline_ready' || plan.state === 'comparison_ready'
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

function PlanListHeader() {
  return (
    <header className="topbar">
      <a
        className="brand"
        href={hashForWorkspace()}
        aria-label="Harness E2E dashboard"
      >
        <span className="brand-copy">
          <strong>iii</strong>
          <span>Harness benchmarks</span>
        </span>
      </a>
      <nav className="topbar-actions" aria-label="Local plan actions">
        <a
          className="button button-secondary"
          href={hashForWorkspace()}
          data-mobile-label="Overview"
        >
          Overview
        </a>
        <a className="button button-primary" href={hashForNewPlan()}>
          ＋ New local plan
        </a>
        <a
          className="button button-secondary"
          href={hashForWorkspace('tests')}
          data-mobile-label="Tests"
        >
          Test catalog
        </a>
        <ThemeToggle />
      </nav>
    </header>
  )
}

function PlanCard({ plan }: { plan: LocalPlan }) {
  const presentation = statePresentation(plan)
  const candidateCount = plan.candidate_execution_ids.length
  const scopeLabel = `${plan.scenarios.length} test${plan.scenarios.length === 1 ? '' : 's'} · ${plan.runs} run${plan.runs === 1 ? '' : 's'} each`

  return (
    <article className="plan-list-card">
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
      <div className="plan-list-card-progress">
        <span
          className={plan.baseline_execution_id ? 'is-complete' : 'is-current'}
        >
          <b>01</b> Baseline
        </span>
        <i aria-hidden="true">→</i>
        <span className={candidateCount > 0 ? 'is-complete' : 'is-current'}>
          <b>02</b> Candidate
        </span>
      </div>
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
  const [query, setQuery] = useState('')
  const [filter, setFilter] = useState<PlanFilter>('all')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const next = await getDashboardDataBridge()
      setBridge(next)
      if (next.mode !== 'local') {
        setPlans([])
        return
      }
      const response = await next.listPlans()
      setPlans(
        [...response.plans].sort((left, right) =>
          right.updated_at.localeCompare(left.updated_at),
        ),
      )
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
      if (!matchesFilter(plan, filter)) return false
      if (!normalized) return true
      return [plan.label, plan.purpose, plan.id, ...plan.scenario_ids]
        .join(' ')
        .toLowerCase()
        .includes(normalized)
    })
  }, [filter, plans, query])

  const activeCount = plans.filter((plan) =>
    ['baseline_running', 'candidate_running'].includes(plan.state),
  ).length
  const readyCount = plans.filter((plan) =>
    ['baseline_ready', 'comparison_ready'].includes(plan.state),
  ).length
  const needsActionCount = plans.filter(
    (plan) => plan.state === 'draft' || plan.state === 'baseline_ready',
  ).length

  return (
    <>
      <a className="skip-link" href="#plans-main">
        Skip to local plans
      </a>
      <PlanListHeader />
      <main id="plans-main" className="page-shell overview-shell plans-shell">
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
                <span>In progress</span>
                <strong>{activeCount}</strong>
                <small>Baseline or candidate runs active now</small>
              </article>
              <article className="panel plans-summary-card">
                <span>Comparison ready</span>
                <strong>{readyCount}</strong>
                <small>Plans with a captured baseline</small>
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
                    <option value="draft">Drafts</option>
                    <option value="running">In progress</option>
                    <option value="ready">Ready or completed</option>
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
                    <PlanCard key={plan.id} plan={plan} />
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
      </main>
      <footer>
        <span>Harness E2E · local plan workspace</span>
        <a href={hashForWorkspace()}>Back to overview</a>
      </footer>
    </>
  )
}
