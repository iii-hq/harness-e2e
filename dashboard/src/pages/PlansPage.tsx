import { Tabs, TabsList, TabsTrigger } from '@iii-dev/console-ui'
import { useCallback, useEffect, useId, useMemo, useState } from 'react'
import {
  DashboardPageActions,
  dashboardHeaderActionClassName,
} from '@/components/DashboardPageActions'
import { MasterTestProfiles } from '@/components/MasterTestProfiles'
import {
  buttonClassName,
  Callout,
  DataTable,
  DataTableRow,
  EmptyState,
  FilterChip,
  FilterChipGroup,
  Input,
  numericCellClassName,
  type OperationalStatus,
  PageHeader,
  StatusBadge,
} from '@/design-system'
import { hashForNewPlan, hashForPlan } from '@/hooks/use-hash-route'
import {
  type DashboardExecutionSummary,
  getDashboardDataBridge,
  type LocalPlan,
  type MasterTestPlan,
} from '@/lib/dashboard-data-source'
import {
  buildPlanComparison,
  formatPlanMetricValue,
  loadExecutionSummaries,
  metricById,
  type PlanMetricId,
} from '@/lib/plan-comparison'

type PlanFilter = 'all' | 'needs_action' | 'running' | 'compared'

export type PlanStatePresentation = {
  status: OperationalStatus
  label: string
  /** The one action this state offers; it opens the plan at that action. */
  action: string
}

/** Audit P-10 / P-14: one status line per plan, "running" everywhere. */
export function planStatePresentation(plan: LocalPlan): PlanStatePresentation {
  if (
    plan.locked &&
    plan.state === 'draft' &&
    plan.incomplete_execution_ids.length
  ) {
    return {
      status: 'incomplete',
      label: 'retry available',
      action: 'retry baseline',
    }
  }
  switch (plan.state) {
    case 'baseline_running':
      return {
        status: 'running',
        label: 'baseline running',
        action: 'open',
      }
    case 'baseline_ready':
      return {
        status: 'unavailable',
        label: 'ready for candidate',
        action: 'run candidate',
      }
    case 'candidate_running':
      return {
        status: 'running',
        label: 'candidate running',
        action: 'open',
      }
    case 'comparison_ready':
      return {
        status: 'unavailable',
        label: 'comparison available',
        action: 'compare',
      }
    default:
      return {
        status: 'incomplete',
        label: 'draft',
        action: 'run baseline',
      }
  }
}

export function matchesFilter(plan: LocalPlan, filter: PlanFilter) {
  if (filter === 'all') return true
  if (filter === 'needs_action')
    return plan.state === 'draft' || plan.state === 'baseline_ready'
  if (filter === 'running')
    return (
      plan.state === 'baseline_running' || plan.state === 'candidate_running'
    )
  return plan.candidate_execution_ids.length > 0
}

function modelLabel(plan: LocalPlan) {
  return plan.model || 'model not set'
}

function shortDate(value: string) {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return 'unknown'
  return new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' }).format(
    date,
  )
}

export function PlanMetricsCell({
  execution,
  source,
}: {
  execution: DashboardExecutionSummary | null
  source: string
}) {
  if (!execution)
    return (
      <span className="text-xs text-ink-muted" title={source}>
        {source === 'Baseline' ? null : `${source} · `}
        No execution metrics
      </span>
    )
  const snapshot = buildPlanComparison(execution, execution)
  const value = (id: PlanMetricId) => {
    const metric = metricById(snapshot, id)
    if (
      id === 'cost' &&
      metric &&
      metric.baseline !== null &&
      metric.baseline > 0 &&
      metric.baseline < 0.0001
    )
      return '<$0.0001'
    return metric ? formatPlanMetricValue(metric, 'baseline') : 'Not reported'
  }
  const tokens = value('tokens')
  const spend = value('cost')
  const time = value('duration')
  return (
    <span
      className="font-mono text-xs tabular-nums text-ink"
      title={`${source} metrics`}
    >
      <span className="ds-visually-hidden">
        {source} metrics: Tokens {tokens}, spend {spend}, time {time}
      </span>
      <span aria-hidden="true">
        {source === 'Baseline' ? null : (
          <span className="whitespace-nowrap">{source} · </span>
        )}
        <span className="whitespace-nowrap">
          {tokens === 'Not reported'
            ? 'Tokens not reported'
            : `${tokens} tokens`}{' '}
          · {spend} · {time}
        </span>
      </span>
    </span>
  )
}

