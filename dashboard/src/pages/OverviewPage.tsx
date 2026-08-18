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
  type OperationalStatus,
  PageHeader,
  Panel,
  StatusBadge,
} from '@/design-system'
import {
  hashForExecution,
  hashForNewPlan,
  hashForPlans,
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
  type FailureCategory,
  formatDate,
  formatDuration,
  formatPercent,
  isExecutionAttention,
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

function metricTone(presentation: ExecutionPresentation): MetricTone {
  if (presentation.attention === 'passed') return 'positive'
  if (presentation.attention === 'needs_attention') return 'negative'
  if (presentation.attention === 'unavailable') return 'unavailable'
  return 'warning'
}

function failureStatus(category: FailureCategory): OperationalStatus {
  if (category === 'hard_gate') return 'hard_gate'
  if (category === 'inconclusive') return 'inconclusive'
  return 'failed'
}

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
      className="overview-v2-metric min-h-40 rounded-none border-0 border-t border-[var(--color-rule)] p-4 md:[&:nth-child(even)]:border-l lg:col-span-2 lg:min-h-0 lg:border-t-0 lg:border-l lg:p-5 lg:[&:nth-child(-n+2)]:border-b [&_.ds-metric-value]:text-[clamp(1.75rem,2.2vw,2.5rem)]"
      label={label}
      value={value}
      detail={caption}
      tone={tone}
      delta={delta}
    />
  )
}

