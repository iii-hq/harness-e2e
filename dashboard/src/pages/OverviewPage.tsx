import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  DashboardPageActions,
  dashboardHeaderActionClassName,
} from '@/components/DashboardPageActions'
import { consumeQuickExecutionRequest } from '@/components/ExecutionSetup'
import { LocalRunnerDialog } from '@/components/LocalRunnerDialog'
import {
  Button,
  buttonClassName,
  MetricCard,
  type MetricTone,
  Panel,
  StatusBadge,
} from '@/design-system'
import {
  hashForExecution,
  hashForNewPlan,
  hashForWorkspace,
  type WorkspaceView,
} from '@/hooks/use-hash-route'
import { useLatestRequest } from '@/hooks/use-latest-request'
import {
  type DashboardDataBridge,
  type DashboardExecutionSummary,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import {
  buildExecutionPresentation,
  categoryLabel,
  categoryMessage,
  type ExecutionPresentation,
  formatDate,
  formatDuration,
  formatPercent,
} from '@/lib/execution-view'
import '@/design-system/styles.css'

function statusCopy(presentation: ExecutionPresentation) {
  if (presentation.attention === 'passed')
    return {
      label: 'Passed',
      title: 'Latest execution passed',
      status: 'passed' as const,
    }
  if (presentation.attention === 'running')
    return {
      label: 'Running',
      title: 'Execution is still running',
      status: 'running' as const,
    }
  if (presentation.attention === 'cancelling')
    return {
      label: 'Cancelling',
      title: 'Cancellation is in progress',
      status: 'cancelling' as const,
    }
  if (presentation.attention === 'cancelled')
    return {
      label: 'Cancelled',
      title: 'Execution was cancelled',
      status: 'cancelled' as const,
    }
  if (presentation.attention === 'incomplete')
    return {
      label: 'Incomplete',
      title: 'Evidence is incomplete',
      status: 'incomplete' as const,
    }
  if (presentation.attention === 'unavailable')
    return {
      label: 'Unavailable',
      title: 'No report evidence is available',
      status: 'unavailable' as const,
    }
  if (presentation.breakdown.hard_gate > 0)
    return {
      label: 'Hard gate failed',
      title: 'A hard gate blocks this execution',
      status: 'hard_gate' as const,
    }
  if (
    presentation.breakdown.inconclusive > 0 &&
    presentation.breakdown.issues === presentation.breakdown.inconclusive
  )
    return {
      label: 'Inconclusive',
      title: 'The latest execution is inconclusive',
      status: 'inconclusive' as const,
    }
  return {
    label: 'Failed',
    title: 'Latest execution needs attention',
    status: 'failed' as const,
  }
}

function modelNames(models: ExecutionPresentation['subjects']) {
  if (models.length === 0) return 'Not reported'
  return models.map((model) => `${model.provider}/${model.model}`).join(', ')
}

function finiteNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function formatTurns(value: unknown) {
  const turns = finiteNumber(value)
  if (turns === null) return 'Turns not captured'
  const rounded = Math.round(turns)
  return `${rounded.toLocaleString()} ${rounded === 1 ? 'turn' : 'turns'}`
}

function metricTone(presentation: ExecutionPresentation): MetricTone {
  if (presentation.attention === 'passed') return 'positive'
  if (presentation.attention === 'needs_attention') return 'negative'
  if (presentation.attention === 'unavailable') return 'unavailable'
  return 'warning'
}

const sectionLabelClassName =
  'font-mono text-[0.6875rem] font-medium uppercase tracking-[0.06em] text-ink-muted'

function SummaryKpi({
  label,
  value,
  caption,
  tone = 'neutral',
  delta,
}: {
  label: string
  value: string
  caption: string
  tone?: MetricTone
  delta?: string
}) {
  return (
    <MetricCard
      className="min-h-0 rounded-[6px] border-0 bg-[var(--surface-fill)] p-4"
      label={label}
      value={value}
      detail={caption}
      tone={tone}
      delta={delta}
    />
  )
}

export function LatestExecution({
  presentation,
}: {
  presentation: ExecutionPresentation
}) {
  const status = statusCopy(presentation)
  const issue = presentation.primaryIssue
  const execution = presentation.execution
  const totalTokens = execution.totals?.total_tokens
  const turnsLabel = formatTurns(execution.totals?.turns)
  const workflow = execution.workflow_metrics
  const workflowStepCount = finiteNumber(workflow?.step_count) ?? 0
  const hasWorkflowMetrics = workflowStepCount > 0
  const workflowTokens = finiteNumber(workflow?.total_tokens)
  const workflowFunctionCalls = finiteNumber(workflow?.function_calls)
  const workflowTokenMetricSteps =
    finiteNumber(workflow?.token_metric_steps) ??
    (workflowTokens !== null ? workflowStepCount : 0)
  const succeededWorkflowSteps = finiteNumber(workflow?.succeeded_steps) ?? 0
  const skippedWorkflowSteps = finiteNumber(workflow?.skipped_steps) ?? 0
  const activeWorkflowSteps =
    (finiteNumber(workflow?.running_steps) ?? 0) +
    (finiteNumber(workflow?.pending_steps) ?? 0)
  const attentionWorkflowSteps =
    (finiteNumber(workflow?.failed_steps) ?? 0) +
    (finiteNumber(workflow?.hard_gate_failed_steps) ?? 0) +
    (finiteNumber(workflow?.cancelled_steps) ?? 0)
  const workflowDurationMs = finiteNumber(workflow?.duration_ms)
  const runtimeSeconds = hasWorkflowMetrics
    ? workflowDurationMs === null
      ? presentation.workflowRuntimeSeconds
      : workflowDurationMs / 1000
    : presentation.modelRuntimeSeconds
  const hardGateCount = finiteNumber(workflow?.hard_gate_count) ?? 0
  const passedHardGateCount =
    finiteNumber(workflow?.passed_hard_gate_count) ?? 0
  const workflowAssetCount = finiteNumber(workflow?.asset_count) ?? 0
  const workflowEvaluationCount = finiteNumber(workflow?.evaluation_count) ?? 0
  const scopeSummary =
    presentation.expectedReports !== null &&
    presentation.receivedReports !== null
      ? `${presentation.receivedReports}/${presentation.expectedReports} reports`
      : null
  return (
    <section aria-labelledby="latest-health-heading">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <div className="grid min-w-0 gap-1">
          <p className={`m-0 ${sectionLabelClassName}`}>Latest execution</p>
          <div className="flex min-w-0 flex-wrap items-center gap-2.5">
            <h2
              className="m-0 font-mono text-sm leading-5 font-semibold tracking-[-0.01em]"
              id="latest-health-heading"
              title={status.title}
            >
              {presentation.label}
            </h2>
            <span className="text-[13px] leading-5 text-[var(--color-ink-faint)]">
              {modelNames(presentation.subjects)}
              {scopeSummary ? ` · ${scopeSummary}` : ''}
            </span>
            <StatusBadge status={status.status} label={status.label} />
          </div>
        </div>
        <div className="ms-auto flex flex-wrap items-center gap-3">
          <span className="font-mono text-xs text-ink-muted">
            {formatDate(presentation.completedAt)} · judge{' '}
            {modelNames(presentation.judges)}
          </span>
          <a
            className={buttonClassName({ variant: 'secondary' })}
            href={hashForExecution(execution.id)}
          >
            {issue ? 'Investigate execution' : 'Open execution'}
          </a>
        </div>
      </div>
      <section
        className="mt-4 grid gap-3 sm:grid-cols-2 xl:grid-cols-4"
        aria-label="Latest execution summary"
      >
        <SummaryKpi
          label="Scenario pass rate"
          value={formatPercent(presentation.passRate)}
          caption={
            presentation.breakdown.total > 0
              ? `${presentation.breakdown.passed + presentation.breakdown.passed_with_concerns} of ${presentation.breakdown.total} scenarios passed`
              : 'Objective scenario outcomes'
          }
          tone={metricTone(presentation)}
        />
        {hasWorkflowMetrics ? (
          <SummaryKpi
            label="Semantic steps"
            value={`${succeededWorkflowSteps}/${workflowStepCount}`}
            caption={
              hardGateCount > 0
                ? `${passedHardGateCount}/${hardGateCount} hard gates passed`
                : `${workflowAssetCount} assets · ${workflowEvaluationCount} evaluations`
            }
            delta={
              attentionWorkflowSteps > 0
                ? 'Needs review'
                : activeWorkflowSteps > 0
                  ? 'In progress'
                  : succeededWorkflowSteps + skippedWorkflowSteps ===
                      workflowStepCount
                    ? 'Complete'
                    : 'Incomplete'
            }
            tone={
              attentionWorkflowSteps > 0
                ? 'negative'
                : activeWorkflowSteps > 0
                  ? 'warning'
                  : succeededWorkflowSteps + skippedWorkflowSteps ===
                      workflowStepCount
                    ? 'positive'
                    : 'unavailable'
            }
          />
        ) : (
          <SummaryKpi
            label="Report coverage"
            value={formatPercent(presentation.coverage)}
            caption={
              presentation.expectedReports !== null &&
              presentation.receivedReports !== null
                ? `${presentation.receivedReports} of ${presentation.expectedReports} reports received`
                : 'Completeness was not published'
            }
            tone={
              presentation.coverage === null
                ? 'unavailable'
                : presentation.coverage >= 1
                  ? 'positive'
                  : 'warning'
            }
          />
        )}
        <SummaryKpi
          label={hasWorkflowMetrics ? 'Workflow runtime' : 'Model runtime'}
          value={formatDuration(runtimeSeconds)}
          caption={
            hasWorkflowMetrics
              ? `${workflowAssetCount} assets · ${workflowEvaluationCount} evaluations`
              : presentation.workflowRuntimeSeconds !== null &&
                  presentation.workflowRuntimeSeconds !==
                    presentation.modelRuntimeSeconds
                ? `${formatDuration(presentation.workflowRuntimeSeconds)} total workflow`
                : 'Wall-clock time of the subject models'
          }
          tone={runtimeSeconds === null ? 'unavailable' : 'neutral'}
        />
        <SummaryKpi
          label={hasWorkflowMetrics ? 'Workflow tokens' : 'Total tokens'}
          value={
            hasWorkflowMetrics
              ? (workflowTokens?.toLocaleString() ?? 'Not captured')
              : typeof totalTokens === 'number'
                ? totalTokens.toLocaleString()
                : '—'
          }
          caption={
            hasWorkflowMetrics
              ? `${turnsLabel} · ${workflowFunctionCalls?.toLocaleString() ?? 'No'} function calls · ${workflowTokenMetricSteps}/${workflowStepCount} steps reported tokens`
              : finiteNumber(execution.totals?.turns) !== null
                ? `${turnsLabel} · subject and judge usage`
                : 'Subject and judge usage'
          }
          delta={
            hasWorkflowMetrics
              ? workflowTokenMetricSteps === 0
                ? 'No data'
                : workflowTokenMetricSteps === workflowStepCount
                  ? 'Complete'
                  : 'Partial'
              : undefined
          }
          tone={
            hasWorkflowMetrics
              ? workflowTokenMetricSteps === 0
                ? 'unavailable'
                : workflowTokenMetricSteps === workflowStepCount
                  ? 'neutral'
                  : 'warning'
              : typeof totalTokens === 'number'
                ? 'neutral'
                : 'unavailable'
          }
        />
      </section>
      {issue && (
        <div
          className="mt-3 grid grid-cols-[auto_minmax(0,1fr)] gap-3 rounded-[6px] bg-[color-mix(in_srgb,var(--danger)_7%,transparent)] p-3"
          aria-live="polite"
        >
          <span
            className="mt-1.5 h-1.5 w-1.5 rounded-full bg-[var(--danger)]"
            aria-hidden="true"
          />
          <div>
            <strong className="text-sm font-semibold text-ink">
              {categoryLabel(issue.category)} needs investigation
            </strong>
            <p className="mt-1 mb-0 text-xs leading-5 text-[var(--color-ink-faint)]">
              {categoryMessage(issue.category, issue.count)}
            </p>
          </div>
        </div>
      )}
    </section>
  )
}

const triggerLabels: Record<string, string> = {
  schedule: 'Scheduled',
  workflow_dispatch: 'Manual',
  local: 'Local',
}

function triggerLabel(event: string) {
  return triggerLabels[event] ?? event
}

function countBy<T>(items: T[], key: (item: T) => string | null) {
  const counts = new Map<string, number>()
  for (const item of items) {
    const value = key(item)
    if (!value) continue
    counts.set(value, (counts.get(value) ?? 0) + 1)
  }
  return counts
}

function ModelList({ models }: { models: ExecutionPresentation['subjects'] }) {
  if (models.length === 0) return <span className="text-ink-muted">—</span>
  return (
    <div className="grid gap-0.5">
      {models.map((model) => (
        <span key={`${model.provider}/${model.model}`} className="text-ink">
          {model.model}
          {model.provider ? (
            <small className="block text-ink-muted">{model.provider}</small>
          ) : null}
        </span>
      ))}
    </div>
  )
}

function isInteractiveTarget(target: EventTarget | null) {
  return (
    target instanceof Element &&
    target.closest('a, button, input, select, textarea, summary') !== null
  )
}

export function ExecutionHistory({
  executions,
}: {
  executions: DashboardExecutionSummary[]
}) {
  const [query, setQuery] = useState('')
  const [status, setStatus] = useState('all')
  const [event, setEvent] = useState('all')
  // The table shows a status derived from the assessment summary, so the
  // filter offers exactly those labels, with counts, instead of the raw
  // backend status that never matched the column (audit E-02 / O-11).
  const rows = useMemo(
    () =>
      executions.map((execution) => {
        const presentation = buildExecutionPresentation(execution)
        return {
          execution,
          presentation,
          status: statusCopy(presentation),
          searchText: [
            presentation.label,
            execution.workflow_name,
            execution.id,
            execution.run_id,
            execution.completed_at,
            formatDate(presentation.completedAt),
            execution.source?.sha,
            ...presentation.subjects.flatMap((model) => [
              model.model,
              `${model.provider}/${model.model}`,
            ]),
          ]
            .filter(Boolean)
            .join(' ')
            .toLowerCase(),
        }
      }),
    [executions],
  )
  const statusCounts = useMemo(
    () => countBy(rows, (row) => row.status.status),
    [rows],
  )
  const statusOptions = useMemo(
    () =>
      [...statusCounts.entries()].map(([value, count]) => ({
        value,
        count,
        label:
          rows.find((row) => row.status.status === value)?.status.label ??
          value,
      })),
    [rows, statusCounts],
  )
  const eventCounts = useMemo(
    () => countBy(rows, (row) => row.execution.event ?? null),
    [rows],
  )
  const filtered = useMemo(() => {
    const normalized = query.trim().toLowerCase()
    return rows.filter((row) => {
      if (status !== 'all' && row.status.status !== status) return false
      if (event !== 'all' && row.execution.event !== event) return false
      return !normalized || row.searchText.includes(normalized)
    })
  }, [event, rows, query, status])
  return (
    <section className="mt-6 min-w-0" aria-labelledby="executions-heading">
      <div className="flex items-baseline gap-3">
        <h2
          className={`m-0 uppercase ${sectionLabelClassName}`}
          id="executions-heading"
        >
          Recent executions
        </h2>
        <span
          className="ms-auto font-mono text-xs text-ink-muted"
          aria-live="polite"
        >
          {filtered.length} of {executions.length} executions
        </span>
      </div>
      <section
        className="mt-3 grid gap-2 sm:grid-cols-2 md:grid-cols-[minmax(14rem,1fr)_auto_auto] [&_input]:min-h-9 [&_input]:w-full [&_input]:rounded-[6px] [&_input]:border-0 [&_input]:bg-[var(--surface-fill)] [&_input]:px-3 [&_input]:text-[13px] [&_input]:text-ink [&_input]:placeholder:text-ink-muted [&_select]:min-h-9 [&_select]:w-full [&_select]:rounded-[6px] [&_select]:border-0 [&_select]:bg-[var(--surface-fill)] [&_select]:px-3 [&_select]:font-mono [&_select]:text-[13px] [&_select]:lowercase [&_select]:text-ink"
        aria-label="Execution filters"
      >
        <label className="sm:col-span-2 md:col-span-1">
          <span className="visually-hidden">Search executions</span>
          <input
            type="search"
            placeholder="Search label, model, id or date"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
        <label>
          <span className="visually-hidden">Filter by result</span>
          <select
            value={status}
            onChange={(event) => setStatus(event.target.value)}
          >
            <option value="all">all results · {executions.length}</option>
            {statusOptions.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label.toLowerCase()} · {option.count}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span className="visually-hidden">Filter by trigger</span>
          <select
            value={event}
            onChange={(event) => setEvent(event.target.value)}
          >
            <option value="all">all triggers · {executions.length}</option>
            {[...eventCounts.entries()].map(([value, count]) => (
              <option key={value} value={value}>
                {triggerLabel(value).toLowerCase()} · {count}
              </option>
            ))}
          </select>
        </label>
      </section>
      <div className="mt-2 min-w-0 overflow-x-auto">
        <table className="w-full min-w-[52rem] border-collapse text-left font-mono text-xs md:text-[13px] [&_a]:font-medium [&_a]:text-ink [&_a]:underline-offset-4 [&_a:hover]:underline [&_td]:border-0 [&_td]:px-3 [&_td]:py-2.5 [&_th]:border-0 [&_th]:px-3 [&_th]:py-2 [&_th]:font-mono [&_th]:text-[11px] [&_th]:font-medium [&_th]:uppercase [&_th]:tracking-[0.06em] [&_th]:text-ink-muted [&_tbody_tr]:cursor-pointer [&_tbody_tr]:transition-colors [&_tbody_tr:hover]:bg-[var(--surface-soft)]">
          <thead>
            <tr>
              <th scope="col">Execution</th>
              <th scope="col">Result</th>
              <th scope="col">Subject</th>
              <th scope="col">Scope</th>
              <th scope="col">Outcome</th>
              <th scope="col">Efficiency</th>
            </tr>
          </thead>
          <tbody>
            {filtered.map(({ execution, presentation, status }) => {
              const href = hashForExecution(execution.id)
              const evidenceNote =
                execution.availability === 'aggregate'
                  ? 'aggregate report'
                  : execution.availability === 'unavailable'
                    ? 'no report retained'
                    : null
              const partialCoverage =
                presentation.coverage !== null && presentation.coverage < 1
              return (
                // The whole row navigates; the label link keeps the keyboard
                // and screen-reader path (audit E-08 / O-15).
                <tr
                  key={execution.id}
                  onClick={(click) => {
                    if (isInteractiveTarget(click.target)) return
                    window.location.hash = href
                  }}
                >
                  <td data-label="Execution">
                    <a href={href}>{presentation.label}</a>
                    <small className="block text-ink-muted">
                      {formatDate(presentation.completedAt)}
                      {execution.event
                        ? ` · ${triggerLabel(execution.event).toLowerCase()}`
                        : ''}
                    </small>
                  </td>
                  <td data-label="Result">
                    <StatusBadge status={status.status} label={status.label} />
                    {evidenceNote ? (
                      <small className="block text-ink-muted">
                        {evidenceNote}
                      </small>
                    ) : null}
                  </td>
                  <td
                    data-label="Subject"
                    title={modelNames(presentation.subjects)}
                  >
                    <ModelList models={presentation.subjects} />
                  </td>
                  {presentation.available ? (
                    <>
                      <td data-label="Scope">
                        <div className="grid gap-0.5">
                          <strong>
                            {presentation.receivedReports ?? '—'}/
                            {presentation.expectedReports ?? '—'}
                          </strong>
                          {partialCoverage ? (
                            <small>
                              {formatPercent(presentation.coverage)} coverage
                            </small>
                          ) : null}
                        </div>
                      </td>
                      <td data-label="Outcome">
                        <div className="grid gap-0.5">
                          <strong>
                            {formatPercent(presentation.passRate)}
                          </strong>
                          {presentation.primaryIssue ? (
                            <small>
                              {categoryMessage(
                                presentation.primaryIssue.category,
                                presentation.primaryIssue.count,
                              )}
                            </small>
                          ) : null}
                        </div>
                      </td>
                      <td data-label="Efficiency">
                        <div className="grid gap-0.5">
                          <strong>
                            {formatDuration(presentation.modelRuntimeSeconds)}
                          </strong>
                          {presentation.execution.totals?.total_tokens ? (
                            <small>
                              {presentation.execution.totals.total_tokens.toLocaleString()}{' '}
                              tokens
                            </small>
                          ) : null}
                        </div>
                      </td>
                    </>
                  ) : (
                    <td
                      data-label="Report"
                      colSpan={3}
                      className="text-ink-muted"
                    >
                      —
                    </td>
                  )}
                </tr>
              )
            })}
          </tbody>
        </table>
        {filtered.length === 0 && (
          <p className="m-0 py-6 text-center text-[13px] text-[var(--color-ink-faint)]">
            No executions match these filters.{' '}
            <button
              type="button"
              className="font-mono text-[var(--accent)] underline-offset-4 hover:underline"
              onClick={() => {
                setQuery('')
                setStatus('all')
                setEvent('all')
              }}
            >
              clear filters
            </button>
          </p>
        )}
      </div>
    </section>
  )
}

function FlaskIcon() {
  return (
    <svg
      width="18"
      height="18"
      viewBox="0 0 18 18"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M7 2h4M9 2v4.2l4.6 7.4a1.6 1.6 0 0 1-1.36 2.4H5.76a1.6 1.6 0 0 1-1.36-2.4L9 6.2" />
      <path d="M6.2 11.5h5.6" />
    </svg>
  )
}

export function OverviewPage({ activeView }: { activeView: WorkspaceView }) {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [executions, setExecutions] = useState<DashboardExecutionSummary[]>([])
  const [catalog, setCatalog] = useState<{
    count: number
    revision: string
  } | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [runnerOpen, setRunnerOpen] = useState(false)
  const beginRequest = useLatestRequest()

  const load = useCallback(async () => {
    const request = beginRequest()
    setLoading(true)
    setError(null)
    try {
      const nextBridge = bridge ?? (await getDashboardDataBridge())
      if (!request.isCurrent()) return
      setBridge(nextBridge)
      const manifest = await nextBridge.listExecutions({ limit: 100 })
      if (!request.isCurrent()) return
      setExecutions(manifest.executions ?? [])
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setLoading(false)
    }
  }, [beginRequest, bridge])

  useEffect(() => {
    void load()
  }, [load])

  useEffect(() => {
    if (!bridge) return
    let cancelled = false
    void bridge
      .listTests({ limit: 100 })
      .then((response) => {
        if (cancelled) return
        setCatalog({
          count: response.rows?.length ?? 0,
          revision: response.revision ?? '',
        })
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [bridge])

  useEffect(() => {
    if (consumeQuickExecutionRequest()) setRunnerOpen(true)
  }, [])

  const latest = executions[0]
    ? buildExecutionPresentation(executions[0])
    : null
  const local = bridge?.mode === 'local'

  const aside = (
    <aside className="grid content-start gap-3" aria-label="Suite controls">
      {local && (
        <div className="grid gap-2.5 rounded-[6px] bg-[var(--surface-fill)] p-4">
          <span className={sectionLabelClassName}>Quick run</span>
          <p className="m-0 text-[13px] leading-5 text-[var(--color-ink-faint)]">
            Run the full suite or a subset against the local stack. Results
            publish here as each scenario finishes.
          </p>
          <Button variant="primary" onClick={() => setRunnerOpen(true)}>
            Run suite
          </Button>
        </div>
      )}
      <div className="grid gap-1.5 rounded-[6px] bg-[var(--surface-fill)] p-4">
        <span className={sectionLabelClassName}>Catalog</span>
        <p className="m-0 text-[13px] leading-5 text-[var(--color-ink-faint)]">
          {catalog ? `${catalog.count} tests` : 'Tests'}
          {catalog?.revision ? (
            <>
              {' · revision '}
              <span className="font-mono text-xs">
                {catalog.revision.slice(-12)}
              </span>
            </>
          ) : null}
        </p>
        <a
          className="font-mono text-xs font-medium text-[var(--accent)] no-underline hover:underline"
          href={hashForWorkspace('tests')}
        >
          Browse tests →
        </a>
      </div>
    </aside>
  )

  return (
    <div className="ds-root min-h-dvh bg-[var(--color-bg)] text-ink">
      {/* biome-ignore lint/a11y/useValidAnchor: a skip link must stay a link; the console owns the hash router, so the handler moves focus instead of changing the route (audit E-15). */}
      <a
        className="skip-link"
        href="#main"
        onClick={(click) => {
          click.preventDefault()
          document.getElementById('main')?.focus()
        }}
      >
        Skip to execution dashboard
      </a>
      <DashboardPageActions
        active={activeView}
        actionsLabel="Overview actions"
        actions={
          local ? (
            <>
              <a
                className={dashboardHeaderActionClassName()}
                href={hashForNewPlan()}
              >
                New plan
              </a>
              <button
                className={dashboardHeaderActionClassName({ primary: true })}
                type="button"
                onClick={() => setRunnerOpen(true)}
              >
                Run suite
              </button>
            </>
          ) : null
        }
      />
      <main
        id="main"
        tabIndex={-1}
        className="page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-16 outline-none md:w-[calc(100%_-_3rem)]"
      >
        {error && (
          <Panel
            className="empty-state grid justify-items-start gap-4 p-8"
            role="alert"
            tone="raised"
          >
            <StatusBadge status="unavailable" label="Data unavailable" />
            <h2>Dashboard data unavailable</h2>
            <p>{error}</p>
            <Button onClick={() => void load()}>Retry</Button>
          </Panel>
        )}
        {!error && loading && (
          <Panel
            className="grid min-h-56 content-center gap-6 p-8"
            aria-busy="true"
            aria-label="Loading execution evidence"
          >
            <div className="h-4 w-40 animate-pulse rounded-full bg-[var(--surface-soft)] motion-reduce:animate-none" />
            <div className="h-16 w-full max-w-3xl animate-pulse rounded-[6px] bg-[var(--surface-soft)] motion-reduce:animate-none" />
          </Panel>
        )}
        {!error && !loading && (
          <div id="overview-content" className="grid gap-0">
            {executions.length === 0 ? (
              <div className="mt-14 grid justify-items-center gap-4">
                <div className="grid w-full max-w-md gap-3 rounded-[6px] bg-[var(--surface-fill)] p-6">
                  <div className="grid grid-cols-[auto_minmax(0,1fr)] items-start gap-3">
                    <span className="mt-0.5 text-[var(--color-ink-faint)]">
                      <FlaskIcon />
                    </span>
                    <div className="grid gap-1.5">
                      <h2 className="m-0 text-base leading-6 font-semibold tracking-[-0.01em]">
                        No executions yet
                      </h2>
                      <p className="m-0 max-w-[44ch] text-[13px] leading-[1.7] text-[var(--color-ink-faint)]">
                        Run the suite once to publish outcomes, retained
                        evidence and efficiency for this stack. Results land
                        here as soon as the first scenario finishes.
                      </p>
                    </div>
                  </div>
                  <div className="flex flex-wrap items-center gap-2 pl-[30px]">
                    {local && (
                      <Button
                        variant="primary"
                        onClick={() => setRunnerOpen(true)}
                      >
                        Run suite
                      </Button>
                    )}
                    <a
                      className={buttonClassName({ variant: 'quiet' })}
                      href={hashForWorkspace('tests')}
                    >
                      Browse tests
                    </a>
                  </div>
                </div>
                <p className="m-0 font-mono text-xs text-ink-muted">
                  {catalog
                    ? `${catalog.count} tests in catalog`
                    : 'Catalog loading'}
                  {local ? ' · local runner ready' : ''}
                </p>
              </div>
            ) : activeView === 'overview' ? (
              <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_300px]">
                <div className="min-w-0">
                  {latest && <LatestExecution presentation={latest} />}
                  <ExecutionHistory executions={executions} />
                </div>
                {aside}
              </div>
            ) : (
              <ExecutionHistory executions={executions} />
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
    </div>
  )
}