function HowPlansWork() {
  return (
    <ol className="m-0 grid list-none gap-2 p-0 text-xs leading-5 text-ink-soft sm:grid-cols-3">
      <li>
        <strong className="block text-ink">Pick the change scope</strong>
        Only the tests that matter for this edit.
      </li>
      <li>
        <strong className="block text-ink">Capture the baseline</strong>
        The plan freezes cases, seeds and policy.
      </li>
      <li>
        <strong className="block text-ink">Run candidates</strong>
        Review objective gates and directional efficiency.
      </li>
    </ol>
  )
}

export function PlanRow({
  plan,
  executionSummaries,
}: {
  plan: LocalPlan
  executionSummaries: Record<string, DashboardExecutionSummary>
}) {
  const presentation = planStatePresentation(plan)
  const href = hashForPlan(plan.id)
  const baseline = plan.baseline_execution_id
    ? (executionSummaries[plan.baseline_execution_id] ?? null)
    : null
  const latestCandidateId = plan.candidate_execution_ids.at(-1) ?? ''
  const candidate = latestCandidateId
    ? (executionSummaries[latestCandidateId] ?? null)
    : null
  const running =
    presentation.status === 'running' && plan.last_attempt_id
      ? (executionSummaries[plan.last_attempt_id] ?? null)
      : null
  const metricsExecution =
    presentation.status === 'running'
      ? running
      : latestCandidateId
        ? candidate
        : baseline
  const metricsSource =
    presentation.status === 'running'
      ? 'Current run'
      : latestCandidateId
        ? `Candidate #${plan.candidate_execution_ids.length}`
        : 'Baseline'
  const title = plan.label || 'Untitled local plan'
  return (
    <DataTableRow href={href} data-plan-state={plan.state}>
      <td data-label="Plan">
        <span className="grid gap-1">
          <a
            className="text-ink no-underline hover:underline"
            href={href}
            aria-label={`Open plan ${title}`}
          >
            <strong className="font-sans text-sm font-semibold">{title}</strong>
          </a>
          {plan.purpose ? (
            <span
              className="line-clamp-2 max-w-[24rem] font-sans text-xs leading-5 text-ink-soft"
              title={plan.purpose}
            >
              {plan.purpose}
            </span>
          ) : null}
          <StatusBadge
            status={presentation.status}
            label={presentation.label}
          />
        </span>
      </td>
      <td data-label="Details">
        <span className="grid gap-0.5 text-xs">
          <span className="text-ink">
            {plan.scenarios.length} test{plan.scenarios.length === 1 ? '' : 's'}{' '}
            · {plan.runs} run{plan.runs === 1 ? '' : 's'} each
          </span>
          <span className="font-mono text-ink-muted">{modelLabel(plan)}</span>
        </span>
      </td>
      <td data-label="Reference">
        <span className="mb-1 block text-xs text-ink-muted">Baseline</span>
        <span className="block text-xs text-ink">
          {baseline
            ? shortDate(baseline.completed_at ?? baseline.started_at ?? '')
            : plan.baseline_execution_id
              ? 'Unavailable'
              : 'Not captured'}
        </span>
      </td>
      <td data-label="Metrics">
        <PlanMetricsCell execution={metricsExecution} source={metricsSource} />
      </td>
      <td data-label="Last activity" className={numericCellClassName}>
        <span className="whitespace-nowrap text-xs text-ink-muted">
          {shortDate(plan.updated_at)}
        </span>
      </td>
      <td className="text-right">
        <a
          className={buttonClassName({
            variant: presentation.action === 'open' ? 'quiet' : 'secondary',
            size: 'compact',
          })}
          href={href}
        >
          {presentation.action}
        </a>
      </td>
    </DataTableRow>
  )
}