function FailureBreakdown({
  presentation,
}: {
  presentation: ExecutionPresentation
}) {
  const allCategories: FailureCategory[] = [
    'infrastructure',
    'resource_limit',
    'subject',
    'judge',
    'hard_gate',
    'inconclusive',
  ]
  const categories = allCategories.filter(
    (key) => presentation.breakdown[key] > 0,
  )
  if (categories.length === 0) {
    return (
      <p className="overview-v2-no-events m-0 text-sm text-[var(--color-ink-faint)]">
        No blocking events were reported.
      </p>
    )
  }
  return (
    <section
      className="overview-v2-breakdown grid gap-2 md:grid-cols-2"
      aria-label="Reliability events"
    >
      {categories.map((category) => (
        <article
          key={category}
          className="overview-v2-breakdown-item grid gap-2 rounded-xl border border-[var(--color-rule)] bg-[var(--color-panel-raised)] p-3"
        >
          <StatusBadge
            status={failureStatus(category)}
            label={categoryLabel(category)}
          />
          <p className="m-0 text-xs leading-5 text-[var(--color-ink-faint)]">
            {categoryMessage(category, presentation.breakdown[category])}
          </p>
        </article>
      ))}
    </section>
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
  return (
    <section
      className="latest-evidence overview-v2-bento mt-6 grid grid-flow-dense grid-cols-1 gap-0 overflow-hidden rounded-[var(--ds-radius-md)] border border-[var(--color-edge)] bg-[var(--color-panel)] md:grid-cols-2 lg:grid-cols-12 lg:grid-rows-2"
      aria-labelledby="latest-health-heading"
    >
      <Panel
        as="article"
        padding="generous"
        className="latest-health overview-v2-signal rounded-none border-0 p-5 md:col-span-2 md:p-6 lg:col-span-8 lg:row-span-2"
      >
        <div className="latest-health-heading grid grid-cols-[minmax(0,1fr)_auto] items-start gap-6">
          <div>
            <p className="overview-v2-context m-0 font-mono text-[0.7rem] font-semibold uppercase tracking-[0.08em] text-[var(--color-accent)]">
              Latest execution
            </p>
            <h2
              className="mt-2 max-w-4xl text-[clamp(1.8rem,3vw,3rem)] leading-[0.98] font-semibold tracking-[-0.045em] text-balance"
              id="latest-health-heading"
            >
              {status.title}
            </h2>
          </div>
          <StatusBadge status={status.status} label={status.label} />
        </div>
        <p className="trend-description mt-3 max-w-3xl text-sm leading-6 text-[var(--color-ink-faint)]">
          {presentation.expectedReports !== null &&
          presentation.receivedReports !== null
            ? `${presentation.receivedReports} of ${presentation.expectedReports} expected reports received.`
            : 'Report completeness was not published for this execution.'}{' '}
          {presentation.available
            ? 'Open the detail to inspect the evidence and recommendation.'
            : 'Only aggregate metadata is retained.'}
        </p>
        <section
          className="latest-health-meta overview-v2-identity mt-5 grid grid-cols-2 gap-px overflow-hidden rounded-lg border border-[var(--color-rule)] bg-[var(--color-rule)] lg:grid-cols-4 [&>span]:grid [&>span]:min-w-0 [&>span]:gap-1.5 [&>span]:bg-[var(--color-panel-raised)] [&>span]:p-3 [&_small]:font-mono [&_small]:text-[0.6rem] [&_small]:uppercase [&_small]:tracking-[0.06em] [&_small]:text-[var(--color-ink-ghost)] [&_strong]:overflow-hidden [&_strong]:font-mono [&_strong]:text-xs [&_strong]:font-medium [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap"
          aria-label="Latest execution identity"
        >
          <span>
            <small>Execution</small>
            <strong title={presentation.label}>{presentation.label}</strong>
          </span>
          <span>
            <small>Subject</small>
            <strong title={modelNames(presentation.subjects)}>
              {modelNames(presentation.subjects)}
            </strong>
          </span>
          <span>
            <small>Judge</small>
            <strong title={modelNames(presentation.judges)}>
              {modelNames(presentation.judges)}
            </strong>
          </span>
          <span>
            <small>Completed</small>
            <strong>{formatDate(presentation.completedAt)}</strong>
          </span>
        </section>
        <div
          className={`latest-first-failure overview-v2-primary-issue mt-4 grid grid-cols-[auto_minmax(0,1fr)] gap-3 rounded-lg border border-[var(--color-rule)] bg-[var(--color-panel-raised)] p-3 ${isExecutionAttention(presentation) ? 'has-failure' : ''}`}
          aria-live="polite"
        >
          <span
            className="latest-signal-icon overview-v2-signal-mark mt-1 h-2.5 w-2.5 rounded-full bg-[var(--color-ok)] shadow-[0_0_0_5px_color-mix(in_srgb,var(--color-ok)_12%,transparent)] [.has-failure_&]:bg-[var(--color-alert)] [.has-failure_&]:shadow-[0_0_0_5px_color-mix(in_srgb,var(--color-alert)_12%,transparent)]"
            aria-hidden="true"
          />
          <div>
            <strong className="text-sm font-semibold text-[var(--color-ink)]">
              {issue
                ? `${categoryLabel(issue.category)} needs investigation`
                : 'No blocking failure in the latest execution'}
            </strong>
            <p className="mt-1 mb-0 text-xs leading-5 text-[var(--color-ink-faint)]">
              {issue
                ? categoryMessage(issue.category, issue.count)
                : 'The result is ready for deeper evidence review.'}
            </p>
          </div>
        </div>
        <div className="overview-v2-breakdown-wrap mt-3">
          <FailureBreakdown presentation={presentation} />
        </div>
        <div className="latest-health-actions overview-v2-signal-actions mt-4 flex flex-wrap gap-2">
          <a
            className={buttonClassName({ variant: 'primary' })}
            href={hashForExecution(execution.id)}
          >
            {issue ? 'Investigate execution' : 'Open execution'}
          </a>
          {execution.workflow_url && (
            <a
              className={buttonClassName({ variant: 'quiet' })}
              href={execution.workflow_url}
            >
              Open workflow ↗
            </a>
          )}
        </div>
      </Panel>
      <section
        className="latest-kpi-grid overview-v2-metrics contents"
        aria-label="Latest execution summary"
      >
        <SummaryKpi
          label="Scenario pass rate"
          value={formatPercent(presentation.passRate)}
          caption="Objective scenario outcomes"
          delta={
            presentation.attention === 'passed'
              ? 'Healthy'
              : presentation.passRate === null
                ? 'No data'
                : 'Attention'
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
            delta={
              presentation.coverage === null
                ? 'No data'
                : presentation.coverage >= 1
                  ? 'Complete'
                  : 'Incomplete'
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
              : presentation.workflowRuntimeSeconds !== null
                ? `${formatDuration(presentation.workflowRuntimeSeconds)} total workflow`
                : 'Workflow duration not reported'
          }
          delta={hasWorkflowMetrics ? 'Persisted' : 'Observed'}
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
              ? `${workflowFunctionCalls?.toLocaleString() ?? 'No'} function calls · ${workflowTokenMetricSteps}/${workflowStepCount} steps reported tokens`
              : 'Retained subject and judge usage'
          }
          delta={
            hasWorkflowMetrics
              ? workflowTokenMetricSteps === 0
                ? 'No data'
                : workflowTokenMetricSteps === workflowStepCount
                  ? 'Complete'
                  : 'Partial'
              : typeof totalTokens === 'number'
                ? 'Latest'
                : 'No data'
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
    </section>
  )
}

function ExecutionHistory({
  executions,
}: {
  executions: DashboardExecutionSummary[]
}) {
  const [query, setQuery] = useState('')
  const [status, setStatus] = useState('all')
  const [event, setEvent] = useState('all')
  const filtered = useMemo(() => {
    const normalized = query.trim().toLowerCase()
    return executions.filter((execution) => {
      if (status !== 'all' && execution.status !== status) return false
      if (event !== 'all' && execution.event !== event) return false
      if (!normalized) return true
      return [
        execution.label,
        execution.id,
        execution.run_id,
        execution.completed_at,
        execution.source?.sha,
      ]
        .filter(Boolean)
        .join(' ')
        .toLowerCase()
        .includes(normalized)
    })
  }, [event, executions, query, status])
  return (
    <Panel
      padding="none"
      className="executions-panel overview-v2-history mt-6 overflow-hidden rounded-[var(--ds-radius-md)] border-[var(--color-edge)] bg-[var(--color-panel)] p-0"
      aria-labelledby="executions-heading"
    >
      <PageHeader
        headingLevel={2}
        context="Immutable run ledger"
        title="Recent executions"
        summary="Search objective outcomes, model evidence, scope, and retained diagnostic detail."
        actions={
          <span className="overview-v2-history-count font-mono text-xs text-[var(--color-ink-ghost)]">
            {filtered.length} of {executions.length} executions
          </span>
        }
        id="executions-heading"
        className="panel-heading executions-heading overview-v2-history-heading border-b border-[var(--color-rule)] p-6 md:p-8"
      />
      <section
        className="table-filters grid gap-3 border-b border-[var(--color-rule)] p-4 sm:grid-cols-2 md:grid-cols-[minmax(16rem,1fr)_auto_auto] md:p-6 [&_input]:min-h-11 [&_input]:w-full [&_input]:rounded-lg [&_input]:border [&_input]:border-[var(--color-edge)] [&_input]:bg-[var(--color-panel-raised)] [&_input]:px-3 [&_input]:text-sm [&_input]:text-[var(--color-ink)] [&_select]:min-h-11 [&_select]:w-full [&_select]:rounded-lg [&_select]:border [&_select]:border-[var(--color-edge)] [&_select]:bg-[var(--color-panel-raised)] [&_select]:px-3 [&_select]:text-sm [&_select]:text-[var(--color-ink)]"
        aria-label="Execution filters"
      >
        <label className="search-field sm:col-span-2 md:col-span-1">
          <span className="visually-hidden">Search executions</span>
          <input
            type="search"
            placeholder="Search label, run, or date"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
        <label>
          <span className="visually-hidden">Filter by status</span>
          <select
            value={status}
            onChange={(event) => setStatus(event.target.value)}
          >
            <option value="all">All statuses</option>
            <option value="passed">Passed</option>
            <option value="hard_gate_failed">Hard gate failed</option>
            <option value="technical_failed">Technical failure</option>
            <option value="infra_failed">Infrastructure failure</option>
            <option value="incomplete">Incomplete</option>
            <option value="cancelled">Cancelled</option>
            <option value="running">Running</option>
          </select>
        </label>
        <label>
          <span className="visually-hidden">Filter by trigger</span>
          <select
            value={event}
            onChange={(event) => setEvent(event.target.value)}
          >
            <option value="all">All triggers</option>
            <option value="schedule">Scheduled</option>
            <option value="workflow_dispatch">Manual</option>
            <option value="local">Local</option>
          </select>
        </label>
      </section>
      <div className="table-wrap overflow-x-auto p-3 md:p-4">
        <table className="execution-table w-full min-w-[68rem] border-collapse text-left text-xs md:text-sm [&_a]:font-semibold [&_a]:text-[var(--color-ink)] [&_a]:underline-offset-4 [&_a:hover]:underline [&_td]:border-b [&_td]:border-[var(--color-rule)] [&_td]:px-4 [&_td]:py-4 [&_th]:border-b [&_th]:border-[var(--color-edge)] [&_th]:px-4 [&_th]:py-3 [&_th]:font-mono [&_th]:text-[0.62rem] [&_th]:font-semibold [&_th]:uppercase [&_th]:tracking-[0.06em] [&_th]:text-[var(--color-ink-ghost)] [&_tbody_tr]:transition-colors [&_tbody_tr:hover]:bg-[var(--color-panel-raised)]">
          <thead>
            <tr>
              <th scope="col">Execution</th>
              <th scope="col">Result</th>
              <th scope="col">Subject</th>
              <th scope="col">Scope</th>
              <th scope="col">Outcome</th>
              <th scope="col">Efficiency</th>
              <th scope="col">Evidence</th>
            </tr>
          </thead>
          <tbody>
            {filtered.map((execution) => {
              const presentation = buildExecutionPresentation(execution)
              const status = statusCopy(presentation)
              return (
                <tr key={execution.id}>
                  <td data-label="Execution">
                    <a href={hashForExecution(execution.id)}>
                      {presentation.label}
                    </a>
                    <small className="block text-ink-muted">
                      {formatDate(presentation.completedAt)}
                    </small>
                  </td>
                  <td data-label="Result">
                    <StatusBadge status={status.status} label={status.label} />
                  </td>
                  <td
                    data-label="Subject"
                    title={modelNames(presentation.subjects)}
                  >
                    {modelNames(presentation.subjects)}
                  </td>
                  <td data-label="Scope">
                    <div className="execution-table-stack">
                      <strong>
                        {presentation.receivedReports ?? '—'}/
                        {presentation.expectedReports ?? '—'}
                      </strong>
                      <small>
                        {formatPercent(presentation.coverage)} coverage
                      </small>
                    </div>
                  </td>
                  <td data-label="Outcome">
                    <div className="execution-table-stack">
                      <strong>{formatPercent(presentation.passRate)}</strong>
                      <small>
                        {presentation.primaryIssue
                          ? categoryMessage(
                              presentation.primaryIssue.category,
                              presentation.primaryIssue.count,
                            )
                          : 'No blocking events'}
                      </small>
                    </div>
                  </td>
                  <td data-label="Efficiency">
                    <div className="execution-table-stack">
                      <strong>
                        {formatDuration(presentation.modelRuntimeSeconds)}
                      </strong>
                      <small>
                        {presentation.execution.totals?.total_tokens
                          ? `${presentation.execution.totals.total_tokens.toLocaleString()} tokens`
                          : 'Tokens not reported'}
                      </small>
                    </div>
                  </td>
                  <td data-label="Evidence">
                    <span
                      className={`data-badge data-${execution.availability ?? 'unavailable'}`}
                    >
                      {execution.availability === 'full'
                        ? 'Diagnostic detail'
                        : execution.availability === 'aggregate'
                          ? 'Aggregate'
                          : 'No report'}
                    </span>
                  </td>
                </tr>
              )
            })}
          </tbody>
        </table>
        {filtered.length === 0 && (
          <p className="table-empty">No executions match these filters.</p>
        )}
      </div>
    </Panel>
  )
}

function LocalComparisonCard() {
  return (
    <Panel
      as="article"
      padding="none"
      className="overview-intelligence-card overview-comparison-card overview-v2-comparison-card overflow-hidden rounded-[var(--ds-radius-md)] border-[var(--color-edge)]"
      aria-labelledby="local-comparison-heading"
    >
      <div className="overview-card-main p-5 md:p-6">
        <div className="overview-card-heading grid grid-cols-[minmax(0,1fr)_auto] items-start gap-4">
          <div>
            <div className="section-kicker font-mono text-[0.68rem] uppercase tracking-[0.08em] text-[var(--color-accent)]">
              Local comparison
            </div>
            <h3
              className="mt-2 text-xl leading-tight font-semibold tracking-[-0.025em] md:text-2xl"
              id="local-comparison-heading"
            >
              Comparable by construction
            </h3>
          </div>
          <span className="overview-card-state overview-card-state-neutral overview-v2-state rounded-full border border-[var(--color-edge)] px-3 py-2 font-mono text-[0.62rem] uppercase tracking-[0.04em] text-[var(--color-ink-ghost)]">
            Decision protocol
          </span>
        </div>
        <p className="overview-card-copy mt-3 max-w-4xl text-sm leading-6 text-[var(--color-ink-faint)]">
          Choose only the tests relevant to the current change. Capture one
          baseline run, then rerun the same cases and seeds after editing the
          Harness.
        </p>
        <section
          className="overview-comparison-status mt-4 grid gap-px overflow-hidden rounded-lg border border-[var(--color-rule)] bg-[var(--color-rule)] md:grid-cols-2 xl:grid-cols-4 [&>div]:grid [&>div]:gap-1.5 [&>div]:bg-[var(--color-panel)] [&>div]:p-4 [&_small]:text-xs [&_small]:leading-5 [&_small]:text-[var(--color-ink-faint)] [&_span]:font-mono [&_span]:text-[0.6rem] [&_span]:uppercase [&_span]:tracking-[0.06em] [&_span]:text-[var(--color-ink-ghost)] [&_strong]:text-sm [&_strong]:font-semibold"
          aria-label="Local comparison strategy"
        >
          <div className="overview-comparison-dimension">
            <span>Selected scope</span>
            <strong>Small and explicit</strong>
            <small>
              The comparison is bound to the developer&apos;s chosen tests, not
              the full catalog.
            </small>
          </div>
          <div className="overview-comparison-dimension">
            <span>Functional decision</span>
            <strong>After 1 paired run</strong>
            <small>
              New deliverable, structural, or technical failures are immediately
              actionable.
            </small>
          </div>
          <div className="overview-comparison-dimension">
            <span>Efficiency</span>
            <strong>Directional at n=1</strong>
            <small>
              Cost and wall-time changes inform the developer but do not block
              the local result.
            </small>
          </div>
          <div className="overview-comparison-dimension">
            <span>More confidence</span>
            <strong>Targeted rerun</strong>
            <small>
              Repeat only a suspicious test instead of rerunning the complete
              selection.
            </small>
          </div>
        </section>
        <section
          className="overview-versions mt-4 grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-center gap-3 rounded-lg border border-[var(--color-rule)] bg-[var(--color-surface-hover)] p-3 [&>div]:grid [&>div]:gap-1 [&>div:last-child]:text-right [&_span]:font-mono [&_span]:text-[0.6rem] [&_span]:uppercase [&_span]:tracking-[0.06em] [&_span]:text-[var(--color-ink-ghost)] [&_strong]:text-xs [&_strong]:font-semibold"
          aria-label="Comparison sequence"
        >
          <div className="overview-version">
            <span>Baseline</span>
            <strong>1 run per selected test</strong>
          </div>
          <span aria-hidden="true">→</span>
          <div className="overview-version">
            <span>Candidate</span>
            <strong>Same comparison plan</strong>
          </div>
        </section>
      </div>
      <div className="overview-card-foot flex flex-wrap items-center justify-between gap-4 border-t border-[var(--color-rule)] bg-[var(--color-panel)] px-5 py-3 text-xs text-[var(--color-ink-ghost)] md:px-6">
        <span>One focused pair is enough to start</span>
        <a
          className="overview-card-action inline-flex min-h-11 items-center border-b border-transparent font-semibold text-[var(--color-ink)] no-underline hover:border-[var(--color-ink)]"
          href={hashForPlans()}
        >
          View local plans <span aria-hidden="true">→</span>
        </a>
      </div>
    </Panel>
  )
}

function OverviewDecisionModel() {
  return (
    <section
      className="overview-decision-model overview-v2-comparison mt-6"
      aria-labelledby="decision-model-heading"
    >
      <PageHeader
        headingLevel={2}
        context="Baseline and candidate"
        title="Measure improvement against a fixed comparison"
        summary="Keep scope, cases, seeds, subject, and judge stable before interpreting functional or efficiency changes."
        id="decision-model-heading"
        className="[&_h2]:text-xl md:[&_h2]:text-2xl"
      />
      <div className="overview-intelligence-grid overview-intelligence-grid-single mt-4">
        <LocalComparisonCard />
      </div>
    </section>
  )
}

export function OverviewPage({ activeView }: { activeView: WorkspaceView }) {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [executions, setExecutions] = useState<DashboardExecutionSummary[]>([])
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
    if (consumeQuickExecutionRequest()) setRunnerOpen(true)
  }, [])

  const latest = executions[0]
    ? buildExecutionPresentation(executions[0])
    : null

  return (
    <div className="ds-root overview-v2-root min-h-dvh bg-[var(--color-bg)] text-[var(--color-ink)]">
      <a className="skip-link" href="#main">
        Skip to execution dashboard
      </a>
      <DashboardPageActions
        active={activeView}
        actionsLabel="Overview actions"
        actions={
          bridge?.mode === 'local' ? (
            <>
              <a
                className={dashboardHeaderActionClassName({ primary: true })}
                href={hashForNewPlan()}
                aria-label="New local plan"
              >
                New plan
              </a>
              <button
                className={dashboardHeaderActionClassName()}
                type="button"
                onClick={() => setRunnerOpen(true)}
                aria-label="Quick execution"
              >
                Quick run
              </button>
            </>
          ) : null
        }
      />
      <main
        id="main"
        className="page-shell overview-v2-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-0 pb-32 md:w-[calc(100%_-_3rem)]"
      >
        {activeView === 'overview' ? (
          <PageHeader
            title="Performance overview"
            summary="Objective outcomes, retained evidence, and execution efficiency for the latest Harness run."
            context="Developer workspace"
            className="overview-v2-operational-header border-b border-[var(--color-rule)] py-6 md:py-8 [&_.ds-page-header-copy]:gap-2 [&_h1]:max-w-full [&_h1]:text-2xl md:[&_h1]:text-3xl"
            actions={
              <>
                <span className="grid gap-1 text-right font-mono text-[0.65rem] text-[var(--color-ink-ghost)] max-md:hidden">
                  <span className="uppercase tracking-[0.08em]">
                    Last published
                  </span>
                  <time
                    className="text-[var(--color-ink-faint)]"
                    dateTime={latest?.completedAt}
                  >
                    {latest
                      ? formatDate(latest.completedAt)
                      : loading
                        ? 'Loading'
                        : 'Waiting for data'}
                  </time>
                </span>
                <a
                  className={buttonClassName({ variant: 'primary' })}
                  href={
                    latest
                      ? hashForExecution(latest.execution.id)
                      : '#overview-content'
                  }
                >
                  Open execution
                </a>
                <a
                  className={buttonClassName({ variant: 'secondary' })}
                  href={hashForPlans()}
                >
                  Compare plans
                </a>
              </>
            }
          />
        ) : (
          <PageHeader
            title="Execution ledger"
            summary="Search immutable runs by objective result, model, scope, and retained evidence."
            context="Harness E2E"
            className="overview-v2-ledger-header border-b border-[var(--color-rule)] py-6 md:py-8 [&_h1]:text-2xl md:[&_h1]:text-3xl"
          />
        )}
        {error && (
          <Panel
            className="empty-state mt-6 grid justify-items-start gap-4 p-8"
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
            className="mt-6 grid min-h-56 content-center gap-6 p-8"
            aria-busy="true"
            aria-label="Loading execution evidence"
          >
            <div className="h-4 w-40 animate-pulse rounded-full bg-[var(--color-surface-hover)] motion-reduce:animate-none" />
            <div className="h-16 w-full max-w-3xl animate-pulse rounded-2xl bg-[var(--color-surface-hover)] motion-reduce:animate-none" />
          </Panel>
        )}
        {!error && !loading && (
          <div id="overview-content" className="grid gap-0">
            {executions.length === 0 ? (
              <Panel className="empty-state mt-6 grid justify-items-start gap-4 p-8">
                <StatusBadge status="unavailable" label="Awaiting evidence" />
                <h2>No executions published</h2>
                <p>The next Harness E2E workflow will appear here.</p>
              </Panel>
            ) : (
              <>
                {activeView === 'overview' && latest && (
                  <>
                    <LatestExecution presentation={latest} />
                    <OverviewDecisionModel />
                    <ExecutionHistory executions={executions} />
                  </>
                )}
                {activeView === 'executions' && (
                  <ExecutionHistory executions={executions} />
                )}
              </>
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
      <footer className="mx-auto flex w-[calc(100%_-_1.5rem)] max-w-[1420px] flex-wrap items-center justify-between gap-4 border-t border-[var(--color-rule)] py-8 font-mono text-xs text-[var(--color-ink-ghost)] md:w-[calc(100%_-_3rem)]">
        <span>Harness E2E · execution evidence</span>
        <a href="https://github.com/iii-hq/harness-e2e">
          Suite documentation <span aria-hidden="true">↗</span>
        </a>
      </footer>
    </div>
  )
}
