import { Tooltip, TooltipContent, TooltipTrigger } from '@iii-dev/console-ui'
import {
  Check,
  Copy,
  Download,
  ExternalLink,
  PencilLine,
  RotateCcw,
  Trash2,
  X,
} from 'lucide-react'
import {
  type FormEvent,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from 'react'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { DisclosureLayer } from '@/components/DisclosureLayer'
import { LiveProgressPanel } from '@/components/LiveProgressPanel'
import {
  DivergingBars,
  type DivergingGroup,
  Sparkline,
  type SparklinePoint,
} from '@/components/PlanCharts'
import { PlanProgress, Requirements } from '@/components/PlanStatus'
import {
  formatPrimaryMetric,
  PRIMARY_SUMMARY_METRICS,
  PrimaryMetricsView,
  primaryMetricChange,
  primaryMetricLabels,
} from '@/components/PrimaryMetricsView'
import { ScenarioChatAction } from '@/components/ScenarioChatAction'
import {
  buttonClassName,
  Callout,
  DataTable,
  DataTableRow,
  Dialog,
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
  type ImportedPlan,
  type JsonObject,
  type LocalPlan,
  type Plan,
} from '@/lib/dashboard-data-source'
import { definitionTitle, shortDefinition } from '@/lib/definition-digest'
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
  metricById,
  type PlanComparison,
  type PlanMetricComparison,
  type PlanMetricId,
  type PlanScenarioComparison,
} from '@/lib/plan-comparison'
import {
  downloadJson,
  type PlanExecution,
  type PlanRequirements,
  planAction,
} from '@/lib/plan-execution'
import {
  aggregatePrimaryMetrics,
  comparePrimaryMetrics,
  type MetricId,
  type PrimaryMetrics,
} from '@/lib/primary-metrics'
import {
  comparisonPrimaryMetrics,
  exportReleaseControlHistory,
  getImportedReference,
  listComparisonExecutions,
  type RcReference,
} from '@/lib/release-control-reference'
import { watchExecution } from '@/lib/watch-execution'

function object(value: unknown): JsonObject {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as JsonObject)
    : {}
}

function text(value: unknown) {
  return typeof value === 'string' ? value : ''
}

function count(value: unknown) {
  return typeof value === 'number' && Number.isInteger(value) ? value : null
}

function model(value: unknown) {
  const entry = object(value)
  return { provider: text(entry.provider), model: text(entry.model) }
}

function frozenSetup(reference: RcReference) {
  const plan = object(reference.execution.plan)
  const materialized = object(reference.materialized)
  const profile = object(materialized.profile)
  const campaigns = Array.isArray(materialized.campaigns)
    ? materialized.campaigns.map(object)
    : []
  const groups = campaigns.flatMap((campaign) =>
    Array.isArray(campaign.groups) ? campaign.groups.map(object) : [],
  )
  const scenarios = [
    ...new Set(
      groups
        .flatMap((group) =>
          Array.isArray(group.scenarios) ? group.scenarios : [],
        )
        .filter((id): id is string => typeof id === 'string'),
    ),
  ]
  const seeds = new Map<string, string>()
  for (const shard of reference.shards) {
    if (!Array.isArray(shard.runs)) continue
    for (const candidate of shard.runs.map(object)) {
      const scenario = text(candidate.scenario_id)
      const seed = candidate.seed
      if (scenario && (typeof seed === 'number' || typeof seed === 'string'))
        seeds.set(scenario, String(seed))
    }
  }
  return {
    subject: model(plan.subject),
    scenarios,
    repetitions: count(profile.repetitions),
    technicalRetries: count(profile.technical_retries),
    seeds,
  }
}

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
      detail: 'The saved scope is running against the captured baseline.',
    }
  if (!plan.baseline_execution_id)
    return plan.incomplete_execution_ids.length > 0
      ? {
          status: 'incomplete',
          label: 'baseline retry available · saved scope',
          detail:
            'The last attempt did not produce a report; retry the same saved scope.',
        }
      : {
          status: 'incomplete',
          label: 'draft · scope editable',
          detail:
            'The scope is defined but no completed baseline report exists.',
        }
  if (plan.candidate_execution_ids.length > 0)
    return {
      status: 'unavailable',
      label: 'comparison ready · saved scope',
      detail:
        'Candidate reports are ready to inspect against the captured baseline.',
    }
  return {
    status: 'unavailable',
    label: 'ready for candidate · saved scope',
    detail: 'Baseline captured; run the saved scope after your change.',
  }
}

