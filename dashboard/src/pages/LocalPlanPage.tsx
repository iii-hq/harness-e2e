import { ExternalLink, Info, PencilLine } from 'lucide-react'
import {
  type CSSProperties,
  type FormEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { DiscardDraftDialog } from '@/components/DiscardDraftDialog'
import {
  ExecutionSetup,
  ExecutionSetupReview,
  requestQuickExecution,
} from '@/components/ExecutionSetup'
import { ScenarioChatAction } from '@/components/ScenarioChatAction'
import { buttonClassName } from '@/design-system'
import { useDirtyNavigation } from '@/hooks/use-dirty-navigation'
import {
  hashForExecution,
  hashForNewPlan,
  hashForPlan,
  hashForPlans,
  hashForWorkspace,
} from '@/hooks/use-hash-route'
import {
  type DashboardDataBridge,
  type DashboardExecutionDetail,
  type DashboardExecutionSummary,
  getDashboardDataBridge,
  type JsonObject,
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

type Model = { provider: string; model: string }
type Catalog = { url: string; models: Model[]; scenarios: string[] }

function modelKey(model: Model) {
  return [model.provider, model.model].join('\n')
}

function modelGroups(models: Model[]) {
  const groups = new Map<string, Model[]>()
  for (const model of models) {
    const entries = groups.get(model.provider) ?? []
    if (!entries.some((entry) => entry.model === model.model))
      entries.push(model)
    groups.set(model.provider, entries)
  }
  return [...groups.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([provider, entries]) => ({
      provider,
      models: entries.sort((left, right) =>
        left.model.localeCompare(right.model),
      ),
    }))
}

function catalogValue(value: JsonObject): Catalog {
  const models = Array.isArray(value.models)
    ? value.models.flatMap((candidate) => {
        if (!candidate || typeof candidate !== 'object') return []
        const item = candidate as JsonObject
        return typeof item.provider === 'string' &&
          typeof item.model === 'string'
          ? [{ provider: item.provider, model: item.model }]
          : []
      })
    : []
  const localScenarioIds = new Set(
    Array.isArray(value.local_scenarios)
      ? value.local_scenarios.flatMap((candidate) => {
          if (!candidate || typeof candidate !== 'object') return []
          const id = (candidate as JsonObject).id
          return typeof id === 'string' ? [id] : []
        })
      : [],
  )
  return {
    url: typeof value.url === 'string' ? value.url : '',
    models,
    scenarios: Array.isArray(value.scenarios)
      ? value.scenarios.filter(
          (item): item is string =>
            typeof item === 'string' && !localScenarioIds.has(item),
        )
      : [],
  }
}

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

function scenarioName(scenario: string) {
  return scenario
    .replace(/[_.]+/g, ' ')
    .replace(/\b\w/g, (character) => character.toUpperCase())
}

function stateLabel(plan: LocalPlan) {
  if (
    plan.locked &&
    plan.state === 'draft' &&
    plan.incomplete_execution_ids.length
  )
    return 'Baseline attempt incomplete · retry available'
  return plan.state.replaceAll('_', ' ')
}

/** New-plan form defaults. One object feeds both the state and the dirty
 * baseline, so an untouched form can never start out dirty (audit PN-01). */
export const PLAN_FORM_DEFAULTS = {
  label: '',
  purpose: '',
  url: '',
  subject: '',
  judge: '',
  // Composite workflows contain non-repeatable steps; zero is the safe,
  // valid default for every new local plan.
  technicalRetries: '0',
  scenarios: [] as string[],
  testQuery: '',
  runs: '1',
  seed: '',
}

export type PlanFormValues = typeof PLAN_FORM_DEFAULTS

export function planFormDirty(
  current: PlanFormValues,
  initial: PlanFormValues,
) {
  return (
    current.label !== initial.label ||
    current.purpose !== initial.purpose ||
    current.url !== initial.url ||
    current.subject !== initial.subject ||
    current.judge !== initial.judge ||
    current.testQuery !== initial.testQuery ||
    current.runs !== initial.runs ||
    current.technicalRetries !== initial.technicalRetries ||
    current.seed !== initial.seed ||
    current.scenarios.join('\u0000') !== initial.scenarios.join('\u0000')
  )
}

type PlanRunRole = 'baseline' | 'candidate'

type PlanRunFeedback = {
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

function roleLabel(role: PlanRunRole) {
  return role === 'baseline' ? 'Baseline' : 'Candidate'
}

function isRoleRunning(plan: LocalPlan, role: PlanRunRole) {
  return plan.state === `${role}_running`
}

function nextPlanAction(plan: LocalPlan): PlanNextAction {
  if (isRoleRunning(plan, 'baseline')) {
    return {
      title: 'Baseline is running',
      detail:
        'The locked scope is executing. Wait for its report before starting a candidate.',
      role: null,
      actionLabel: 'View active execution',
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
      actionLabel: 'View active execution',
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
      actionLabel: retry ? 'Retry baseline' : 'Run baseline',
      executionId: null,
      state: 'ready',
    }
  }
  if (plan.candidate_execution_ids.length > 0) {
    const latestCandidate = plan.candidate_execution_ids.at(-1) ?? null
    return {
      title: 'Candidate results are ready',
      detail:
        'Review the latest execution first. You can run another candidate later with this same locked scope.',
      role: 'candidate',
      actionLabel: 'View latest candidate',
      executionId: latestCandidate,
      state: 'complete',
    }
  }
  return {
    title: 'Run the candidate',
    detail:
      'Make the Harness change, then rerun this exact scope to produce a local comparison.',
    role: 'candidate',
    actionLabel: 'Run candidate',
    executionId: null,
    state: 'ready',
  }
}

export function PlanLifecycle({
  plan,
  starting,
  feedback,
  onStart,
}: {
  plan: LocalPlan
  starting: PlanRunRole | null
  feedback: PlanRunFeedback | null
  onStart: (role: PlanRunRole) => void
}) {
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

  return (
    <section
      className="panel plan-panel-composite plan-lifecycle-panel"
      aria-labelledby="plan-lifecycle-title"
    >
      <div
        className={`plan-next-action plan-next-action-${nextAction.state}`}
        aria-live="polite"
      >
        <div className="plan-next-action-copy">
          <div className="plan-next-action-heading">
            <h2 id="plan-lifecycle-title">{nextAction.title}</h2>
            {plan.locked && (
              <span className="status-pill status-pass">Locked</span>
            )}
          </div>
          <p>{nextAction.detail}</p>
        </div>
        <div className="plan-next-action-controls">
          {nextAction.executionId ? (
            <a
              className="button button-primary"
              href={hashForExecution(nextAction.executionId)}
            >
              {nextAction.actionLabel}
            </a>
          ) : nextAction.role ? (
            <button
              className="button button-primary"
              type="button"
              onClick={() => onStart(nextAction.role as PlanRunRole)}
              disabled={!canStart}
            >
              {starting === nextAction.role
                ? `Starting ${roleLabel(nextAction.role).toLowerCase()}…`
                : nextAction.actionLabel}
            </button>
          ) : null}
          {nextAction.state === 'complete' && (
            <>
              {plan.baseline_execution_id && (
                <a
                  className="button button-secondary"
                  href={hashForExecution(plan.baseline_execution_id)}
                >
                  View baseline execution
                </a>
              )}
              <button
                className="button button-secondary"
                type="button"
                onClick={() => onStart('candidate')}
                disabled={starting !== null || runningRole !== null}
              >
                Run another candidate
              </button>
            </>
          )}
        </div>
      </div>
      {activeFeedback && (
        <div
          className={`plan-run-feedback plan-run-feedback-${activeFeedback.phase}`}
          role={activeFeedback.phase === 'error' ? 'alert' : 'status'}
          aria-live="polite"
        >
          <span aria-hidden="true">
            {activeFeedback.phase === 'error' ? '!' : '•'}
          </span>
          <div>
            <strong>{activeFeedback.message}</strong>
            {activeFeedback.executionId && (
              <a href={hashForExecution(activeFeedback.executionId)}>
                Open active execution →
              </a>
            )}
          </div>
        </div>
      )}
    </section>
  )
}

const planVerdictPresentation: Record<
  PlanVerdict,
  { label: string; tone: string }
> = {
  improved: { label: 'Improved', tone: 'status-pass' },
  stable: { label: 'Stable', tone: 'status-neutral' },
  regressed: { label: 'Regressed', tone: 'status-fail' },
  inconclusive: { label: 'Inconclusive', tone: 'status-incomplete' },
}

function executionStatus(
  summary: DashboardExecutionSummary | null,
  fallback: 'running' | 'incomplete' | null = null,
) {
  if (!summary) {
    return fallback === 'running'
      ? { label: 'Running', tone: 'status-incomplete' }
      : fallback === 'incomplete'
        ? { label: 'Incomplete', tone: 'status-incomplete' }
        : { label: 'Unavailable', tone: 'status-neutral' }
  }
  const state = buildExecutionPresentation(summary).attention
  if (state === 'passed') return { label: 'Passed', tone: 'status-pass' }
  if (state === 'needs_attention')
    return { label: 'Needs attention', tone: 'status-fail' }
  if (state === 'running' || state === 'cancelling')
    return { label: titleCase(state), tone: 'status-incomplete' }
  return { label: titleCase(state), tone: 'status-neutral' }
}

const PLAN_COMPARISON_TABLE_METRICS = [
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
  'function_calls',
  'function_errors',
]

const SCENARIO_BASELINE_ID = '__visual_baseline__'

type ExecutionHistoryRow = {
  id: string
  role: 'baseline' | 'candidate' | 'attempt'
  label: string
  detail: string
  summary: DashboardExecutionSummary | null
  fallback: 'running' | 'incomplete' | null
  candidateNumber: number | null
}

function executionHistoryRows(
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

function planExecutionLabel(plan: LocalPlan, executionId: string | null) {
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
    <div className="plan-execution-name-control">
      {editing ? (
        <form onSubmit={(event) => void submit(event)}>
          <label>
            <span className="visually-hidden">Name {fallbackLabel}</span>
            <input
              aria-label={`Name ${fallbackLabel}`}
              maxLength={80}
              placeholder={fallbackLabel}
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
            />
          </label>
          <button
            className="button button-primary"
            disabled={saving}
            type="submit"
          >
            {saving ? 'Saving…' : 'Save'}
          </button>
          <button
            className="button button-secondary"
            disabled={saving}
            type="button"
            onClick={() => {
              setDraft(label)
              setError(null)
              setEditing(false)
            }}
          >
            Cancel
          </button>
        </form>
      ) : (
        <button
          aria-label={`Rename ${label.trim() || fallbackLabel}`}
          className="plan-execution-rename-button"
          title={`Rename ${label.trim() || fallbackLabel}`}
          type="button"
          onClick={() => setEditing(true)}
        >
          <PencilLine aria-hidden="true" size={14} strokeWidth={1.8} />
        </button>
      )}
      {error && <small role="alert">{error}</small>}
    </div>
  )
}

function finiteExecutionMetric(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function executionMetricValue(
  summary: DashboardExecutionSummary | null,
  metric: 'tokens' | 'duration' | 'turns' | 'calls' | 'errors',
) {
  const totals = summary?.totals
  const value =
    metric === 'tokens'
      ? totals?.total_tokens
      : metric === 'duration'
        ? totals?.wall_time_seconds
        : metric === 'turns'
          ? totals?.turns
          : metric === 'calls'
            ? totals?.function_calls
            : totals?.function_call_errors
  const numeric = finiteExecutionMetric(value)
  if (numeric === null) return 'Not reported'
  if (metric === 'duration') return formatDuration(numeric)
  return Math.round(numeric).toLocaleString('en-US')
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
  // and a second completed execution to compare with it. Keep the empty and
  // baseline-only states focused on the lifecycle actions instead of showing
  // a table full of context-only values.
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
  const comparisonColumnStyle = {
    '--plan-execution-column-count': Math.max(comparisonColumns.length, 1),
  } as CSSProperties
  return (
    <section
      className="panel plan-panel-composite plan-execution-history-panel"
      aria-labelledby="plan-execution-history-title"
    >
      <div className="panel-heading plan-panel-heading">
        <div>
          <div className="section-kicker">Plan comparison</div>
          <h2 id="plan-execution-history-title">Baseline and candidates</h2>
          <p>
            Choose a visual baseline and any number of candidates. This view
            never changes the official baseline stored with the plan.
          </p>
        </div>
        <span className="coverage-note">
          {loading
            ? 'Loading…'
            : `${visualBaselineId ? '1 visual baseline' : 'Baseline pending'} · ${comparisonCandidateIds.length} selected`}
        </span>
      </div>
      {error && (
        <div className="plan-panel-section plan-history-error" role="status">
          Execution summaries could not be loaded. Retained IDs and report links
          remain available. <span>{error}</span>
        </div>
      )}
      {rows.length === 0 ? (
        <div className="plan-panel-section plan-panel-empty">
          <strong>No execution has started</strong>
          <p>Capture the baseline to begin this plan history.</p>
        </div>
      ) : (
        <>
          <div className="plan-comparison-selection plan-panel-section">
            <label className="plan-visual-baseline-field">
              <span>Visual baseline</span>
              <select
                value={visualBaselineId ?? ''}
                disabled={loading || selectableRows.length === 0}
                onChange={(event) => onVisualBaselineChange(event.target.value)}
              >
                {selectableRows.map((row) => (
                  <option key={row.id} value={row.id}>
                    {planExecutionLabel(plan, row.id)}
                  </option>
                ))}
              </select>
              <small>The official plan baseline remains unchanged.</small>
            </label>
            <fieldset className="plan-candidate-selection">
              <legend>Compare candidates</legend>
              <small>Select one or more executions to show in the table.</small>
              <div className="plan-candidate-options">
                {selectableRows
                  .filter((row) => row.id !== visualBaselineId)
                  .map((row) => {
                    const selected = comparisonCandidateIds.includes(row.id)
                    return (
                      <label
                        className={`plan-candidate-option${selected ? ' is-selected' : ''}`}
                        key={row.id}
                      >
                        <input
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
          <div className="plan-execution-table-wrap">
            <table
              className="plan-execution-table"
              style={comparisonColumnStyle}
            >
              <caption className="visually-hidden">
                Plan metrics in rows, with the visual baseline and selected
                candidates in columns. Best values are highlighted.
              </caption>
              <thead>
                <tr>
                  <th scope="col">Metric</th>
                  {comparisonColumns.map(
                    ({ row, isVisualBaseline, selected }) => (
                      <th
                        className={[
                          'plan-execution-column-header',
                          isVisualBaseline ? 'is-baseline' : '',
                          selected ? 'is-selected' : '',
                        ]
                          .filter(Boolean)
                          .join(' ')}
                        data-execution-id={row.id}
                        key={`${row.role}:${row.id}`}
                        scope="col"
                        title={row.id}
                      >
                        <div className="plan-execution-column-title">
                          <span
                            className={`plan-execution-column-role${isVisualBaseline ? ' is-baseline' : ' is-candidate'}`}
                          >
                            {isVisualBaseline ? 'Reference' : 'Candidate'}
                          </span>
                          <strong>{planExecutionLabel(plan, row.id)}</strong>
                          <span>
                            {[
                              row.detail,
                              row.summary?.completed_at
                                ? formatDate(row.summary.completed_at)
                                : 'Date unavailable',
                            ]
                              .filter(Boolean)
                              .join(' · ')}
                          </span>
                        </div>
                      </th>
                    ),
                  )}
                </tr>
              </thead>
              <tbody>
                {PLAN_COMPARISON_TABLE_METRICS.map((id) => {
                  const entries = comparisonColumns.map((column) => {
                    const metric = column.metricSource
                      ? metricById(column.metricSource, id)
                      : null
                    const side: 'baseline' | 'candidate' = column.rowComparison
                      ? 'candidate'
                      : 'baseline'
                    return {
                      column,
                      metric,
                      side,
                      value: metric?.[side] ?? null,
                    }
                  })
                  const descriptor = entries.find(
                    ({ metric }) => metric,
                  )?.metric
                  const winnerIds = new Set(
                    planMetricWinnerIds(
                      entries.map(({ column, value }) => ({
                        id: column.row.id,
                        value,
                      })),
                      descriptor?.direction ?? 'context',
                    ),
                  )
                  const directionLabel =
                    descriptor?.direction === 'higher'
                      ? 'Higher is better'
                      : descriptor?.direction === 'lower'
                        ? 'Lower is better'
                        : 'Context only'
                  return (
                    <tr data-metric-id={id} key={id}>
                      <th scope="row">
                        <span className="plan-execution-metric-label">
                          <strong>{descriptor?.label ?? titleCase(id)}</strong>
                          <button
                            className="plan-metric-direction"
                            aria-describedby={`plan-metric-direction-${id}`}
                            type="button"
                          >
                            <Info
                              aria-hidden="true"
                              size={12}
                              strokeWidth={1.8}
                            />
                            <span
                              className="plan-metric-direction-tooltip"
                              id={`plan-metric-direction-${id}`}
                              role="tooltip"
                            >
                              {directionLabel}
                            </span>
                          </button>
                        </span>
                      </th>
                      {entries.map(({ column, metric, side }) => {
                        const winner = winnerIds.has(column.row.id)
                        return (
                          <td
                            className={[
                              column.isVisualBaseline ? 'is-baseline' : '',
                              column.selected ? 'is-selected' : '',
                              winner ? 'is-winner' : '',
                            ]
                              .filter(Boolean)
                              .join(' ')}
                            data-execution-id={column.row.id}
                            key={column.row.id}
                          >
                            {metric ? (
                              <span className="plan-execution-metric">
                                <span className="plan-execution-metric-value">
                                  <strong>
                                    {formatPlanMetricValue(metric, side)}
                                  </strong>
                                  {winner && (
                                    <span className="plan-winner-badge">
                                      Best
                                    </span>
                                  )}
                                </span>
                                {!column.isVisualBaseline &&
                                column.rowComparison ? (
                                  <small
                                    className={`metric-tone-${metric.tone}`}
                                  >
                                    {formatPlanMetricDelta(metric)}
                                  </small>
                                ) : !column.isVisualBaseline ? (
                                  <small className="metric-tone-unavailable">
                                    Not comparable
                                  </small>
                                ) : null}
                              </span>
                            ) : (
                              'Not reported'
                            )}
                          </td>
                        )
                      })}
                    </tr>
                  )
                })}
              </tbody>
            </table>
          </div>
          {scenarioComparisons.some(
            ({ comparison }) => comparison.scenarios.length > 0,
          ) ? (
            <div className="plan-panel-section plan-scenario-detail-section">
              <PlanScenarioComparisonTable comparisons={scenarioComparisons} />
            </div>
          ) : null}
        </>
      )}
    </section>
  )
}

export function PlanNonComparableAttempts({
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

  return (
    <section
      className="panel plan-run-history"
      aria-labelledby="plan-run-history-title"
    >
      <div className="plan-run-history-heading">
        <div>
          <strong id="plan-run-history-title">Execution history</strong>
          <span>
            Every retained run, with its captured usage and result. Incomplete
            attempts remain excluded from metric winners.
          </span>
        </div>
        <span>
          {rows.length} execution{rows.length === 1 ? '' : 's'}
        </span>
      </div>
      <ul className="plan-run-history-list">
        <li className="plan-run-history-columns" aria-hidden="true">
          <span>Execution</span>
          <span>Result</span>
          <span>Captured</span>
          <span>Tokens</span>
          <span>Duration</span>
          <span>Turns</span>
          <span>Calls</span>
          <span>Errors</span>
          <span />
        </li>
        {rows.map((row) => {
          const status = executionStatus(row.summary, row.fallback)
          const timestamp =
            row.summary?.completed_at || row.summary?.started_at || ''
          const roleLabel =
            row.role === 'baseline'
              ? 'Baseline'
              : row.role === 'candidate'
                ? `Candidate ${row.candidateNumber ?? ''}`.trim()
                : 'Attempt'
          const displayLabel =
            row.role === 'attempt'
              ? row.label
              : planExecutionLabel(plan, row.id)
          return (
            <li key={`${row.role}:${row.id}`}>
              <div className="plan-run-record-identity">
                <span>{roleLabel}</span>
                <div>
                  <strong>{displayLabel}</strong>
                  {row.role === 'candidate' && onRenameCandidate && (
                    <ExecutionNameControl
                      executionId={row.id}
                      fallbackLabel={`Candidate #${row.candidateNumber ?? 1}`}
                      label={plan.candidate_labels?.[row.id] ?? ''}
                      onRename={onRenameCandidate}
                    />
                  )}
                </div>
                <code title={row.id}>{row.id}</code>
              </div>
              <span className={`plan-execution-result ${status.tone}`}>
                {status.label}
              </span>
              <time dateTime={timestamp || undefined}>
                {timestamp ? formatDate(timestamp) : 'Date unavailable'}
              </time>
              <span className="plan-run-record-metric" data-label="Tokens">
                {executionMetricValue(row.summary, 'tokens')}
              </span>
              <span className="plan-run-record-metric" data-label="Duration">
                {executionMetricValue(row.summary, 'duration')}
              </span>
              <span className="plan-run-record-metric" data-label="Turns">
                {executionMetricValue(row.summary, 'turns')}
              </span>
              <span className="plan-run-record-metric" data-label="Calls">
                {executionMetricValue(row.summary, 'calls')}
              </span>
              <span className="plan-run-record-metric" data-label="Errors">
                {executionMetricValue(row.summary, 'errors')}
              </span>
              <a
                aria-label={`Open report for ${displayLabel}`}
                className="plan-run-report-link"
                href={hashForExecution(row.id)}
                title={`Open report for ${displayLabel}`}
              >
                <ExternalLink aria-hidden="true" size={14} strokeWidth={1.8} />
                <span>Report</span>
              </a>
            </li>
          )
        })}
      </ul>
    </section>
  )
}

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
    <div className="plan-scenario-comparison">
      <div className="plan-scenario-comparison-heading">
        <div>
          <h3>Metrics by test</h3>
          <p>
            Compare outcomes and efficiency for each test. Expand a row for
            exact values and deltas.
          </p>
        </div>
        <span>
          {scenarioIds.length} {scenarioIds.length === 1 ? 'test' : 'tests'} ·{' '}
          {comparisons.length}{' '}
          {comparisons.length === 1 ? 'candidate' : 'candidates'}
        </span>
      </div>
      <div className="plan-scenario-disclosures">
        {scenarioIds.map((scenarioId) => {
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
              className="plan-scenario-disclosure"
              key={scenarioId}
              open={scenarioId === 'security_review'}
            >
              <summary>
                <span className="plan-scenario-identity">
                  <strong>{scenarioName(scenarioId)}</strong>
                  <code>{scenarioId}</code>
                </span>
                <span className="plan-scenario-objective">
                  <small>Objective</small>
                  <b>
                    {titleCase(firstScenario.baseline_status)} →{' '}
                    {scenarioColumns
                      .map(({ scenario }) =>
                        titleCase(scenario?.candidate_status ?? 'not reported'),
                      )
                      .join(' · ')}
                  </b>
                </span>
                <span className="plan-scenario-signals">
                  {PLAN_SCENARIO_SUMMARY_METRICS.map((metricId) => {
                    const metrics = metricLists.map(
                      (list) => list.find(({ id }) => id === metricId) ?? null,
                    )
                    const descriptor = metrics.find((metric) => metric)
                    if (!descriptor) return null
                    const winnerIds = scenarioMetricWinnerIds(
                      descriptor,
                      metrics,
                      comparisons,
                    )
                    return (
                      <span className="plan-scenario-signal" key={metricId}>
                        <small>{descriptor.label}</small>
                        <span className="plan-scenario-signal-values">
                          <b
                            className={`plan-scenario-signal-value${winnerIds.has(SCENARIO_BASELINE_ID) ? ' is-winner' : ''}`}
                            title="Visual baseline"
                          >
                            {formatPlanMetricValue(descriptor, 'baseline')}
                            {winnerIds.has(SCENARIO_BASELINE_ID) && (
                              <em>Best</em>
                            )}
                          </b>
                          {metrics.map((metric, index) => (
                            <span
                              className="plan-scenario-signal-candidate"
                              key={comparisons[index].id}
                            >
                              <i aria-hidden="true">→</i>
                              <b
                                className={`plan-scenario-signal-value${winnerIds.has(comparisons[index].id) ? ' is-winner' : ''}`}
                                title={comparisons[index].label}
                              >
                                {metric
                                  ? formatPlanMetricValue(metric, 'candidate')
                                  : 'Not reported'}
                                {winnerIds.has(comparisons[index].id) && (
                                  <em>Best</em>
                                )}
                              </b>
                            </span>
                          ))}
                        </span>
                      </span>
                    )
                  })}
                </span>
                <span className="plan-scenario-chevron" aria-hidden="true" />
              </summary>
              <div className="plan-scenario-metric-detail">
                <table
                  className="plan-scenario-metric-grid"
                  style={
                    {
                      '--plan-scenario-column-count': comparisons.length,
                    } as CSSProperties
                  }
                >
                  <caption className="visually-hidden">
                    Metrics for {scenarioName(scenarioId)}, comparing the visual
                    baseline with selected candidates.
                  </caption>
                  <thead>
                    <tr>
                      <th scope="col">Metric</th>
                      <th scope="col">
                        <span className="plan-scenario-column-heading">
                          <small>Reference</small>
                          <strong>Baseline</strong>
                          <ScenarioChatAction
                            compact
                            executionId={
                              comparisons[0]?.comparison.baseline?.id
                            }
                            scenarioId={scenarioId}
                          />
                        </span>
                      </th>
                      {comparisons.map((column) => (
                        <th key={column.id} scope="col">
                          <span className="plan-scenario-column-heading">
                            <small>Candidate</small>
                            <strong>{column.label}</strong>
                            <ScenarioChatAction
                              compact
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
                      const winnerIds = scenarioMetricWinnerIds(
                        descriptor,
                        metrics,
                        comparisons,
                      )
                      return (
                        <tr data-scenario-metric-id={metricId} key={metricId}>
                          <th scope="row">{descriptor.label}</th>
                          <td
                            className={
                              winnerIds.has(SCENARIO_BASELINE_ID)
                                ? 'is-winner'
                                : undefined
                            }
                          >
                            <span className="plan-scenario-metric-value">
                              <b>
                                {formatPlanMetricValue(descriptor, 'baseline')}
                              </b>
                              {winnerIds.has(SCENARIO_BASELINE_ID) && (
                                <span className="plan-winner-badge">Best</span>
                              )}
                            </span>
                          </td>
                          {metrics.map((metric, index) => (
                            <td
                              className={
                                winnerIds.has(comparisons[index].id)
                                  ? 'is-winner'
                                  : undefined
                              }
                              key={comparisons[index].id}
                            >
                              {metric ? (
                                <span className="plan-scenario-candidate-metric">
                                  <span className="plan-scenario-metric-value">
                                    <b>
                                      {formatPlanMetricValue(
                                        metric,
                                        'candidate',
                                      )}
                                    </b>
                                    {winnerIds.has(comparisons[index].id) && (
                                      <span className="plan-winner-badge">
                                        Best
                                      </span>
                                    )}
                                  </span>
                                  <small
                                    className={`metric-tone-${metric.tone}`}
                                  >
                                    {formatPlanMetricDelta(metric)}
                                  </small>
                                </span>
                              ) : (
                                'Not reported'
                              )}
                            </td>
                          ))}
                        </tr>
                      )
                    })}
                  </tbody>
                </table>
              </div>
            </details>
          )
        })}
      </div>
    </div>
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

export function PlanComparisonPanel({
  comparison,
  baselineExecutionId,
  candidateExecutionId,
  candidateNumber,
  baselineLabel = 'Baseline',
  candidateLabel,
  loading,
  error,
}: {
  comparison: PlanComparison | null
  baselineExecutionId: string | null
  candidateExecutionId: string | null
  candidateNumber: number | null
  baselineLabel?: string
  candidateLabel?: string
  loading: boolean
  error: string | null
}) {
  const resolvedCandidateLabel =
    candidateLabel ?? `Candidate #${candidateNumber ?? '—'}`
  if (!baselineExecutionId || !candidateExecutionId) {
    return null
  }
  if (loading) {
    return (
      <section
        className="panel plan-panel-padded plan-comparison-panel"
        aria-labelledby="plan-comparison-title"
        aria-live="polite"
        aria-busy="true"
        tabIndex={-1}
      >
        <div className="section-kicker">Execution comparison</div>
        <h2 id="plan-comparison-title">
          {baselineLabel} vs {resolvedCandidateLabel}
        </h2>
        <p>Loading baseline and candidate evidence…</p>
      </section>
    )
  }
  if (error || !comparison) {
    return (
      <section
        className="panel plan-panel-padded plan-comparison-panel"
        aria-labelledby="plan-comparison-title"
        role="alert"
        tabIndex={-1}
      >
        <div className="section-kicker">Execution comparison</div>
        <h2 id="plan-comparison-title">
          {baselineLabel} vs {resolvedCandidateLabel} unavailable
        </h2>
        <p>{error || 'The retained execution details could not be read.'}</p>
      </section>
    )
  }
  const verdict = planVerdictPresentation[comparison.verdict]
  return (
    <section
      className={`panel plan-panel-composite plan-comparison-panel plan-comparison-${comparison.verdict}`}
      aria-labelledby="plan-comparison-title"
      aria-live="polite"
      tabIndex={-1}
    >
      <div className="panel-heading plan-panel-heading">
        <div>
          <div className="section-kicker">Execution comparison</div>
          <h2 id="plan-comparison-title">
            {baselineLabel} vs {resolvedCandidateLabel}
          </h2>
          <strong className="plan-comparison-headline">
            {comparison.headline}
          </strong>
          <p>{comparison.detail}</p>
        </div>
        <span className={`status-pill ${verdict.tone}`}>{verdict.label}</span>
      </div>
      <div className="plan-comparison-provenance plan-panel-section">
        <a href={hashForExecution(baselineExecutionId)}>
          <span>{baselineLabel}</span>
          <code>{baselineExecutionId}</code>
        </a>
        <i aria-hidden="true">→</i>
        <a href={hashForExecution(candidateExecutionId)}>
          <span>{resolvedCandidateLabel}</span>
          <code>{candidateExecutionId}</code>
        </a>
      </div>
      <div className="plan-panel-section">
        <PlanScenarioComparisonTable
          comparisons={[
            {
              id: candidateExecutionId,
              label: resolvedCandidateLabel,
              comparison,
            },
          ]}
        />
      </div>
    </section>
  )
}

export function LocalPlanCreatePage() {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [catalog, setCatalog] = useState<Catalog | null>(null)
  const [label, setLabel] = useState(PLAN_FORM_DEFAULTS.label)
  const [purpose, setPurpose] = useState(PLAN_FORM_DEFAULTS.purpose)
  const [url, setUrl] = useState(PLAN_FORM_DEFAULTS.url)
  const [subject, setSubject] = useState(PLAN_FORM_DEFAULTS.subject)
  const [judge, setJudge] = useState(PLAN_FORM_DEFAULTS.judge)
  const [scenarios, setScenarios] = useState<string[]>(
    PLAN_FORM_DEFAULTS.scenarios,
  )
  const [testQuery, setTestQuery] = useState(PLAN_FORM_DEFAULTS.testQuery)
  const [runs, setRuns] = useState(PLAN_FORM_DEFAULTS.runs)
  const [technicalRetries, setTechnicalRetries] = useState(
    PLAN_FORM_DEFAULTS.technicalRetries,
  )
  const [seed, setSeed] = useState(PLAN_FORM_DEFAULTS.seed)
  const [loading, setLoading] = useState(true)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const initialValues = useRef<PlanFormValues>({ ...PLAN_FORM_DEFAULTS })

  const loadCatalog = useCallback(async (source: DashboardDataBridge) => {
    const loaded = catalogValue(await source.getCatalog())
    const firstModel = loaded.models[0] ? modelKey(loaded.models[0]) : ''
    setCatalog(loaded)
    setUrl((current) => current || loaded.url)
    setSubject((current) => current || firstModel)
    initialValues.current = {
      ...initialValues.current,
      url: initialValues.current.url || loaded.url,
      subject: initialValues.current.subject || firstModel,
    }
  }, [])

  useEffect(() => {
    let cancelled = false
    void getDashboardDataBridge()
      .then(async (next) => {
        if (cancelled) return
        setBridge(next)
        if (next.mode !== 'local')
          throw new Error(
            'Local plans are available only in the local dashboard',
          )
        await loadCatalog(next)
      })
      .catch((cause) => {
        if (!cancelled) setError(errorText(cause))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [loadCatalog])

  // Audit PN-17: the plan page can retry a catalog load without a reload.
  const refreshCatalog = async () => {
    if (bridge?.mode !== 'local') return
    setLoading(true)
    setError(null)
    try {
      await loadCatalog(bridge)
    } catch (cause) {
      setError(errorText(cause))
    } finally {
      setLoading(false)
    }
  }

  const selectedSubject = catalog?.models.find(
    (item) => modelKey(item) === subject,
  )
  const selectedJudge = catalog?.models.find((item) => modelKey(item) === judge)
  const groupedModels = useMemo(
    () => modelGroups(catalog?.models ?? []),
    [catalog],
  )
  const modelOptions = groupedModels.map((group) => ({
    provider: group.provider,
    models: group.models.map((item) => ({
      label: item.model,
      value: modelKey(item),
    })),
  }))
  const runsPerTest = Math.max(1, Number(runs) || 1)
  const retryCount = Math.max(0, Number(technicalRetries) || 0)
  const plannedRuns = scenarios.length * runsPerTest
  const hasPlanLabel = label.trim().length > 0
  const canCreate =
    !loading &&
    !submitting &&
    Boolean(selectedSubject) &&
    scenarios.length > 0 &&
    hasPlanLabel

  const dirty = planFormDirty(
    {
      label,
      purpose,
      url,
      subject,
      judge,
      scenarios,
      testQuery,
      runs,
      technicalRetries,
      seed,
    },
    initialValues.current,
  )

  const dirtyNavigation = useDirtyNavigation(dirty && !submitting)

  const create = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (
      bridge?.mode !== 'local' ||
      !selectedSubject ||
      scenarios.length === 0 ||
      !hasPlanLabel
    ) {
      setError(
        'Add a plan label, choose an execution model and select at least one test.',
      )
      return
    }
    setSubmitting(true)
    setError(null)
    try {
      const plan = await bridge.createPlan({
        label: label.trim(),
        purpose: purpose.trim(),
        url,
        model: selectedSubject.model,
        provider: selectedSubject.provider,
        judge_model: selectedJudge?.model ?? '',
        judge_provider: selectedJudge?.provider ?? '',
        scenarios,
        runs: runsPerTest,
        technical_retries: retryCount,
        seed: seed ? Number(seed) : null,
      })
      initialValues.current = {
        label: label.trim(),
        purpose: purpose.trim(),
        url,
        subject,
        judge,
        scenarios,
        testQuery,
        runs,
        technicalRetries,
        seed,
      }
      window.location.hash = hashForPlan(plan.id)
    } catch (cause) {
      setError(errorText(cause))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <>
      <DiscardDraftDialog
        open={dirtyNavigation.pendingHash !== null}
        warning={dirtyNavigation.warning}
        onKeep={dirtyNavigation.cancelNavigation}
        onDiscard={dirtyNavigation.confirmNavigation}
      />
      <DashboardPageActions active="plans" />
      <div className="ds-root mx-auto w-[calc(100%_-_1.5rem)] max-w-[1420px] py-8 sm:w-[calc(100%_-_3rem)] sm:py-10">
        <header className="grid items-end gap-6 border-b border-[var(--color-rule)] pb-6 lg:grid-cols-[minmax(0,1fr)_auto]">
          <div className="min-w-0">
            <p className="m-0 font-mono text-[0.68rem] font-semibold uppercase tracking-[0.08em] text-[var(--color-accent)]">
              Execution setup
            </p>
            <h1
              className="mt-2 mb-0 max-w-4xl font-mono text-xl font-semibold tracking-[-0.01em] text-ink"
              id="plan-create-title"
            >
              Create a benchmark plan
            </h1>
            <p className="mt-3 mb-0 max-w-3xl text-sm leading-6 text-[var(--color-ink-faint)]">
              Save a focused scope, capture its baseline, then run the same
              scenarios after your change to measure improvement.
            </p>
          </div>
          <nav
            className="grid min-w-[18rem] grid-cols-2 gap-px overflow-hidden rounded-lg border border-[var(--color-rule)] bg-[var(--color-rule)] max-[390px]:min-w-0"
            aria-label="Execution setup mode"
          >
            <a
              className="grid min-h-14 content-center gap-0.5 bg-panel-raised px-4 text-xs text-[var(--color-ink-faint)] no-underline transition-colors hover:text-ink"
              href={hashForWorkspace()}
              onClick={requestQuickExecution}
            >
              <strong className="font-semibold">Quick execution</strong>
              <span className="text-[0.65rem] text-ink-muted">
                One result now
              </span>
            </a>
            <span
              className="grid min-h-14 content-center gap-0.5 bg-[var(--color-surface-hover)] px-4 text-xs text-ink"
              aria-current="page"
            >
              <strong className="font-semibold">Reusable plan</strong>
              <span className="text-[0.65rem] text-ink-muted">
                Baseline + candidates
              </span>
            </span>
          </nav>
        </header>

        {error && (
          <section
            className="mt-6 rounded-lg border border-[color-mix(in_srgb,var(--color-alert)_35%,transparent)] bg-[color-mix(in_srgb,var(--color-alert)_8%,var(--surface))] p-5"
            role="alert"
          >
            <h2 className="m-0 text-sm font-semibold text-danger">
              Plan cannot be created
            </h2>
            <p className="mt-2 mb-0 text-xs leading-5 text-[var(--color-ink-faint)]">
              {error}
            </p>
          </section>
        )}

        <form
          className="mt-6 grid min-w-0 gap-px overflow-hidden rounded-[6px] border border-[var(--color-edge)] bg-[var(--color-rule)] lg:grid-cols-12"
          onSubmit={create}
        >
          <div className="min-w-0 bg-panel lg:col-span-8">
            <ExecutionSetup
              idPrefix="plan-create"
              mode="plan"
              label={label}
              purpose={purpose}
              url={url}
              subject={subject}
              judge={judge}
              modelGroups={modelOptions}
              availableScenarios={catalog?.scenarios ?? []}
              selectedScenarios={scenarios}
              query={testQuery}
              runs={runs}
              technicalRetries={technicalRetries}
              seed={seed}
              disabled={submitting}
              catalogLoading={loading}
              catalogSummary={
                loading
                  ? 'Loading local catalog'
                  : catalog
                    ? `${catalog.models.length} registered model${catalog.models.length === 1 ? '' : 's'} · ${catalog.scenarios.length} tests`
                    : 'Catalog unavailable'
              }
              onRefreshCatalog={() => void refreshCatalog()}
              onLabelChange={setLabel}
              onPurposeChange={setPurpose}
              onUrlChange={setUrl}
              onSubjectChange={setSubject}
              onJudgeChange={setJudge}
              onSelectedScenariosChange={setScenarios}
              onQueryChange={setTestQuery}
              onRunsChange={setRuns}
              onTechnicalRetriesChange={setTechnicalRetries}
              onSeedChange={setSeed}
            />
          </div>

          <div className="min-w-0 bg-panel-raised lg:col-span-4">
            <ExecutionSetupReview
              mode="plan"
              status={canCreate ? 'Ready' : 'Incomplete'}
              subject={
                selectedSubject
                  ? `${selectedSubject.provider} / ${selectedSubject.model}`
                  : ''
              }
              judge={
                selectedJudge
                  ? `${selectedJudge.provider} / ${selectedJudge.model}`
                  : ''
              }
              url={url}
              selectedScenarios={scenarios.length}
              plannedRuns={plannedRuns}
              runsPerScenario={runsPerTest}
              technicalRetries={retryCount}
              ready={canCreate}
            >
              <p
                className="m-0 text-xs leading-5 text-ink-muted"
                id="plan-create-requirements"
              >
                {hasPlanLabel && scenarios.length > 0
                  ? 'Ready to create a draft. The scope stays editable until the baseline starts.'
                  : `Before creating: ${hasPlanLabel ? 'select at least one test' : 'add a plan label'}${hasPlanLabel || scenarios.length === 0 ? '' : ' and select at least one test'}.`}
              </p>
              <button
                className={buttonClassName({
                  variant: 'primary',
                  size: 'large',
                  className: 'w-full',
                })}
                type="submit"
                disabled={!canCreate}
                aria-describedby="plan-create-requirements"
              >
                {submitting ? 'creating…' : 'create draft plan'}
              </button>
              <a
                className={buttonClassName({
                  variant: 'secondary',
                  size: 'large',
                  className: 'w-full no-underline',
                })}
                href={hashForPlans()}
              >
                cancel
              </a>
            </ExecutionSetupReview>
          </div>
        </form>
      </div>
    </>
  )
}

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
  useEffect(() => {
    if (
      !plan ||
      !['baseline_running', 'candidate_running'].includes(plan.state)
    )
      return
    const timer = window.setInterval(() => {
      void load().catch(() => undefined)
    }, 2_000)
    return () => window.clearInterval(timer)
  }, [load, plan])

  const executionIds = useMemo(
    () =>
      [
        plan?.baseline_execution_id ?? '',
        ...(plan?.candidate_execution_ids ?? []),
        ...(plan?.incomplete_execution_ids ?? []),
        plan?.last_attempt_id ?? '',
      ].filter(Boolean),
    [
      plan?.baseline_execution_id,
      plan?.candidate_execution_ids,
      plan?.incomplete_execution_ids,
      plan?.last_attempt_id,
    ],
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
    if (!bridge || !plan) return
    let cancelled = false
    setHistoryLoading(true)
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
  }, [bridge, executionIds, plan])

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

  const readiness = useMemo(() => {
    if (!plan)
      return {
        label: 'Loading',
        tone: 'status-incomplete',
        detail: 'Reading the local plan.',
      }
    if (isRoleRunning(plan, 'baseline'))
      return {
        label: 'Baseline in progress',
        tone: 'status-incomplete',
        detail:
          'The baseline is running; candidate actions stay unavailable until its report is complete.',
      }
    if (isRoleRunning(plan, 'candidate'))
      return {
        label: 'Candidate in progress',
        tone: 'status-incomplete',
        detail: 'The locked scope is running against the captured baseline.',
      }
    if (!plan.baseline_execution_id)
      return {
        label:
          plan.incomplete_execution_ids.length > 0
            ? 'Baseline retry available'
            : 'Baseline required',
        tone: 'status-incomplete',
        detail:
          plan.incomplete_execution_ids.length > 0
            ? 'The last attempt did not produce a report; retry the same locked scope.'
            : 'The scope is defined but no completed baseline report exists.',
      }
    if (plan.candidate_execution_ids.length > 0)
      return {
        label: 'Comparison available',
        tone: 'status-neutral',
        detail:
          'Candidate reports are ready to inspect against the locked baseline.',
      }
    if (plan.incomplete_execution_ids.length)
      return {
        label: 'Candidate retry available',
        tone: 'status-neutral',
        detail:
          'An incomplete attempt is retained for diagnosis, but it does not block a retry or count as comparison evidence.',
      }
    return {
      label: 'Scope ready',
      tone: 'status-neutral',
      detail:
        'Baseline and locked scope are structurally ready for a candidate.',
    }
  }, [plan])

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

  return (
    <>
      <DashboardPageActions
        active="plans"
        context={plan ? plan.label || plan.id : undefined}
      />
      <div className="page-shell overview-shell plan-detail-shell">
        {loading && (
          <section className="panel plan-panel-padded">
            <p className="table-empty">Loading plan…</p>
          </section>
        )}
        {loadError && !plan && (
          <section className="empty-state" role="alert">
            <h2>Plan unavailable</h2>
            <p>{loadError}</p>
            <a className="button" href={hashForNewPlan()}>
              Create another plan
            </a>
          </section>
        )}
        {plan && (
          <>
            <section
              className="page-heading plan-detail-heading"
              aria-labelledby="plan-detail-title"
            >
              <div>
                <div className="eyebrow">
                  <span className="live-dot" aria-hidden="true" />
                  Local plan · {stateLabel(plan)}
                </div>
                <h1 id="plan-detail-title">{plan.label || plan.id}</h1>
                <p>
                  {plan.purpose ||
                    'Explicit local baseline and candidate scope.'}
                </p>
              </div>
              <div className="plan-detail-state">
                <strong
                  className={`status-pill ${plan.locked ? 'status-pass' : 'status-neutral'}`}
                >
                  {plan.locked ? 'Locked after baseline' : 'Editable draft'}
                </strong>
                <small>
                  {plan.locked ? 'Scope frozen' : 'Scope can still change'}
                </small>
              </div>
            </section>
            <section className="plan-status-grid">
              <article className="plan-status-card">
                <div className="plan-status-card-heading">
                  <div className="section-kicker">Comparison readiness</div>
                  <strong className={`status-pill ${readiness.tone}`}>
                    {readiness.label}
                  </strong>
                </div>
                <p>{readiness.detail}</p>
                <small>
                  Statistical maturity is reported separately; it does not block
                  a local baseline.
                </small>
              </article>
              <article className="plan-status-card">
                <div className="section-kicker">Scope snapshot</div>
                <strong>
                  {plan.scenarios.length} tests · {plan.runs} run
                  {plan.runs === 1 ? '' : 's'} each
                </strong>
                <p>
                  {plan.model} · {plan.provider}
                </p>
                <small>
                  {plan.judge_model
                    ? `Judge: ${plan.judge_provider} · ${plan.judge_model}`
                    : 'Judge: default policy'}
                </small>
              </article>
            </section>
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
            <PlanLifecycle
              plan={plan}
              starting={starting}
              feedback={runFeedback}
              onStart={(role) => void start(role)}
            />
            <PlanNonComparableAttempts
              plan={plan}
              summaries={executionSummaries}
              onRenameCandidate={renameCandidate}
            />
          </>
        )}
      </div>
    </>
  )
}
