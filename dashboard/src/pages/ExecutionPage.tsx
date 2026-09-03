import { useEffect, useMemo, useState } from 'react'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { ScenarioMatrix } from '@/components/ScenarioMatrix'
import { TranscriptDialog } from '@/components/TranscriptDialog'
import {
  buttonClassName,
  MetricCard,
  type OperationalStatus,
  PageHeader,
  Panel,
  StatusBadge,
} from '@/design-system'
import { hashForExecution, hashForWorkspace } from '@/hooks/use-hash-route'
import {
  type AssessmentRunMetrics,
  type AssessmentRunView,
  aggregateAssessmentMetrics,
  buildAssessmentWorkspace,
  buildHarnessRecommendation,
} from '@/lib/assessment-view'
import {
  type DashboardExecutionDetail,
  type DashboardExecutionSummary,
  type ExecutionTotals,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import {
  buildExecutionPresentation,
  categoryMessage,
  type ExecutionPresentation,
  formatDate,
  formatDuration,
  formatPercent,
  titleCase,
} from '@/lib/execution-view'
import { buildScenarioMatrix } from '@/lib/scenario-matrix'
import {
  type GeneralRunMetrics,
  summedGeneralRunMetricsFromDetail,
  type WorkflowMetricsSummary,
  workflowMetricsFromDetail,
} from '@/lib/workflow-metrics'
import '@/design-system/styles.css'

type DetailSection = 'summary' | 'results' | 'technical'

const detailPanel =
  'detail-section scroll-mt-28 overflow-hidden rounded-[var(--ds-radius-md)] bg-[var(--surface-fill)] p-5'

function sectionFromAnchor(anchor: string | null | undefined): DetailSection {
  if (
    anchor === 'results' ||
    anchor === 'assessments' ||
    anchor === 'scenarios'
  )
    return 'results'
  if (anchor === 'evidence' || anchor === 'raw-data') return 'technical'
  if (anchor === 'technical' || anchor === 'configuration') return 'technical'
  return 'summary'
}

function summaryFromDetail(
  detail: DashboardExecutionDetail,
  fallback?: DashboardExecutionSummary,
): DashboardExecutionSummary {
  const nested =
    detail.execution && typeof detail.execution === 'object'
      ? detail.execution
      : {}
  return {
    ...(fallback ?? {}),
    ...detail,
    id:
      detail.id ||
      fallback?.id ||
      String((nested as Record<string, unknown>).id ?? ''),
    label:
      detail.label ||
      fallback?.label ||
      String((nested as Record<string, unknown>).label ?? ''),
    status: detail.status || fallback?.status || 'incomplete',
    subjects: detail.subjects ?? fallback?.subjects ?? [],
  }
}

function executionStatus(presentation: ExecutionPresentation): {
  status: OperationalStatus
  label: string
} {
  if (presentation.attention === 'passed')
    return { status: 'passed', label: 'Passed' }
  if (presentation.attention === 'running')
    return { status: 'running', label: 'Running' }
  if (presentation.attention === 'cancelling')
    return { status: 'cancelling', label: 'Cancelling' }
  if (presentation.attention === 'cancelled')
    return { status: 'cancelled', label: 'Cancelled' }
  if (presentation.attention === 'incomplete')
    return { status: 'incomplete', label: 'Incomplete' }
  if (presentation.attention === 'unavailable')
    return { status: 'unavailable', label: 'Unavailable' }
  if (presentation.breakdown.hard_gate > 0)
    return { status: 'hard_gate', label: 'Hard gate failed' }
  if (
    presentation.breakdown.inconclusive > 0 &&
    presentation.breakdown.inconclusive === presentation.breakdown.issues
  )
    return { status: 'inconclusive', label: 'Inconclusive' }
  return { status: 'failed', label: 'Failed' }
}

function statusFromContract(
  value: string | null | undefined,
): OperationalStatus {
  if (value === 'passed' || value === 'pass') return 'passed'
  if (value === 'hard_gate_failed') return 'hard_gate'
  if (value === 'unavailable' || value === 'not_evaluated') return 'unavailable'
  if (value === 'inconclusive') return 'inconclusive'
  if (
    value === 'pass_with_concerns' ||
    value === 'passed_with_concerns' ||
    value === 'partial'
  )
    return 'recommendation'
  if (value === 'running') return 'running'
  if (value === 'cancelled') return 'cancelled'
  if (value === 'incomplete') return 'incomplete'
  return 'failed'
}

function friendlyModelName(model: string) {
  const compact = model.slice(model.lastIndexOf('/') + 1)
  const readable = compact
    .replace(/^gpt-/i, 'GPT-')
    .replace(
      /[-_]+([a-z])/gi,
      (_, letter: string) => ` ${letter.toUpperCase()}`,
    )
  return readable.charAt(0).toUpperCase() + readable.slice(1)
}

function ModelIdentityCard({
  label,
  models,
}: {
  label: string
  models: ExecutionPresentation['subjects']
}) {
  return (
    <div className="min-w-0 bg-panel-raised p-3">
      <div className="font-mono text-[0.6875rem] font-semibold uppercase tracking-[0.06em] text-ink-muted">
        {label}
      </div>
      {models.length ? (
        models.map((model) => (
          <div
            key={`${model.provider}/${model.model}`}
            className="mt-1 min-w-0"
          >
            <strong
              className="block break-words text-sm text-ink"
              title={`${model.provider}/${model.model}`}
            >
              {friendlyModelName(model.model)}
            </strong>
            {model.provider && (
              <code className="mt-0.5 block break-all font-mono text-[0.6875rem] text-ink-muted">
                {model.provider}
              </code>
            )}
          </div>
        ))
      ) : (
        <strong className="mt-1 block text-sm text-ink-muted">
          Not reported
        </strong>
      )}
    </div>
  )
}

type SummaryExecutionMetrics = {
  durationSeconds: number | null
  runCount: number
  workflow: WorkflowMetricsSummary | null
  totalTokens: number | null
  turns: number | null
  functionCalls: number | null
  functionCallErrors: number | null
  totalCostUsd: number | null
}

export function buildSummaryExecutionMetrics(
  presentation: ExecutionPresentation,
  aggregate: AssessmentRunMetrics,
  runCount: number,
  workflow: WorkflowMetricsSummary | null,
  totals: ExecutionTotals | null,
  testTotals: GeneralRunMetrics | null = null,
): SummaryExecutionMetrics {
  const reportedRuns =
    runCount || presentation.receivedReports || presentation.breakdown.total
  const workflowTokens =
    workflow && workflow.tokenMetricSteps > 0 ? workflow.totalTokens : null
  const workflowCalls =
    workflow && workflow.functionCallMetricSteps > 0
      ? workflow.functionCalls
      : null
  const workflowErrors =
    workflow && workflow.functionCallErrorMetricSteps > 0
      ? workflow.functionCallErrors
      : null
  return {
    durationSeconds:
      presentation.workflowRuntimeSeconds ??
      presentation.modelRuntimeSeconds ??
      (aggregate.durationMs === null ? null : aggregate.durationMs / 1000),
    runCount: reportedRuns,
    workflow,
    totalTokens:
      testTotals?.totalTokens ??
      finiteMetric(totals?.total_tokens) ??
      aggregate.totalTokens ??
      workflowTokens,
    turns: finiteMetric(totals?.turns) ?? aggregate.turns,
    functionCalls:
      testTotals?.functionCalls ??
      finiteMetric(totals?.function_calls) ??
      aggregate.functionCalls ??
      workflowCalls,
    functionCallErrors:
      testTotals?.functionCallErrors ??
      finiteMetric(totals?.function_call_errors) ??
      aggregate.functionCallErrors ??
      workflowErrors,
    totalCostUsd: testTotals?.costUsd ?? finiteMetric(totals?.total_cost_usd),
  }
}

function finiteMetric(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function formatMetricCount(value: number | null) {
  return value == null ? '—' : Math.round(value).toLocaleString('en-US')
}

function formatReportedCost(value: number | null) {
  if (value == null) return '—'
  if (value > 0 && value < 0.0001) return '<$0.0001'
  return `$${value.toFixed(4)}`
}

function DecisionBoundary({ label, value }: { label: string; value: string }) {
  return (
    <div className="grid content-start gap-2 bg-panel-raised p-4">
      <dt className="font-mono text-[0.6875rem] font-semibold uppercase tracking-[0.06em] text-ink-muted">
        {label}
      </dt>
      <dd className="m-0">
        <StatusBadge
          status={statusFromContract(value)}
          label={titleCase(value)}
        />
      </dd>
    </div>
  )
}

const NO_RUN_STATES = new Set([
  'cancelled',
  'cancelling',
  'running',
  'incomplete',
  'unavailable',
])

// The decision headline names the two layers of the assessment contract as
// two labelled fields instead of the earlier "<status> objectively; AI
// <verdict>" concatenation (audit ED-04).
export function decisionTitle(
  systemStatus: string,
  advisoryStatus: string,
  hasRun: boolean,
): string {
  if (!hasRun && NO_RUN_STATES.has(systemStatus)) {
    return `${titleCase(systemStatus)} · no assessment retained`
  }
  if (systemStatus === 'passed' && advisoryStatus === 'pass_with_concerns') {
    return 'Passed objectively; advisory review found gaps'
  }
  return `Objective result: ${titleCase(systemStatus)} · advisory: ${titleCase(advisoryStatus)}`
}

export function DecisionSection({
  presentation,
  detail,
  primaryRun,
}: {
  presentation: ExecutionPresentation
  detail: DashboardExecutionDetail
  primaryRun: AssessmentRunView | null
}) {
  const aiResult = primaryRun?.finalAssessment.result
  const systemStatus = primaryRun?.systemStatus ?? presentation.attention
  const advisoryStatus =
    aiResult?.verdict ??
    primaryRun?.finalAssessment.availability ??
    'unavailable'
  const effectiveStatus = primaryRun?.effectiveStatus ?? systemStatus
  const title = decisionTitle(systemStatus, advisoryStatus, Boolean(primaryRun))
  const recommendation = primaryRun
    ? aiResult?.recommendation || buildHarnessRecommendation(primaryRun)
    : NO_RUN_STATES.has(systemStatus)
      ? 'No scenario report was retained. Rerun the execution to obtain one.'
      : 'Inspect the retained execution evidence before deciding whether to rerun.'
  const effectiveDiffers = effectiveStatus !== systemStatus
  const summary = detail.assessment_summary
  const issue = presentation.primaryIssue

  return (
    <Panel
      as="section"
      id="summary"
      padding="generous"
      className="scroll-mt-28 rounded-[var(--ds-radius-md)] border-[var(--color-edge)] p-5 md:p-6"
      aria-labelledby="summary-heading"
    >
      <PageHeader
        headingLevel={2}
        headingId="summary-heading"
        context="Decision"
        title={title}
        summary={
          aiResult?.summary ??
          (issue
            ? categoryMessage(issue.category, issue.count)
            : 'The retained objective result is ready for evidence review.')
        }
        className=""
        actions={
          <div className="flex flex-wrap justify-end gap-2">
            <StatusBadge
              status={statusFromContract(systemStatus)}
              label={`System: ${titleCase(systemStatus)}`}
            />
            <StatusBadge
              status={statusFromContract(advisoryStatus)}
              label={`AI: ${titleCase(advisoryStatus)}`}
            />
          </div>
        }
      />

      <div className="mt-6 grid overflow-hidden rounded-lg border border-[var(--color-rule)] lg:grid-cols-[minmax(0,1.35fr)_minmax(24rem,0.65fr)]">
        <section className="grid content-start gap-3 p-4 md:p-5">
          {aiResult?.diagnosis ? (
            <>
              <p className="m-0 font-mono text-[0.6875rem] font-semibold uppercase tracking-[0.06em] text-[var(--color-accent)]">
                What happened
              </p>
              <p className="m-0 max-w-4xl text-sm leading-6 text-ink">
                {aiResult.diagnosis}
              </p>
            </>
          ) : null}
          <p className="m-0 font-mono text-[0.6875rem] font-semibold uppercase tracking-[0.06em] text-[var(--color-accent)]">
            Recommended next step
          </p>
          <p className="m-0 max-w-4xl text-sm leading-6 text-ink">
            {recommendation}
          </p>
          {aiResult?.concerns?.[0] ? (
            <p className="m-0 border-l-2 border-[var(--color-warn)] pl-3 text-xs leading-5 text-[var(--color-ink-faint)]">
              <strong className="text-warning">Primary concern:</strong>{' '}
              {aiResult.concerns[0]}
            </p>
          ) : null}
          <a
            className={buttonClassName({
              variant: 'quiet',
              size: 'compact',
              className: 'mt-1 w-fit',
            })}
            href={hashForExecution(detail.id, 'results')}
          >
            Inspect retained evidence
          </a>
        </section>
        <div className="grid content-start border-t border-[var(--color-rule)] lg:border-t-0 lg:border-l">
          <dl className="m-0 grid gap-px bg-[var(--color-rule)] sm:grid-cols-3 lg:grid-cols-1">
            <DecisionBoundary label="Objective system" value={systemStatus} />
            <DecisionBoundary label="Advisory AI" value={advisoryStatus} />
            {effectiveDiffers ? (
              <DecisionBoundary
                label="Effective harness"
                value={effectiveStatus}
              />
            ) : null}
          </dl>
          <p className="m-0 bg-panel-raised px-4 pb-4 text-xs leading-5 text-[var(--color-ink-faint)]">
            The objective result is authoritative; the advisory AI never
            overrides it
            {effectiveDiffers
              ? ', and the effective status is the one retained in the report.'
              : '.'}
          </p>
        </div>
      </div>

      <dl className="mt-4 grid grid-cols-2 gap-px overflow-hidden rounded-lg border border-[var(--color-rule)] bg-[var(--color-rule)] lg:grid-cols-4">
        {[
          ['Assessments', summary?.assessment_count],
          ['Passed', summary?.assessment_outcomes?.passed],
          ['Partial', summary?.assessment_outcomes?.partial],
          ['Evidence references', summary?.evidence_reference_count],
        ].map(([label, value]) => (
          <div key={String(label)} className="bg-panel-raised p-3">
            <dt className="font-mono text-[0.6875rem] uppercase tracking-[0.06em] text-ink-muted">
              {label}
            </dt>
            <dd className="mt-1 mb-0 text-xl font-semibold text-ink">
              {typeof value === 'number' ? value.toLocaleString('en-US') : '—'}
            </dd>
          </div>
        ))}
      </dl>
    </Panel>
  )
}

function ResultsSection({
  detail,
  onTranscript,
}: {
  detail: DashboardExecutionDetail
  onTranscript: (run: AssessmentRunView, title: string) => void
}) {
  const runCount = (detail.reports ?? []).reduce(
    (total, record) =>
      total +
      (record.report?.scenarios ?? []).reduce(
        (scenarioTotal, scenario) =>
          scenarioTotal + (scenario.runs?.length ?? 0),
        0,
      ),
    0,
  )
  const scenarioCount = buildScenarioMatrix(detail).summary.total
  return (
    <Panel
      as="section"
      id="results"
      padding="generous"
      className={`${detailPanel} p-5 md:p-6`}
      aria-labelledby="results-heading"
    >
      <PageHeader
        headingLevel={2}
        headingId="results-heading"
        context="Benchmark results"
        title="Scenario results"
        summary="Compare objective outcomes, advisory conclusions, runtime, and scenario structure. Expand one row to inspect its benchmark metrics and workflow."
        className=""
        actions={
          <span className="font-mono text-xs text-ink-muted">
            {scenarioCount} {scenarioCount === 1 ? 'scenario' : 'scenarios'} ·{' '}
            {runCount} {runCount === 1 ? 'run' : 'runs'}
          </span>
        }
      />
      <div className="mt-6">
        <ScenarioMatrix detail={detail} onTranscript={onTranscript} />
      </div>
    </Panel>
  )
}

function compactObject(value: unknown): string | null {
  if (!value || typeof value !== 'object') return null
  const entries = Object.entries(value as Record<string, unknown>).filter(
    ([, entry]) => entry !== null && entry !== undefined && entry !== '',
  )
  if (entries.length === 0) return null
  return entries
    .map(([key, entry]) =>
      typeof entry === 'string' && /^[0-9a-f]{40}$/i.test(entry)
        ? `${key} ${entry.slice(0, 12)}`
        : `${key} ${typeof entry === 'object' ? JSON.stringify(entry) : String(entry)}`,
    )
    .join(' · ')
}

// Provenance rows: only fields with a value, timestamps in the reader's
// locale next to the duration, nested records flattened without null keys
// (audit ED-15).
export function provenanceEntries(
  detail: DashboardExecutionDetail,
  presentation: ExecutionPresentation,
): Array<[string, string]> {
  const started = Date.parse(presentation.startedAt)
  const completed = Date.parse(presentation.completedAt)
  const duration =
    Number.isFinite(started) &&
    Number.isFinite(completed) &&
    completed >= started
      ? formatDuration((completed - started) / 1000)
      : null
  const rows: Array<[string, string | null | undefined]> = [
    ['execution id', detail.id],
    ['run id', detail.run_id],
    ['attempt', detail.attempt == null ? null : String(detail.attempt)],
    ['status', detail.status],
    ['availability', detail.availability],
    ['event', detail.event],
    ['actor', detail.actor],
    [
      'started',
      presentation.startedAt ? formatDate(presentation.startedAt) : null,
    ],
    [
      'completed',
      presentation.completedAt
        ? `${formatDate(presentation.completedAt)}${duration ? ` · ${duration}` : ''}`
        : null,
    ],
    ['source', compactObject(detail.source)],
    ['release', compactObject(detail.release)],
  ]
  return rows.filter((row): row is [string, string] => Boolean(row[1]))
}

function TechnicalSection({
  detail,
  presentation,
}: {
  detail: DashboardExecutionDetail
  presentation: ExecutionPresentation
}) {
  const entries = provenanceEntries(detail, presentation)
  const raw = JSON.stringify(detail, null, 2)
  return (
    <Panel
      as="section"
      id="technical"
      padding="generous"
      className={`${detailPanel} bg-panel-raised p-5 md:p-6`}
      aria-labelledby="technical-heading"
    >
      <PageHeader
        headingLevel={2}
        headingId="technical-heading"
        context="Provenance"
        title="Raw fields and immutable identity"
        summary="Identifiers retained for reproducibility."
        className=""
        actions={
          <button
            type="button"
            className={buttonClassName({ variant: 'quiet', size: 'compact' })}
            onClick={() => void navigator.clipboard?.writeText(raw)}
          >
            Copy JSON
          </button>
        }
      />
      <dl className="m-0 mt-5 grid grid-cols-[max-content_minmax(0,1fr)] gap-x-6 gap-y-2 font-mono text-xs">
        {entries.map(([key, value]) => (
          <div key={key} className="contents">
            <dt className="text-[0.6875rem] font-medium uppercase tracking-[0.06em] text-ink-muted">
              {key}
            </dt>
            <dd className="m-0 break-all text-ink">{value}</dd>
          </div>
        ))}
      </dl>
      <details className="mt-5 rounded-[6px] bg-panel">
        <summary className="min-h-11 cursor-pointer px-4 py-3 text-sm font-semibold text-ink">
          Preview raw JSON
        </summary>
        <pre className="max-h-[560px] overflow-auto p-4 font-mono text-xs text-[var(--color-ink-faint)]">
          {raw}
        </pre>
      </details>
    </Panel>
  )
}

export function ExecutionPage({
  executionId,
  anchor,
}: {
  executionId: string
  anchor?: string | null
}) {
  const [summary, setSummary] = useState<DashboardExecutionSummary | null>(null)
  const [detail, setDetail] = useState<DashboardExecutionDetail | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [transcript, setTranscript] = useState<{
    run: AssessmentRunView
    title: string
  } | null>(null)
  useEffect(() => {
    if (!anchor || !detail) return
    window.requestAnimationFrame(() =>
      document
        .getElementById(sectionFromAnchor(anchor))
        ?.scrollIntoView({ block: 'start' }),
    )
  }, [anchor, detail])

  useEffect(() => {
    let active = true
    setError(null)
    setDetail(null)
    void (async () => {
      try {
        const nextBridge = await getDashboardDataBridge()
        const manifest = await nextBridge.listExecutions({
          ids: [executionId],
          limit: 1,
        })
        const nextSummary = manifest.executions[0] ?? null
        const nextDetail = await nextBridge.getExecution(executionId)
        if (!active) return
        setSummary(nextSummary ?? summaryFromDetail(nextDetail))
        setDetail(nextDetail)
      } catch (cause) {
        if (active)
          setError(cause instanceof Error ? cause.message : String(cause))
      }
    })()
    return () => {
      active = false
    }
  }, [executionId])

  const presentation = useMemo(
    () => (summary ? buildExecutionPresentation(summary) : null),
    [summary],
  )
  const assessmentModel = useMemo(
    () => buildAssessmentWorkspace(detail),
    [detail],
  )
  const summaryMetrics = useMemo(
    () =>
      presentation
        ? buildSummaryExecutionMetrics(
            presentation,
            aggregateAssessmentMetrics(assessmentModel.runs),
            assessmentModel.runs.length,
            detail ? workflowMetricsFromDetail(detail) : null,
            detail?.totals ?? null,
            detail ? summedGeneralRunMetricsFromDetail(detail) : null,
          )
        : null,
    [assessmentModel.runs, detail, presentation],
  )
  const scenarioMatrix = useMemo(
    () => (detail ? buildScenarioMatrix(detail) : null),
    [detail],
  )

  if (error)
    return (
      <div className="ds-root min-h-dvh bg-[var(--color-bg)] text-ink">
        <main
          id="main"
          className="mx-auto grid min-h-dvh w-[min(52rem,calc(100%_-_1.5rem))] place-items-center py-8"
        >
          <Panel
            role="alert"
            padding="generous"
            className="w-full rounded-[var(--ds-radius-md)] border-[var(--color-alert)]"
          >
            <PageHeader
              context="Execution unavailable"
              title="Execution not found"
              summary={error}
              actions={
                <a
                  className={buttonClassName({ variant: 'secondary' })}
                  href={hashForWorkspace('executions')}
                >
                  Back to executions
                </a>
              }
            />
          </Panel>
        </main>
      </div>
    )
  if (!detail || !presentation)
    return (
      <div className="ds-root min-h-dvh bg-[var(--color-bg)] text-ink">
        <main
          id="main"
          className="mx-auto grid min-h-dvh w-[min(52rem,calc(100%_-_1.5rem))] place-items-center py-8"
        >
          <Panel
            className="w-full rounded-[var(--ds-radius-md)]"
            aria-busy="true"
          >
            <p className="m-0 font-mono text-xs uppercase tracking-[0.06em] text-ink-muted">
              Loading execution report…
            </p>
          </Panel>
        </main>
      </div>
    )

  const primaryRun = assessmentModel.runs[0] ?? null
  const aiResult = primaryRun?.finalAssessment.result
  const status = executionStatus(presentation)
  const workflow = summaryMetrics?.workflow ?? null
  const hasRustWorkflow = Boolean(workflow && workflow.stepCount > 0)
  const scenarioSummary = scenarioMatrix?.summary
  const scenarioCount = scenarioSummary?.total ?? 0
  const workflowScenarioCount =
    scenarioMatrix?.items.filter((item) => item.workflowSteps.length > 0)
      .length ?? 0
  const scenarioAttention =
    (scenarioSummary?.failed ?? 0) +
    (scenarioSummary?.hardGate ?? 0) +
    (scenarioSummary?.inconclusive ?? 0) +
    (scenarioSummary?.unavailable ?? 0) +
    (scenarioSummary?.running ?? 0) +
    (scenarioSummary?.incomplete ?? 0)
  const runtimeSeconds =
    presentation.modelRuntimeSeconds ?? summaryMetrics?.durationSeconds ?? null
  const coverageComplete =
    presentation.coverage != null &&
    (presentation.coverage === 1 || presentation.coverage >= 100)
  const noRun = !presentation.available || scenarioCount === 0
  const subjectNames = presentation.subjects
    .map((model) => friendlyModelName(model.model))
    .join(', ')
  const judgeNames = presentation.judges
    .map((model) => friendlyModelName(model.model))
    .join(', ')
  const headerSummary = [
    scenarioCount
      ? `${scenarioCount} ${scenarioCount === 1 ? 'scenario' : 'scenarios'}`
      : 'no scenario report retained',
    status.label.toLowerCase(),
    runtimeSeconds !== null ? formatDuration(runtimeSeconds) : null,
    subjectNames
      ? judgeNames
        ? `${subjectNames} judged by ${judgeNames}`
        : subjectNames
      : null,
  ]
    .filter(Boolean)
    .join(' · ')
  return (
    <div className="ds-root min-h-dvh bg-[var(--color-bg)] text-ink">
      {/* biome-ignore lint/a11y/useValidAnchor: a skip link must stay a link; the console owns the hash router, so the handler moves focus instead of changing the route (audit ED-16). */}
      <a
        className="skip-link"
        href="#main"
        onClick={(click) => {
          click.preventDefault()
          document.getElementById('main')?.focus()
        }}
      >
        Skip to execution details
      </a>
      <DashboardPageActions active="executions" />
      <main
        id="main"
        tabIndex={-1}
        className="page-shell detail-shell w-[min(1380px,calc(100%_-_3rem))] pt-6 outline-none max-[640px]:w-[calc(100%_-_1.5rem)]"
      >
        <nav
          className="breadcrumbs mb-5 flex min-w-0 items-center gap-2 overflow-hidden font-mono text-xs text-ink-muted"
          aria-label="Breadcrumb"
        >
          <a href={hashForWorkspace('executions')}>Executions</a>
          <span aria-hidden="true">/</span>
          <span className="truncate text-ink" aria-current="page">
            {presentation.label}
          </span>
        </nav>
        <PageHeader
          id="detail-summary"
          headingId="detail-heading"
          context="Execution detail"
          title={presentation.label}
          summary={headerSummary}
          className="border-b border-[var(--color-rule)] pb-5 [&_.ds-page-header-copy]:gap-2 [&_h1]:max-w-full [&_h1]:break-words [&_h1]:text-xl"
          actions={
            <div className="flex flex-wrap justify-end gap-2">
              <StatusBadge status={status.status} label={status.label} />
              {scenarioCount === 1 && aiResult ? (
                <StatusBadge
                  status={statusFromContract(aiResult.verdict)}
                  label={`AI: ${titleCase(aiResult.verdict)}`}
                />
              ) : null}
              <span className="inline-flex items-center rounded-[6px] bg-[var(--surface-fill)] px-2 py-1 font-mono text-[11px] lowercase text-[var(--color-ink-faint)]">
                {detail.availability === 'full'
                  ? 'Full evidence'
                  : detail.availability === 'aggregate'
                    ? 'Aggregate'
                    : 'No report'}
              </span>
            </div>
          }
        />

        <Panel
          padding="none"
          className="mt-5 overflow-hidden rounded-[var(--ds-radius-md)] border-[var(--color-edge)]"
        >
          <section
            className="grid grid-cols-2 gap-px bg-[var(--color-rule)] lg:grid-cols-4"
            aria-label="Execution identity"
          >
            <div className="bg-panel-raised p-3">
              <div className="font-mono text-[0.6875rem] font-semibold uppercase tracking-[0.06em] text-ink-muted">
                Scenarios
              </div>
              <strong className="mt-1 block truncate text-sm text-ink">
                {scenarioCount
                  ? `${scenarioCount} ${scenarioCount === 1 ? 'result' : 'results'}`
                  : 'Not reported'}
              </strong>
            </div>
            <ModelIdentityCard label="Subject" models={presentation.subjects} />
            <ModelIdentityCard label="Judge" models={presentation.judges} />
            <div className="bg-panel-raised p-3">
              <div className="font-mono text-[0.6875rem] font-semibold uppercase tracking-[0.06em] text-ink-muted">
                Completed
              </div>
              <strong className="mt-1 block text-sm text-ink">
                {formatDate(presentation.completedAt)}
              </strong>
            </div>
          </section>
          {noRun ? (
            <p className="m-0 border-t border-[var(--color-edge)] p-4 font-mono text-xs text-[var(--color-ink-faint)]">
              No scenario report was retained for this execution
              {presentation.attention === 'cancelled'
                ? ' because it was cancelled'
                : presentation.attention === 'running' ||
                    presentation.attention === 'cancelling'
                  ? ' yet; it is still running'
                  : ''}
              . Metrics appear once a report exists.
            </p>
          ) : (
            <section
              className="grid grid-cols-2 gap-3 border-t border-[var(--color-edge)] p-4 lg:grid-cols-[repeat(auto-fit,minmax(12rem,1fr))]"
              aria-label="Execution metrics"
            >
              <MetricCard
                className="bg-panel"
                label="Objective scenarios"
                value={`${scenarioSummary?.passed ?? 0}/${scenarioCount}`}
                detail={`${scenarioSummary?.hardGate ?? 0} hard gate · ${scenarioSummary?.failed ?? 0} failed`}
                delta={scenarioAttention > 0 ? 'Needs attention' : undefined}
                tone={scenarioAttention > 0 ? 'negative' : 'positive'}
              />
              <MetricCard
                className="bg-panel"
                label="Report coverage"
                value={formatPercent(presentation.coverage)}
                detail={`${presentation.receivedReports ?? 0} of ${presentation.expectedReports ?? 0} reports received`}
                delta={
                  presentation.coverage != null && !coverageComplete
                    ? 'Missing reports'
                    : undefined
                }
                tone={
                  presentation.coverage == null
                    ? 'unavailable'
                    : coverageComplete
                      ? 'positive'
                      : 'warning'
                }
              />
              <MetricCard
                className="bg-panel"
                label="Execution runtime"
                value={formatDuration(runtimeSeconds)}
                detail={
                  presentation.modelRuntimeSeconds != null
                    ? 'Reported wall-clock time'
                    : `Observed across ${summaryMetrics?.runCount ?? 0} runs`
                }
                tone={runtimeSeconds === null ? 'unavailable' : 'neutral'}
              />
              <MetricCard
                className="bg-panel"
                label="Total tokens"
                value={formatMetricCount(summaryMetrics?.totalTokens ?? null)}
                detail={
                  summaryMetrics?.turns != null
                    ? `${formatMetricCount(summaryMetrics.turns)} turns · subject execution usage`
                    : 'Subject execution usage'
                }
                tone={
                  summaryMetrics?.totalTokens == null
                    ? 'unavailable'
                    : 'neutral'
                }
              />
              {summaryMetrics?.functionCalls != null ? (
                <MetricCard
                  className="bg-panel"
                  label="Function calls"
                  value={formatMetricCount(summaryMetrics.functionCalls)}
                  detail={
                    summaryMetrics.functionCallErrors == null
                      ? 'Error count not captured'
                      : `${formatMetricCount(summaryMetrics.functionCallErrors)} errors`
                  }
                  tone={
                    (summaryMetrics.functionCallErrors ?? 0) > 0
                      ? 'warning'
                      : 'neutral'
                  }
                />
              ) : null}
              {summaryMetrics?.totalCostUsd != null ? (
                <MetricCard
                  className="bg-panel"
                  label="Reported cost"
                  value={formatReportedCost(summaryMetrics.totalCostUsd)}
                  detail="Consolidated execution cost"
                />
              ) : null}
              {hasRustWorkflow ? (
                <MetricCard
                  className="bg-panel"
                  label="Workflow scenarios"
                  value={`${workflowScenarioCount}/${scenarioCount}`}
                  detail={`${workflow?.stepCount ?? 0} persisted steps`}
                />
              ) : null}
            </section>
          )}
        </Panel>
        <div className="detail-main grid min-w-0 gap-5">
          {noRun && !primaryRun ? null : (
            <DecisionSection
              presentation={presentation}
              detail={detail}
              primaryRun={primaryRun}
            />
          )}
          <ResultsSection
            detail={detail}
            onTranscript={(run, title) => setTranscript({ run, title })}
          />
          <TechnicalSection detail={detail} presentation={presentation} />
        </div>
      </main>
      {transcript && (
        <TranscriptDialog
          title={transcript.title}
          messages={transcript.run.transcript?.messages}
          open
          onClose={() => setTranscript(null)}
        />
      )}
      <footer className="mx-auto mt-8 flex w-[min(1380px,calc(100%_-_3rem))] flex-wrap items-center justify-between gap-3 border-t border-[var(--color-rule)] py-6 text-xs text-ink-muted max-[640px]:w-[calc(100%_-_1.5rem)]">
        <span>
          Execution report · <code className="font-mono">{detail.id}</code>
        </span>
        <a
          className="text-[var(--color-ink-faint)] underline-offset-4 hover:underline"
          href={hashForWorkspace('executions')}
        >
          Back to all executions
        </a>
      </footer>
    </div>
  )
}
