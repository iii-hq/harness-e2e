import {
  type CSSProperties,
  type FormEvent,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from 'react'
import { ProviderModelDropdown } from '@/components/ProviderModelDropdown'
import { ThemeToggle } from '@/components/ThemeToggle'
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
  return {
    url: typeof value.url === 'string' ? value.url : '',
    models,
    scenarios: Array.isArray(value.scenarios)
      ? value.scenarios.filter(
          (item): item is string => typeof item === 'string',
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
      <div className="panel-heading plan-panel-heading">
        <div>
          <div className="section-kicker">Execution controls</div>
          <h2 id="plan-lifecycle-title">Plan actions</h2>
        </div>
        {plan.locked && <span className="status-pill status-pass">Locked</span>}
      </div>
      <div
        className={`plan-next-action plan-next-action-${nextAction.state}`}
        aria-live="polite"
      >
        <div>
          <span className="section-kicker">Next action</span>
          <h3>{nextAction.title}</h3>
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
  'tokens',
  'duration',
  'cost',
] as const

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
  return comparable
    .filter((entry) => Math.abs(entry.value - winnerValue) < 1e-9)
    .map((entry) => entry.id)
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
          className="plan-execution-rename-button"
          type="button"
          onClick={() => setEditing(true)}
        >
          Rename
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
  metric: 'tokens' | 'duration' | 'calls' | 'errors',
) {
  const totals = summary?.totals
  const value =
    metric === 'tokens'
      ? totals?.total_tokens
      : metric === 'duration'
        ? totals?.wall_time_seconds
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
  onVisualBaselineChange,
  onToggleCandidate,
  onRenameCandidate,
  loading,
  error = null,
}: {
  plan: LocalPlan
  summaries: Record<string, DashboardExecutionSummary>
  visualBaselineId: string | null
  comparisonCandidateIds: string[]
  selectedCandidateId: string | null
  onVisualBaselineChange: (id: string) => void
  onToggleCandidate: (id: string, selected: boolean) => void
  onRenameCandidate?: (executionId: string, label: string) => Promise<void>
  loading: boolean
  error?: string | null
}) {
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
              <legend>Candidates in table</legend>
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
              style={
                {
                  '--plan-execution-column-count': Math.max(
                    comparisonColumns.length,
                    1,
                  ),
                } as CSSProperties
              }
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
                      >
                        <div className="plan-execution-column-title">
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
                          <code title={row.id}>{row.id}</code>
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
                        <strong>{descriptor?.label ?? titleCase(id)}</strong>
                        <span>{directionLabel}</span>
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
                                      Winner
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
                                  <small>Not comparable</small>
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
          <section
            className="plan-run-history plan-panel-section"
            aria-labelledby="plan-run-history-title"
          >
            <div className="plan-run-history-heading">
              <div>
                <strong id="plan-run-history-title">Execution history</strong>
                <span>
                  Every retained run, with its captured usage and result.
                  Incomplete attempts remain excluded from metric winners.
                </span>
              </div>
              <span>
                {rows.length} execution{rows.length === 1 ? '' : 's'}
              </span>
            </div>
            <ul className="plan-run-history-list">
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
                    <div className="plan-run-record-main">
                      <div className="plan-run-record-identity">
                        <span>{roleLabel}</span>
                        <strong>{displayLabel}</strong>
                        <code title={row.id}>{row.id}</code>
                      </div>
                      <div className="plan-run-record-state">
                        <span
                          className={`plan-execution-result ${status.tone}`}
                        >
                          {status.label}
                        </span>
                        <time dateTime={timestamp || undefined}>
                          {timestamp
                            ? formatDate(timestamp)
                            : 'Date unavailable'}
                        </time>
                      </div>
                      <div className="plan-run-record-actions">
                        {row.role === 'candidate' && onRenameCandidate && (
                          <ExecutionNameControl
                            executionId={row.id}
                            fallbackLabel={`Candidate #${row.candidateNumber ?? 1}`}
                            label={plan.candidate_labels?.[row.id] ?? ''}
                            onRename={onRenameCandidate}
                          />
                        )}
                        <a href={hashForExecution(row.id)}>Open report</a>
                      </div>
                    </div>
                    <dl className="plan-run-record-metrics">
                      <div>
                        <dt>Tokens</dt>
                        <dd>{executionMetricValue(row.summary, 'tokens')}</dd>
                      </div>
                      <div>
                        <dt>Duration</dt>
                        <dd>{executionMetricValue(row.summary, 'duration')}</dd>
                      </div>
                      <div>
                        <dt>Function calls</dt>
                        <dd>{executionMetricValue(row.summary, 'calls')}</dd>
                      </div>
                      <div>
                        <dt>Function errors</dt>
                        <dd>{executionMetricValue(row.summary, 'errors')}</dd>
                      </div>
                    </dl>
                  </li>
                )
              })}
            </ul>
          </section>
        </>
      )}
    </section>
  )
}

