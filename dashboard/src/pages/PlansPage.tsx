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
  Dialog,
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
import { hashForNewPlan, hashForPlan } from '@/hooks/use-hash-route'
import {
  type DashboardExecutionSummary,
  getDashboardDataBridge,
  type LocalPlan,
  type MasterTestPlan,
} from '@/lib/dashboard-data-source'
import {
  buildExecutionPresentation,
  formatDate,
  statusCopy,
} from '@/lib/execution-view'
import {
  buildPlanComparison,
  formatPlanMetricValue,
  loadExecutionSummaries,
  metricById,
  type PlanMetricComparison,
  type PlanMetricId,
} from '@/lib/plan-comparison'
import { discoverReleaseControlHistory, exportReleaseControlHistory, type RcHistoryPlan } from '@/lib/release-control-reference'

type PlanFilter = 'all' | 'needs_action' | 'running' | 'compared'

type ReleaseControlPlan = {
  key: string
  executions: DashboardExecutionSummary[]
}

export function releaseControlPlans(executions: DashboardExecutionSummary[]) {
  const plans = new Map<string, DashboardExecutionSummary[]>()
  for (const execution of executions) {
    const key = execution.release_control?.profile
    if (!key) continue
    plans.set(key, [...(plans.get(key) ?? []), execution])
  }
  return [...plans]
    .map(([key, history]) => ({
      key,
      executions: history.sort((left, right) =>
        (right.started_at ?? '').localeCompare(left.started_at ?? ''),
      ),
    }))
    .sort((left, right) =>
      (right.executions[0]?.started_at ?? '').localeCompare(
        left.executions[0]?.started_at ?? '',
      ),
    )
}

export function ReleaseControlPlans({
  plans,
  loading,
  error,
  reload,
}: {
  plans: ReleaseControlPlan[]
  loading: boolean
  error: string | null
  reload: () => void
}) {
  if (error)
    return (
      <EmptyState
        className="mt-5"
        tone="error"
        title="Release Control history is unavailable"
        description={error}
        actions={
          <button
            type="button"
            className={buttonClassName({ variant: 'secondary' })}
            onClick={reload}
          >
            try again
          </button>
        }
      />
    )
  if (loading)
    return (
      <p className="mt-5 font-mono text-xs text-ink-muted" role="status">
        loading Release Control history…
      </p>
    )
  if (plans.length === 0)
    return (
      <EmptyState
        className="mt-5"
        title="No Release Control history found"
        description="Keep an authenticated Release Control tab connected to this personal Engine, then try again."
      />
    )
  return (
    <DataTable
      className="mt-5"
      caption="Reference plans from Release Control"
      collapse
      minWidth="44rem"
    >
      <thead>
        <tr>
          <th scope="col">Plan</th>
          <th scope="col">Latest result</th>
          <th scope="col">History</th>
          <th scope="col">Last run</th>
          <th scope="col">
            <span className="ds-visually-hidden">Open</span>
          </th>
        </tr>
      </thead>
      <tbody>
        {plans.map((plan) => {
          const latest = plan.executions[0]
          return (
            <DataTableRow key={plan.key} href={hashForPlan(`rc:${plan.key}`)}>
              <td data-label="Plan" className="ds-table-sticky-col">
                <span className="block font-mono text-xs font-medium text-ink">
                  {plan.key}
                </span>
                <span className="font-mono text-label text-ink-muted">
                  Reference: Release Control
                </span>
              </td>
              <td data-label="Latest result">
                {latest ? (
                  <StatusBadge
                    {...statusCopy(buildExecutionPresentation(latest))}
                    label={latest.status.replaceAll('_', ' ')}
                  />
                ) : (
                  '—'
                )}
              </td>
              <td data-label="History" className="font-mono text-xs">
                {plan.executions.length} execution
                {plan.executions.length === 1 ? '' : 's'}
              </td>
              <td
                data-label="Last run"
                className="font-mono text-xs text-ink-muted"
              >
                {latest
                  ? formatDate(latest.completed_at ?? latest.started_at ?? '')
                  : '—'}
              </td>
              <td className="text-right">
                <a
                  className={buttonClassName({
                    variant: 'quiet',
                    size: 'compact',
                  })}
                  href={hashForPlan(`rc:${plan.key}`)}
                >
                  open
                </a>
              </td>
            </DataTableRow>
          )
        })}
      </tbody>
    </DataTable>
  )
}

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
        detail: 'Comparing the saved scope against the baseline.',
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

