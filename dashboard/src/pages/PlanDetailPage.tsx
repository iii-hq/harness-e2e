import { ExternalLink, PencilLine } from 'lucide-react'
import {
  type FormEvent,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from 'react'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { ScenarioChatAction } from '@/components/ScenarioChatAction'
import {
  buttonClassName,
  Callout,
  DataTable,
  DataTableRow,
  EmptyState,
  Input,
  numericCellClassName,
  type OperationalStatus,
  PageHeader,
  Panel,
  Select,
  StatusBadge,
} from '@/design-system'
import {
  hashForComparison,
  hashForExecution,
  hashForNewPlan,
  hashForPlans,
  hashForTestHistory,
} from '@/hooks/use-hash-route'
import {
  type DashboardDataBridge,
  type DashboardExecutionDetail,
  type DashboardExecutionSummary,
  getDashboardDataBridge,
  type LocalPlan,
} from '@/lib/dashboard-data-source'
import {
  buildExecutionPresentation,
  formatDate,
  formatDuration,
  titleCase,
} from '@/lib/execution-view'
import {
  buildPlanComparison,
  formatPlanMetricDelta,
  formatPlanMetricValue,
  loadExecutionSummaries,
  type MetricDirection,
  metricById,
  type PlanComparison,
  type PlanMetricComparison,
  type PlanMetricId,
  type PlanScenarioComparison,
  type PlanVerdict,
} from '@/lib/plan-comparison'

/* ------------------------------------------------------------- helpers */

export type PlanRunRole = 'baseline' | 'candidate'

export type PlanRunFeedback = {
  role: PlanRunRole
  phase: 'starting' | 'running' | 'error'
  message: string
  executionId: string | null
}

type PlanNextAction = {
  title: string
  detail: string
  role: PlanRunRole | null
  actionLabel: string
  executionId: string | null
  state: 'ready' | 'running' | 'complete'
}

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

function roleLabel(role: PlanRunRole) {
  return role === 'baseline' ? 'Baseline' : 'Candidate'
}

export function isRoleRunning(plan: LocalPlan, role: PlanRunRole) {
  return plan.state === `${role}_running`
}

function scenarioName(scenario: string) {
  return scenario
    .replace(/[_.]+/g, ' ')
    .replace(/\b\w/g, (character) => character.toUpperCase())
}

const metricToneClass: Record<PlanMetricComparison['tone'], string> = {
  positive: 'text-success',
  negative: 'text-danger',
  neutral: 'text-ink-soft',
  unavailable: 'text-ink-muted',
}

/**
 * Audit PD-05: the plan has one status. It combines the lifecycle with the
 * lock, so nothing else on the page needs to repeat either.
 */
export function planReadiness(plan: LocalPlan): {
  status: OperationalStatus
  label: string
  detail: string
} {
  if (isRoleRunning(plan, 'baseline'))
    return {
      status: 'running',
      label: 'baseline running',
      detail:
        'The baseline is running; candidate actions stay unavailable until its report is complete.',
    }
  if (isRoleRunning(plan, 'candidate'))
    return {
      status: 'running',
      label: 'candidate running',
      detail: 'The locked scope is running against the captured baseline.',
    }
  if (!plan.baseline_execution_id)
    return plan.incomplete_execution_ids.length > 0
      ? {
          status: 'incomplete',
          label: 'baseline retry available · scope locked',
          detail:
            'The last attempt did not produce a report; retry the same locked scope.',
        }
      : {
          status: 'incomplete',
          label: 'draft · scope fixed at creation',
          detail:
            'The scope is defined but no completed baseline report exists.',
        }
  if (plan.candidate_execution_ids.length > 0)
    return {
      status: 'unavailable',
      label: 'comparison ready · scope locked',
      detail:
        'Candidate reports are ready to inspect against the locked baseline.',
    }
  return {
    status: 'unavailable',
    label: 'ready for candidate · scope locked',
    detail: 'Baseline captured; run the same scope after your change.',
  }
}

export function nextPlanAction(plan: LocalPlan): PlanNextAction {
  if (isRoleRunning(plan, 'baseline')) {
    return {
      title: 'Baseline is running',
      detail:
        'The locked scope is executing. Wait for its report before starting a candidate.',
      role: null,
      actionLabel: 'view active execution',
      executionId: plan.last_attempt_id,
      state: 'running',
    }
  }
  if (isRoleRunning(plan, 'candidate')) {
    return {
      title: 'Candidate is running',
      detail:
        'The same locked scope is executing against the captured baseline. This page refreshes automatically.',
      role: null,
      actionLabel: 'view active execution',
      executionId: plan.last_attempt_id,
      state: 'running',
    }
  }
  if (!plan.baseline_execution_id) {
    const retry = plan.incomplete_execution_ids.length > 0
    return {
      title: retry ? 'Retry the baseline' : 'Capture the baseline',
      detail: retry
        ? 'The previous attempt did not produce a report. Retry the same locked scope.'
        : 'Run this scope before the Harness change. Starting it freezes the scope, seeds and policy.',
      role: 'baseline',
      actionLabel: retry ? 'retry baseline' : 'run baseline',
      executionId: null,
      state: 'ready',
    }
  }
  if (plan.candidate_execution_ids.length > 0) {
    return {
      title: 'Candidate results are ready',
      detail:
        'Review the latest execution first. You can run another candidate later with this same locked scope.',
      role: 'candidate',
      actionLabel: 'view latest candidate',
      executionId: plan.candidate_execution_ids.at(-1) ?? null,
      state: 'complete',
    }
  }
  return {
    title: 'Run the candidate',
    detail:
      'Make the Harness change, then rerun this exact scope to produce a local comparison.',
    role: 'candidate',
    actionLabel: 'run candidate',
    executionId: null,
    state: 'ready',
  }
}

function scopeSentence(plan: LocalPlan) {
  return `${plan.scenarios.length} test${plan.scenarios.length === 1 ? '' : 's'} · ${plan.runs} run${plan.runs === 1 ? '' : 's'} each · ${plan.model || 'model not set'}`
}

function lastRunSentence(summary: DashboardExecutionSummary | null) {
  if (!summary?.totals) return null
  const seconds = summary.totals.wall_time_seconds
  const tokens = summary.totals.total_tokens
  const parts = [
    typeof seconds === 'number' ? formatDuration(seconds) : null,
    typeof tokens === 'number'
      ? `${Math.round(tokens).toLocaleString('en-US')} tokens`
      : null,
  ].filter(Boolean)
  return parts.length ? `last run took ${parts.join(' and ')}` : null
}

/* ----------------------------------------------------------- lifecycle */

/**
 * The plan's one primary action, first on the page (audit PD-04), with an
 * inline confirmation before a run starts (PD-08) and the baseline's own
 * outcome stated before candidates compare against it (PD-09).
 */
export function PlanLifecycle({
  plan,
  starting,
  feedback,
  onStart,
  baselineSummary = null,
  lastRunSummary = null,
}: {
  plan: LocalPlan
  starting: PlanRunRole | null
  feedback: PlanRunFeedback | null
  onStart: (role: PlanRunRole) => void
  baselineSummary?: DashboardExecutionSummary | null
  lastRunSummary?: DashboardExecutionSummary | null
}) {
  const [confirming, setConfirming] = useState<PlanRunRole | null>(null)
  const runningRole: PlanRunRole | null = isRoleRunning(plan, 'baseline')
    ? 'baseline'
    : isRoleRunning(plan, 'candidate')
      ? 'candidate'
      : null
  const activeFeedback =
    feedback && feedback.phase !== 'running' ? feedback : null
  const nextAction = nextPlanAction(plan)
  const canStart =
    nextAction.role !== null && starting === null && runningRole === null
  const baselineAttention =
    baselineSummary &&
    buildExecutionPresentation(baselineSummary).attention === 'needs_attention'
  const lastRun = lastRunSentence(lastRunSummary)

  const confirmRole = (role: PlanRunRole) => {
    setConfirming(null)
    onStart(role)
  }

  return (
    <Panel
      as="section"
      padding="default"
      aria-labelledby="plan-lifecycle-title"
      data-plan-lifecycle={nextAction.state}
    >
      <div
        className="grid gap-4 @[840px]:grid-cols-[minmax(0,1fr)_auto] @[840px]:items-start"
        aria-live="polite"
      >
        <div className="min-w-0">
          <h2 id="plan-lifecycle-title" className="m-0 text-base font-semibold">
            {nextAction.title}
          </h2>
          <p className="mt-1 mb-0 max-w-[48rem] text-sm leading-6 text-ink-soft">
            {nextAction.detail}
          </p>
          {baselineAttention && plan.baseline_execution_id ? (
            <p className="mt-2 mb-0 max-w-[48rem] text-sm leading-6 text-warning">
              The baseline was captured with failing tests; candidates compare
              against that failing baseline.{' '}
              <a
                className="text-ink underline underline-offset-4"
                href={hashForExecution(plan.baseline_execution_id)}
              >
                open the baseline report
              </a>
            </p>
          ) : null}
        </div>
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          {nextAction.executionId ? (
            <a
              className={buttonClassName({ variant: 'primary' })}
              href={hashForExecution(nextAction.executionId)}
            >
              {nextAction.actionLabel}
            </a>
          ) : nextAction.role ? (
            <button
              className={buttonClassName({ variant: 'primary' })}
              type="button"
              onClick={() => setConfirming(nextAction.role)}
              disabled={!canStart || confirming !== null}
            >
              {starting === nextAction.role
                ? `starting ${roleLabel(nextAction.role).toLowerCase()}…`
                : nextAction.actionLabel}
            </button>
          ) : null}
          {nextAction.state === 'complete' ? (
            <>
              {plan.baseline_execution_id ? (
                <a
                  className={buttonClassName({ variant: 'secondary' })}
                  href={hashForExecution(plan.baseline_execution_id)}
                >
                  view baseline execution
                </a>
              ) : null}
              <button
                className={buttonClassName({ variant: 'secondary' })}
                type="button"
                onClick={() => setConfirming('candidate')}
                disabled={
                  starting !== null ||
                  runningRole !== null ||
                  confirming !== null
                }
              >
                run another candidate
              </button>
            </>
          ) : null}
        </div>
      </div>
      {confirming ? (
        <div className="mt-4">
          <Callout
            tone="info"
            title={`confirm · ${confirming === 'baseline' ? (plan.incomplete_execution_ids.length ? 'retry baseline' : 'run baseline') : `run candidate #${plan.candidate_execution_ids.length + 1}`}`}
          >
            <p className="m-0">
              {confirming === 'baseline'
                ? 'This locks the scope, seeds and policy and starts a run: '
                : 'This reruns the locked scope: '}
              <span className="font-mono">{scopeSentence(plan)}</span>
              {lastRun ? ` · ${lastRun}` : ''}.
            </p>
            <div className="mt-3 flex flex-wrap gap-2">
              <button
                className={buttonClassName({
                  variant: 'primary',
                  size: 'compact',
                })}
                type="button"
                onClick={() => confirmRole(confirming)}
              >
                {confirming === 'baseline'
                  ? plan.incomplete_execution_ids.length
                    ? 'confirm · retry baseline'
                    : 'confirm · run baseline'
                  : 'confirm · run candidate'}
              </button>
              <button
                className={buttonClassName({
                  variant: 'secondary',
                  size: 'compact',
                })}
                type="button"
                onClick={() => setConfirming(null)}
              >
                cancel
              </button>
            </div>
          </Callout>
        </div>
      ) : null}
      {activeFeedback ? (
        <div className="mt-4" data-plan-run-feedback={activeFeedback.phase}>
          <Callout
            tone={activeFeedback.phase === 'error' ? 'danger' : 'info'}
            title={activeFeedback.message}
          >
            {activeFeedback.executionId ? (
              <a
                className="text-ink underline underline-offset-4"
                href={hashForExecution(activeFeedback.executionId)}
              >
                open active execution
              </a>
            ) : null}
          </Callout>
        </div>
      ) : null}
    </Panel>
  )
}

/* --------------------------------------------------------------- scope */

/** Audit PD-06: the scope is visible, test by test, not just counted. */
export function PlanScope({ plan }: { plan: LocalPlan }) {
  const created = formatDate(plan.created_at)
  return (
    <Panel as="section" padding="default" aria-labelledby="plan-scope-title">
      <h2 id="plan-scope-title" className="m-0 text-base font-semibold">
        scope{plan.locked ? ' · locked at baseline' : ''}
      </h2>
      <ul className="m-0 mt-3 flex list-none flex-wrap gap-2 p-0">
        {plan.scenarios.map((scenario) => (
          <li key={`${scenario.scenario_id}:${scenario.scenario_version}`}>
            <a
              className={buttonClassName({
                variant: 'secondary',
                size: 'compact',
              })}
              href={hashForTestHistory(scenario.scenario_id)}
            >
              {scenario.scenario_id} v{scenario.scenario_version}
            </a>
          </li>
        ))}
        {plan.scenarios.length === 0
          ? plan.scenario_ids.map((id) => (
              <li key={id}>
                <a
                  className={buttonClassName({
                    variant: 'secondary',
                    size: 'compact',
                  })}
                  href={hashForTestHistory(id)}
                >
                  {id}
                </a>
              </li>
            ))
          : null}
      </ul>
      <dl className="m-0 mt-4 grid gap-x-6 gap-y-2 text-sm @[640px]:grid-cols-[auto_minmax(0,1fr)]">
        <dt className="ds-label">runs</dt>
        <dd className="m-0 font-mono">
          {plan.runs} per test · {plan.technical_retries} retr
          {plan.technical_retries === 1 ? 'y' : 'ies'} ·{' '}
          {plan.seed === null ? 'canonical seed' : `seed ${plan.seed}`}
        </dd>
        <dt className="ds-label">model</dt>
        <dd className="m-0 font-mono">
          {plan.model || 'not set'}
          {plan.provider ? ` · ${plan.provider}` : ''}
        </dd>
        <dt className="ds-label">judge</dt>
        <dd className="m-0 font-mono">
          {plan.judge_model
            ? `${plan.judge_model}${plan.judge_provider ? ` · ${plan.judge_provider}` : ''}`
            : 'automatic · default protocol'}
        </dd>
        <dt className="ds-label">endpoint</dt>
        <dd className="m-0 break-all font-mono text-ink-soft">{plan.url}</dd>
        <dt className="ds-label">created</dt>
        <dd className="m-0 font-mono text-ink-soft">{created}</dd>
      </dl>
    </Panel>
  )
}

/* -------------------------------------------------------- run history */

function executionStatus(
  summary: DashboardExecutionSummary | null,
  fallback: 'running' | 'incomplete' | null = null,
): { status: OperationalStatus; label: string } {
  if (!summary) {
    return fallback === 'running'
      ? { status: 'running', label: 'running' }
      : fallback === 'incomplete'
        ? { status: 'incomplete', label: 'incomplete' }
        : { status: 'unavailable', label: 'unavailable' }
  }
  const state = buildExecutionPresentation(summary).attention
  if (state === 'passed') return { status: 'passed', label: 'passed' }
  if (state === 'needs_attention')
    return { status: 'failed', label: 'needs attention' }
  if (state === 'running' || state === 'cancelling')
    return { status: state, label: state }
  return { status: 'unavailable', label: titleCase(state).toLowerCase() }
}

type ExecutionHistoryRow = {
  id: string
  role: 'baseline' | 'candidate' | 'attempt'
  label: string
  detail: string
  summary: DashboardExecutionSummary | null
  fallback: 'running' | 'incomplete' | null
  candidateNumber: number | null
}

export function executionHistoryRows(
  plan: LocalPlan,
  summaries: Record<string, DashboardExecutionSummary>,
): ExecutionHistoryRow[] {
  const rows: ExecutionHistoryRow[] = []
  const retained = new Set<string>()
  if (plan.baseline_execution_id) {
    retained.add(plan.baseline_execution_id)
    rows.push({
      id: plan.baseline_execution_id,
      role: 'baseline',
      label: 'Baseline',
      detail: '',
      summary: summaries[plan.baseline_execution_id] ?? null,
      fallback: null,
      candidateNumber: null,
    })
  }
  if (
    plan.last_attempt_id &&
    ['baseline_running', 'candidate_running'].includes(plan.state)
  ) {
    retained.add(plan.last_attempt_id)
    const baselineRun = plan.state === 'baseline_running'
    rows.push({
      id: plan.last_attempt_id,
      role: baselineRun ? 'baseline' : 'candidate',
      label: baselineRun ? 'Baseline in progress' : 'Candidate in progress',
      detail: 'Active execution',
      summary: summaries[plan.last_attempt_id] ?? null,
      fallback: 'running',
      candidateNumber: null,
    })
  }
  for (
    let index = plan.candidate_execution_ids.length - 1;
    index >= 0;
    index--
  ) {
    const id = plan.candidate_execution_ids[index]
    retained.add(id)
    rows.push({
      id,
      role: 'candidate',
      label: `Candidate #${index + 1}`,
      detail: index === plan.candidate_execution_ids.length - 1 ? 'Latest' : '',
      summary: summaries[id] ?? null,
      fallback: null,
      candidateNumber: index + 1,
    })
  }
  for (const id of [...plan.incomplete_execution_ids].reverse()) {
    if (retained.has(id)) continue
    rows.push({
      id,
      role: 'attempt',
      label: 'Incomplete attempt',
      detail: 'Excluded from comparison',
      summary: summaries[id] ?? null,
      fallback: 'incomplete',
      candidateNumber: null,
    })
  }
  return rows
}

export function selectedPlanCandidate(
  current: string | null,
  pinned: boolean,
  candidateIds: string[],
) {
  if (pinned && current && candidateIds.includes(current)) return current
  return candidateIds.at(-1) ?? null
}

export function planMetricWinnerIds(
  values: Array<{ id: string; value: number | null }>,
  direction: MetricDirection,
) {
  if (direction === 'context') return []
  const comparable = values.filter(
    (entry): entry is { id: string; value: number } => entry.value !== null,
  )
  if (comparable.length < 2) return []
  const winnerValue = comparable.reduce(
    (best, entry) =>
      direction === 'higher'
        ? Math.max(best, entry.value)
        : Math.min(best, entry.value),
    comparable[0].value,
  )
  const winners = comparable
    .filter((entry) => Math.abs(entry.value - winnerValue) < 1e-9)
    .map((entry) => entry.id)
  return winners.length === 1 ? winners : []
}

export function planExecutionLabel(
  plan: LocalPlan,
  executionId: string | null,
) {
  if (!executionId) return 'Execution unavailable'
  if (executionId === plan.baseline_execution_id) return 'Official baseline'
  const candidateIndex = plan.candidate_execution_ids.indexOf(executionId)
  if (candidateIndex < 0) return executionId
  return (
    plan.candidate_labels?.[executionId]?.trim() ||
    `Candidate #${candidateIndex + 1}`
  )
}

function ExecutionNameControl({
  executionId,
  fallbackLabel,
  label,
  onRename,
}: {
  executionId: string
  fallbackLabel: string
  label: string
  onRename: (executionId: string, label: string) => Promise<void>
}) {
  const [draft, setDraft] = useState(label)
  const [editing, setEditing] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => setDraft(label), [label])

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    setSaving(true)
    setError(null)
    try {
      await onRename(executionId, draft)
      setEditing(false)
    } catch (cause) {
      setError(errorText(cause))
    } finally {
      setSaving(false)
    }
  }

  return (
    <span
      className="inline-flex flex-wrap items-center gap-2"
      data-rename-control
    >
      {editing ? (
        <form
          className="flex flex-wrap items-center gap-2"
          onSubmit={(event) => void submit(event)}
        >
          <Input
            aria-label={`Name ${fallbackLabel}`}
            className="w-56"
            maxLength={80}
            placeholder={fallbackLabel}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
          />
          <button
            className={buttonClassName({ variant: 'primary', size: 'compact' })}
            disabled={saving}
            type="submit"
          >
            {saving ? 'saving…' : 'save'}
          </button>
          <button
            className={buttonClassName({
              variant: 'secondary',
              size: 'compact',
            })}
            disabled={saving}
            type="button"
            onClick={() => {
              setDraft(label)
              setError(null)
              setEditing(false)
            }}
          >
            cancel
          </button>
        </form>
      ) : (
        <button
          aria-label={`Rename ${label.trim() || fallbackLabel}`}
          className={buttonClassName({ variant: 'quiet', size: 'compact' })}
          title={`Rename ${label.trim() || fallbackLabel}`}
          type="button"
          onClick={() => setEditing(true)}
        >
          <PencilLine aria-hidden="true" size={14} strokeWidth={1.8} />
          rename
        </button>
      )}
      {error ? (
        <small className="text-xs text-danger" role="alert">
          {error}
        </small>
      ) : null}
    </span>
  )
}

