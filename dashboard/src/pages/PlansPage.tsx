import { useCallback, useEffect, useMemo, useState } from 'react'
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
  DeltaValue,
  EmptyState,
  FilterChip,
  FilterChipGroup,
  Input,
  numericCellClassName,
  type OperationalStatus,
  PageHeader,
  StatusBadge,
} from '@/design-system'
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
  type MasterTestPlan,
} from '@/lib/dashboard-data-source'
import { formatDate } from '@/lib/execution-view'
import {
  buildPlanComparison,
  formatPlanMetricValue,
  loadExecutionSummaries,
  metricById,
  type PlanComparison,
  type PlanMetricComparison,
  type PlanMetricId,
  type PlanVerdict,
} from '@/lib/plan-comparison'
import { type ProfilePlan, running } from '@/lib/profile-plan'

type PlanFilter = 'all' | 'needs_action' | 'running' | 'compared' | 'regressed'

export type PlanStatePresentation = {
  status: OperationalStatus
  label: string
  detail: string
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
      detail: 'The last baseline attempt was incomplete.',
      action: 'retry baseline',
    }
  }
  switch (plan.state) {
    case 'baseline_running':
      return {
        status: 'running',
        label: 'baseline running',
        detail: 'Capturing the official baseline.',
        action: 'open',
      }
    case 'baseline_ready':
      return {
        status: 'unavailable',
        label: 'ready for candidate',
        detail:
          'No candidate yet. Make the Harness change, then rerun this exact scope.',
        action: 'run candidate',
      }
    case 'candidate_running':
      return {
        status: 'running',
        label: 'candidate running',
        detail: 'Comparing the locked scope against the baseline.',
        action: 'open',
      }
    case 'comparison_ready':
      return {
        status: 'unavailable',
        label: 'comparison available',
        detail: 'Candidate results are available for review.',
        action: 'compare',
      }
    default:
      return {
        status: 'incomplete',
        label: 'draft',
        detail: 'Baseline not captured yet.',
        action: 'run baseline',
      }
  }
}