function matchesFilter(plan: LocalPlan, filter: PlanFilter) {
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

function compact(value: number) {
  return new Intl.NumberFormat('en-US', {
    notation: Math.abs(value) >= 1000 ? 'compact' : 'standard',
    maximumFractionDigits: Math.abs(value) >= 100 ? 0 : 1,
  }).format(value)
}

const CORE_DELTAS: Array<{ id: PlanMetricId; label: string }> = [
  { id: 'coverage', label: 'coverage' },
  { id: 'technical_failures', label: 'technical failures' },
  { id: 'tokens', label: 'tokens' },
  { id: 'duration', label: 'time' },
]

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
        betterWhen="neither"
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
  const coverage = value('coverage')
  const tokens = value('tokens')
  const duration = value('duration')
  const turns = value('turns')
  return (
    <span className="grid gap-0.5 font-mono text-xs tabular-nums">
      <span className="text-ink">
        {[
          coverage ? `${coverage} coverage` : null,
          tokens ? `${tokens} tokens` : null,
        ]
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
 * The "latest candidate vs baseline" column: the signed
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
        <span className="text-ink">{comparison.headline}</span>
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
          <span className="text-xs text-ink-muted">Local plan</span>
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

function isLocalPlan(plan: import('@/lib/dashboard-data-source').Plan): plan is LocalPlan {
  return plan.origin !== 'remote'
}

export function PlansPage() {
  const viewsId = useId()
  const [tab, setTab] = useState<'mine' | 'profiles'>('mine')
  const [plans, setPlans] = useState<import('@/lib/dashboard-data-source').Plan[]>([])
  const [masterPlan, setMasterPlan] = useState<MasterTestPlan | null>(null)
  const [executionSummaries, setExecutionSummaries] = useState<
    Record<string, DashboardExecutionSummary>
  >({})
  const [query, setQuery] = useState('')
  const [filter, setFilter] = useState<PlanFilter>('all')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [comparisonError, setComparisonError] = useState<string | null>(null)
  const [importing, setImporting] = useState(false)
  const [importError, setImportError] = useState<string | null>(null)
  const [importOpen, setImportOpen] = useState(false)
  const [remotePlans, setRemotePlans] = useState<RcHistoryPlan[]>([])
  const [remotePlanKey, setRemotePlanKey] = useState('')

  const importHistory = async (file: File) => {
    setImporting(true)
    setImportError(null)
    try {
      const json = await file.text()
      const bytes = new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(json)))
      const sha256 = `sha256:${[...bytes].map((byte) => byte.toString(16).padStart(2, '0')).join('')}`
      const bridge = await getDashboardDataBridge()
      await bridge.planControl({ action: 'import_history', history: { json, sha256 } })
      await load({ silent: true })
    } catch (cause) {
      setImportError(cause instanceof Error ? cause.message : String(cause))
    } finally { setImporting(false) }
  }
  const openImport = async () => {
    setImportOpen(true); setImportError(null); setRemotePlans([])
    try { setRemotePlans(await discoverReleaseControlHistory()) }
    catch (cause) { setImportError(cause instanceof Error ? cause.message : String(cause)) }
  }
  const importRemote = async () => {
    if (!remotePlanKey) return
    setImporting(true); setImportError(null)
    try {
      const history = await exportReleaseControlHistory(remotePlanKey)
      const bridge = await getDashboardDataBridge()
      await bridge.planControl({ action: 'import_history', history })
      await load({ silent: true }); setImportOpen(false)
    } catch (cause) { setImportError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setImporting(false) }
  }

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
      const executionIds = orderedPlans.filter(isLocalPlan).flatMap((plan) => [
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
      plans.filter((plan) => isLocalPlan(plan) && matchesFilter(plan, candidate)).length
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
      if (isLocalPlan(plan) && !matchesFilter(plan, filter)) return false
      if (!normalized) return true
      return [
        plan.label,
        plan.purpose,
        plan.id,
        ...(isLocalPlan(plan) ? [plan.model, plan.provider, ...plan.scenario_ids] : [plan.source.plan_key, plan.source.instance_id]),
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
          <><button type="button" className={dashboardHeaderActionClassName()} onClick={() => void openImport()}>{importing ? 'importing…' : 'import history'}</button><a className={dashboardHeaderActionClassName({ primary: true })} href={hashForNewPlan()}>new plan</a></>
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
        {importError ? <Callout className="mt-4" tone="warning" title="History import failed">{importError}</Callout> : null}

        <Tabs
          value={tab}
          onValueChange={(value) => setTab(value as typeof tab)}
        >
          <TabsList className="mt-5 flex-wrap" aria-label="Plan views">
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
                  <Callout
                    tone="warning"
                    title="Comparison metrics unavailable"
                  >
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
                        <th scope="col">Scope · model</th>
                        <th scope="col">Baseline</th>
                        <th scope="col">Latest candidate vs baseline</th>
                        <th scope="col" className={numericCellClassName}>
                          Last activity
                        </th>
                        <th scope="col">
                          <span className="ds-visually-hidden">Action</span>
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {filteredPlans.map((plan) => isLocalPlan(plan) ? (
                        <PlanRow key={plan.id} plan={plan} executionSummaries={executionSummaries} />
                      ) : (
                        <DataTableRow key={plan.id} href={hashForPlan(plan.id)}>
                          <td data-label="Plan" className="ds-table-sticky-col"><span className="font-medium text-ink">{plan.label}</span><span className="ml-2 rounded bg-[var(--surface-fill)] px-1 font-mono text-label text-ink-muted">remote</span><span className="block font-mono text-label text-ink-muted">{plan.source.instance_id} · {plan.source.plan_key}</span></td>
                          <td data-label="Scope · model">{plan.purpose || '—'}</td><td data-label="Baseline">historical import</td><td data-label="Latest candidate vs baseline">{plan.execution_ids.length} execution{plan.execution_ids.length === 1 ? '' : 's'}</td><td data-label="Last activity" className={numericCellClassName}>{shortDate(plan.updated_at)}</td><td className="text-right"><a className={buttonClassName({variant:'quiet',size:'compact'})} href={hashForPlan(plan.id)}>open</a></td>
                        </DataTableRow>
                      ))}
                    </tbody>
                  </DataTable>
                )}
              </div>
            </>
          )}
        </div>
      </div>
      <Dialog open={importOpen} onClose={() => !importing && setImportOpen(false)} title="Import Release Control history" description="History is copied into this Harness. Later reading, comparison and reproduction use the local copy.">
            <div className="grid gap-4"><label className="grid gap-1 text-sm">Release Control plan<select value={remotePlanKey} onChange={(event) => setRemotePlanKey(event.target.value)}><option value="">Select a plan…</option>{remotePlans.map((plan) => <option key={plan.key} value={plan.key}>{plan.key}{plan.active ? '' : ' (inactive)'}</option>)}</select></label><div className="flex gap-2"><button className={buttonClassName({ variant: 'primary' })} type="button" disabled={!remotePlanKey || importing} onClick={() => void importRemote()}>{importing ? 'importing…' : 'import selected history'}</button><label className={buttonClassName({ variant: 'secondary' })}><input className="ds-visually-hidden" type="file" accept="application/json,.json" onChange={(event) => { const file = event.currentTarget.files?.[0]; if (file) void importHistory(file); event.currentTarget.value = '' }} />import JSON file</label></div>{importError ? <Callout tone="warning" title="Import unavailable">{importError}</Callout> : null}</div>
      </Dialog>
    </>
  )
}