function finiteExecutionMetric(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

type RunMetric = 'tokens' | 'duration' | 'turns' | 'calls' | 'errors'

function executionMetricNumber(
  summary: DashboardExecutionSummary | null,
  metric: RunMetric,
) {
  const totals = summary?.totals
  return finiteExecutionMetric(
    metric === 'tokens'
      ? totals?.total_tokens
      : metric === 'duration'
        ? totals?.wall_time_seconds
        : metric === 'turns'
          ? totals?.turns
          : metric === 'calls'
            ? totals?.function_calls
            : totals?.function_call_errors,
  )
}

export function executionMetricValue(
  summary: DashboardExecutionSummary | null,
  metric: RunMetric,
) {
  const numeric = executionMetricNumber(summary, metric)
  if (numeric === null) return '—'
  if (metric === 'duration') return formatDuration(numeric)
  return Math.round(numeric).toLocaleString('en-US')
}

const RUN_METRICS: Array<{ id: RunMetric; label: string }> = [
  { id: 'tokens', label: 'Tokens' },
  { id: 'duration', label: 'Duration' },
  { id: 'turns', label: 'Turns' },
  { id: 'calls', label: 'Calls' },
  { id: 'errors', label: 'Errors' },
]

/**
 * Every retained run as a timeline row. Columns that no run reports are
 * hidden (audit PD-12); incomplete attempts stay listed but excluded from
 * winners. Exported under its historical name for the callers and tests.
 */
export function PlanRunHistory({
  plan,
  summaries,
  onRenameCandidate,
}: {
  plan: LocalPlan
  summaries: Record<string, DashboardExecutionSummary>
  onRenameCandidate?: (executionId: string, label: string) => Promise<void>
}) {
  const rows = executionHistoryRows(plan, summaries)
  if (rows.length === 0) return null
  const metrics = RUN_METRICS.filter(({ id }) =>
    rows.some((row) => executionMetricNumber(row.summary, id) !== null),
  )

  return (
    <Panel
      as="section"
      padding="default"
      aria-labelledby="plan-run-history-title"
      data-plan-run-history
    >
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h2
            id="plan-run-history-title"
            className="m-0 text-base font-semibold"
          >
            runs · {rows.length}
          </h2>
          <p className="mt-1 mb-0 text-xs leading-5 text-ink-soft">
            Every retained run with its result and usage. Incomplete attempts
            remain excluded from metric winners.
          </p>
        </div>
      </div>
      <div className="mt-4">
        <DataTable caption={`Plan runs, ${rows.length}`} collapse>
          <thead>
            <tr>
              <th scope="col">Run</th>
              <th scope="col">Result</th>
              <th scope="col">Captured</th>
              {metrics.map((metric) => (
                <th
                  key={metric.id}
                  scope="col"
                  className={numericCellClassName}
                >
                  {metric.label}
                </th>
              ))}
              <th scope="col">
                <span className="ds-visually-hidden">Report</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => {
              const status = executionStatus(row.summary, row.fallback)
              const timestamp =
                row.summary?.completed_at || row.summary?.started_at || ''
              const role =
                row.role === 'baseline'
                  ? 'baseline'
                  : row.role === 'candidate'
                    ? `candidate ${row.candidateNumber ?? ''}`.trim()
                    : 'attempt'
              const displayLabel =
                row.role === 'attempt'
                  ? row.label
                  : planExecutionLabel(plan, row.id)
              return (
                <DataTableRow
                  key={`${row.role}:${row.id}`}
                  data-run-role={row.role}
                >
                  <td data-label="Run">
                    <span className="grid gap-1">
                      <span className="ds-label">{role}</span>
                      <span className="flex flex-wrap items-center gap-2">
                        <strong className="font-mono text-[0.8125rem] text-ink">
                          {displayLabel}
                        </strong>
                        {row.role === 'candidate' && onRenameCandidate ? (
                          <ExecutionNameControl
                            executionId={row.id}
                            fallbackLabel={`Candidate #${row.candidateNumber ?? 1}`}
                            label={plan.candidate_labels?.[row.id] ?? ''}
                            onRename={onRenameCandidate}
                          />
                        ) : null}
                      </span>
                      <code
                        className="font-mono text-label text-ink-muted"
                        title={row.id}
                      >
                        {row.id.length > 18
                          ? `${row.id.slice(0, 12)}…${row.id.slice(-4)}`
                          : row.id}
                      </code>
                    </span>
                  </td>
                  <td data-label="Result">
                    <StatusBadge status={status.status} label={status.label} />
                    {row.detail ? (
                      <span className="mt-1 block text-xs text-ink-muted">
                        {row.detail}
                      </span>
                    ) : null}
                  </td>
                  <td data-label="Captured">
                    <time
                      className="text-xs text-ink-soft"
                      dateTime={timestamp || undefined}
                    >
                      {timestamp ? formatDate(timestamp) : 'date unavailable'}
                    </time>
                  </td>
                  {metrics.map((metric) => (
                    <td
                      key={metric.id}
                      data-label={metric.label}
                      className={numericCellClassName}
                    >
                      {executionMetricValue(row.summary, metric.id)}
                    </td>
                  ))}
                  <td className="text-right">
                    <a
                      aria-label={`Open report for ${displayLabel}`}
                      className={buttonClassName({
                        variant: 'secondary',
                        size: 'compact',
                      })}
                      href={hashForExecution(row.id)}
                      title={`Open report for ${displayLabel}`}
                    >
                      <ExternalLink
                        aria-hidden="true"
                        size={13}
                        strokeWidth={1.8}
                      />
                      report
                    </a>
                  </td>
                </DataTableRow>
              )
            })}
          </tbody>
        </DataTable>
      </div>
    </Panel>
  )
}