const verdictStatus: Record<PlanVerdict, OperationalStatus> = {
  improved: 'passed',
  stable: 'unavailable',
  regressed: 'failed',
  inconclusive: 'inconclusive',
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

function matchesProfileFilter(plan: ProfilePlan, filter: PlanFilter) {
  if (filter === 'all') return true
  if (filter === 'running') return running(plan.state)
  if (filter === 'needs_action')
    return (
      !plan.compatible ||
      plan.state === 'draft' ||
      plan.state === 'interrupted' ||
      plan.state === 'cancelled'
    )
  if (filter === 'compared')
    return plan.history.some(
      (execution) =>
        execution.role === 'candidate' && !running(execution.state),
    )
  return false
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

function compact(value: number) {
  return new Intl.NumberFormat('en-US', {
    notation: Math.abs(value) >= 1000 ? 'compact' : 'standard',
    maximumFractionDigits: Math.abs(value) >= 100 ? 0 : 1,
  }).format(value)
}

const CORE_DELTAS: Array<{ id: PlanMetricId; label: string }> = [
  { id: 'pass_rate', label: 'pass' },
  { id: 'quality', label: 'score' },
  { id: 'tokens', label: 'tokens' },
  { id: 'duration', label: 'time' },
]

/**
 * Audit P-02: one rule for colour. The number keeps the ink; the delta is
 * signed and carries the direction; red means an objective regression,
 * amber a worse efficiency figure, no colour no change.
 */
function MetricDelta({
  label,
  metric,
}: {
  label: string
  metric: PlanMetricComparison
}) {
  if (metric.delta === null) return null
  const absolute =
    metric.format === 'percent_points' || metric.format === 'score'
  const value = absolute ? metric.delta : (metric.delta_percent ?? metric.delta)
  const unit =
    metric.format === 'percent_points'
      ? 'pp'
      : metric.format === 'score'
        ? 'pts'
        : metric.delta_percent !== null
          ? '%'
          : ''
  return (
    <span className="inline-flex items-baseline gap-1 whitespace-nowrap">
      <span className="text-ink-muted">{label}</span>
      <DeltaValue
        value={value}
        format={(magnitude) => `${compact(magnitude)}${unit}`}
        betterWhen={
          metric.direction === 'higher'
            ? 'higher'
            : metric.direction === 'lower'
              ? 'lower'
              : 'neither'
        }
      />
    </span>
  )
}

/** The baseline column: the four core figures, only when captured. */
export function PlanBaselineCell({
  plan,
  baseline,
}: {
  plan: LocalPlan
  baseline: DashboardExecutionSummary | null
}) {
  if (!baseline) {
    const presentation = planStatePresentation(plan)
    return (
      <span className="text-ink-muted">
        {presentation.label === 'retry available'
          ? 'incomplete'
          : 'not captured'}
      </span>
    )
  }
  const snapshot = buildPlanComparison(baseline, baseline)
  const value = (id: PlanMetricId) => {
    const metric = metricById(snapshot, id)
    return metric && metric.baseline !== null
      ? formatPlanMetricValue(metric, 'baseline')
      : null
  }
  const pass = value('pass_rate')
  const tokens = value('tokens')
  const duration = value('duration')
  const turns = value('turns')
  return (
    <span className="grid gap-0.5 font-mono text-xs tabular-nums">
      <span className="text-ink">
        {[pass, tokens ? `${tokens} tokens` : null]
          .filter(Boolean)
          .join(' · ') || 'no figures reported'}
      </span>
      {duration || turns ? (
        <span className="text-ink-muted">
          {[duration, turns ? `${turns} turns` : null]
            .filter(Boolean)
            .join(' · ')}
        </span>
      ) : null}
    </span>
  )
}

/**
 * The "latest candidate vs baseline" column: the verdict and the signed
 * core deltas, or the sentence that says why there is nothing to compare.
 */
export function PlanComparisonSummary({
  plan,
  baseline,
  candidate,
  running,
}: {
  plan: LocalPlan
  baseline: DashboardExecutionSummary | null
  candidate: DashboardExecutionSummary | null
  running?: DashboardExecutionSummary | null
}) {
  const presentation = planStatePresentation(plan)
  const candidateCount = plan.candidate_execution_ids.length
  if (presentation.status === 'running') {
    const expected = running?.totals?.expected_reports ?? null
    const received = running?.totals?.received_reports ?? null
    return (
      <span className="grid gap-0.5 text-xs">
        <StatusBadge status="running" label={presentation.label} />
        <span className="text-ink-muted">
          {running?.started_at
            ? `started ${formatDate(running.started_at)}`
            : 'in progress'}
          {expected !== null && received !== null
            ? ` · ${received}/${expected} tests`
            : ''}
        </span>
      </span>
    )
  }
  if (candidateCount === 0) {
    return (
      <span className="block max-w-[26rem] text-xs leading-5 text-ink-muted">
        {baseline ? presentation.detail : '—'}
      </span>
    )
  }
  const comparison = buildPlanComparison(baseline, candidate)
  const deltas = CORE_DELTAS.map(({ id, label }) => {
    const metric = metricById(comparison, id)
    return metric && metric.delta !== null ? (
      <MetricDelta key={id} label={label} metric={metric} />
    ) : null
  }).filter(Boolean)
  return (
    <span className="grid gap-1 text-xs">
      <span className="flex flex-wrap items-center gap-2">
        <StatusBadge
          status={verdictStatus[comparison.verdict]}
          label={comparison.verdict}
        />
        <span className="text-ink-muted">
          candidate #{candidateCount}
          {candidate?.completed_at
            ? ` · ${formatDate(candidate.completed_at)}`
            : ''}
        </span>
      </span>
      {deltas.length > 0 ? (
        <span className="flex flex-wrap gap-x-3 gap-y-1 font-mono tabular-nums">
          {deltas}
        </span>
      ) : (
        <span className="max-w-[26rem] leading-5 text-ink-muted">
          {comparison.detail}
        </span>
      )}
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

function PlanRow({
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
  const title = plan.label || 'Untitled local plan'
  return (
    <DataTableRow href={href} data-plan-state={plan.state}>
      <td data-label="Plan">
        <span className="grid gap-1">
          <StatusBadge
            status={presentation.status}
            label={presentation.label}
          />
          <a
            className="font-mono text-[0.8125rem] font-semibold text-ink no-underline hover:underline"
            href={href}
            aria-label={`Open plan ${title}`}
          >
            {title}
          </a>
          {plan.purpose ? (
            <span
              className="line-clamp-2 max-w-[28rem] text-xs leading-5 text-ink-soft"
              title={plan.purpose}
            >
              {plan.purpose}
            </span>
          ) : null}
        </span>
      </td>
      <td data-label="Scope · model">
        <span className="grid gap-0.5 text-xs">
          <span className="text-ink">
            {plan.scenarios.length} test{plan.scenarios.length === 1 ? '' : 's'}{' '}
            · {plan.runs} run{plan.runs === 1 ? '' : 's'} each
          </span>
          <span className="font-mono text-ink-muted">{modelLabel(plan)}</span>
        </span>
      </td>
      <td data-label="Baseline">
        <PlanBaselineCell plan={plan} baseline={baseline} />
      </td>
      <td data-label="Latest candidate vs baseline">
        <PlanComparisonSummary
          plan={plan}
          baseline={baseline}
          candidate={candidate}
          running={running}
        />
      </td>
      <td data-label="Updated" className={numericCellClassName}>
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
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [tab, setTab] = useState<'mine' | 'profiles'>('mine')
  const [profilePlans, setProfilePlans] = useState<ProfilePlan[]>([])
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
      setBridge(next)
      if (next.mode !== 'local') {
        setPlans([])
        setMasterPlan(null)
        return
      }
      const response = await next.listPlans()
      setMasterPlan(response.master_plan ?? null)
      setProfilePlans(response.profile_plans ?? [])
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

  const comparisonFor = useCallback(
    (plan: LocalPlan) => {
      const candidateId = plan.candidate_execution_ids.at(-1) ?? ''
      if (!candidateId) return null
      return buildPlanComparison(
        plan.baseline_execution_id
          ? executionSummaries[plan.baseline_execution_id]
          : null,
        executionSummaries[candidateId],
      )
    },
    [executionSummaries],
  )

  const counts = useMemo(() => {
    const count = (candidate: PlanFilter) =>
      plans.filter((plan) =>
        matchesFilter(plan, candidate, comparisonFor(plan)),
      ).length +
      profilePlans.filter((plan) => matchesProfileFilter(plan, candidate))
        .length
    return {
      all: plans.length + profilePlans.length,
      needs_action: count('needs_action'),
      running: count('running'),
      compared: count('compared'),
      regressed: count('regressed'),
    }
  }, [comparisonFor, plans, profilePlans])

  // Audit P-13: a list with a running plan refreshes itself.
  useEffect(() => {
    if (counts.running === 0 && !profilePlans.some((p) => running(p.state)))
      return
    const timer = window.setInterval(() => void load({ silent: true }), 5_000)
    return () => window.clearInterval(timer)
  }, [counts.running, load, profilePlans])

  const filteredPlans = useMemo(() => {
    const normalized = query.trim().toLowerCase()
    return plans.filter((plan) => {
      if (!matchesFilter(plan, filter, comparisonFor(plan))) return false
      if (!normalized) return true
      return [plan.label, plan.purpose, plan.id, ...plan.scenario_ids]
        .join(' ')
        .toLowerCase()
        .includes(normalized)
    })
  }, [comparisonFor, filter, plans, query])

  const filteredProfilePlans = profilePlans.filter(
    (plan) =>
      matchesProfileFilter(plan, filter) &&
      [
        plan.configuration.label,
        plan.configuration.model,
        plan.configuration.provider,
        plan.snapshot.profile.label,
        plan.snapshot.profile.purpose,
        ...plan.snapshot.scenario_ids,
      ]
        .join(' ')
        .toLowerCase()
        .includes(query.trim().toLowerCase()),
  )
  const totalPlans = plans.length + profilePlans.length
  const totalFiltered = filteredPlans.length + filteredProfilePlans.length
  const local = bridge?.mode === 'local'
  const filtered = query.trim() !== '' || filter !== 'all'
  const filters: Array<{ id: PlanFilter; label: string }> = [
    { id: 'all', label: 'all' },
    { id: 'needs_action', label: 'needs action' },
    { id: 'running', label: 'running' },
    { id: 'compared', label: 'compared' },
    { id: 'regressed', label: 'regressed' },
  ]

  return (
    <>
      <DashboardPageActions
        active="plans"
        actionsLabel="Local plan actions"
        actions={
          local ? (
            <a
              className={dashboardHeaderActionClassName({ primary: true })}
              href={hashForNewPlan()}
            >
              new plan
            </a>
          ) : null
        }
      />
      <div className="ds-root page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-16 md:w-[calc(100%_-_3rem)]">
        <PageHeader
          title="plans"
          summary="Configure one execution model per plan, run its coverage and follow the results."
          actions={
            local && plans.length > 0 ? (
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

        {local ? (
          <nav className="mt-5 flex gap-2" aria-label="Plan views">
            <button
              type="button"
              aria-pressed={tab === 'mine'}
              className={buttonClassName({
                variant: tab === 'mine' ? 'primary' : 'secondary',
              })}
              onClick={() => setTab('mine')}
            >
              My plans
            </button>
            <button
              type="button"
              aria-pressed={tab === 'profiles'}
              className={buttonClassName({
                variant: tab === 'profiles' ? 'primary' : 'secondary',
              })}
              onClick={() => setTab('profiles')}
            >
              Profiles
            </button>
          </nav>
        ) : null}
        {local && masterPlan && tab === 'profiles' ? (
          <>
            <MasterTestProfiles plan={masterPlan} />
            <a
              className={buttonClassName({ variant: 'secondary' })}
              href={`${hashForNewPlan()}/manual`}
            >
              Create a custom plan manually
            </a>
          </>
        ) : null}

        {tab === 'profiles' ? null : !local && !loading ? (
          <div className="mt-6">
            <EmptyState
              title="Available only in the local dashboard"
              description="Published and view-only reports keep historical evidence, but do not expose local plan state or controls."
              actions={
                <a
                  className={buttonClassName({ variant: 'secondary' })}
                  href={hashForWorkspace()}
                >
                  back to overview
                </a>
              }
            />
          </div>
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
                    className={
                      candidate.id === 'regressed' &&
                      counts.regressed > 0 &&
                      filter !== 'regressed'
                        ? 'text-danger'
                        : undefined
                    }
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

            {local && tab === 'mine' && filteredProfilePlans.length > 0 ? (
              <div className="mt-5">
                <DataTable caption="My profile plans" collapse>
                  <thead>
                    <tr>
                      <th>Plan</th>
                      <th>Profile</th>
                      <th>Model</th>
                      <th>State</th>
                      <th>Last execution</th>
                      <th>Action</th>
                    </tr>
                  </thead>
                  <tbody>
                    {filteredProfilePlans.map((plan) => (
                      <DataTableRow key={plan.id} href={hashForPlan(plan.id)}>
                        <td data-label="Plan">
                          <a
                            className="font-semibold text-ink hover:underline"
                            href={hashForPlan(plan.id)}
                          >
                            {plan.configuration.label}
                          </a>
                        </td>
                        <td data-label="Profile">
                          {plan.snapshot.profile.label}
                        </td>
                        <td
                          data-label="Model"
                          className="break-words font-mono"
                        >
                          {plan.configuration.model}
                        </td>
                        <td data-label="State">
                          {plan.compatible
                            ? plan.state
                            : 'Incompatible revision'}
                        </td>
                        <td data-label="Last execution">
                          {plan.last_execution
                            ? `${shortDate(plan.last_execution.started_at)} · ${plan.last_execution.finished}/${plan.last_execution.planned} slots`
                            : 'No executions'}
                        </td>
                        <td data-label="Action">
                          <a
                            className={buttonClassName({
                              variant: 'secondary',
                              size: 'compact',
                            })}
                            href={hashForPlan(plan.id)}
                          >
                            {running(plan.state)
                              ? 'Follow execution'
                              : plan.snapshot.protected_supervisor_required
                                ? 'Export plan'
                                : plan.state === 'draft'
                                  ? 'Run plan'
                                  : 'View results'}
                          </a>
                        </td>
                      </DataTableRow>
                    ))}
                  </tbody>
                </DataTable>
              </div>
            ) : null}
            {profilePlans.length > 0 && plans.length > 0 ? (
              <h2 className="mt-8 text-sm font-semibold text-ink">
                Custom and legacy plans
              </h2>
            ) : null}

            {comparisonError && !error ? (
              <div className="mt-4">
                <Callout tone="warning" title="Comparison metrics unavailable">
                  Execution summaries could not be loaded. Plans remain
                  available, but comparisons are marked unavailable.{' '}
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
              ) : filteredPlans.length === 0 &&
                filteredProfilePlans.length > 0 ? null : totalPlans === 0 ? (
                <EmptyState
                  title="No local plans yet"
                  description={
                    <span className="grid gap-4">
                      <span>
                        Start with only the tests relevant to the Harness change
                        in front of you.
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
                  caption={`Local plans, ${filteredPlans.length} of ${plans.length}`}
                  collapse
                  minWidth="64rem"
                >
                  <thead>
                    <tr>
                      <th scope="col">Plan</th>
                      <th scope="col">Scope · model</th>
                      <th scope="col">Baseline</th>
                      <th scope="col">Latest candidate vs baseline</th>
                      <th scope="col" className={numericCellClassName}>
                        Updated
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
    </>
  )
}