export function PlansPage() {
  const viewsId = useId()
  const [tab, setTab] = useState<'mine' | 'profiles'>('mine')
  const [plans, setPlans] = useState<LocalPlan[]>([])
  const [masterPlan, setMasterPlan] = useState<MasterTestPlan | null>(null)
  const [executionSummaries, setExecutionSummaries] = useState<
    Record<string, DashboardExecutionSummary>
  >({})
  const [query, setQuery] = useState('')
  const [filter, setFilter] = useState<PlanFilter>('all')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [comparisonError, setComparisonError] = useState<string | null>(null)
  const load = useCallback(async ({ silent = false } = {}) => {
    if (!silent) setLoading(true)
    setError(null)
    setComparisonError(null)
    try {
      const next = await getDashboardDataBridge()
      const response = await next.listPlans()
      setMasterPlan(response.master_plan ?? null)
      const orderedPlans = [...response.plans].sort((left, right) =>
        right.updated_at.localeCompare(left.updated_at),
      )
      setPlans(orderedPlans)
      const executionIds = orderedPlans.flatMap((plan) => [
        plan.baseline_execution_id ?? '',
        plan.candidate_execution_ids.at(-1) ?? '',
        plan.last_attempt_id ?? '',
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
      if (!silent) setLoading(false)
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  const counts = useMemo(() => {
    const count = (candidate: PlanFilter) =>
      plans.filter((plan) => matchesFilter(plan, candidate)).length
    return {
      all: plans.length,
      needs_action: count('needs_action'),
      running: count('running'),
      compared: count('compared'),
    }
  }, [plans])

  // Audit P-13: a list with a running plan refreshes itself.
  useEffect(() => {
    if (counts.running === 0) return
    const timer = window.setInterval(() => void load({ silent: true }), 5_000)
    return () => window.clearInterval(timer)
  }, [counts.running, load])

  const filteredPlans = useMemo(() => {
    const normalized = query.trim().toLowerCase()
    return plans.filter((plan) => {
      if (!matchesFilter(plan, filter)) return false
      if (!normalized) return true
      return [
        plan.label,
        plan.purpose,
        plan.id,
        plan.model,
        plan.provider,
        ...plan.scenario_ids,
      ]
        .join(' ')
        .toLowerCase()
        .includes(normalized)
    })
  }, [filter, plans, query])

  const totalPlans = plans.length
  const totalFiltered = filteredPlans.length
  const filtered = query.trim() !== '' || filter !== 'all'
  const filters: Array<{ id: PlanFilter; label: string }> = [
    { id: 'all', label: 'all' },
    { id: 'needs_action', label: 'needs action' },
    { id: 'running', label: 'running' },
    { id: 'compared', label: 'compared' },
  ]

  return (
    <>
      <DashboardPageActions
        active="plans"
        actionsLabel="Local plan actions"
        actions={
          <a
            className={dashboardHeaderActionClassName({ primary: true })}
            href={hashForNewPlan()}
          >
            new plan
          </a>
        }
      />
      <div className="ds-root page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-16 md:w-[calc(100%_-_3rem)]">
        <PageHeader
          title="plans"
          summary="Configure one execution model per plan, run its coverage and follow the results."
          actions={
            plans.length > 0 ? (
              <details className="group text-xs text-ink-soft">
                <summary className="cursor-pointer list-none font-mono text-ink-muted marker:hidden hover:text-ink">
                  how plans work
                </summary>
                <div className="mt-3 max-w-[48rem]">
                  <HowPlansWork />
                </div>
              </details>
            ) : undefined
          }
        />
        <Tabs
          value={tab}
          onValueChange={(value) => setTab(value as typeof tab)}
        >
          <TabsList
            className="mt-5 flex-wrap"
            style={{ overflow: 'visible' }}
            aria-label="Plan views"
          >
            {(
              [
                ['mine', 'My plans'],
                ['profiles', 'Templates'],
              ] as const
            ).map(([value, label]) => (
              <TabsTrigger
                key={value}
                value={value}
                id={`${viewsId}-${value}`}
                aria-controls={`${viewsId}-panel`}
                icon={false}
              >
                {label}
              </TabsTrigger>
            ))}
          </TabsList>
        </Tabs>
        <div
          id={`${viewsId}-panel`}
          role="tabpanel"
          aria-labelledby={`${viewsId}-${tab}`}
          // biome-ignore lint/a11y/noNoninteractiveTabindex: tab panels must be keyboard-focusable.
          tabIndex={0}
        >
          {masterPlan && tab === 'profiles' ? (
            <>
              <MasterTestProfiles plan={masterPlan} />
              <a
                className={buttonClassName({ variant: 'secondary' })}
                href={hashForNewPlan()}
              >
                Create a custom plan manually
              </a>
            </>
          ) : null}

          {tab === 'profiles' ? (
            !masterPlan ? (
              <EmptyState
                className="mt-6"
                title="No templates available"
                description="Create a custom plan to choose tests and execution settings."
                actions={
                  <a
                    className={buttonClassName({ variant: 'secondary' })}
                    href={hashForNewPlan()}
                  >
                    Create a custom plan
                  </a>
                }
              />
            ) : null
          ) : (
            <>
              <section
                className="mt-5 flex flex-wrap items-center gap-3"
                aria-label="Plan filters"
              >
                <div className="w-full max-w-xs">
                  <Input
                    type="search"
                    value={query}
                    placeholder="Search label, purpose or test…"
                    aria-label="Search plans"
                    onChange={(event) => setQuery(event.target.value)}
                  />
                </div>
                <FilterChipGroup label="Plan state">
                  {filters.map((candidate) => (
                    <FilterChip
                      key={candidate.id}
                      active={filter === candidate.id}
                      count={counts[candidate.id]}
                      onClick={() => setFilter(candidate.id)}
                    >
                      {candidate.label}
                    </FilterChip>
                  ))}
                </FilterChipGroup>
                <span
                  className="ms-auto font-mono text-xs text-ink-muted"
                  aria-live="polite"
                >
                  {loading
                    ? 'loading…'
                    : filtered
                      ? `${totalFiltered} of ${totalPlans} plans`
                      : `${totalPlans} plan${totalPlans === 1 ? '' : 's'}`}
                </span>
              </section>

              {comparisonError && !error ? (
                <div className="mt-4">
                  <Callout tone="warning" title="Execution metrics unavailable">
                    Execution summaries could not be loaded. Plans remain
                    available, but their metrics are marked unavailable.{' '}
                    <span className="font-mono">{comparisonError}</span>
                  </Callout>
                </div>
              ) : null}

              <div className="mt-4">
                {error ? (
                  <EmptyState
                    tone="error"
                    title="Plans could not be loaded"
                    description={error}
                    actions={
                      <button
                        className={buttonClassName({ variant: 'secondary' })}
                        type="button"
                        onClick={() => void load()}
                      >
                        try again
                      </button>
                    }
                  />
                ) : loading ? (
                  <div className="grid gap-2" aria-busy="true" role="status">
                    <span className="ds-visually-hidden">
                      Loading local plans
                    </span>
                    {['first', 'second', 'third'].map((placeholder) => (
                      <div
                        key={placeholder}
                        className="h-16 animate-pulse rounded-[6px] bg-[var(--surface-fill)] motion-reduce:animate-none"
                      />
                    ))}
                  </div>
                ) : totalPlans === 0 ? (
                  <EmptyState
                    title="No local plans yet"
                    description={
                      <span className="grid gap-4">
                        <span>
                          Start with only the tests relevant to the Harness
                          change in front of you.
                        </span>
                        <HowPlansWork />
                      </span>
                    }
                    actions={
                      <a
                        className={buttonClassName({ variant: 'primary' })}
                        href={hashForNewPlan()}
                      >
                        new plan
                      </a>
                    }
                  />
                ) : filteredPlans.length === 0 ? (
                  <EmptyState
                    title="No plans match these filters"
                    description="Try another state or search term."
                    actions={
                      <button
                        className={buttonClassName({
                          variant: 'secondary',
                          size: 'compact',
                        })}
                        type="button"
                        onClick={() => {
                          setQuery('')
                          setFilter('all')
                        }}
                      >
                        clear filters
                      </button>
                    }
                  />
                ) : (
                  <DataTable
                    caption={`Plans, ${filteredPlans.length} of ${plans.length}`}
                    collapse
                    minWidth="64rem"
                  >
                    <thead>
                      <tr>
                        <th scope="col">Plan</th>
                        <th scope="col">Details</th>
                        <th scope="col">Reference</th>
                        <th scope="col">Metrics</th>
                        <th scope="col" className={numericCellClassName}>
                          Last activity
                        </th>
                        <th scope="col">
                          <span className="ds-visually-hidden">Action</span>
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {filteredPlans.map((plan) => (
                        <PlanRow
                          key={plan.id}
                          plan={plan}
                          executionSummaries={executionSummaries}
                        />
                      ))}
                    </tbody>
                  </DataTable>
                )}
              </div>
            </>
          )}
        </div>
      </div>
    </>
  )
}