export { PlanRunHistory as PlanNonComparableAttempts }

/* ----------------------------------------------------------- comparison */

const planVerdictStatus: Record<PlanVerdict, OperationalStatus> = {
  improved: 'passed',
  stable: 'unavailable',
  regressed: 'failed',
  inconclusive: 'inconclusive',
}

export const PLAN_COMPARISON_TABLE_METRICS = [
  'pass_rate',
  'quality',
  'duration',
  'tokens',
  'cost',
  'turns',
  'function_calls',
  'function_errors',
] as const

const PLAN_SCENARIO_TABLE_METRICS: PlanMetricId[] = [
  'pass_rate',
  'quality',
  'duration',
  'tokens',
  'cost',
  'turns',
  'function_calls',
  'function_errors',
]

const PLAN_SCENARIO_SUMMARY_METRICS: PlanMetricId[] = [
  'quality',
  'duration',
  'tokens',
  'turns',
]

const SCENARIO_BASELINE_ID = '__visual_baseline__'

function directionLabel(direction: MetricDirection | undefined) {
  return direction === 'higher'
    ? 'Higher is better'
    : direction === 'lower'
      ? 'Lower is better'
      : 'Context only'
}

export function PlanExecutionHistory({
  plan,
  summaries,
  visualBaselineId,
  comparisonCandidateIds,
  selectedCandidateId,
  scenarioComparison = null,
  onVisualBaselineChange,
  onToggleCandidate,
  loading,
  error = null,
}: {
  plan: LocalPlan
  summaries: Record<string, DashboardExecutionSummary>
  visualBaselineId: string | null
  comparisonCandidateIds: string[]
  selectedCandidateId: string | null
  scenarioComparison?: PlanComparison | null
  onVisualBaselineChange: (id: string) => void
  onToggleCandidate: (id: string, selected: boolean) => void
  loading: boolean
  error?: string | null
}) {
  // Comparison controls are only useful once there is an official baseline
  // and a second completed execution to compare with it.
  if (
    !plan.baseline_execution_id ||
    plan.candidate_execution_ids.length === 0
  ) {
    return null
  }

  const rows = executionHistoryRows(plan, summaries)
  const baseline = visualBaselineId
    ? (summaries[visualBaselineId] ?? null)
    : null
  const selectableRows = rows.filter(
    (row) => row.role !== 'attempt' && row.fallback !== 'running',
  )
  const visualBaselineRow = rows.find((row) => row.id === visualBaselineId)
  const comparisonRows = rows.filter((row) =>
    comparisonCandidateIds.includes(row.id),
  )
  const comparisonColumns = [
    ...(visualBaselineRow ? [visualBaselineRow] : []),
    ...comparisonRows,
  ].map((row) => {
    const isVisualBaseline = row.id === visualBaselineId
    const rowComparison =
      !isVisualBaseline && baseline && row.summary
        ? buildPlanComparison(baseline, row.summary)
        : null
    const snapshot = row.summary
      ? buildPlanComparison(row.summary, row.summary)
      : null
    return {
      row,
      isVisualBaseline,
      rowComparison,
      metricSource: rowComparison ?? snapshot,
      selected: row.id === selectedCandidateId,
    }
  })
  const scenarioComparisons = comparisonColumns.flatMap((column) => {
    if (column.isVisualBaseline) return []
    const comparison =
      column.selected && scenarioComparison
        ? scenarioComparison
        : column.rowComparison
    return comparison
      ? [
          {
            id: column.row.id,
            label: planExecutionLabel(plan, column.row.id),
            comparison,
          },
        ]
      : []
  })
  const selectedColumn = comparisonColumns.find((column) => column.selected)
  const headlineComparison = selectedColumn?.rowComparison ?? null
  // Audit PD-12: a metric that no column reports is not a row.
  const metricRows = PLAN_COMPARISON_TABLE_METRICS.map((id) => {
    const entries = comparisonColumns.map((column) => {
      const metric = column.metricSource
        ? metricById(column.metricSource, id)
        : null
      const side: 'baseline' | 'candidate' = column.rowComparison
        ? 'candidate'
        : 'baseline'
      return { column, metric, side, value: metric?.[side] ?? null }
    })
    return { id, entries }
  }).filter(({ entries }) => entries.some(({ value }) => value !== null))
  const hiddenMetrics = PLAN_COMPARISON_TABLE_METRICS.length - metricRows.length

  return (
    <Panel
      as="section"
      padding="default"
      aria-labelledby="plan-execution-history-title"
      data-plan-comparison
    >
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div className="min-w-0">
          <h2
            id="plan-execution-history-title"
            className="m-0 text-base font-semibold"
          >
            baseline and candidates
          </h2>
          <p className="mt-1 mb-0 max-w-[48rem] text-xs leading-5 text-ink-soft">
            Choose a visual baseline and any number of candidates. This view
            never changes the official baseline stored with the plan.
          </p>
        </div>
        <span className="font-mono text-xs text-ink-muted">
          {loading
            ? 'loading…'
            : `${visualBaselineId ? '1 visual baseline' : 'Baseline pending'} · ${comparisonCandidateIds.length} selected`}
        </span>
      </div>
      {error ? (
        <div className="mt-4">
          <Callout
            tone="warning"
            title="Execution summaries could not be loaded"
          >
            Retained ids and report links remain available.{' '}
            <span className="font-mono">{error}</span>
          </Callout>
        </div>
      ) : null}
      {headlineComparison ? (
        <div className="mt-4 flex flex-wrap items-center gap-3 text-sm">
          <StatusBadge
            status={planVerdictStatus[headlineComparison.verdict]}
            label={headlineComparison.verdict}
          />
          <span className="text-ink-soft">{headlineComparison.headline}</span>
        </div>
      ) : null}
      <div className="mt-4 grid gap-4 @[840px]:grid-cols-[minmax(16rem,0.9fr)_minmax(0,1.1fr)]">
        <div className="grid gap-2 text-xs font-semibold text-ink-soft">
          <label htmlFor="plan-visual-baseline">Visual baseline</label>
          <Select
            id="plan-visual-baseline"
            value={visualBaselineId ?? ''}
            disabled={loading || selectableRows.length === 0}
            onChange={(event) => onVisualBaselineChange(event.target.value)}
          >
            {selectableRows.map((row) => (
              <option key={row.id} value={row.id}>
                {planExecutionLabel(plan, row.id)}
              </option>
            ))}
          </Select>
          <span className="font-normal text-ink-muted">
            The official plan baseline remains unchanged.
          </span>
        </div>
        <fieldset className="m-0 min-w-0 border-0 p-0">
          <legend className="text-xs font-semibold text-ink-soft">
            Compare candidates
          </legend>
          <span className="mt-1 block text-xs text-ink-muted">
            Select one or more executions to show in the table.
          </span>
          <div className="mt-2 flex flex-wrap gap-2">
            {selectableRows
              .filter((row) => row.id !== visualBaselineId)
              .map((row) => {
                const selected = comparisonCandidateIds.includes(row.id)
                return (
                  <label
                    className={`inline-flex min-h-9 cursor-pointer items-center gap-2 rounded-[6px] px-3 font-mono text-xs ${
                      selected
                        ? 'bg-[var(--surface-selected)] text-ink'
                        : 'bg-[var(--surface-fill)] text-ink-soft hover:bg-[var(--surface-soft)]'
                    }`}
                    key={row.id}
                    data-candidate-option={selected ? 'selected' : 'idle'}
                  >
                    <input
                      className="size-4 accent-[var(--accent)]"
                      type="checkbox"
                      checked={selected}
                      onChange={(event) =>
                        onToggleCandidate(row.id, event.target.checked)
                      }
                    />
                    <span>{planExecutionLabel(plan, row.id)}</span>
                  </label>
                )
              })}
          </div>
        </fieldset>
      </div>
      <div className="mt-4">
        <DataTable
          caption="Plan metrics in rows, with the visual baseline and selected candidates in columns. Best values are highlighted."
          minWidth={`${12 + comparisonColumns.length * 12}rem`}
          collapse
          className="[&_.is-winner]:font-semibold [&_.is-winner]:text-success"
        >
          <thead>
            <tr>
              <th scope="col">Metric</th>
              {comparisonColumns.map(({ row, isVisualBaseline, selected }) => (
                <th
                  className={
                    [
                      isVisualBaseline ? 'is-baseline' : '',
                      selected ? 'is-selected' : '',
                    ]
                      .filter(Boolean)
                      .join(' ') || undefined
                  }
                  data-execution-id={row.id}
                  key={`${row.role}:${row.id}`}
                  scope="col"
                  title={row.id}
                >
                  <span className="grid gap-0.5 normal-case tracking-normal">
                    <span className="ds-label">
                      {isVisualBaseline ? 'Reference' : 'Candidate'}
                      {selected ? ' · selected' : ''}
                    </span>
                    <strong className="font-mono text-[0.8125rem] font-semibold text-ink">
                      {planExecutionLabel(plan, row.id)}
                    </strong>
                    <span className="font-mono text-label font-normal text-ink-muted">
                      {[
                        row.detail,
                        row.summary?.completed_at
                          ? formatDate(row.summary.completed_at)
                          : 'Date unavailable',
                      ]
                        .filter(Boolean)
                        .join(' · ')}
                    </span>
                  </span>
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {metricRows.map(({ id, entries }) => {
              const descriptor = entries.find(({ metric }) => metric)?.metric
              const winnerIds = new Set(
                planMetricWinnerIds(
                  entries.map(({ column, value }) => ({
                    id: column.row.id,
                    value,
                  })),
                  descriptor?.direction ?? 'context',
                ),
              )
              const direction = directionLabel(descriptor?.direction)
              return (
                <tr data-metric-id={id} key={id}>
                  <th scope="row" className="normal-case tracking-normal">
                    <span className="grid gap-0.5">
                      <strong className="font-mono text-[0.8125rem] font-semibold text-ink">
                        {descriptor?.label ?? titleCase(id)}
                      </strong>
                      <span
                        className="font-mono text-label font-normal text-ink-muted"
                        role="tooltip"
                        id={`plan-metric-direction-${id}`}
                      >
                        {direction}
                      </span>
                    </span>
                  </th>
                  {entries.map(({ column, metric, side }) => {
                    const winner = winnerIds.has(column.row.id)
                    return (
                      <td
                        className={
                          [
                            column.isVisualBaseline ? 'is-baseline' : '',
                            column.selected ? 'is-selected' : '',
                            winner ? 'is-winner' : '',
                          ]
                            .filter(Boolean)
                            .join(' ') || undefined
                        }
                        data-execution-id={column.row.id}
                        data-label={planExecutionLabel(plan, column.row.id)}
                        key={column.row.id}
                      >
                        {metric ? (
                          <span className="grid gap-0.5 font-mono tabular-nums">
                            <span className="flex items-baseline gap-2">
                              <strong>
                                {formatPlanMetricValue(metric, side)}
                              </strong>
                              {winner ? (
                                <span className="ds-label text-success">
                                  Best
                                </span>
                              ) : null}
                            </span>
                            {!column.isVisualBaseline &&
                            column.rowComparison ? (
                              <small
                                className={`text-label ${metricToneClass[metric.tone]}`}
                              >
                                {formatPlanMetricDelta(metric)}
                              </small>
                            ) : !column.isVisualBaseline ? (
                              <small className="text-label text-ink-muted">
                                Not comparable
                              </small>
                            ) : null}
                          </span>
                        ) : (
                          <span className="text-ink-muted">—</span>
                        )}
                      </td>
                    )
                  })}
                </tr>
              )
            })}
          </tbody>
        </DataTable>
        {hiddenMetrics > 0 ? (
          <p className="mt-2 mb-0 font-mono text-label text-ink-muted">
            {hiddenMetrics} metric{hiddenMetrics === 1 ? '' : 's'} not reported
            by this harness · hidden
          </p>
        ) : null}
      </div>
      {scenarioComparisons.some(
        ({ comparison }) => comparison.scenarios.length > 0,
      ) ? (
        <div className="mt-6">
          <PlanScenarioComparisonTable comparisons={scenarioComparisons} />
        </div>
      ) : null}
    </Panel>
  )
}

function scenarioMetricWinnerIds(
  descriptor: PlanMetricComparison,
  metrics: Array<PlanMetricComparison | null>,
  comparisons: Array<{ id: string }>,
) {
  return new Set(
    planMetricWinnerIds(
      [
        { id: SCENARIO_BASELINE_ID, value: descriptor.baseline },
        ...metrics.map((metric, index) => ({
          id: comparisons[index].id,
          value: metric?.candidate ?? null,
        })),
      ],
      descriptor.direction,
    ),
  )
}

function scenarioMetrics(scenario: PlanScenarioComparison) {
  const available = [...scenario.metrics, ...scenario.execution_metrics]
  return PLAN_SCENARIO_TABLE_METRICS.flatMap((id) => {
    const metric = available.find((candidate) => candidate.id === id)
    return metric ? [metric] : []
  })
}

/** Audit PD-13: the summary row names the test and its objective; exact
 * values live in the expanded table. */
function PlanScenarioComparisonTable({
  comparisons,
}: {
  comparisons: Array<{
    id: string
    label: string
    comparison: PlanComparison
  }>
}) {
  const scenarioIds = [
    ...new Set(
      comparisons.flatMap(({ comparison }) =>
        comparison.scenarios.map((scenario) => scenario.id),
      ),
    ),
  ].sort()
  if (scenarioIds.length === 0) return null
  return (
    <section aria-labelledby="plan-by-test-title" data-plan-by-test>
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h3 id="plan-by-test-title" className="m-0 text-sm font-semibold">
            by test
          </h3>
          <p className="mt-1 mb-0 text-xs leading-5 text-ink-soft">
            Outcome and efficiency per test. Expand a row for exact values and
            deltas.
          </p>
        </div>
        <span className="font-mono text-xs text-ink-muted">
          {scenarioIds.length} {scenarioIds.length === 1 ? 'test' : 'tests'} ·{' '}
          {comparisons.length}{' '}
          {comparisons.length === 1 ? 'candidate' : 'candidates'}
        </span>
      </div>
      <div className="mt-3 grid gap-2">
        {scenarioIds.map((scenarioId, index) => {
          const scenarioColumns = comparisons.map((column) => ({
            ...column,
            scenario:
              column.comparison.scenarios.find(
                (scenario) => scenario.id === scenarioId,
              ) ?? null,
          }))
          const firstScenario = scenarioColumns.find(
            ({ scenario }) => scenario,
          )?.scenario
          if (!firstScenario) return null
          const metricLists = scenarioColumns.map(({ scenario }) =>
            scenario ? scenarioMetrics(scenario) : [],
          )
          return (
            <details
              className="group rounded-[6px] bg-[var(--surface-fill)]"
              key={scenarioId}
              data-scenario-id={scenarioId}
              open={index === 0}
            >
              <summary className="grid cursor-pointer list-none gap-2 px-4 py-3 marker:hidden @[720px]:grid-cols-[minmax(12rem,1fr)_minmax(0,1.4fr)_auto] @[720px]:items-center">
                <span className="min-w-0">
                  <strong className="block truncate font-mono text-[0.8125rem] text-ink">
                    {scenarioName(scenarioId)}
                  </strong>
                  <code className="font-mono text-label text-ink-muted">
                    {scenarioId}
                  </code>
                </span>
                <span className="font-mono text-xs text-ink-soft">
                  {titleCase(firstScenario.baseline_status)} →{' '}
                  {scenarioColumns
                    .map(({ scenario }) =>
                      titleCase(scenario?.candidate_status ?? 'not reported'),
                    )
                    .join(' · ')}
                </span>
                <span className="flex flex-wrap gap-x-3 gap-y-1 font-mono text-label text-ink-muted">
                  {PLAN_SCENARIO_SUMMARY_METRICS.map((metricId) => {
                    const metrics = metricLists.map(
                      (list) => list.find(({ id }) => id === metricId) ?? null,
                    )
                    const descriptor = metrics.find((metric) => metric)
                    if (!descriptor) return null
                    const first = metrics.find((metric) => metric)
                    return (
                      <span key={metricId}>
                        {descriptor.label.toLowerCase()}{' '}
                        <span
                          className={first ? metricToneClass[first.tone] : ''}
                        >
                          {first ? formatPlanMetricDelta(first) : '—'}
                        </span>
                      </span>
                    )
                  })}
                </span>
              </summary>
              <div className="px-4 pt-1 pb-3">
                <DataTable
                  caption={`Metrics for ${scenarioName(scenarioId)}, comparing the visual baseline with selected candidates.`}
                  minWidth={`${14 + comparisons.length * 10}rem`}
                  collapse
                  className="[&_.is-winner]:font-semibold [&_.is-winner]:text-success"
                  data-scenario-metrics
                >
                  <thead>
                    <tr>
                      <th scope="col">Metric</th>
                      <th scope="col">
                        <span className="flex flex-wrap items-center gap-2 normal-case tracking-normal">
                          <span className="grid">
                            <span className="ds-label">Reference</span>
                            <strong className="font-mono text-[0.8125rem] text-ink">
                              Baseline
                            </strong>
                          </span>
                          <ScenarioChatAction
                            compact
                            label="ask about this run"
                            executionId={
                              comparisons[0]?.comparison.baseline?.id
                            }
                            scenarioId={scenarioId}
                          />
                        </span>
                      </th>
                      {comparisons.map((column) => (
                        <th key={column.id} scope="col">
                          <span className="flex flex-wrap items-center gap-2 normal-case tracking-normal">
                            <span className="grid">
                              <span className="ds-label">Candidate</span>
                              <strong className="font-mono text-[0.8125rem] text-ink">
                                {column.label}
                              </strong>
                            </span>
                            <ScenarioChatAction
                              compact
                              label="ask about this run"
                              executionId={column.id}
                              scenarioId={scenarioId}
                            />
                          </span>
                        </th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {PLAN_SCENARIO_TABLE_METRICS.map((metricId) => {
                      const metrics = metricLists.map(
                        (list) =>
                          list.find(({ id }) => id === metricId) ?? null,
                      )
                      const descriptor = metrics.find((metric) => metric)
                      if (!descriptor) return null
                      // Audit PD-12: a metric nobody reports is not a row.
                      if (
                        descriptor.baseline === null &&
                        metrics.every((metric) => metric?.candidate == null)
                      )
                        return null
                      const winnerIds = scenarioMetricWinnerIds(
                        descriptor,
                        metrics,
                        comparisons,
                      )
                      return (
                        <tr data-scenario-metric-id={metricId} key={metricId}>
                          <th
                            scope="row"
                            className="normal-case tracking-normal"
                          >
                            <span className="font-mono text-[0.8125rem] font-semibold text-ink">
                              {descriptor.label}
                            </span>
                          </th>
                          <td
                            className={
                              winnerIds.has(SCENARIO_BASELINE_ID)
                                ? 'is-winner'
                                : undefined
                            }
                            data-label="Baseline"
                          >
                            <span className="flex items-baseline gap-2 font-mono tabular-nums">
                              <b>
                                {formatPlanMetricValue(descriptor, 'baseline')}
                              </b>
                              {winnerIds.has(SCENARIO_BASELINE_ID) ? (
                                <span className="ds-label text-success">
                                  Best
                                </span>
                              ) : null}
                            </span>
                          </td>
                          {metrics.map((metric, index) => (
                            <td
                              className={
                                winnerIds.has(comparisons[index].id)
                                  ? 'is-winner'
                                  : undefined
                              }
                              data-label={comparisons[index].label}
                              key={comparisons[index].id}
                            >
                              {metric ? (
                                <span className="grid gap-0.5 font-mono tabular-nums">
                                  <span className="flex items-baseline gap-2">
                                    <b>
                                      {formatPlanMetricValue(
                                        metric,
                                        'candidate',
                                      )}
                                    </b>
                                    {winnerIds.has(comparisons[index].id) ? (
                                      <span className="ds-label text-success">
                                        Best
                                      </span>
                                    ) : null}
                                  </span>
                                  <small
                                    className={`text-label ${metricToneClass[metric.tone]}`}
                                  >
                                    {formatPlanMetricDelta(metric)}
                                  </small>
                                </span>
                              ) : (
                                <span className="text-ink-muted">—</span>
                              )}
                            </td>
                          ))}
                        </tr>
                      )
                    })}
                  </tbody>
                </DataTable>
              </div>
            </details>
          )
        })}
      </div>
    </section>
  )
}

/* ------------------------------------------------------------------ page */

export function LocalPlanDetailPage({ planId }: { planId: string }) {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [plan, setPlan] = useState<LocalPlan | null>(null)
  const [executionSummaries, setExecutionSummaries] = useState<
    Record<string, DashboardExecutionSummary>
  >({})
  const [historyLoading, setHistoryLoading] = useState(false)
  const [historyError, setHistoryError] = useState<string | null>(null)
  const [visualBaselineOverride, setVisualBaselineOverride] = useState<
    string | null
  >(null)
  const [excludedComparisonIds, setExcludedComparisonIds] = useState<string[]>(
    [],
  )
  const [selectedCandidateId, setSelectedCandidateId] = useState<string | null>(
    null,
  )
  const [baselineDetail, setBaselineDetail] =
    useState<DashboardExecutionDetail | null>(null)
  const [candidateDetail, setCandidateDetail] =
    useState<DashboardExecutionDetail | null>(null)
  const [comparisonLoading, setComparisonLoading] = useState(false)
  const [comparisonError, setComparisonError] = useState<string | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [starting, setStarting] = useState<PlanRunRole | null>(null)
  const [runFeedback, setRunFeedback] = useState<PlanRunFeedback | null>(null)

  const load = useCallback(async () => {
    const next = bridge ?? (await getDashboardDataBridge())
    setBridge(next)
    if (next.mode !== 'local')
      throw new Error('Local plans are available only in the local dashboard')
    setPlan(await next.getPlan(planId))
  }, [bridge, planId])

  useEffect(() => {
    void load()
      .catch((cause) => setLoadError(errorText(cause)))
      .finally(() => setLoading(false))
  }, [load])
  const running =
    plan !== null &&
    ['baseline_running', 'candidate_running'].includes(plan.state)
  // Audit PD-15: only the plan polls; summaries reload when the ids change.
  useEffect(() => {
    if (!running) return
    const timer = window.setInterval(() => {
      void load().catch(() => undefined)
    }, 2_000)
    return () => window.clearInterval(timer)
  }, [load, running])

  const executionIdKey = [
    plan?.baseline_execution_id ?? '',
    ...(plan?.candidate_execution_ids ?? []),
    ...(plan?.incomplete_execution_ids ?? []),
    plan?.last_attempt_id ?? '',
  ]
    .filter(Boolean)
    .join(' ')
  const executionIds = useMemo(
    () => (executionIdKey ? executionIdKey.split(' ') : []),
    [executionIdKey],
  )
  const comparableExecutionIds = useMemo(
    () =>
      [
        plan?.baseline_execution_id ?? '',
        ...(plan?.candidate_execution_ids ?? []),
      ].filter(Boolean),
    [plan?.baseline_execution_id, plan?.candidate_execution_ids],
  )
  const visualBaselineId =
    visualBaselineOverride &&
    comparableExecutionIds.includes(visualBaselineOverride)
      ? visualBaselineOverride
      : (plan?.baseline_execution_id ?? comparableExecutionIds[0] ?? null)
  const comparisonCandidateIds = useMemo(
    () =>
      comparableExecutionIds.filter(
        (id) => id !== visualBaselineId && !excludedComparisonIds.includes(id),
      ),
    [comparableExecutionIds, excludedComparisonIds, visualBaselineId],
  )

  useEffect(() => {
    setSelectedCandidateId((current) =>
      selectedPlanCandidate(current, false, comparisonCandidateIds),
    )
  }, [comparisonCandidateIds])

  useEffect(() => {
    if (!bridge || executionIds.length === 0) return
    let cancelled = false
    setHistoryLoading((current) => current || true)
    setHistoryError(null)
    void loadExecutionSummaries(bridge.listExecutions, executionIds)
      .then((summaries) => {
        if (cancelled) return
        setExecutionSummaries(summaries)
      })
      .catch((cause) => {
        if (cancelled) return
        setExecutionSummaries({})
        setHistoryError(errorText(cause))
      })
      .finally(() => {
        if (!cancelled) setHistoryLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [bridge, executionIds])
  // While a run is active, refresh its summary for progress without the
  // loading state flickering (the ids do not change until it completes).
  useEffect(() => {
    if (!bridge || !running || !plan?.last_attempt_id) return
    const attemptId = plan.last_attempt_id
    const timer = window.setInterval(() => {
      void loadExecutionSummaries(bridge.listExecutions, [attemptId])
        .then((summaries) =>
          setExecutionSummaries((current) => ({ ...current, ...summaries })),
        )
        .catch(() => undefined)
    }, 5_000)
    return () => window.clearInterval(timer)
  }, [bridge, plan?.last_attempt_id, running])

  useEffect(() => {
    const baselineId = visualBaselineId
    if (!bridge || !baselineId || !selectedCandidateId) {
      setBaselineDetail(null)
      setCandidateDetail(null)
      setComparisonError(null)
      setComparisonLoading(false)
      return
    }
    let cancelled = false
    setComparisonLoading(true)
    setComparisonError(null)
    void Promise.all([
      bridge.getExecution(baselineId),
      bridge.getExecution(selectedCandidateId),
    ])
      .then(([baseline, candidate]) => {
        if (cancelled) return
        setBaselineDetail(baseline)
        setCandidateDetail(candidate)
      })
      .catch((cause) => {
        if (cancelled) return
        setBaselineDetail(null)
        setCandidateDetail(null)
        setComparisonError(errorText(cause))
      })
      .finally(() => {
        if (!cancelled) setComparisonLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [bridge, selectedCandidateId, visualBaselineId])

  const start = async (role: PlanRunRole) => {
    if (!bridge || !plan) return
    setStarting(role)
    setRunFeedback({
      role,
      phase: 'starting',
      message: `Starting ${roleLabel(role).toLowerCase()}…`,
      executionId: null,
    })
    try {
      const nextPlan = await bridge.startPlan(plan.id, role)
      setPlan(nextPlan)
      setRunFeedback({
        role,
        phase: 'running',
        message: isRoleRunning(nextPlan, role)
          ? `${roleLabel(role)} is running. This page refreshes automatically while the report is collected.`
          : `${roleLabel(role)} started. Check the execution detail for the latest report.`,
        executionId: nextPlan.last_attempt_id,
      })
    } catch (cause) {
      setRunFeedback({
        role,
        phase: 'error',
        message: `Could not start ${roleLabel(role).toLowerCase()}: ${errorText(cause)}`,
        executionId: null,
      })
    } finally {
      setStarting(null)
    }
  }

  const comparison = useMemo(() => {
    if (!visualBaselineId || !selectedCandidateId) return null
    const detailsMatch =
      baselineDetail?.id === visualBaselineId &&
      candidateDetail?.id === selectedCandidateId
    const baseline = detailsMatch
      ? baselineDetail
      : executionSummaries[visualBaselineId]
    const candidate = detailsMatch
      ? candidateDetail
      : executionSummaries[selectedCandidateId]
    return buildPlanComparison(
      baseline,
      candidate,
      detailsMatch
        ? { baseline: baselineDetail, candidate: candidateDetail }
        : undefined,
    )
  }, [
    baselineDetail,
    candidateDetail,
    executionSummaries,
    selectedCandidateId,
    visualBaselineId,
  ])
  const changeVisualBaseline = (id: string) => {
    if (!plan || !comparableExecutionIds.includes(id)) return
    const availableCandidates = comparableExecutionIds.filter(
      (candidateId) =>
        candidateId !== id && !excludedComparisonIds.includes(candidateId),
    )
    setVisualBaselineOverride(id === plan.baseline_execution_id ? null : id)
    setSelectedCandidateId((current) =>
      current && current !== id && availableCandidates.includes(current)
        ? current
        : (availableCandidates.at(-1) ?? null),
    )
    setBaselineDetail(null)
    setCandidateDetail(null)
    setComparisonError(null)
  }
  const toggleComparisonCandidate = (id: string, selected: boolean) => {
    setExcludedComparisonIds((current) =>
      selected
        ? current.filter((item) => item !== id)
        : current.includes(id)
          ? current
          : [...current, id],
    )
    if (!selected && selectedCandidateId === id) {
      setSelectedCandidateId(
        comparisonCandidateIds
          .filter((candidateId) => candidateId !== id)
          .at(-1) ?? null,
      )
    }
  }
  const renameCandidate = async (executionId: string, label: string) => {
    if (!bridge || !plan) return
    const candidateLabels = { ...(plan.candidate_labels ?? {}) }
    const normalized = label.trim()
    if (normalized) candidateLabels[executionId] = normalized
    else delete candidateLabels[executionId]
    const nextPlan = await bridge.updatePlan(plan.id, {
      candidate_labels: candidateLabels,
    })
    setPlan(nextPlan)
  }

  const readiness = plan ? planReadiness(plan) : null
  const latestCandidateId = plan?.candidate_execution_ids.at(-1) ?? null
  const baselineSummary = plan?.baseline_execution_id
    ? (executionSummaries[plan.baseline_execution_id] ?? null)
    : null
  const lastRunSummary =
    (latestCandidateId ? executionSummaries[latestCandidateId] : null) ??
    baselineSummary

  return (
    <>
      <DashboardPageActions
        active="plans"
        context={plan ? plan.label || plan.id : undefined}
        actionsLabel="Plan actions"
        actions={
          plan?.baseline_execution_id && latestCandidateId ? (
            <a
              className={buttonClassName({
                variant: 'secondary',
                size: 'compact',
              })}
              href={hashForComparison(
                plan.baseline_execution_id,
                latestCandidateId,
              )}
            >
              open in compare
            </a>
          ) : null
        }
      />
      <div className="ds-root page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-16 md:w-[calc(100%_-_3rem)]">
        {loading ? (
          <div className="grid gap-3" aria-busy="true" role="status">
            <span className="ds-visually-hidden">Loading plan</span>
            {['first', 'second'].map((placeholder) => (
              <div
                key={placeholder}
                className="h-24 animate-pulse rounded-[6px] bg-[var(--surface-fill)] motion-reduce:animate-none"
              />
            ))}
          </div>
        ) : null}
        {loadError && !plan ? (
          <EmptyState
            tone="error"
            title="Plan unavailable"
            description={loadError}
            actions={
              <>
                <a
                  className={buttonClassName({ variant: 'secondary' })}
                  href={hashForPlans()}
                >
                  back to plans
                </a>
                <a
                  className={buttonClassName({ variant: 'primary' })}
                  href={hashForNewPlan()}
                >
                  new plan
                </a>
              </>
            }
          />
        ) : null}
        {plan && readiness ? (
          <div className="grid gap-5">
            <PageHeader
              breadcrumb={[
                { label: 'plans', href: hashForPlans() },
                { label: plan.label || plan.id },
              ]}
              title={plan.label || plan.id}
              summary={plan.purpose || ''}
              actions={
                <StatusBadge
                  status={readiness.status}
                  label={readiness.label}
                />
              }
            />
            <PlanLifecycle
              plan={plan}
              starting={starting}
              feedback={runFeedback}
              onStart={(role) => void start(role)}
              baselineSummary={baselineSummary}
              lastRunSummary={lastRunSummary}
            />
            <PlanScope plan={plan} />
            <PlanRunHistory
              plan={plan}
              summaries={executionSummaries}
              onRenameCandidate={renameCandidate}
            />
            <PlanExecutionHistory
              plan={plan}
              summaries={executionSummaries}
              visualBaselineId={visualBaselineId}
              comparisonCandidateIds={comparisonCandidateIds}
              selectedCandidateId={selectedCandidateId}
              scenarioComparison={
                comparisonLoading || comparisonError ? null : comparison
              }
              onVisualBaselineChange={changeVisualBaseline}
              onToggleCandidate={toggleComparisonCandidate}
              loading={historyLoading}
              error={historyError}
            />
          </div>
        ) : null}
      </div>
    </>
  )
}