export function nextPlanAction(plan: LocalPlan): PlanNextAction {
  if (isRoleRunning(plan, 'baseline')) {
    return {
      title: 'Baseline is running',
      detail:
        'The saved scope is executing. Wait for its report before starting a candidate.',
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
        'The same saved scope is executing against the captured baseline. This page refreshes automatically.',
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
        ? 'The previous attempt did not produce a report. Retry the same saved scope.'
        : 'Run this scope before the Harness change. Starting it captures the scope, seeds and policy.',
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
        'Review the latest execution first. You can run another candidate later with this same saved scope.',
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

export function PlanRunDialog({
  open,
  onClose,
  plan,
  starting,
  feedback,
  onStart,
  requirements,
  baselineSummary = null,
  lastRunSummary = null,
  importedScope,
}: {
  open: boolean
  onClose: () => void
  plan: LocalPlan | null
  importedScope?: React.ReactNode
  starting: PlanRunRole | null
  feedback: PlanRunFeedback | null
  onStart: (role: PlanRunRole) => void
  requirements?: PlanRequirements | null
  baselineSummary?: DashboardExecutionSummary | null
  lastRunSummary?: DashboardExecutionSummary | null
}) {
  const nextAction: PlanNextAction = plan
    ? nextPlanAction(plan)
    : {
        title: 'Run plan',
        detail:
          'Run the imported test scope on the current local Harness. Results are kept with this plan history.',
        role: 'baseline',
        actionLabel: 'Run plan',
        executionId: null,
        state: 'ready',
      }
  const role = nextAction.role
  const baselineAttention =
    baselineSummary &&
    buildExecutionPresentation(baselineSummary).attention === 'needs_attention'
  const lastRun = lastRunSentence(lastRunSummary)
  const action = !plan
    ? 'Run plan'
    : role === 'baseline'
      ? plan?.incomplete_execution_ids.length
        ? 'Retry baseline'
        : 'Run baseline'
      : `Run candidate #${plan?.candidate_execution_ids.length + 1}`
  return (
    <Dialog
      open={open}
      onClose={() => !starting && onClose()}
      title={role ? `${action}?` : nextAction.title}
      description={
        role === 'baseline'
          ? nextAction.detail
          : 'Run the saved test scope with the same model, seeds and policy.'
      }
      bodyPadding
      footer={
        <div className="flex flex-wrap justify-end gap-2">
          <button
            type="button"
            className={buttonClassName({ variant: 'secondary' })}
            disabled={starting !== null}
            onClick={onClose}
          >
            Cancel
          </button>
          {role ? (
            <button
              type="button"
              className={buttonClassName({ variant: 'primary' })}
              disabled={starting !== null || plan?.compatible === false}
              aria-busy={starting !== null}
              onClick={() => onStart(role)}
            >
              {starting ? 'Starting…' : action}
            </button>
          ) : nextAction.executionId ? (
            <a
              className={buttonClassName({ variant: 'primary' })}
              href={hashForExecution(nextAction.executionId)}
            >
              View active execution
            </a>
          ) : null}
        </div>
      }
    >
      <div className="grid gap-4">
        {plan ? (
          <p className="m-0 text-sm font-medium">{scopeSentence(plan)}</p>
        ) : (
          importedScope
        )}
        {lastRun ? (
          <p className="m-0 text-xs text-ink-muted">{lastRun}</p>
        ) : null}
        {baselineAttention ? (
          <Callout tone="warning" title="Baseline contains failing tests">
            This candidate will compare against that failing baseline.
          </Callout>
        ) : null}
        {nextAction.state === 'complete' && nextAction.executionId ? (
          <a
            className="text-sm text-ink underline"
            href={hashForExecution(nextAction.executionId)}
          >
            Review latest candidate
          </a>
        ) : null}
        {requirements ? <Requirements value={requirements} /> : null}
        {feedback && feedback.phase !== 'running' ? (
          <div data-plan-run-feedback={feedback.phase} aria-live="polite">
            <Callout
              tone={feedback.phase === 'error' ? 'danger' : 'info'}
              title={feedback.message}
            />
          </div>
        ) : null}
      </div>
    </Dialog>
  )
}

/* --------------------------------------------------------------- scope */

export function PlanScope({
  plan,
  reference,
}: {
  plan: Plan
  reference?: RcReference | null
}) {
  const frozen = reference ? frozenSetup(reference) : null
  const scope =
    plan.origin === 'remote'
      ? {
          scenarios: [],
          scenario_ids: frozen?.scenarios ?? [],
          runs: frozen?.repetitions,
          technical_retries: frozen?.technicalRetries,
          seed: null,
          ...model(object(plan.configuration).subject),
        }
      : plan
  const scenarios = scope.scenarios.length
    ? scope.scenarios.map((scenario) => ({
        id: scenario.scenario_id,
        definition: shortDefinition(scenario.behavior_sha256),
        title: definitionTitle(scenario.behavior_sha256),
      }))
    : scope.scenario_ids.map((id) => ({
        id,
        definition: null,
        title: undefined,
      }))
  return (
    <section
      id="plan-scope"
      className="min-w-0 rounded-[6px] bg-[var(--surface-fill)] p-5"
      aria-labelledby="plan-scope-title"
      data-plan-scope
    >
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <h2 id="plan-scope-title" className="m-0 text-base font-semibold">
          Test scope
        </h2>
        <span className="text-xs text-ink-muted">
          {scenarios.length} {scenarios.length === 1 ? 'test' : 'tests'} ·{' '}
          {scope.runs} {scope.runs === 1 ? 'run' : 'runs'} per test
        </span>
      </div>
      <dl className="m-0 mt-4 flex flex-wrap gap-x-8 gap-y-3 text-xs">
        {[
          ['Model', scope.model || 'Not set'],
          ['Provider', scope.provider || 'Not set'],
          ['Technical retries', scope.technical_retries],
          [
            'Seed',
            plan.origin === 'remote'
              ? 'Frozen per test'
              : (scope.seed ?? 'Canonical'),
          ],
        ].map(([label, value]) => (
          <div key={label} className="grid gap-1">
            <dt className="text-ink-muted">{label}</dt>
            <dd className="m-0 font-mono text-ink">{value}</dd>
          </div>
        ))}
      </dl>
      <ul className="m-0 mt-4 flex list-none flex-wrap gap-2 p-0">
        {scenarios.map((scenario) => (
          <li key={scenario.id}>
            <a
              className={buttonClassName({
                variant: 'secondary',
                size: 'compact',
              })}
              href={hashForTestHistory(scenario.id)}
              title={scenario.title}
            >
              {scenarioName(scenario.id)}
              {scenario.definition ? (
                <span className="font-mono text-label text-ink-muted">
                  {scenario.definition}
                </span>
              ) : null}
            </a>
          </li>
        ))}
      </ul>
    </section>
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
  detail: string
  summary: DashboardExecutionSummary | null
  fallback: 'running' | 'incomplete' | null
}

export function executionHistoryRows(
  plan: Plan,
  summaries: Record<string, DashboardExecutionSummary>,
  executionIds?: string[],
): ExecutionHistoryRow[] {
  const rows: ExecutionHistoryRow[] = []
  const retained = new Set<string>()
  if (plan.origin !== 'remote') {
    if (plan.baseline_execution_id) {
      retained.add(plan.baseline_execution_id)
      rows.push({
        id: plan.baseline_execution_id,
        role: 'baseline',
        detail: '',
        summary: summaries[plan.baseline_execution_id] ?? null,
        fallback: null,
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
        detail: 'Active execution',
        summary: summaries[plan.last_attempt_id] ?? null,
        fallback: 'running',
      })
    }
    for (
      let index = plan.candidate_execution_ids.length - 1;
      index >= 0;
      index--
    ) {
      const id = plan.candidate_execution_ids[index]
      if (retained.has(id)) continue
      retained.add(id)
      rows.push({
        id,
        role: 'candidate',
        detail:
          index === plan.candidate_execution_ids.length - 1 ? 'Latest' : '',
        summary: summaries[id] ?? null,
        fallback: null,
      })
    }
    for (const id of [...plan.incomplete_execution_ids].reverse()) {
      if (retained.has(id)) continue
      rows.push({
        id,
        role: 'attempt',
        detail: 'Incomplete results',
        summary: summaries[id] ?? null,
        fallback: 'incomplete',
      })
    }
  }
  for (const id of executionIds ??
    (plan.origin === 'remote' ? plan.execution_ids : [])) {
    if (rows.some((row) => row.id === id)) continue
    rows.push({
      id,
      role: 'attempt',
      detail: '',
      summary: summaries[id] ?? null,
      fallback: null,
    })
  }
  return rows.sort((a, b) => {
    const left = Date.parse(a.summary?.started_at ?? '')
    const right = Date.parse(b.summary?.started_at ?? '')
    return (
      (Number.isFinite(left) ? left : Infinity) -
      (Number.isFinite(right) ? right : Infinity)
    )
  })
}

function executionHistoryLabel(
  plan: Plan,
  row: Pick<ExecutionHistoryRow, 'id' | 'summary'>,
) {
  const label =
    row.summary?.execution_label ||
    (plan.origin !== 'remote' ? plan.candidate_labels?.[row.id] : null)
  if (label) return label
  const recordedLabel = row.summary?.label
  if (recordedLabel && recordedLabel !== plan.label && recordedLabel !== row.id)
    return recordedLabel
  return (
    row.summary?.subjects
      .map((subject) => subject.model)
      .filter(Boolean)
      .join(', ') ||
    (plan.origin !== 'remote' && row.summary?.origin !== 'remote'
      ? plan.model
      : 'Execution')
  )
}

export function selectedPlanCandidate(
  current: string | null,
  pinned: boolean,
  candidateIds: string[],
) {
  if (pinned && current && candidateIds.includes(current)) return current
  return candidateIds.at(-1) ?? null
}

export function planExecutionLabel(
  plan: Plan,
  executionId: string | null,
  summary?: DashboardExecutionSummary | null,
) {
  if (!executionId) return 'Execution unavailable'
  if (summary) return executionHistoryLabel(plan, { id: executionId, summary })
  if (plan.origin === 'remote') {
    const index = plan.execution_ids.indexOf(executionId)
    return index < 0 ? executionId : `Imported execution #${index + 1}`
  }
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
            aria-label={
              saving ? 'Saving execution name' : 'Save execution name'
            }
            title="Save execution name"
            type="submit"
          >
            <Check aria-hidden="true" size={14} />
          </button>
          <button
            className={buttonClassName({
              variant: 'secondary',
              size: 'compact',
            })}
            disabled={saving}
            aria-label="Cancel rename"
            title="Cancel rename"
            type="button"
            onClick={() => {
              setDraft(label)
              setError(null)
              setEditing(false)
            }}
          >
            <X aria-hidden="true" size={14} />
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

type RunMetric = 'score' | 'tokens' | 'duration' | 'turns' | 'calls' | 'errors'

function executionMetricNumber(
  summary: DashboardExecutionSummary | null,
  metric: RunMetric,
) {
  if (metric === 'score') {
    const scores =
      summary?.subjects.flatMap((subject) =>
        subject.scenarios.map((scenario) =>
          finiteExecutionMetric(scenario.mean_score),
        ),
      ) ?? []
    return scores.length > 0 && scores.every((score) => score !== null)
      ? scores.reduce((total, score) => total + score, 0) / scores.length
      : null
  }
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
  if (metric === 'score')
    return numeric.toLocaleString('en-US', { maximumFractionDigits: 2 })
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
 * comparison.
 */
export function PlanRunHistory({
  plan,
  summaries,
  onRenameExecution,
  executionIds,
}: {
  plan: Plan
  executionIds?: string[]
  summaries: Record<string, DashboardExecutionSummary>
  onRenameExecution?: (executionId: string, label: string) => Promise<void>
}) {
  const rows = executionHistoryRows(plan, summaries, executionIds)
  if (rows.length === 0) return null
  const metrics = RUN_METRICS.filter(({ id }) =>
    rows.some((row) => executionMetricNumber(row.summary, id) !== null),
  )

  return (
    <section
      id="plan-executions"
      className="min-w-0"
      aria-labelledby="plan-run-history-title"
      data-plan-run-history
    >
      <div className="mb-4 flex flex-wrap items-baseline justify-between gap-2">
        <h2 id="plan-run-history-title" className="m-0 text-base font-semibold">
          Executions
        </h2>
        <span className="text-xs text-ink-muted">
          {rows.length} {rows.length === 1 ? 'execution' : 'executions'}
        </span>
      </div>
      <DataTable caption={`Plan runs, ${rows.length}`} collapse>
        <thead>
          <tr>
            <th scope="col">Run</th>
            <th scope="col">Result</th>
            <th scope="col" className={numericCellClassName}>
              Score / 100
            </th>
            {metrics.map((metric) => (
              <th key={metric.id} scope="col" className={numericCellClassName}>
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
            const displayLabel = executionHistoryLabel(plan, row)
            const imported = row.summary?.origin === 'remote'
            const startedAt = row.summary?.started_at
            const canRename =
              imported || (plan.origin !== 'remote' && row.role === 'candidate')
            return (
              <DataTableRow
                key={`${row.role}:${row.id}`}
                data-run-role={row.role}
                data-execution-id={row.id}
              >
                <td data-label="Run">
                  <span className="grid gap-1">
                    <span className="flex flex-wrap items-center gap-2">
                      <strong className="font-mono text-[0.8125rem] text-ink">
                        {displayLabel}
                      </strong>
                      <span className="rounded bg-panel px-1.5 py-0.5 text-label text-ink-muted">
                        {imported ? 'release-control' : 'local'}
                      </span>
                      {canRename && onRenameExecution ? (
                        <ExecutionNameControl
                          executionId={row.id}
                          fallbackLabel={displayLabel}
                          label={
                            imported
                              ? (row.summary?.execution_label ?? '')
                              : plan.origin !== 'remote'
                                ? (plan.candidate_labels?.[row.id] ?? '')
                                : ''
                          }
                          onRename={onRenameExecution}
                        />
                      ) : null}
                    </span>
                    {startedAt && Number.isFinite(Date.parse(startedAt)) ? (
                      <time
                        className="text-label text-ink-muted"
                        dateTime={startedAt}
                      >
                        {formatDate(startedAt)}
                      </time>
                    ) : (
                      <span className="text-label text-ink-muted">
                        Execution date unavailable
                      </span>
                    )}
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
                <td data-label="Score / 100" className={numericCellClassName}>
                  {executionMetricValue(row.summary, 'score')}
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
                  </a>
                </td>
              </DataTableRow>
            )
          })}
        </tbody>
      </DataTable>
    </section>
  )
}

export { PlanRunHistory as PlanNonComparableAttempts }

/* ----------------------------------------------------------- comparison */

/** Every metric the plan compares, in the order the all-metrics layer lists
 *  them. Audit PD-12 still applies: a metric no column reports is not a row. */
export const PLAN_COMPARISON_TABLE_METRICS = [
  'coverage',
  'technical_failures',
  'tokens',
  'tokens_per_completion',
  'failed_attempt_tokens',
  'duration',
  'cost',
  'function_calls',
  'function_errors',
  'turns',
] as const

const PLAN_SCENARIO_TABLE_METRICS: PlanMetricId[] = [
  'duration',
  'tokens',
  'tokens_per_completion',
  'failed_attempt_tokens',
  'cost',
  'turns',
  'function_calls',
  'function_errors',
]

const PLAN_SCENARIO_SUMMARY_METRICS: PlanMetricId[] = [
  'duration',
  'tokens',
  'turns',
]

type ComparisonColumn = {
  row: ExecutionHistoryRow
  isVisualBaseline: boolean
  rowComparison: PlanComparison | null
  metricSource: PlanComparison | null
  selected: boolean
}

type MetricRow = {
  id: PlanMetricId
  entries: Array<{
    column: ComparisonColumn
    metric: PlanMetricComparison | null
    side: 'baseline' | 'candidate'
    value: number | null
  }>
}

export type PlanComparisonInput = {
  plan: Plan
  executionIds?: string[]
  summaries: Record<string, DashboardExecutionSummary>
  visualBaselineId: string | null
  comparisonCandidateIds: string[]
  selectedCandidateId: string | null
  scenarioComparison?: PlanComparison | null
}

export type PlanComparisonModel = {
  rows: ExecutionHistoryRow[]
  baseline: DashboardExecutionSummary | null
  selectableRows: ExecutionHistoryRow[]
  columns: ComparisonColumn[]
  selectedColumn: ComparisonColumn | undefined
  headline: PlanComparison | null
  scenarioComparisons: Array<{
    id: string
    label: string
    comparison: PlanComparison
  }>
  metricRows: MetricRow[]
  hiddenMetrics: number
}

/** Everything the overview and its layers read: one derivation, shared, so
 *  the tiles, the chart, the tables and the scents never disagree. */
export function buildPlanComparisonModel(
  input: PlanComparisonInput,
): PlanComparisonModel | null {
  const {
    plan,
    summaries,
    executionIds,
    visualBaselineId,
    comparisonCandidateIds,
    selectedCandidateId,
    scenarioComparison = null,
  } = input
  if (!visualBaselineId || comparisonCandidateIds.length === 0) return null
  const rows = executionHistoryRows(plan, summaries, executionIds)
  const baseline = visualBaselineId
    ? (summaries[visualBaselineId] ?? null)
    : null
  const selectableRows = rows.filter((row) => row.fallback !== 'running')
  const visualBaselineRow = rows.find((row) => row.id === visualBaselineId)
  const comparisonRows = rows.filter((row) =>
    comparisonCandidateIds.includes(row.id),
  )
  const columns: ComparisonColumn[] = [
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
  const scenarioComparisons = columns.flatMap((column) => {
    if (column.isVisualBaseline) return []
    const comparison =
      column.selected && scenarioComparison
        ? scenarioComparison
        : column.rowComparison
    return comparison
      ? [
          {
            id: column.row.id,
            label: planExecutionLabel(plan, column.row.id, column.row.summary),
            comparison,
          },
        ]
      : []
  })
  const selectedColumn = columns.find((column) => column.selected)
  const metricRows: MetricRow[] = PLAN_COMPARISON_TABLE_METRICS.map((id) => {
    const entries = columns.map((column) => {
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
  return {
    rows,
    baseline,
    selectableRows,
    columns,
    selectedColumn,
    headline: selectedColumn?.rowComparison ?? null,
    scenarioComparisons,
    metricRows,
    hiddenMetrics: PLAN_COMPARISON_TABLE_METRICS.length - metricRows.length,
  }
}

export type PlanTrendTile = {
  id: MetricId
  label: string
  value: string
  partial: boolean
  delta: string | null
  tone: PlanMetricComparison['tone']
  reference: number | null
  points: SparklinePoint[]
}

function formatWith(
  descriptor: PlanMetricComparison | null,
  value: number | null,
) {
  if (value === null) return 'not reported'
  return descriptor
    ? formatPlanMetricValue({ ...descriptor, baseline: value }, 'baseline')
    : String(value)
}

/** One tile per metric: the selected candidate's value, its delta against the
 *  reference, and every completed execution in capture order behind it. */
export function planTrendTiles(
  plan: Plan,
  model: PlanComparisonModel,
  visualBaselineId: string | null,
  metricsByExecution: Record<string, PrimaryMetrics>,
  excludeEmptyTests = false,
): PlanTrendTile[] {
  const orderedIds = [
    ...new Set(
      [
        ...(plan.origin === 'remote'
          ? plan.execution_ids
          : [plan.baseline_execution_id, ...plan.candidate_execution_ids]),
        ...model.rows
          .filter((row) => row.fallback !== 'running')
          .map((row) => row.id),
      ].filter((id): id is string => Boolean(id)),
    ),
  ]
  const running =
    plan.origin !== 'remote' &&
    ['baseline_running', 'candidate_running'].includes(plan.state) &&
    plan.last_attempt_id &&
    !orderedIds.includes(plan.last_attempt_id)
      ? plan.last_attempt_id
      : null
  const selected = model.selectedColumn
  const baseline = visualBaselineId
    ? metricsByExecution[visualBaselineId]
    : undefined
  const candidate = selected ? metricsByExecution[selected.row.id] : undefined
  const selectedMetrics =
    baseline && candidate
      ? comparePrimaryMetrics(baseline, candidate, excludeEmptyTests)
      : null
  const scope = new Set(selectedMetrics?.tests.map((test) => test.key))
  const historyMetrics = Object.fromEntries(
    orderedIds.map((executionId) => {
      const metrics = metricsByExecution[executionId]
      return [
        executionId,
        metrics && excludeEmptyTests && selectedMetrics
          ? comparePrimaryMetrics(
              selectedMetrics.baseline,
              aggregatePrimaryMetrics(
                metrics.tests.filter((test) => scope.has(test.key)),
              ),
              false,
            ).candidate
          : metrics,
      ]
    }),
  )
  return PRIMARY_SUMMARY_METRICS.map((id) => {
    const a = selectedMetrics?.baseline.metrics[id] ?? baseline?.metrics[id]
    const b = selectedMetrics?.candidate.metrics[id] ?? candidate?.metrics[id]
    const points: SparklinePoint[] = orderedIds.map((executionId) => {
      const metric =
        executionId === visualBaselineId
          ? a
          : historyMetrics[executionId]?.metrics[id]
      const value = metric?.value ?? metric?.observed ?? null
      return {
        id: executionId,
        label: `${planExecutionLabel(plan, executionId, model.rows.find((row) => row.id === executionId)?.summary)} · ${formatPrimaryMetric(id, value)}${metric?.value === null && metric.observed !== null ? ' · Partial' : ''}`,
        value,
        role:
          executionId === visualBaselineId
            ? 'baseline'
            : executionId === selected?.row.id
              ? 'selected'
              : 'other',
      }
    })
    if (running)
      points.push({
        id: running,
        label: 'running',
        value: null,
        role: 'running',
      })
    const metric = selected ? b : a
    return {
      id,
      label: primaryMetricLabels[id],
      value: formatPrimaryMetric(id, metric?.value ?? metric?.observed ?? null),
      partial: metric?.value === null && metric.observed !== null,
      delta: selected ? primaryMetricChange(id, a, b).label : null,
      tone:
        metric?.value == null && metric?.observed == null
          ? 'unavailable'
          : 'neutral',
      reference: a?.value ?? a?.observed ?? null,
      points,
    }
  })
}

/** A metric moved when its delta rounds to something a reader can see: a
 *  tenth of a second on a five-minute run prints as "−0.0%" and is noise. */
function hasMoved(metric: PlanMetricComparison): boolean {
  if (metric.delta === null || Math.abs(metric.delta) <= 1e-9) return false
  return metric.delta_percent === null || Math.abs(metric.delta_percent) >= 0.05
}

export function planMovementGroups(
  baseline: PrimaryMetrics | undefined,
  candidate: PrimaryMetrics | undefined,
  metricId: MetricId,
  excludeEmptyTests = false,
): DivergingGroup[] {
  if (!baseline || !candidate) return []
  return comparePrimaryMetrics(
    baseline,
    candidate,
    excludeEmptyTests,
  ).tests.map((test) => {
    const a = test.baseline?.metrics[metricId]
    const b = test.candidate?.metrics[metricId]
    const change = primaryMetricChange(metricId, a, b)
    const partial = [a, b].some(
      (metric) => metric?.value === null && metric.observed !== null,
    )
    return {
      id: test.key,
      title: scenarioName(test.label),
      subtitle: `A ${formatPrimaryMetric(metricId, a?.value ?? a?.observed ?? null)} · B ${formatPrimaryMetric(metricId, b?.value ?? b?.observed ?? null)}${partial ? ' · Partial' : ''}`,
      rows: [
        {
          id: metricId,
          label: '',
          change: change.percentage,
          valueLabel: change.label,
        },
      ],
      unchanged: '',
    }
  })
}

const LAYER_SEPARATOR = ' \u00a0·\u00a0 '

/** Closed-row scents for the layers under the overview (audit ED-26). */
export function planLayerScents(model: PlanComparisonModel): {
  byTest: string
  metrics: string
} {
  const selected =
    model.scenarioComparisons.find(
      (entry) => entry.id === model.selectedColumn?.row.id,
    ) ?? model.scenarioComparisons[0]
  const byTest = selected
    ? selected.comparison.scenarios
        .map((scenario) => {
          const moved = [...scenario.metrics, ...scenario.execution_metrics]
            .filter(
              (metric) =>
                PLAN_SCENARIO_SUMMARY_METRICS.includes(
                  metric.id as PlanMetricId,
                ) && hasMoved(metric),
            )
            .map(
              (metric) =>
                `${metric.label.toLowerCase()} ${formatPlanMetricDelta(metric).split(' · ').at(-1)}`,
            )
          return [scenarioName(scenario.id).toLowerCase(), ...moved].join(' · ')
        })
        .join(LAYER_SEPARATOR)
    : 'exact values per test once a candidate completes'
  const reference = model.columns[0]
  const candidate = model.selectedColumn ?? model.columns[1]
  const metrics = model.metricRows.map((row) => {
    const left = row.entries.find((entry) => entry.column === reference)
    const right = row.entries.find((entry) => entry.column === candidate)
    const descriptor = left?.metric ?? right?.metric ?? null
    const label = descriptor?.label.toLowerCase() ?? titleCase(row.id)
    return `${label} ${formatWith(descriptor, left?.value ?? null)} → ${formatWith(descriptor, right?.value ?? null)}`
  })
  if (model.hiddenMetrics > 0)
    metrics.push(
      `${model.hiddenMetrics} metric${model.hiddenMetrics === 1 ? '' : 's'} not reported`,
    )
  return { byTest, metrics: metrics.join(' · ') }
}

/** Audit PD-05 / ED-26: the plan's comparison is the page. Layer 0 is the
 *  filter row, the observations, the trend tiles and what moved by test; exact
 *  tables open on demand below. */
export function PlanExecutionHistory({
  plan,
  summaries,
  executionIds,
  visualBaselineId,
  comparisonCandidateIds,
  selectedCandidateId,
  scenarioComparison = null,
  metricsByExecution,
  excludeEmptyTests = false,
  onVisualBaselineChange,
  onToggleCandidate,
  loading,
  error = null,
}: PlanComparisonInput & {
  metricsByExecution: Record<string, PrimaryMetrics>
  excludeEmptyTests?: boolean
  onVisualBaselineChange: (id: string) => void
  onToggleCandidate: (id: string, selected: boolean) => void
  loading: boolean
  error?: string | null
}) {
  const [selectedMetric, setSelectedMetric] = useState<MetricId>('score')
  const model = buildPlanComparisonModel({
    plan,
    summaries,
    executionIds,
    visualBaselineId,
    comparisonCandidateIds,
    selectedCandidateId,
    scenarioComparison,
  })
  if (!model) return null
  const tiles = planTrendTiles(
    plan,
    model,
    visualBaselineId,
    metricsByExecution,
    excludeEmptyTests,
  )
  const groups = planMovementGroups(
    visualBaselineId ? metricsByExecution[visualBaselineId] : undefined,
    selectedCandidateId ? metricsByExecution[selectedCandidateId] : undefined,
    selectedMetric,
    excludeEmptyTests,
  )
  const referenceLabel = visualBaselineId
    ? planExecutionLabel(plan, visualBaselineId, summaries[visualBaselineId])
    : 'reference'
  const candidateLabel = selectedCandidateId
    ? planExecutionLabel(
        plan,
        selectedCandidateId,
        summaries[selectedCandidateId],
      )
    : 'candidate'
  const executionCount = tiles[0]?.points.length ?? 0

  return (
    <Panel
      as="section"
      padding="default"
      aria-labelledby="plan-execution-history-title"
      data-plan-comparison
    >
      <h2 id="plan-execution-history-title" className="ds-visually-hidden">
        baseline and candidates
      </h2>
      {/* One filter row scopes every chart and table below it. It changes the
          visual reference only; the official baseline stays with the plan. */}
      <div
        className="flex flex-wrap items-center justify-between gap-3"
        data-plan-filter-row
      >
        <div className="flex flex-wrap items-center gap-2">
          <label
            className="inline-flex items-center gap-2 font-mono text-xs text-ink-muted"
            htmlFor="plan-visual-baseline"
          >
            reference
            <Select
              id="plan-visual-baseline"
              value={visualBaselineId ?? ''}
              disabled={loading || model.selectableRows.length === 0}
              onChange={(event) => onVisualBaselineChange(event.target.value)}
            >
              {model.selectableRows.map((row) => (
                <option key={row.id} value={row.id}>
                  {planExecutionLabel(plan, row.id, row.summary)}
                </option>
              ))}
            </Select>
          </label>
          <fieldset className="m-0 flex min-w-0 flex-wrap items-center gap-2 p-0">
            <legend className="ds-visually-hidden">Compare candidates</legend>
            <span className="ds-label">candidates</span>
            {model.selectableRows
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
                    <span>{planExecutionLabel(plan, row.id, row.summary)}</span>
                  </label>
                )
              })}
          </fieldset>
        </div>
        <span className="font-mono text-label text-ink-muted">
          {loading
            ? 'loading…'
            : 'recorded execution history never changes here'}
        </span>
      </div>
      {error ? (
        <div className="mt-4">
          <Callout tone="warning" title="Execution metrics could not be loaded">
            Retained ids and report links remain available.{' '}
            <span className="font-mono">{error}</span>
          </Callout>
        </div>
      ) : null}
      {model.headline ? (
        <div className="mt-5" data-plan-observations>
          <p className="m-0 text-sm leading-6">
            <strong className="font-semibold">
              {model.headline.headline}.
            </strong>{' '}
            <span className="text-ink-soft">{model.headline.detail}</span>
          </p>
        </div>
      ) : null}
      <div className="mt-5 grid gap-3" data-plan-trend>
        <div className="flex flex-wrap items-baseline justify-between gap-3">
          <span className="ds-label">
            Group metrics ·{' '}
            {selectedCandidateId
              ? `${candidateLabel} vs ${referenceLabel}`
              : referenceLabel}
          </span>
          <span className="font-mono text-label text-ink-muted">
            {executionCount} executions · gray is A · hollow means no metric or
            still running
          </span>
        </div>
        <div className="grid min-w-0 gap-3 @[560px]:grid-cols-2 @[960px]:grid-cols-3">
          {tiles.map((tile) => (
            <article
              className="grid min-w-0 gap-1.5 rounded-[6px] bg-panel p-3"
              key={tile.id}
              data-trend-metric={tile.id}
            >
              <span className="ds-label">{tile.label}</span>
              <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
                <strong className="font-mono text-xl font-semibold tracking-tight text-ink">
                  {tile.value}
                </strong>
                {tile.delta ? (
                  <span
                    className={`font-mono text-label ${metricToneClass[tile.tone]}`}
                  >
                    {tile.delta}
                  </span>
                ) : null}
              </div>
              {tile.partial ? (
                <span className="text-xs text-ink-muted">
                  {tile.id === 'score' ? 'Partial mean' : 'Observed subtotal'}
                </span>
              ) : null}
              <Sparkline
                points={tile.points}
                reference={tile.reference}
                label={`${tile.label} across executions`}
              />
            </article>
          ))}
        </div>
      </div>
      {groups.length > 0 ? (
        <div className="mt-5 grid min-w-0 gap-2" data-plan-what-moved>
          <div className="flex flex-wrap items-baseline justify-between gap-3">
            <span className="ds-label">
              Change by test · relative change vs {referenceLabel.toLowerCase()}{' '}
              · left decreases, right increases
            </span>
            <label
              className="inline-flex items-center gap-2 text-sm"
              htmlFor="plan-movement-metric"
            >
              Metric
              <Select
                id="plan-movement-metric"
                value={selectedMetric}
                onChange={(event) =>
                  setSelectedMetric(event.target.value as MetricId)
                }
              >
                {Object.entries(primaryMetricLabels).map(([id, label]) => (
                  <option key={id} value={id}>
                    {label}
                  </option>
                ))}
              </Select>
            </label>
          </div>
          <div className="rounded-[6px] bg-panel px-4 pt-3 pb-2">
            <DivergingBars
              groups={groups}
              label={`Relative change in ${primaryMetricLabels[selectedMetric]} per test, ${candidateLabel} against ${referenceLabel}`}
            />
            <p className="mt-2 mb-0 font-mono text-label text-ink-muted">
              Bars show percentage change relative to A. Exact A/B values appear
              with each test.
            </p>
          </div>
        </div>
      ) : null}
    </Panel>
  )
}

/** The all-metrics table: every metric in rows, the reference and each
 *  selected candidate in columns. */
export function PlanMetricsTable({
  plan,
  model,
}: {
  plan: Plan
  model: PlanComparisonModel
}) {
  return (
    <div className="min-w-0">
      <DataTable
        caption="Plan metrics in rows, with the visual baseline and selected candidates in columns."
        minWidth={`${12 + model.columns.length * 12}rem`}
        collapse
      >
        <thead>
          <tr>
            <th scope="col">Metric</th>
            {model.columns.map(({ row, isVisualBaseline, selected }) => (
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
                    {planExecutionLabel(plan, row.id, row.summary)}
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
          {model.metricRows.map(({ id, entries }) => {
            const descriptor = entries.find(({ metric }) => metric)?.metric
            return (
              <tr data-metric-id={id} key={id}>
                <th scope="row" className="normal-case tracking-normal">
                  <span className="grid gap-0.5">
                    <strong className="font-mono text-[0.8125rem] font-semibold text-ink">
                      {descriptor?.label ?? titleCase(id)}
                    </strong>
                  </span>
                </th>
                {entries.map(({ column, metric, side }) => {
                  return (
                    <td
                      className={
                        [
                          column.isVisualBaseline ? 'is-baseline' : '',
                          column.selected ? 'is-selected' : '',
                        ]
                          .filter(Boolean)
                          .join(' ') || undefined
                      }
                      data-execution-id={column.row.id}
                      data-label={planExecutionLabel(
                        plan,
                        column.row.id,
                        column.row.summary,
                      )}
                      key={column.row.id}
                    >
                      {metric ? (
                        <span className="grid gap-0.5 font-mono tabular-nums">
                          <span className="flex items-baseline gap-2">
                            <strong>
                              {formatPlanMetricValue(metric, side)}
                            </strong>
                          </span>
                          {!column.isVisualBaseline && column.rowComparison ? (
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
      {model.hiddenMetrics > 0 ? (
        <p className="mt-2 mb-0 font-mono text-label text-ink-muted">
          {model.hiddenMetrics} metric{model.hiddenMetrics === 1 ? '' : 's'} not
          reported by this harness · hidden
        </p>
      ) : null}
    </div>
  )
}

/** The layers under the overview: criterion evidence by test and
 *  run statistics. */
export function PlanComparisonLayers(props: PlanComparisonInput) {
  const [open, setOpen] = useState<{ byTest: boolean; metrics: boolean }>({
    byTest: false,
    metrics: false,
  })
  const model = buildPlanComparisonModel(props)
  if (!model) return null
  const scents = planLayerScents(model)
  const scenarioCount = new Set(
    model.scenarioComparisons.flatMap(({ comparison }) =>
      comparison.scenarios.map((scenario) => scenario.id),
    ),
  ).size
  return (
    <>
      {scenarioCount > 0 ? (
        <DisclosureLayer
          id="plan-by-test"
          label={`by test · ${scenarioCount} ${scenarioCount === 1 ? 'test' : 'tests'}`}
          scent={scents.byTest}
          open={open.byTest}
          onToggle={(next) =>
            setOpen((current) => ({ ...current, byTest: next }))
          }
        >
          <div className="grid min-w-0 gap-4">
            <PlanScenarioComparisonTable
              comparisons={model.scenarioComparisons}
            />
          </div>
        </DisclosureLayer>
      ) : null}
      <DisclosureLayer
        id="plan-diagnostic-metrics"
        label={`Run statistics · ${model.metricRows.length}`}
        scent={scents.metrics}
        open={open.metrics}
        onToggle={(next) =>
          setOpen((current) => ({ ...current, metrics: next }))
        }
      >
        <PlanMetricsTable plan={props.plan} model={model} />
      </DisclosureLayer>
    </>
  )
}

/* ---------------------------------------------------------- provenance */

function shortHash(value: string) {
  return value.length > 22 ? `${value.slice(0, 19)}…` : value
}

export function planProvenanceEntries(
  plan: LocalPlan,
): Array<[string, string]> {
  const rows: Array<[string, string | null | undefined]> = [
    ['plan id', plan.id],
    ['scope hash', plan.scope_hash],
    ['endpoint', plan.url],
    ['created', formatDate(plan.created_at)],
    ['updated', plan.updated_at ? formatDate(plan.updated_at) : null],
    ['official baseline', plan.baseline_execution_id],
    [
      'candidates',
      plan.candidate_execution_ids.length
        ? plan.candidate_execution_ids.join(' · ')
        : null,
    ],
    ...plan.scenarios.map((scenario): [string, string] => [
      `${scenario.scenario_id} · ${shortDefinition(scenario.behavior_sha256)}`,
      [
        scenario.case_id ? `case ${scenario.case_id}` : null,
        `seed ${scenario.seed}`,
        scenario.contract_sha256
          ? `contract ${shortHash(scenario.contract_sha256)}`
          : null,
        scenario.inputs_sha256
          ? `inputs ${shortHash(scenario.inputs_sha256)}`
          : null,
      ]
        .filter(Boolean)
        .join(' · '),
    ]),
  ]
  return rows.filter((row): row is [string, string] => Boolean(row[1]))
}

export function planProvenanceScent(plan: LocalPlan): string {
  return [
    plan.id,
    `scope ${shortHash(plan.scope_hash)}`,
    `endpoint ${plan.url}`,
    `created ${formatDate(plan.created_at)}`,
    plan.updated_at ? `updated ${formatDate(plan.updated_at)}` : null,
  ]
    .filter(Boolean)
    .join(' · ')
}

/** Raw fields and immutable identity, the same shape the execution page uses. */
export function PlanProvenance({ plan }: { plan: LocalPlan }) {
  return (
    <dl
      className="m-0 grid min-w-0 grid-cols-[max-content_minmax(0,1fr)] gap-x-6 gap-y-2 font-mono text-xs"
      data-plan-provenance
    >
      {planProvenanceEntries(plan).map(([key, value]) => (
        <div key={key} className="contents">
          <dt className="ds-label">{key}</dt>
          <dd className="m-0 break-all text-ink">{value}</dd>
        </div>
      ))}
    </dl>
  )
}

function scenarioMetrics(scenario: PlanScenarioComparison) {
  const available = [...scenario.metrics, ...scenario.execution_metrics]
  return [
    ...PLAN_SCENARIO_TABLE_METRICS,
    ...available
      .filter((metric) => metric.id.startsWith('criterion:'))
      .map((metric) => metric.id),
  ].flatMap((id) => {
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
      {/* The layer row above is the heading; this names what the tables add. */}
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h3 id="plan-by-test-title" className="ds-label m-0">
            exact values
          </h3>
          <p className="mt-1 mb-0 text-xs leading-5 text-ink-soft">
            Evidence and consumption per test. Expand a row for exact values and
            deltas. Criterion means include retained points from incomplete
            tasks. Criterion differences use matched repetitions only. Missing
            evidence is not zero.
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
                  Retained observations
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
                    {[
                      ...new Set(
                        metricLists.flatMap((list) =>
                          list.map((metric) => metric.id),
                        ),
                      ),
                    ].map((metricId) => {
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
                          <td data-label="Baseline">
                            <span className="flex items-baseline gap-2 font-mono tabular-nums">
                              <b>
                                {formatPlanMetricValue(descriptor, 'baseline')}
                              </b>
                            </span>
                            {descriptor.evidence ? (
                              <small className="font-mono text-label text-ink-muted">
                                {descriptor.evidence.baseline_observed}/
                                {descriptor.evidence.baseline_planned ?? '—'}{' '}
                                evaluated
                              </small>
                            ) : null}
                          </td>
                          {metrics.map((metric, index) => (
                            <td
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
                                  </span>
                                  <small
                                    className={`text-label ${metricToneClass[metric.tone]}`}
                                  >
                                    {formatPlanMetricDelta(metric)}
                                  </small>
                                  {metric.evidence ? (
                                    <small className="text-label text-ink-muted">
                                      {metric.evidence.candidate_observed}/
                                      {metric.evidence.candidate_planned ?? '—'}{' '}
                                      evaluated · {metric.evidence.paired}{' '}
                                      matched repetitions
                                      {metric.evidence.paired > 0 ? (
                                        <>
                                          {' '}
                                          · paired means{' '}
                                          {formatPlanMetricValue(
                                            {
                                              ...metric,
                                              baseline:
                                                metric.evidence.paired_baseline,
                                            },
                                            'baseline',
                                          )}{' '}
                                          →{' '}
                                          {formatPlanMetricValue(
                                            {
                                              ...metric,
                                              candidate:
                                                metric.evidence
                                                  .paired_candidate,
                                            },
                                            'candidate',
                                          )}
                                        </>
                                      ) : null}
                                    </small>
                                  ) : null}
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
  const [requirements, setRequirements] = useState<PlanRequirements | null>(
    null,
  )
  const [activeExecution, setActiveExecution] = useState<PlanExecution | null>(
    null,
  )
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [plan, setPlan] = useState<LocalPlan | null>(null)
  const [importedPlan, setImportedPlan] = useState<ImportedPlan | null>(null)
  const [reference, setReference] = useState<RcReference | null>(null)
  const [relatedExecutionIds, setRelatedExecutionIds] = useState<string[]>([])
  const [updatingHistory, setUpdatingHistory] = useState(false)
  const displayPlan = importedPlan ?? plan
  const frozen = reference ? frozenSetup(reference) : null
  const configuredSubject = model(object(importedPlan?.configuration).subject)
  const runSubject =
    configuredSubject.provider && configuredSubject.model
      ? configuredSubject
      : frozen?.subject

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
  const [executionDetails, setExecutionDetails] = useState<
    Record<string, DashboardExecutionDetail>
  >({})
  const [excludeEmptyTests, setExcludeEmptyTests] = useState(false)
  const [trendLoading, setTrendLoading] = useState(false)
  const [trendError, setTrendError] = useState<string | null>(null)
  const [comparisonLoading, setComparisonLoading] = useState(false)
  const [comparisonError, setComparisonError] = useState<string | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [starting, setStarting] = useState<PlanRunRole | null>(null)
  const [runOpen, setRunOpen] = useState(false)
  const [deleteOpen, setDeleteOpen] = useState(false)
  const [deleting, setDeleting] = useState(false)
  const [runFeedback, setRunFeedback] = useState<PlanRunFeedback | null>(null)
  // Audit ED-26: the layers under the overview open on demand; the plan route
  // carries no anchor, so the reader's toggles are the only state.
  const [openLayers, setOpenLayers] = useState<Record<string, boolean>>({})

  const load = useCallback(async () => {
    const next = bridge ?? (await getDashboardDataBridge())
    setBridge(next)
    const [loaded, allPlans, executions] = await Promise.all([
      next.getPlan(planId),
      next.listPlans(),
      listComparisonExecutions(),
    ])
    const remotePlans = allPlans.plans.filter(
      (entry): entry is ImportedPlan =>
        entry.origin === 'remote' &&
        (entry.id === loaded.id ||
          (loaded.origin !== 'remote' &&
            executions.some(
              (execution) =>
                entry.execution_ids.includes(execution.id) &&
                execution.run_id === loaded.reference_execution_id,
            ))),
    )
    const importedIds = new Set(
      remotePlans.flatMap((entry) => entry.execution_ids),
    )
    const sourceIds = new Set(
      executions
        .filter((entry) => importedIds.has(entry.id))
        .map((entry) => entry.run_id),
    )
    const localPlans = allPlans.plans.filter(
      (entry): entry is LocalPlan =>
        entry.origin !== 'remote' &&
        (entry.id === loaded.id ||
          Boolean(
            entry.reference_execution_id &&
              sourceIds.has(entry.reference_execution_id),
          )),
    )
    const localPlanIds = new Set(localPlans.map((entry) => entry.id))
    const eligible = executions.filter(
      (entry) =>
        importedIds.has(entry.id) ||
        (entry.origin !== 'remote' &&
          typeof entry.plan_id === 'string' &&
          localPlanIds.has(entry.plan_id)),
    )
    setRelatedExecutionIds(eligible.map((entry) => entry.id))
    setExecutionSummaries(
      Object.fromEntries(eligible.map((entry) => [entry.id, entry])),
    )
    setImportedPlan(loaded.origin === 'remote' ? loaded : null)
    if (loaded.origin === 'remote') {
      const latest = executions
        .filter((entry) => loaded.execution_ids.includes(entry.id))
        .sort(
          (a, b) =>
            Date.parse(b.started_at ?? '') - Date.parse(a.started_at ?? ''),
        )[0]
      const snapshot = latest ? await getImportedReference(latest.id) : null
      setReference(snapshot)
      const configured = model(object(loaded.configuration).subject)
      const subject =
        configured.provider && configured.model
          ? configured
          : snapshot
            ? frozenSetup(snapshot).subject
            : configured
      const linked = localPlans
        .filter(
          (entry) =>
            entry.reference_execution_id === latest?.run_id &&
            entry.model === subject.model &&
            entry.provider === subject.provider,
        )
        .sort((a, b) => b.updated_at.localeCompare(a.updated_at))
      setPlan(linked[0] ?? null)
    } else {
      setPlan(loaded)
      setReference(null)
    }
  }, [bridge, planId])

  useEffect(() => {
    void load()
      .catch((cause) => setLoadError(errorText(cause)))
      .finally(() => setLoading(false))
  }, [load])
  const running =
    plan !== null &&
    ['baseline_running', 'candidate_running'].includes(plan.state)
  const localPlanId = plan?.id
  // Audit PD-15: only the plan polls; summaries reload when the ids change.
  useEffect(() => {
    if (!running || !bridge || !localPlanId) return
    const timer = window.setInterval(() => {
      void bridge
        .getPlan(localPlanId)
        .then((next) => {
          if (next.origin === 'remote') return
          setPlan(next)
          if (!['baseline_running', 'candidate_running'].includes(next.state))
            void load()
        })
        .catch(() => undefined)
    }, 2_000)
    return () => window.clearInterval(timer)
  }, [bridge, load, running, localPlanId])

  useEffect(() => {
    if (!bridge || !running || !plan?.last_attempt_id?.startsWith('plan-')) {
      setActiveExecution(null)
      return
    }
    let live = true
    const refresh = () =>
      planAction<PlanExecution>(bridge, {
        action: 'execution',
        execution_id: plan.last_attempt_id,
      })
        .then((value) => {
          if (live) setActiveExecution(value)
        })
        .catch(() => undefined)
    void refresh()
    const timer = setInterval(() => void refresh(), 2000)
    return () => {
      live = false
      clearInterval(timer)
    }
  }, [bridge, running, plan?.last_attempt_id])
  const exportPlan = async () => {
    if (!bridge || !plan) return
    try {
      downloadJson(
        await planAction(bridge, { action: 'export', plan_id: plan.id }),
        `${plan.id}.json`,
      )
    } catch (cause) {
      setLoadError(errorText(cause))
    }
  }
  const cancelPlan = async () => {
    if (!bridge || !activeExecution) return
    try {
      setActiveExecution(
        await planAction<PlanExecution>(bridge, {
          action: 'cancel',
          execution_id: activeExecution.id,
        }),
      )
      await load()
    } catch (cause) {
      setLoadError(errorText(cause))
    }
  }
  const deletePlan = async () => {
    if (!bridge || !plan) return
    setDeleting(true)
    setLoadError(null)
    try {
      await bridge.deletePlan(plan.id)
      window.location.hash = hashForPlans()
    } catch (cause) {
      setLoadError(errorText(cause))
    } finally {
      setDeleting(false)
    }
  }

  const executionIdKey = [
    ...relatedExecutionIds,
    ...(importedPlan?.execution_ids ?? []),
    plan?.baseline_execution_id ?? '',
    ...(plan?.candidate_execution_ids ?? []),
    ...(plan?.incomplete_execution_ids ?? []),
    plan?.last_attempt_id ?? '',
  ]
    .filter(Boolean)
    .join('\u0000')
  const executionIds = useMemo(
    () => (executionIdKey ? executionIdKey.split('\u0000') : []),
    [executionIdKey],
  )
  const comparableExecutionIds = useMemo(
    () =>
      displayPlan
        ? executionHistoryRows(displayPlan, executionSummaries, [
            ...new Set(executionIds),
          ]).map((row) => row.id)
        : [],
    [displayPlan, executionIds, executionSummaries],
  )
  const visualBaselineId =
    visualBaselineOverride &&
    comparableExecutionIds.includes(visualBaselineOverride)
      ? visualBaselineOverride
      : (comparableExecutionIds[0] ?? null)
  const comparisonCandidateIds = useMemo(
    () =>
      comparableExecutionIds.filter(
        (id) => id !== visualBaselineId && !excludedComparisonIds.includes(id),
      ),
    [comparableExecutionIds, visualBaselineId, excludedComparisonIds],
  )

  useEffect(() => {
    setSelectedCandidateId((current) =>
      current === ''
        ? ''
        : selectedPlanCandidate(current, true, comparisonCandidateIds),
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
    let active = true
    const stop = watchExecution(bridge, attemptId, async () => {
      const summaries = await loadExecutionSummaries(bridge.listExecutions, [
        attemptId,
      ])
      if (active)
        setExecutionSummaries((current) => ({ ...current, ...summaries }))
    })
    return () => {
      active = false
      stop()
    }
  }, [bridge, plan?.last_attempt_id, running])

  const baselineDetail = visualBaselineId
    ? (executionDetails[visualBaselineId] ?? null)
    : null
  const candidateDetail = selectedCandidateId
    ? (executionDetails[selectedCandidateId] ?? null)
    : null

  useEffect(() => {
    if (!bridge || !visualBaselineId) return
    let cancelled = false
    setComparisonLoading(true)
    setComparisonError(null)
    void Promise.all(
      [visualBaselineId, selectedCandidateId]
        .filter((id): id is string => Boolean(id))
        .map(async (id) => [id, await bridge.getExecution(id)] as const),
    )
      .then((entries) => {
        if (!cancelled)
          setExecutionDetails((current) => ({
            ...current,
            ...Object.fromEntries(entries),
          }))
      })
      .catch((cause) => {
        if (!cancelled) setComparisonError(errorText(cause))
      })
      .finally(() => {
        if (!cancelled) setComparisonLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [bridge, selectedCandidateId, visualBaselineId])

  useEffect(() => {
    if (!bridge || !openLayers.trends) return
    let cancelled = false
    setTrendLoading(true)
    setTrendError(null)
    void Promise.allSettled(
      comparableExecutionIds.map(
        async (id) => [id, await bridge.getExecution(id)] as const,
      ),
    )
      .then((results) => {
        if (cancelled) return
        setExecutionDetails((current) => ({
          ...current,
          ...Object.fromEntries(
            results.flatMap((result) =>
              result.status === 'fulfilled' ? [result.value] : [],
            ),
          ),
        }))
        const failed = results.find((result) => result.status === 'rejected')
        if (failed?.status === 'rejected')
          setTrendError(errorText(failed.reason))
      })
      .finally(() => {
        if (!cancelled) setTrendLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [bridge, comparableExecutionIds, openLayers.trends])

  useEffect(() => {
    const activeId = plan?.last_attempt_id
    if (!bridge || !running || !activeId) return
    let active = true
    const stop = watchExecution(bridge, activeId, async () => {
      const detail = await bridge.getExecution(activeId)
      if (active)
        setExecutionDetails((current) => ({ ...current, [activeId]: detail }))
    })
    return () => {
      active = false
      stop()
    }
  }, [bridge, running, plan?.last_attempt_id])

  const start = async (role: PlanRunRole) => {
    if (!bridge || (!plan && !reference)) return
    setStarting(role)
    setRunFeedback({
      role,
      phase: 'starting',
      message: `Starting ${roleLabel(role).toLowerCase()}…`,
      executionId: null,
    })
    try {
      let runPlan = plan
      if (!runPlan) {
        if (!reference || !importedPlan) return
        runPlan = await planAction<LocalPlan>(bridge, {
          action: 'reproduce_reference',
          reference_execution_id: reference.execution.id,
          label: importedPlan.label,
          subject: runSubject,
          materialized: reference.materialized,
          shards: reference.shards,
        })
      }
      setPlan(runPlan)
      const checked = await planAction<PlanRequirements>(bridge, {
        action: 'requirements',
        plan_id: runPlan.id,
      })
      setRequirements(checked)
      if (!checked.ready) {
        setRunFeedback({
          role,
          phase: 'error',
          message:
            'Execution requirements need attention. Your saved plan is preserved.',
          executionId: null,
        })
        return
      }
      const nextPlan = await bridge.startPlan(runPlan.id, role)
      setPlan(nextPlan)
      setSelectedCandidateId(nextPlan.last_attempt_id)
      setRunOpen(false)
      setRunFeedback({
        role,
        phase: 'running',
        message: isRoleRunning(nextPlan, role)
          ? `${roleLabel(role)} is running. This page refreshes automatically while the report is collected.`
          : `${roleLabel(role)} started. Check the execution detail for the latest report.`,
        executionId: nextPlan.last_attempt_id,
      })
      void load().catch((cause) => setLoadError(errorText(cause)))
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

  const metricsByExecution = useMemo(
    () =>
      Object.fromEntries(
        Object.entries(executionDetails).flatMap(([id, detail]) => {
          const metrics = comparisonPrimaryMetrics(detail, null).baseline
          return metrics ? [[id, metrics] as const] : []
        }),
      ),
    [executionDetails],
  )
  const baselineMetrics = visualBaselineId
    ? metricsByExecution[visualBaselineId]
    : undefined
  const candidateMetrics = selectedCandidateId
    ? metricsByExecution[selectedCandidateId]
    : undefined

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
    if (id === visualBaselineId || !comparableExecutionIds.includes(id)) return
    const availableCandidates = comparableExecutionIds.filter(
      (candidateId) =>
        candidateId !== id && !excludedComparisonIds.includes(candidateId),
    )
    setVisualBaselineOverride(id)
    setSelectedCandidateId((current) =>
      current && current !== id && availableCandidates.includes(current)
        ? current
        : (availableCandidates.at(-1) ?? null),
    )
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
  const renameExecution = async (executionId: string, label: string) => {
    if (!bridge) return
    if (executionSummaries[executionId]?.origin === 'remote') {
      await bridge.planControl({
        action: 'rename_imported_execution',
        execution_id: executionId,
        label,
      })
      const summaries = await loadExecutionSummaries(bridge.listExecutions, [
        executionId,
      ])
      setExecutionSummaries((current) => ({ ...current, ...summaries }))
      return
    }
    if (!plan) return
    const candidateLabels = { ...(plan.candidate_labels ?? {}) }
    const normalized = label.trim()
    if (normalized) candidateLabels[executionId] = normalized
    else delete candidateLabels[executionId]
    const nextPlan = await bridge.updatePlan(plan.id, {
      candidate_labels: candidateLabels,
    })
    setPlan(nextPlan)
  }

  const readiness = plan
    ? planReadiness(plan)
    : importedPlan
      ? {
          status: 'unavailable' as const,
          label: 'imported local copy',
          detail: '',
        }
      : null
  const latestCandidateId = plan?.candidate_execution_ids.at(-1) ?? null
  const baselineSummary = plan?.baseline_execution_id
    ? (executionSummaries[plan.baseline_execution_id] ?? null)
    : null
  const lastRunSummary =
    running && plan?.last_attempt_id
      ? (executionSummaries[plan.last_attempt_id] ?? null)
      : ((latestCandidateId ? executionSummaries[latestCandidateId] : null) ??
        baselineSummary)
  const comparisonInput = displayPlan
    ? {
        plan: displayPlan,
        executionIds: comparableExecutionIds,
        summaries: executionSummaries,
        visualBaselineId,
        comparisonCandidateIds,
        selectedCandidateId,
        scenarioComparison:
          comparisonLoading || comparisonError ? null : comparison,
      }
    : null

  function executionChoiceLabel(id: string) {
    const summary = executionSummaries[id]
    return [
      summary?.origin === 'remote' ? 'Release Control' : 'Local',
      displayPlan
        ? executionHistoryLabel(displayPlan, { id, summary: summary ?? null })
        : 'Execution',
      summary?.started_at ? formatDate(summary.started_at) : null,
      summary ? titleCase(summary.status) : null,
    ]
      .filter(Boolean)
      .join(' · ')
  }

  const updateHistory = async () => {
    if (!bridge || !importedPlan) return
    setUpdatingHistory(true)
    try {
      await bridge.planControl({
        action: 'import_history',
        history: await exportReleaseControlHistory(
          importedPlan.source.plan_key,
        ),
      })
      await load()
    } catch (cause) {
      setLoadError(errorText(cause))
    } finally {
      setUpdatingHistory(false)
    }
  }

  return (
    <>
      <DashboardPageActions
        active="plans"
        context={displayPlan ? displayPlan.label || displayPlan.id : undefined}
        actionsLabel="Plan actions"
        actions={
          comparableExecutionIds.length > 1 ? (
            <button
              type="button"
              className={buttonClassName({
                variant: 'secondary',
                size: 'compact',
              })}
              onClick={() =>
                document
                  .getElementById('plan-metrics')
                  ?.scrollIntoView({ block: 'start' })
              }
            >
              View comparison
            </button>
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
        {loadError && !displayPlan ? (
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
        {displayPlan && readiness ? (
          <div className="grid gap-5">
            <PageHeader
              className="pm-page-header execution-header"
              breadcrumb={[
                { label: 'plans', href: hashForPlans() },
                { label: displayPlan.label || displayPlan.id },
              ]}
              title={displayPlan.label || displayPlan.id}
              summary={
                <>
                  <StatusBadge
                    status={readiness.status}
                    label={readiness.label}
                  />
                  <span>{displayPlan.purpose}</span>
                </>
              }
              actions={
                <>
                  {running && plan?.last_attempt_id ? (
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <a
                          className={buttonClassName({ variant: 'secondary' })}
                          href={hashForExecution(plan.last_attempt_id)}
                          aria-label="View active execution"
                        >
                          <ExternalLink size={16} aria-hidden="true" />
                        </a>
                      </TooltipTrigger>
                      <TooltipContent>View active execution</TooltipContent>
                    </Tooltip>
                  ) : (
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <button
                          type="button"
                          className={buttonClassName({ variant: 'secondary' })}
                          aria-label={
                            plan?.baseline_execution_id
                              ? 'Re-run plan'
                              : 'Run plan'
                          }
                          disabled={
                            starting !== null ||
                            running ||
                            (!plan &&
                              (!frozen?.scenarios.length ||
                                frozen.repetitions === null ||
                                frozen.technicalRetries === null ||
                                !runSubject?.model ||
                                !runSubject.provider ||
                                !frozen.scenarios.every((scenario) =>
                                  frozen.seeds.has(scenario),
                                ))) ||
                            plan?.compatible === false
                          }
                          onClick={() => {
                            setRunFeedback(null)
                            setRequirements(null)
                            setRunOpen(true)
                          }}
                        >
                          <RotateCcw size={16} aria-hidden="true" />
                        </button>
                      </TooltipTrigger>
                      <TooltipContent>
                        {plan?.baseline_execution_id
                          ? 'Re-run plan'
                          : 'Run plan'}
                      </TooltipContent>
                    </Tooltip>
                  )}
                  {importedPlan ? (
                    <button
                      type="button"
                      className={buttonClassName({ variant: 'quiet' })}
                      disabled={updatingHistory}
                      onClick={() => void updateHistory()}
                    >
                      {updatingHistory ? 'Updating…' : 'Update history'}
                    </button>
                  ) : null}
                  {plan && !importedPlan ? (
                    <>
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <a
                            className={buttonClassName({ variant: 'quiet' })}
                            href={`${hashForNewPlan()}/edit/${plan.id}`}
                            aria-label="Edit plan"
                          >
                            <PencilLine size={16} aria-hidden="true" />
                          </a>
                        </TooltipTrigger>
                        <TooltipContent>Edit plan</TooltipContent>
                      </Tooltip>
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <a
                            className={buttonClassName({ variant: 'quiet' })}
                            href={`${hashForNewPlan()}/duplicate/${plan.id}`}
                            aria-label="Duplicate plan"
                          >
                            <Copy size={16} aria-hidden="true" />
                          </a>
                        </TooltipTrigger>
                        <TooltipContent>Duplicate plan</TooltipContent>
                      </Tooltip>
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <button
                            type="button"
                            className={buttonClassName({ variant: 'quiet' })}
                            onClick={() => void exportPlan()}
                            aria-label="Export plan"
                          >
                            <Download size={16} aria-hidden="true" />
                          </button>
                        </TooltipTrigger>
                        <TooltipContent>Export plan</TooltipContent>
                      </Tooltip>
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <button
                            type="button"
                            className={buttonClassName({ variant: 'quiet' })}
                            onClick={() => setDeleteOpen(true)}
                            aria-label="Delete plan"
                          >
                            <Trash2 size={16} aria-hidden="true" />
                          </button>
                        </TooltipTrigger>
                        <TooltipContent>Delete plan</TooltipContent>
                      </Tooltip>
                    </>
                  ) : null}
                </>
              }
            />
            {loadError ? (
              <Callout tone="warning" title="Plan action unavailable">
                {loadError}
              </Callout>
            ) : null}
            <PlanRunDialog
              open={runOpen}
              onClose={() => setRunOpen(false)}
              plan={plan}
              importedScope={
                frozen ? (
                  <div className="grid gap-3 text-sm">
                    <p>
                      {frozen.scenarios.length} tests · {frozen.repetitions} run
                      per test · {runSubject?.provider} / {runSubject?.model}
                    </p>
                    <p>
                      Technical retries: {frozen.technicalRetries} · Seeds:{' '}
                      {[...frozen.seeds]
                        .map(([scenario, seed]) => `${scenario} ${seed}`)
                        .join(' · ')}
                    </p>
                    {runSubject?.model !== frozen.subject.model ? (
                      <Callout tone="info">
                        The retained execution used {frozen.subject.provider} /{' '}
                        {frozen.subject.model}. This run uses the current plan
                        model shown above.
                      </Callout>
                    ) : null}
                  </div>
                ) : undefined
              }
              starting={starting}
              feedback={runFeedback}
              onStart={(role) => void start(role)}
              requirements={requirements}
              baselineSummary={baselineSummary}
              lastRunSummary={lastRunSummary}
            />
            {plan?.compatible === false ? (
              <Callout tone="warning" title="Saved scope unavailable">
                A saved scenario contract is unavailable in this runner. The
                plan and its evidence remain available.
              </Callout>
            ) : null}
            {lastRunSummary?.live_progress ? (
              <LiveProgressPanel
                progress={lastRunSummary.live_progress}
                running={running}
              />
            ) : null}
            {lastRunSummary?.live_progress_error ? (
              <p className="text-sm text-warning" role="status">
                {lastRunSummary.live_progress_error}
              </p>
            ) : null}
            {activeExecution ? (
              <>
                <PlanProgress execution={activeExecution} />
                <button
                  className={buttonClassName({
                    variant: 'secondary',
                    className: 'justify-self-start',
                  })}
                  disabled={activeExecution.state === 'cancelling'}
                  type="button"
                  onClick={() => void cancelPlan()}
                >
                  Cancel execution
                </button>
              </>
            ) : null}
            {visualBaselineId ? (
              <section
                id="plan-metrics"
                className="min-w-0"
                aria-label="Plan execution metrics"
              >
                <h2 className="pm-results-title">
                  {selectedCandidateId
                    ? 'Compare executions'
                    : 'Execution results'}
                </h2>
                <div className="pm-execution-selectors">
                  <label
                    className="grid min-w-0 gap-2 text-sm"
                    htmlFor="metrics-baseline"
                  >
                    A · Reference
                    <Select
                      id="metrics-baseline"
                      title={executionChoiceLabel(visualBaselineId)}
                      value={visualBaselineId}
                      onChange={(event) =>
                        changeVisualBaseline(event.target.value)
                      }
                    >
                      {comparableExecutionIds.map((id) => (
                        <option key={id} value={id}>
                          {executionChoiceLabel(id)}
                        </option>
                      ))}
                    </Select>
                  </label>
                  <label
                    className="grid min-w-0 gap-2 text-sm"
                    htmlFor="metrics-candidate"
                  >
                    B · Compare with
                    <Select
                      id="metrics-candidate"
                      title={
                        selectedCandidateId
                          ? executionChoiceLabel(selectedCandidateId)
                          : 'Execution only'
                      }
                      value={selectedCandidateId ?? ''}
                      onChange={(event) =>
                        setSelectedCandidateId(event.target.value)
                      }
                    >
                      <option value="">Execution only</option>
                      {comparisonCandidateIds.map((id) => (
                        <option key={id} value={id}>
                          {executionChoiceLabel(id)}
                        </option>
                      ))}
                    </Select>
                  </label>
                  <a
                    className={buttonClassName({
                      variant: 'quiet',
                      size: 'compact',
                    })}
                    href={hashForExecution(visualBaselineId)}
                  >
                    Open execution A
                  </a>
                  {selectedCandidateId ? (
                    <a
                      className={buttonClassName({
                        variant: 'quiet',
                        size: 'compact',
                      })}
                      href={hashForExecution(selectedCandidateId)}
                    >
                      Open execution B
                    </a>
                  ) : null}
                </div>
                {comparisonLoading ? (
                  <p role="status" className="text-sm text-ink-muted">
                    Loading execution metrics…
                  </p>
                ) : null}
                {comparisonError ? (
                  <Callout tone="warning" title="Execution metrics unavailable">
                    {comparisonError}
                  </Callout>
                ) : null}
                {!comparisonLoading &&
                !comparisonError &&
                baselineMetrics &&
                (!selectedCandidateId || candidateMetrics) ? (
                  <PrimaryMetricsView
                    showTests
                    summaryOnly
                    excludeEmptyTests={excludeEmptyTests}
                    onExcludeEmptyTestsChange={setExcludeEmptyTests}
                    key={`${visualBaselineId}:${selectedCandidateId ?? ''}`}
                    baseline={baselineMetrics}
                    baselineExecutionId={visualBaselineId}
                    candidateExecutionId={selectedCandidateId ?? undefined}
                    candidate={candidateMetrics ?? undefined}
                    baselineLabel={executionChoiceLabel(visualBaselineId)}
                    candidateLabel={
                      selectedCandidateId
                        ? executionChoiceLabel(selectedCandidateId)
                        : undefined
                    }
                  />
                ) : null}
              </section>
            ) : null}
            <div className="grid min-w-0 gap-3">
              <PlanRunHistory
                plan={displayPlan}
                executionIds={comparableExecutionIds}
                summaries={executionSummaries}
                onRenameExecution={renameExecution}
              />
              <PlanScope plan={displayPlan} reference={reference} />
              {comparisonInput && comparisonCandidateIds.length > 0 ? (
                <DisclosureLayer
                  id="plan-trends"
                  label="Execution history and diagnostics"
                  scent="Trends, result contracts and recorded evidence"
                  open={openLayers.trends ?? false}
                  onToggle={(open) =>
                    setOpenLayers((current) => ({ ...current, trends: open }))
                  }
                >
                  <PlanExecutionHistory
                    {...comparisonInput}
                    metricsByExecution={metricsByExecution}
                    excludeEmptyTests={excludeEmptyTests}
                    onVisualBaselineChange={changeVisualBaseline}
                    onToggleCandidate={toggleComparisonCandidate}
                    loading={
                      historyLoading || trendLoading || comparisonLoading
                    }
                    error={historyError || trendError || comparisonError}
                  />
                  <PlanComparisonLayers {...comparisonInput} />
                </DisclosureLayer>
              ) : null}
              {plan ? (
                <DisclosureLayer
                  id="plan-provenance"
                  label="provenance"
                  scent={planProvenanceScent(plan)}
                  open={openLayers.provenance ?? false}
                  onToggle={(open) =>
                    setOpenLayers((current) => ({
                      ...current,
                      provenance: open,
                    }))
                  }
                >
                  <PlanProvenance plan={plan} />
                </DisclosureLayer>
              ) : importedPlan ? (
                <p className="text-xs text-ink-muted">
                  Release Control · {importedPlan.source.instance_id} ·{' '}
                  {importedPlan.source.plan_key}
                </p>
              ) : null}
            </div>
          </div>
        ) : null}
      </div>
      <Dialog
        open={deleteOpen}
        onClose={() => !deleting && setDeleteOpen(false)}
        size="sm"
        title="Delete plan?"
        description={`This permanently removes ${plan?.label || plan?.id || 'this plan'} and its plan history. Any active execution is cancelled first.`}
        footer={
          <div className="flex justify-end gap-2">
            <button
              type="button"
              className={buttonClassName({ variant: 'secondary' })}
              disabled={deleting}
              onClick={() => setDeleteOpen(false)}
            >
              cancel
            </button>
            <button
              type="button"
              className={buttonClassName({ variant: 'primary' })}
              disabled={deleting}
              aria-busy={deleting}
              onClick={() => void deletePlan()}
            >
              {deleting ? 'deleting…' : 'delete plan'}
            </button>
          </div>
        }
      />
    </>
  )
}
