import { useEffect, useMemo, useState } from 'react'
import { AppHeader } from '@/components/AppHeader'
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
  'detail-section scroll-mt-28 overflow-hidden rounded-[var(--ds-radius-md)] border border-[var(--ds-color-line-strong)] bg-[var(--ds-color-surface)] p-5 md:p-6'

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
    <div className="min-w-0 bg-[var(--ds-color-surface-raised)] p-3">
      <div className="font-mono text-[0.6rem] font-semibold uppercase tracking-[0.06em] text-[var(--ds-color-ink-muted)]">
        {label}
      </div>
      {models.length ? (
        models.map((model) => (
          <div
            key={`${model.provider}/${model.model}`}
            className="mt-1 min-w-0"
          >
            <strong
              className="block break-words text-sm text-[var(--ds-color-ink)]"
              title={`${model.provider}/${model.model}`}
            >
              {friendlyModelName(model.model)}
            </strong>
            {model.provider && (
              <code className="mt-0.5 block break-all font-mono text-[0.59rem] text-[var(--ds-color-ink-muted)]">
                {model.provider}
              </code>
            )}
          </div>
        ))
      ) : (
        <strong className="mt-1 block text-sm text-[var(--ds-color-ink-muted)]">
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

function DecisionBoundary({
  label,
  value,
  caption,
}: {
  label: string
  value: string
  caption: string
}) {
  return (
    <div className="grid content-start gap-2 bg-[var(--ds-color-surface-raised)] p-4">
      <dt className="font-mono text-[0.62rem] font-semibold uppercase tracking-[0.06em] text-[var(--ds-color-ink-muted)]">
        {label}
      </dt>
      <dd className="m-0">
        <StatusBadge
          status={statusFromContract(value)}
          label={titleCase(value)}
        />
      </dd>
      <p className="m-0 text-xs leading-5 text-[var(--ds-color-ink-soft)]">
        {caption}
      </p>
    </div>
  )
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
  const objectivePassed = systemStatus === 'passed'
  const title =
    objectivePassed && advisoryStatus === 'pass_with_concerns'
      ? 'Passed objectively; advisory review found gaps'
      : `${titleCase(systemStatus)} objectively; AI ${titleCase(advisoryStatus)}`
  const recommendation = primaryRun
    ? aiResult?.recommendation || buildHarnessRecommendation(primaryRun)
    : 'Inspect the retained execution evidence before deciding whether to rerun.'
  const summary = detail.assessment_summary
  const issue = presentation.primaryIssue

  return (
    <Panel
      as="section"
      id="summary"
      padding="generous"
      className="scroll-mt-28 rounded-[var(--ds-radius-md)] border-[var(--ds-color-line-strong)] p-5 md:p-6"
      aria-labelledby="summary-heading"
    >
      <PageHeader
        headingLevel={2}
        context="Decision"
        title={title}
        summary={
          aiResult?.summary ??
          (issue
            ? categoryMessage(issue.category, issue.count)
            : 'The retained objective result is ready for evidence review.')
        }
        className="[&_h2]:text-[clamp(1.5rem,2.6vw,2.35rem)]"
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

      <div className="mt-6 grid overflow-hidden rounded-lg border border-[var(--ds-color-line)] lg:grid-cols-[minmax(0,1.35fr)_minmax(24rem,0.65fr)]">
        <section className="grid content-start gap-3 p-4 md:p-5">
          <p className="m-0 font-mono text-[0.65rem] font-semibold uppercase tracking-[0.06em] text-[var(--ds-color-brand-text)]">
            Recommended next step
          </p>
          <p className="m-0 max-w-4xl text-sm leading-6 text-[var(--ds-color-ink)]">
            {recommendation}
          </p>
          {aiResult?.concerns?.[0] ? (
            <p className="m-0 border-l-2 border-[var(--ds-color-warning)] pl-3 text-xs leading-5 text-[var(--ds-color-ink-soft)]">
              <strong className="text-[var(--ds-color-warning)]">
                Primary concern:
              </strong>{' '}
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
        <dl className="m-0 grid gap-px border-t border-[var(--ds-color-line)] bg-[var(--ds-color-line)] sm:grid-cols-3 lg:grid-cols-1 lg:border-t-0 lg:border-l">
          <DecisionBoundary
            label="Objective system"
            value={systemStatus}
            caption="Deterministic gates and execution outcome."
          />
          <DecisionBoundary
            label="Advisory AI"
            value={advisoryStatus}
            caption="Qualitative guidance; never overrides the system."
          />
          <DecisionBoundary
            label="Effective harness"
            value={effectiveStatus}
            caption="Canonical final status retained in the report."
          />
        </dl>
      </div>

      <dl className="mt-4 grid grid-cols-2 gap-px overflow-hidden rounded-lg border border-[var(--ds-color-line)] bg-[var(--ds-color-line)] lg:grid-cols-4">
        {[
          ['Assessments', summary?.assessment_count],
          ['Passed', summary?.assessment_outcomes?.passed],
          ['Partial', summary?.assessment_outcomes?.partial],
          ['Evidence references', summary?.evidence_reference_count],
        ].map(([label, value]) => (
          <div
            key={String(label)}
            className="bg-[var(--ds-color-surface-raised)] p-3"
          >
            <dt className="font-mono text-[0.6rem] uppercase tracking-[0.06em] text-[var(--ds-color-ink-muted)]">
              {label}
            </dt>
            <dd className="mt-1 mb-0 text-xl font-semibold text-[var(--ds-color-ink)]">
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
        context="Benchmark results"
        title="Scenario results"
        summary="Compare objective outcomes, advisory conclusions, runtime, and scenario structure. Expand one row to inspect its benchmark metrics and workflow."
        className="[&_h2]:text-[clamp(1.5rem,2.4vw,2.2rem)]"
        actions={
          <span className="font-mono text-xs text-[var(--ds-color-ink-muted)]">
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

function TechnicalSection({
  detail,
  presentation,
}: {
  detail: DashboardExecutionDetail
  presentation: ExecutionPresentation
}) {
  const technical = {
    id: detail.id,
    run_id: detail.run_id,
    attempt: detail.attempt,
    status: detail.status,
    source: detail.source,
    release: detail.release,
    event: detail.event,
    actor: detail.actor,
    started_at: presentation.startedAt,
    completed_at: presentation.completedAt,
    availability: detail.availability,
  }
  return (
    <Panel
      as="section"
      id="technical"
      padding="generous"
      className={`${detailPanel} bg-[var(--ds-color-surface-raised)] p-5 md:p-6`}
      aria-labelledby="technical-heading"
    >
      <PageHeader
        headingLevel={2}
        context="Provenance"
        title="Raw fields and immutable identity"
        summary="Internal identifiers remain available for reproducibility without carrying the first-read experience."
        className="[&_h2]:text-[clamp(1.5rem,2.4vw,2.2rem)]"
      />
      <div className="mt-5 grid gap-2 sm:grid-cols-2">
        {Object.entries(technical).map(([key, value]) => (
          <div
            key={key}
            className="rounded-lg border border-[var(--ds-color-line)] bg-[var(--ds-color-surface)] p-3"
          >
            <small className="font-mono text-[0.6rem] font-semibold uppercase tracking-[0.06em] text-[var(--ds-color-ink-muted)]">
              {key.replaceAll('_', ' ')}
            </small>
            <code className="mt-2 block break-all font-mono text-xs text-[var(--ds-color-ink-soft)]">
              {typeof value === 'object'
                ? JSON.stringify(value)
                : String(value ?? 'Not reported')}
            </code>
          </div>
        ))}
      </div>
      <details className="mt-5 rounded-lg border border-[var(--ds-color-line)] bg-[var(--ds-color-surface)]">
        <summary className="min-h-11 cursor-pointer px-4 py-3 text-sm font-semibold text-[var(--ds-color-ink)]">
          Preview raw JSON
        </summary>
        <pre className="max-h-[560px] overflow-auto border-t border-[var(--ds-color-line)] p-4 font-mono text-xs text-[var(--ds-color-ink-muted)]">
          {JSON.stringify(detail, null, 2)}
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
  const [section, setSection] = useState<DetailSection>(
    sectionFromAnchor(anchor),
  )

  useEffect(() => {
    setSection(sectionFromAnchor(anchor))
  }, [anchor])

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

  useEffect(() => {
    if (anchor || !presentation) return
    const initialSection: DetailSection =
      presentation.attention === 'needs_attention' ? 'results' : 'summary'
    setSection(initialSection)
    if (initialSection === 'results')
      window.requestAnimationFrame(() =>
        document
          .getElementById(initialSection)
          ?.scrollIntoView({ block: 'start' }),
      )
  }, [anchor, presentation])

  if (error)
    return (
      <div className="ds-root min-h-dvh bg-[var(--ds-color-canvas)] text-[var(--ds-color-ink)]">
        <a className="skip-link" href={hashForWorkspace()}>
          Back to executions
        </a>
        <main
          id="main"
          className="mx-auto grid min-h-dvh w-[min(52rem,calc(100%_-_1.5rem))] place-items-center py-8"
        >
          <Panel
            role="alert"
            padding="generous"
            className="w-full rounded-[var(--ds-radius-md)] border-[var(--ds-color-danger)]"
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
      <div className="ds-root min-h-dvh bg-[var(--ds-color-canvas)] text-[var(--ds-color-ink)]">
        <main
          id="main"
          className="mx-auto grid min-h-dvh w-[min(52rem,calc(100%_-_1.5rem))] place-items-center py-8"
        >
          <Panel
            className="w-full rounded-[var(--ds-radius-md)]"
            aria-busy="true"
          >
            <p className="m-0 font-mono text-xs uppercase tracking-[0.06em] text-[var(--ds-color-ink-muted)]">
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
  return (
    <div className="ds-root min-h-dvh bg-[var(--ds-color-canvas)] text-[var(--ds-color-ink)]">
      <a className="skip-link" href={hashForExecution(executionId, section)}>
        Skip to execution details
      </a>
      <AppHeader active="executions" />
      <main
        id="main"
        className="page-shell detail-shell w-[min(1380px,calc(100%_-_3rem))] pt-6 max-[640px]:w-[calc(100%_-_1.5rem)]"
      >
        <nav
          className="breadcrumbs mb-5 flex min-w-0 items-center gap-2 overflow-hidden font-mono text-[0.64rem] text-[var(--ds-color-ink-muted)]"
          aria-label="Breadcrumb"
        >
          <a href={hashForWorkspace()}>Overview</a>
          <span aria-hidden="true">/</span>
          <a href={hashForWorkspace('executions')}>Executions</a>
          <span aria-hidden="true">/</span>
          <span className="truncate">{presentation.label}</span>
        </nav>
        <PageHeader
          id="detail-summary"
          context="Execution detail"
          title={presentation.label}
          summary={`${presentation.receivedReports ?? '—'} of ${presentation.expectedReports ?? '—'} expected reports received. Objective outcomes remain authoritative; advisory AI is shown separately.`}
          className="border-b border-[var(--ds-color-line)] pb-6 [&_.ds-page-header-copy]:gap-2 [&_h1]:max-w-full [&_h1]:break-words [&_h1]:text-2xl md:[&_h1]:text-3xl"
          actions={
            <div className="flex flex-wrap justify-end gap-2">
              <StatusBadge status={status.status} label={status.label} />
              {scenarioCount === 1 && aiResult ? (
                <StatusBadge
                  status={statusFromContract(aiResult.verdict)}
                  label={`AI: ${titleCase(aiResult.verdict)}`}
                />
              ) : null}
              <span className="inline-flex min-h-8 items-center rounded-full border border-[var(--ds-color-line-strong)] px-3 font-mono text-[0.62rem] uppercase tracking-[0.04em] text-[var(--ds-color-ink-muted)]">
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
          className="mt-5 overflow-hidden rounded-[var(--ds-radius-md)] border-[var(--ds-color-line-strong)]"
        >
          <section
            className="grid grid-cols-2 gap-px bg-[var(--ds-color-line)] lg:grid-cols-4"
            aria-label="Execution identity"
          >
            <div className="bg-[var(--ds-color-surface-raised)] p-3">
              <div className="font-mono text-[0.6rem] font-semibold uppercase tracking-[0.06em] text-[var(--ds-color-ink-muted)]">
                Scenarios
              </div>
              <strong className="mt-1 block truncate text-sm text-[var(--ds-color-ink)]">
                {scenarioCount
                  ? `${scenarioCount} ${scenarioCount === 1 ? 'result' : 'results'}`
                  : 'Not reported'}
              </strong>
            </div>
            <ModelIdentityCard label="Subject" models={presentation.subjects} />
            <ModelIdentityCard label="Judge" models={presentation.judges} />
            <div className="bg-[var(--ds-color-surface-raised)] p-3">
              <div className="font-mono text-[0.6rem] font-semibold uppercase tracking-[0.06em] text-[var(--ds-color-ink-muted)]">
                Completed
              </div>
              <strong className="mt-1 block text-sm text-[var(--ds-color-ink)]">
                {formatDate(presentation.completedAt)}
              </strong>
            </div>
          </section>
          <section
            className="grid grid-flow-dense grid-cols-1 gap-0 border-t border-[var(--ds-color-line)] sm:grid-cols-2 lg:grid-cols-4"
            aria-label="Execution metrics"
          >
            <MetricCard
              className="min-h-40 rounded-none border-0 border-t border-[var(--ds-color-line)] p-4 first:border-t-0 sm:border-t-0 sm:border-l sm:first:border-l-0 lg:p-5 [&_.ds-metric-value]:text-[clamp(1.9rem,3vw,2.8rem)]"
              label="Objective scenarios"
              value={
                scenarioCount
                  ? `${scenarioSummary?.passed ?? 0}/${scenarioCount}`
                  : '—'
              }
              detail={`${scenarioSummary?.hardGate ?? 0} hard gate · ${scenarioSummary?.failed ?? 0} failed`}
              delta={scenarioAttention > 0 ? 'Needs attention' : 'Passed'}
              tone={scenarioAttention > 0 ? 'negative' : 'positive'}
            />
            <MetricCard
              className="min-h-40 rounded-none border-0 border-t border-[var(--ds-color-line)] p-4 sm:border-t-0 sm:border-l lg:p-5 [&_.ds-metric-value]:text-[clamp(1.9rem,3vw,2.8rem)]"
              label="Report coverage"
              value={formatPercent(presentation.coverage)}
              detail={`${presentation.receivedReports ?? 0} of ${presentation.expectedReports ?? 0} reports received`}
              delta={
                presentation.coverage == null
                  ? 'Not reported'
                  : coverageComplete
                    ? 'Complete'
                    : 'Missing reports'
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
              className="min-h-40 rounded-none border-0 border-t border-[var(--ds-color-line)] p-4 sm:border-l lg:border-t-0 lg:p-5 [&_.ds-metric-value]:text-[clamp(1.9rem,3vw,2.8rem)]"
              label="Execution runtime"
              value={formatDuration(runtimeSeconds)}
              detail={
                presentation.modelRuntimeSeconds != null
                  ? 'Reported wall-clock time'
                  : `${summaryMetrics?.runCount ?? 0} observed runs`
              }
              delta={
                presentation.modelRuntimeSeconds != null
                  ? 'Wall clock'
                  : 'Observed'
              }
              tone={runtimeSeconds === null ? 'unavailable' : 'neutral'}
            />
            <MetricCard
              className="min-h-40 rounded-none border-0 border-t border-[var(--ds-color-line)] p-4 sm:border-l lg:border-t-0 lg:p-5 [&_.ds-metric-value]:text-[clamp(1.9rem,3vw,2.8rem)]"
              label="Workflow scenarios"
              value={
                scenarioCount
                  ? `${workflowScenarioCount}/${scenarioCount}`
                  : '—'
              }
              detail={
                hasRustWorkflow
                  ? `${workflow?.stepCount ?? 0} persisted steps`
                  : 'No workflow steps retained'
              }
              delta={hasRustWorkflow ? 'Composite' : 'Standard'}
              tone={hasRustWorkflow ? 'neutral' : 'unavailable'}
            />
            <MetricCard
              className="min-h-40 rounded-none border-0 border-t border-[var(--ds-color-line)] p-4 lg:p-5 [&_.ds-metric-value]:text-[clamp(1.9rem,3vw,2.8rem)]"
              label="Total tokens"
              value={formatMetricCount(summaryMetrics?.totalTokens ?? null)}
              detail="Consolidated subject execution usage"
              delta="Usage"
              tone={
                summaryMetrics?.totalTokens == null ? 'unavailable' : 'neutral'
              }
            />
            <MetricCard
              className="min-h-40 rounded-none border-0 border-t border-l border-[var(--ds-color-line)] p-4 lg:p-5 [&_.ds-metric-value]:text-[clamp(1.9rem,3vw,2.8rem)]"
              label="Function calls"
              value={formatMetricCount(summaryMetrics?.functionCalls ?? null)}
              detail={
                summaryMetrics?.functionCallErrors == null
                  ? 'Error count not captured'
                  : `${formatMetricCount(summaryMetrics.functionCallErrors)} errors`
              }
              delta="Tools"
              tone={
                summaryMetrics?.functionCalls == null
                  ? 'unavailable'
                  : 'neutral'
              }
            />
            <MetricCard
              className="min-h-40 rounded-none border-0 border-t border-l border-[var(--ds-color-line)] p-4 lg:p-5 [&_.ds-metric-value]:text-[clamp(1.9rem,3vw,2.8rem)]"
              label="Reported cost"
              value={formatReportedCost(summaryMetrics?.totalCostUsd ?? null)}
              detail="Consolidated execution cost"
              delta="USD"
              tone={
                summaryMetrics?.totalCostUsd == null ? 'unavailable' : 'neutral'
              }
            />
          </section>
        </Panel>
        <div className="detail-main grid min-w-0 gap-5">
          <DecisionSection
            presentation={presentation}
            detail={detail}
            primaryRun={primaryRun}
          />
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
      <footer className="mx-auto mt-8 flex w-[min(1380px,calc(100%_-_3rem))] flex-wrap items-center justify-between gap-3 border-t border-[var(--ds-color-line)] py-6 text-xs text-[var(--ds-color-ink-muted)] max-[640px]:w-[calc(100%_-_1.5rem)]">
        <span>Harness E2E · public execution report</span>
        <a
          className="text-[var(--ds-color-ink-soft)] underline-offset-4 hover:underline"
          href={hashForWorkspace('executions')}
        >
          Back to all executions
        </a>
      </footer>
    </div>
  )
}