function ScenarioMetric({
  scenario,
  id,
}: {
  scenario: PlanScenarioComparison
  id: 'pass_rate' | 'quality' | 'tokens' | 'duration'
}) {
  const metric = scenario.metrics.find((item) => item.id === id)
  if (!metric) return <span>Not reported</span>
  return (
    <span className="plan-scenario-metric">
      <span>
        {formatPlanMetricValue(metric, 'baseline')} →{' '}
        {formatPlanMetricValue(metric, 'candidate')}
      </span>
      <small className={`metric-tone-${metric.tone}`}>
        {formatPlanMetricDelta(metric)}
      </small>
    </span>
  )
}

function PlanScenarioComparisonTable({
  scenarios,
}: {
  scenarios: PlanScenarioComparison[]
}) {
  if (scenarios.length === 0) return null
  return (
    <div className="plan-scenario-comparison">
      <div className="plan-scenario-comparison-heading">
        <div>
          <div className="section-kicker">Test breakdown</div>
          <h3>Comparable scope by test</h3>
        </div>
        <span>{scenarios.length} tests</span>
      </div>
      <div className="plan-scenario-table-wrap">
        <table className="plan-scenario-table">
          <thead>
            <tr>
              <th scope="col">Test</th>
              <th scope="col">Objective result</th>
              <th scope="col">Pass rate</th>
              <th scope="col">Quality</th>
              <th scope="col">Tokens</th>
              <th scope="col">Duration</th>
            </tr>
          </thead>
          <tbody>
            {scenarios.map((scenario) => (
              <tr key={scenario.id}>
                <th scope="row" data-label="Test">
                  <strong>{scenarioName(scenario.id)}</strong>
                  <code>{scenario.id}</code>
                  {!scenario.compatible && <small>{scenario.reason}</small>}
                </th>
                <td data-label="Objective result">
                  <span>
                    {titleCase(scenario.baseline_status)} →{' '}
                    {titleCase(scenario.candidate_status)}
                  </span>
                </td>
                <td data-label="Pass rate">
                  <ScenarioMetric scenario={scenario} id="pass_rate" />
                </td>
                <td data-label="Quality">
                  <ScenarioMetric scenario={scenario} id="quality" />
                </td>
                <td data-label="Tokens">
                  <ScenarioMetric scenario={scenario} id="tokens" />
                </td>
                <td data-label="Duration">
                  <ScenarioMetric scenario={scenario} id="duration" />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  )
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
    return (
      <section className="panel plan-panel-padded plan-comparison-empty">
        <div className="section-kicker">Execution comparison</div>
        <h2>Baseline vs candidate</h2>
        <p>
          Select a visual baseline and at least one candidate to calculate
          objective and directional deltas.
        </p>
      </section>
    )
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
        <PlanScenarioComparisonTable scenarios={comparison.scenarios} />
      </div>
    </section>
  )
}

function PlanHeader({ local = true }: { local?: boolean }) {
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
      <nav className="topbar-actions" aria-label="Plan actions">
        <a
          className="button button-secondary"
          href={hashForWorkspace()}
          data-mobile-label="Overview"
        >
          Overview
        </a>
        <a
          className="button button-secondary"
          href={hashForPlans()}
          data-mobile-label="Plans"
        >
          Plans
        </a>
        {local && (
          <a
            className="button button-secondary"
            href={hashForWorkspace('tests')}
            data-mobile-label="Tests"
          >
            Test catalog
          </a>
        )}
        <ThemeToggle />
      </nav>
    </header>
  )
}

export function LocalPlanCreatePage() {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [catalog, setCatalog] = useState<Catalog | null>(null)
  const [label, setLabel] = useState('')
  const [purpose, setPurpose] = useState('')
  const [url, setUrl] = useState('')
  const [subject, setSubject] = useState('')
  const [judge, setJudge] = useState('')
  const [scenarios, setScenarios] = useState<string[]>([])
  const [testQuery, setTestQuery] = useState('')
  const [runs, setRuns] = useState('1')
  const [technicalRetries, setTechnicalRetries] = useState('1')
  const [seed, setSeed] = useState('')
  const [loading, setLoading] = useState(true)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

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
        const loaded = catalogValue(await next.getCatalog())
        if (cancelled) return
        setCatalog(loaded)
        setUrl(loaded.url)
        setSubject(
          loaded.models[0]
            ? `${loaded.models[0].provider}\n${loaded.models[0].model}`
            : '',
        )
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
  }, [])

  const selectedSubject = catalog?.models.find(
    (item) => `${item.provider}\n${item.model}` === subject,
  )
  const selectedJudge = catalog?.models.find(
    (item) => `${item.provider}\n${item.model}` === judge,
  )
  const groupedModels = useMemo(
    () => modelGroups(catalog?.models ?? []),
    [catalog],
  )
  const visibleScenarios = useMemo(() => {
    const query = testQuery.trim().toLocaleLowerCase()
    const all = catalog?.scenarios ?? []
    if (!query) return all
    return all.filter((scenario) =>
      `${scenario} ${scenarioName(scenario)}`
        .toLocaleLowerCase()
        .includes(query),
    )
  }, [catalog?.scenarios, testQuery])
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

  const toggleScenario = (scenario: string, checked: boolean) => {
    setScenarios((current) => {
      if (checked)
        return current.includes(scenario) ? current : [...current, scenario]
      return current.filter((item) => item !== scenario)
    })
  }

  const selectVisibleScenarios = () => {
    setScenarios((current) => [
      ...current,
      ...visibleScenarios.filter((scenario) => !current.includes(scenario)),
    ])
  }

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
      window.location.hash = hashForPlan(plan.id)
    } catch (cause) {
      setError(errorText(cause))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <>
      <a className="skip-link" href="#plan-create-main">
        Skip to plan creation
      </a>
      <PlanHeader />
      <main
        id="plan-create-main"
        className="page-shell overview-shell plan-create-shell"
      >
        <section className="page-heading" aria-labelledby="plan-create-title">
          <div>
            <div className="eyebrow">
              <span className="live-dot" aria-hidden="true" />
              Local evidence plan
            </div>
            <h1 id="plan-create-title">Create a focused local plan</h1>
            <p>
              Capture only the tests that matter for this change. The baseline
              locks this scope; every candidate will reuse it exactly.
            </p>
          </div>
          <div className="sync-block">
            <span>Evidence default</span>
            <strong>1 run / test</strong>
          </div>
        </section>
        {error && (
          <section className="empty-state" role="alert">
            <h2>Plan cannot be created</h2>
            <p>{error}</p>
          </section>
        )}
        <section className="panel plan-form-panel">
          <form className="plan-form" onSubmit={create}>
            <section
              className="plan-form-section"
              aria-labelledby="plan-intent-title"
            >
              <div className="plan-form-section-heading">
                <span>01</span>
                <div>
                  <h2 id="plan-intent-title">What are you validating?</h2>
                  <p>
                    A short label makes this local evidence easy to find again.
                  </p>
                </div>
              </div>
              <div className="plan-form-fields plan-form-intent-fields">
                <label>
                  <span>Plan label</span>
                  <input
                    value={label}
                    required
                    maxLength={120}
                    placeholder="Before harness change"
                    onChange={(event) => setLabel(event.target.value)}
                  />
                </label>
                <label>
                  <span>
                    Purpose <small>optional</small>
                  </span>
                  <textarea
                    value={purpose}
                    rows={2}
                    placeholder="Validate the loop-engineering change"
                    onChange={(event) => setPurpose(event.target.value)}
                  />
                </label>
              </div>
            </section>
            <section
              className="plan-form-section"
              aria-labelledby="plan-execution-title"
            >
              <div className="plan-form-section-heading">
                <span>02</span>
                <div>
                  <h2 id="plan-execution-title">Execution setup</h2>
                  <p>The subject and judge are recorded with the plan.</p>
                </div>
              </div>
              <div className="plan-form-fields plan-form-execution-fields">
                <label className="plan-url-field">
                  <span>iii WebSocket URL</span>
                  <input
                    required
                    value={url}
                    onChange={(event) => setUrl(event.target.value)}
                  />
                </label>
                <div>
                  <span>Execution model</span>
                  <ProviderModelDropdown
                    required
                    ariaLabel="Execution model"
                    value={subject}
                    disabled={loading}
                    onChange={setSubject}
                    groups={groupedModels.map((group) => ({
                      provider: group.provider,
                      models: group.models.map((item) => ({
                        label: item.model,
                        value: modelKey(item),
                      })),
                    }))}
                    placeholder="Choose a model"
                  />
                </div>
                <div>
                  <span>
                    Judge model <small>optional</small>
                  </span>
                  <ProviderModelDropdown
                    ariaLabel="Judge model"
                    value={judge}
                    disabled={loading}
                    onChange={setJudge}
                    groups={groupedModels.map((group) => ({
                      provider: group.provider,
                      models: group.models.map((item) => ({
                        label: item.model,
                        value: modelKey(item),
                      })),
                    }))}
                    placeholder="Use default judge"
                  />
                </div>
              </div>
            </section>
            <fieldset className="plan-scope-field">
              <legend>
                <span>03</span>
                Tests <small>choose the smallest useful scope</small>
              </legend>
              <div className="plan-scope-intro">
                <div>
                  <h2>Build the test scope</h2>
                  <p>
                    Start empty. Search for the behaviors touched by your
                    change, then select only those tests.
                  </p>
                </div>
                <output aria-live="polite">
                  <strong>{scenarios.length}</strong>
                  <span>
                    {scenarios.length === 1
                      ? 'test selected'
                      : 'tests selected'}
                    {' · '}
                    {plannedRuns} logical {plannedRuns === 1 ? 'run' : 'runs'}
                  </span>
                </output>
              </div>
              <div className="plan-scope-controls">
                <label className="plan-test-search">
                  <span>Find a test</span>
                  <input
                    type="search"
                    value={testQuery}
                    placeholder="Search by name or id"
                    onChange={(event) => setTestQuery(event.target.value)}
                  />
                </label>
                <div className="plan-scope-actions">
                  <button
                    type="button"
                    onClick={selectVisibleScenarios}
                    disabled={visibleScenarios.length === 0}
                  >
                    Select visible ({visibleScenarios.length})
                  </button>
                  <button
                    type="button"
                    onClick={() => setScenarios([])}
                    disabled={scenarios.length === 0}
                  >
                    Clear selection
                  </button>
                </div>
              </div>
              <div className="plan-test-options">
                {visibleScenarios.map((scenario) => {
                  const selected = scenarios.includes(scenario)
                  return (
                    <label
                      className={`plan-test-option${selected ? ' is-selected' : ''}`}
                      key={scenario}
                    >
                      <input
                        type="checkbox"
                        checked={selected}
                        onChange={(event) =>
                          toggleScenario(scenario, event.target.checked)
                        }
                      />
                      <span className="plan-test-option-copy">
                        <strong>{scenarioName(scenario)}</strong>
                        <code>{scenario}</code>
                      </span>
                    </label>
                  )
                })}
                {!visibleScenarios.length && (
                  <p className="plan-test-empty">
                    No tests match “{testQuery}”. Try another name or clear the
                    search.
                  </p>
                )}
              </div>
              <p className="plan-scope-note">
                The scope becomes immutable when the baseline starts. Technical
                retries do not add evidence samples.
              </p>
            </fieldset>
            <details className="plan-advanced">
              <summary>
                <span className="plan-advanced-summary-copy">
                  <span>Optional controls</span>
                  <strong>Sampling and retries</strong>
                  <small>
                    Keep the default unless you deliberately need more local
                    evidence.
                  </small>
                </span>
                <span className="plan-advanced-summary-meta">
                  {runsPerTest} {runsPerTest === 1 ? 'run' : 'runs'} / test ·{' '}
                  {retryCount} {retryCount === 1 ? 'retry' : 'retries'}
                </span>
              </summary>
              <div className="plan-advanced-body">
                <p className="plan-advanced-intro">
                  Runs create logical evidence samples. Retries only recover a
                  technical failure and never increase the evidence count.
                </p>
                <div className="plan-advanced-grid">
                  <label className="plan-advanced-control">
                    <span>
                      <strong>Runs per test</strong>
                      <small>default: 1</small>
                    </span>
                    <p>
                      Increase only when you want repeatable local evidence.
                    </p>
                    <input
                      type="number"
                      min="1"
                      max="20"
                      value={runs}
                      onChange={(event) => setRuns(event.target.value)}
                    />
                  </label>
                  <label className="plan-advanced-control">
                    <span>
                      <strong>Technical retries</strong>
                      <small>default: 1</small>
                    </span>
                    <p>Retries a technical failure without adding a sample.</p>
                    <input
                      type="number"
                      min="0"
                      max="3"
                      value={technicalRetries}
                      onChange={(event) =>
                        setTechnicalRetries(event.target.value)
                      }
                    />
                  </label>
                  <label className="plan-advanced-control">
                    <span>
                      <strong>Seed</strong>
                      <small>optional</small>
                    </span>
                    <p>Leave blank to resolve the canonical case seeds.</p>
                    <input
                      type="number"
                      min="0"
                      placeholder="Canonical"
                      value={seed}
                      onChange={(event) => setSeed(event.target.value)}
                    />
                  </label>
                </div>
              </div>
            </details>
            <div className="plan-form-actions">
              <p
                id="plan-create-requirements"
                className="plan-create-requirements"
              >
                {hasPlanLabel && scenarios.length > 0
                  ? 'Ready to create a draft. The scope stays editable until you start the baseline.'
                  : `Before creating: ${hasPlanLabel ? 'select at least one test' : 'add a plan label'}${hasPlanLabel || scenarios.length === 0 ? '' : ' and select at least one test'}.`}
              </p>
              <a className="button" href={hashForPlans()}>
                Cancel
              </a>
              <button
                className="button button-primary"
                type="submit"
                disabled={!canCreate}
                aria-describedby="plan-create-requirements"
              >
                {submitting ? 'Creating…' : 'Create draft plan'}
              </button>
            </div>
          </form>
        </section>
      </main>
      <footer>
        <span>Harness E2E · local plans</span>
        <a href={hashForWorkspace()}>Back to home</a>
      </footer>
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
  const selectedCandidateNumber =
    plan && selectedCandidateId
      ? plan.candidate_execution_ids.indexOf(selectedCandidateId) + 1
      : null
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
      <a className="skip-link" href="#plan-detail-main">
        Skip to plan detail
      </a>
      <PlanHeader />
      <main
        id="plan-detail-main"
        className="page-shell overview-shell plan-detail-shell"
      >
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
              className="page-heading"
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
              <div className="sync-block">
                <span>Scope state</span>
                <strong>
                  {plan.locked ? 'Locked after baseline' : 'Editable draft'}
                </strong>
              </div>
            </section>
            <section className="plan-status-grid">
              <article className="panel plan-status-card">
                <div className="section-kicker">Comparison readiness</div>
                <strong className={`status-pill ${readiness.tone}`}>
                  {readiness.label}
                </strong>
                <p>{readiness.detail}</p>
                <small>
                  Statistical maturity is reported separately; it does not block
                  a local baseline.
                </small>
              </article>
              <article className="panel plan-status-card">
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
              onVisualBaselineChange={changeVisualBaseline}
              onToggleCandidate={toggleComparisonCandidate}
              onRenameCandidate={renameCandidate}
              loading={historyLoading}
              error={historyError}
            />
            <PlanComparisonPanel
              comparison={comparison}
              baselineExecutionId={visualBaselineId}
              candidateExecutionId={selectedCandidateId}
              candidateNumber={
                selectedCandidateNumber && selectedCandidateNumber > 0
                  ? selectedCandidateNumber
                  : null
              }
              baselineLabel={`Visual baseline · ${planExecutionLabel(plan, visualBaselineId)}`}
              candidateLabel={planExecutionLabel(plan, selectedCandidateId)}
              loading={comparisonLoading}
              error={comparisonError}
            />
            <PlanLifecycle
              plan={plan}
              starting={starting}
              feedback={runFeedback}
              onStart={(role) => void start(role)}
            />
          </>
        )}
      </main>
      <footer>
        <span>Harness E2E · local plan detail</span>
        <a href={hashForPlans()}>Back to plans</a>
      </footer>
    </>
  )
}
