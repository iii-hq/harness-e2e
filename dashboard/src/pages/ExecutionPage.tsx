import { useEffect, useMemo, useState } from 'react'
import { AssessmentWorkspace } from '@/components/AssessmentWorkspace'
import { SemanticTestFlow } from '@/components/SemanticTestFlow'
import { ThemeToggle } from '@/components/ThemeToggle'
import { TranscriptDialog } from '@/components/TranscriptDialog'
import { hashForExecution, hashForWorkspace } from '@/hooks/use-hash-route'
import {
  type AssessmentRunMetrics,
  type AssessmentRunView,
  aggregateAssessmentMetrics,
  buildAssessmentWorkspace,
} from '@/lib/assessment-view'
import {
  type DashboardExecutionDetail,
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
  titleCase,
} from '@/lib/execution-view'

type DetailSection = 'summary' | 'results' | 'technical'

const detailPanel =
  'detail-section scroll-mt-[82px] overflow-hidden rounded-[10px] border border-line-strong border-l-[3px] bg-panel px-7 py-[26px] max-[560px]:px-[18px] max-[560px]:py-[22px]'
const detailHeading =
  'm-0 text-[clamp(1.35rem,2vw,1.85rem)] font-[570] tracking-[-0.045em]'

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

function modelNames(presentation: ExecutionPresentation['subjects']) {
  return presentation.length
    ? presentation.map((model) => `${model.provider}/${model.model}`).join(', ')
    : 'Not reported'
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
    <div className="min-w-0 rounded-lg border border-line bg-panel-faint px-3 py-2.5">
      <div className="section-kicker">{label}</div>
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
              <code className="mt-0.5 block break-all text-[0.59rem] text-ink-muted">
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

function StatusPill({ presentation }: { presentation: ExecutionPresentation }) {
  const tone =
    presentation.attention === 'passed'
      ? 'status-pass'
      : presentation.attention === 'needs_attention'
        ? 'status-fail'
        : 'status-incomplete'
  const label =
    presentation.attention === 'passed'
      ? 'Passed'
      : presentation.attention === 'needs_attention'
        ? 'Needs attention'
        : titleCase(presentation.attention)
  return <span className={`status-pill ${tone}`}>{label}</span>
}

function MetricCard({
  label,
  value,
  caption,
}: {
  label: string
  value: string
  caption: string
}) {
  return (
    <article className="kpi-card min-h-0 rounded-none border-0 border-r border-b border-line bg-transparent p-[22px] max-[760px]:border-r-0">
      <div className="kpi-label">{label}</div>
      <div className="kpi-value mt-5 text-[clamp(1.8rem,3vw,2.8rem)]">
        {value}
      </div>
      <div className="kpi-delta">{caption}</div>
    </article>
  )
}

function SummaryMetric({
  label,
  value,
  caption,
}: {
  label: string
  value: string
  caption: string
}) {
  return (
    <div className="rounded-lg border border-line bg-panel-subtle p-3">
      <small className="section-kicker">{label}</small>
      <strong className="mt-2 block text-lg font-semibold text-ink">
        {value}
      </strong>
      <span className="mt-1 block text-xs text-ink-muted">{caption}</span>
    </div>
  )
}

type SummaryExecutionMetrics = {
  totalTokens: number | null
  functionCalls: number | null
  functionErrors: number | null
  durationSeconds: number | null
  runCount: number
}

function finiteMetric(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function buildSummaryExecutionMetrics(
  presentation: ExecutionPresentation,
  aggregate: AssessmentRunMetrics,
  runCount: number,
): SummaryExecutionMetrics {
  const totals = presentation.execution.totals
  const reportedRuns =
    runCount || presentation.receivedReports || presentation.breakdown.total
  return {
    totalTokens: finiteMetric(totals?.total_tokens) ?? aggregate.totalTokens,
    functionCalls:
      finiteMetric(totals?.function_calls) ?? aggregate.functionCalls,
    functionErrors:
      finiteMetric(totals?.function_call_errors) ??
      aggregate.functionCallErrors,
    durationSeconds:
      presentation.workflowRuntimeSeconds ??
      presentation.modelRuntimeSeconds ??
      (aggregate.durationMs === null ? null : aggregate.durationMs / 1000),
    runCount: reportedRuns,
  }
}

function SummarySection({
  presentation,
  metrics,
}: {
  presentation: ExecutionPresentation
  metrics: SummaryExecutionMetrics
}) {
  const issue = presentation.primaryIssue
  return (
    <section
      id="summary"
      className={`${detailPanel} border-l-brand`}
      aria-labelledby="summary-heading"
    >
      <div className="section-kicker mb-2">01 · Summary</div>
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <h2 id="summary-heading" className={detailHeading}>
            {presentation.label}
          </h2>
          <p className="mt-3 max-w-3xl text-sm leading-6 text-ink-muted">
            {presentation.attention === 'passed'
              ? 'All retained scenarios passed their objective system checks.'
              : issue
                ? `${categoryLabel(issue.category)} is the first actionable signal in this execution.`
                : 'The execution is still collecting enough evidence for a definitive result.'}
          </p>
        </div>
        <StatusPill presentation={presentation} />
      </div>
      <div className="mt-6 grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
        <div className="rounded-lg border border-line bg-panel-subtle p-3">
          <small className="section-kicker">Subject</small>
          <strong className="mt-2 block break-words text-sm text-ink">
            {modelNames(presentation.subjects)}
          </strong>
        </div>
        <div className="rounded-lg border border-line bg-panel-subtle p-3">
          <small className="section-kicker">Judge</small>
          <strong className="mt-2 block break-words text-sm text-ink">
            {modelNames(presentation.judges)}
          </strong>
        </div>
        <div className="rounded-lg border border-line bg-panel-subtle p-3">
          <small className="section-kicker">Completed</small>
          <strong className="mt-2 block text-sm text-ink">
            {formatDate(presentation.completedAt)}
          </strong>
        </div>
        <div className="rounded-lg border border-line bg-panel-subtle p-3">
          <small className="section-kicker">Action</small>
          <strong className="mt-2 block text-sm text-ink">
            {issue ? 'Investigate results' : 'Review results'}
          </strong>
        </div>
      </div>
      <div className="mt-5">
        <div className="section-kicker">Execution indicators</div>
        <div className="mt-2 grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
          <SummaryMetric
            label="Total tokens"
            value={
              metrics.totalTokens === null
                ? 'Not reported'
                : Math.round(metrics.totalTokens).toLocaleString('en-US')
            }
            caption={`${metrics.runCount} diagnostic runs`}
          />
          <SummaryMetric
            label="Function calls"
            value={
              metrics.functionCalls === null
                ? 'Not reported'
                : Math.round(metrics.functionCalls).toLocaleString('en-US')
            }
            caption="Subject execution calls"
          />
          <SummaryMetric
            label="Workflow duration"
            value={formatDuration(metrics.durationSeconds)}
            caption="End-to-end execution time"
          />
          <SummaryMetric
            label="Function errors"
            value={
              metrics.functionErrors === null
                ? 'Not reported'
                : Math.round(metrics.functionErrors).toLocaleString('en-US')
            }
            caption="Calls that returned errors"
          />
        </div>
      </div>
      {issue && (
        <div className="mt-5 rounded-lg border border-danger/25 bg-danger/5 p-4">
          <div>
            <strong className="block text-sm text-ink">
              {categoryMessage(issue.category, issue.count)}
            </strong>
            <span className="mt-1 block text-sm text-ink-muted">
              Objective system outcomes remain authoritative over advisory AI
              conclusions.
            </span>
          </div>
        </div>
      )}
    </section>
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
  return (
    <section
      id="results"
      className={`${detailPanel} border-l-brand`}
      aria-labelledby="results-heading"
    >
      <div className="section-kicker mb-2">02 · Results</div>
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h2 id="results-heading" className={detailHeading}>
            Scenarios and assessments
          </h2>
          <p className="mt-2 max-w-2xl text-sm leading-6 text-ink-muted">
            Objective outcomes, advisory interpretations, and run-level evidence
            stay distinct while sharing one diagnostic flow.
          </p>
        </div>
        <span className="coverage-note">{runCount} diagnostic runs</span>
      </div>
      <SemanticTestFlow detail={detail} />
      <div className="mt-6">
        <AssessmentWorkspace detail={detail} onTranscript={onTranscript} />
      </div>
    </section>
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
    <section
      id="technical"
      className={`${detailPanel} border-l-line-strong bg-panel-faint`}
      aria-labelledby="technical-heading"
    >
      <div className="section-kicker mb-2">03 · Technical</div>
      <h2 id="technical-heading" className={detailHeading}>
        Provenance and raw fields
      </h2>
      <p className="mt-2 max-w-2xl text-sm leading-6 text-ink-muted">
        Internal identifiers remain available for reproducibility without
        carrying the first-read experience.
      </p>
      <div className="mt-5 grid gap-2 sm:grid-cols-2">
        {Object.entries(technical).map(([key, value]) => (
          <div key={key} className="rounded-lg border border-line bg-panel p-3">
            <small className="section-kicker">{key.replaceAll('_', ' ')}</small>
            <code className="mt-2 block break-all text-xs text-ink-soft">
              {typeof value === 'object'
                ? JSON.stringify(value)
                : String(value ?? 'Not reported')}
            </code>
          </div>
        ))}
      </div>
      <details className="mt-5 rounded-lg border border-line bg-panel-subtle">
        <summary className="min-h-11 cursor-pointer px-4 py-3 text-sm font-semibold">
          Preview raw JSON
        </summary>
        <pre className="max-h-[560px] overflow-auto border-t border-line p-4 text-xs text-ink-muted">
          {JSON.stringify(detail, null, 2)}
        </pre>
      </details>
    </section>
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
    if (anchor)
      window.requestAnimationFrame(() =>
        document
          .getElementById(sectionFromAnchor(anchor))
          ?.scrollIntoView({ block: 'start' }),
      )
  }, [anchor])

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
          )
        : null,
    [assessmentModel.runs, presentation],
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
      <>
        <a className="skip-link" href={hashForWorkspace()}>
          Back to executions
        </a>
        <main id="main" className="page-shell detail-shell">
          <section className="empty-state" role="alert">
            <div className="empty-icon" aria-hidden="true">
              !
            </div>
            <h1>Execution not found</h1>
            <p>{error}</p>
            <a className="button" href={hashForWorkspace()}>
              Back to executions
            </a>
          </section>
        </main>
      </>
    )
  if (!detail || !presentation)
    return (
      <main id="main" className="page-shell detail-shell">
        <section className="detail-loading" aria-busy="true">
          Loading execution report…
        </section>
      </main>
    )

  const navigation: Array<{ id: DetailSection; label: string }> = [
    { id: 'summary', label: 'Summary' },
    { id: 'results', label: 'Results' },
    { id: 'technical', label: 'Technical' },
  ]
  return (
    <>
      <a className="skip-link" href={hashForExecution(executionId, section)}>
        Skip to execution details
      </a>
      <header className="topbar min-h-[68px]">
        <a
          className="brand"
          href="https://github.com/iii-hq/harness-e2e"
          aria-label="iii Harness E2E"
        >
          <span className="brand-copy">
            <strong>iii</strong>
            <span>Harness benchmarks</span>
          </span>
        </a>
        <nav className="topbar-actions" aria-label="Execution actions">
          <ThemeToggle />
        </nav>
      </header>
      <main
        id="main"
        className="page-shell detail-shell w-[min(1420px,calc(100%-48px))] pt-[30px] max-[840px]:w-[min(1420px,calc(100%-30px))]"
      >
        <nav
          className="breadcrumbs mb-5 items-center font-mono text-[0.64rem]"
          aria-label="Breadcrumb"
        >
          <a href={hashForWorkspace()}>Executions</a>
          <span aria-hidden="true">/</span>
          <span>{presentation.label}</span>
        </nav>
        <section
          id="detail-summary"
          className="execution-summary grid grid-cols-[minmax(0,1.35fr)_minmax(420px,0.65fr)] overflow-hidden rounded-[10px] border border-line-strong border-t-[3px] border-t-brand bg-panel max-[1120px]:grid-cols-1"
          aria-labelledby="execution-heading"
        >
          <div className="execution-summary-main min-w-0 p-[30px] max-[560px]:px-[18px] max-[560px]:py-[22px]">
            <div className="eyebrow mb-3">
              <span className="live-dot" aria-hidden="true" />
              Execution report
            </div>
            <h1
              id="execution-heading"
              className="m-0 max-w-full break-words text-[clamp(2rem,4vw,3.6rem)] leading-[0.98] font-[550]"
            >
              {presentation.label}
            </h1>
            <fieldset className="m-0 mt-4 grid max-w-3xl gap-2 border-0 p-0 sm:grid-cols-2">
              <legend className="sr-only">Execution models</legend>
              <ModelIdentityCard
                label="Subject"
                models={presentation.subjects}
              />
              <ModelIdentityCard label="Judge" models={presentation.judges} />
            </fieldset>
            <div className="mt-5 flex flex-wrap gap-2">
              <StatusPill presentation={presentation} />
              <span className="data-badge">
                {detail.availability === 'full'
                  ? 'Diagnostic detail'
                  : detail.availability === 'aggregate'
                    ? 'Aggregate'
                    : 'No report'}
              </span>
            </div>
          </div>
          <section
            className="kpi-grid detail-kpis m-0 grid grid-cols-2 gap-0 border-l border-line bg-panel-faint max-[1120px]:border-t max-[1120px]:border-l-0 max-[760px]:grid-cols-1"
            aria-label="Execution metrics"
          >
            <MetricCard
              label="Scenario pass rate"
              value={formatPercent(presentation.passRate)}
              caption={`${formatPercent(presentation.coverage)} coverage`}
            />
            <MetricCard
              label="Model runtime"
              value={formatDuration(presentation.modelRuntimeSeconds)}
              caption={
                presentation.workflowRuntimeSeconds !== null
                  ? `${formatDuration(presentation.workflowRuntimeSeconds)} workflow`
                  : 'Workflow duration not reported'
              }
            />
            <MetricCard
              label="Results"
              value={String(presentation.breakdown.total || '—')}
              caption={`${presentation.breakdown.issues} requiring attention`}
            />
            <MetricCard
              label="Evidence"
              value={String(assessmentModel.runs.length)}
              caption="Assessment runs retained"
            />
          </section>
        </section>
        <nav
          className="detail-index hidden sticky top-3 z-20 mb-4 mt-[18px] grid-cols-3 gap-0 overflow-hidden rounded-[9px] border border-line-strong bg-glass p-0 shadow-[0_12px_30px_rgba(0,0,0,0.12)] backdrop-blur-[18px] max-[840px]:static max-[840px]:grid max-[560px]:grid-cols-2"
          aria-label="Execution detail sections"
        >
          {navigation.map((item, index) => (
            <a
              key={item.id}
              href={hashForExecution(executionId, item.id)}
              className={`flex min-h-[52px] items-center gap-2.5 border-r border-line px-4 text-left no-underline ${section === item.id ? 'bg-panel-soft text-ink' : 'text-ink-muted hover:bg-panel-soft hover:text-ink'}`}
              aria-current={section === item.id ? 'page' : undefined}
            >
              <span className="font-mono text-[0.6rem]" aria-hidden="true">
                0{index + 1}
              </span>
              <strong className="text-[0.74rem] font-semibold">
                {item.label}
              </strong>
            </a>
          ))}
        </nav>
        <div className="detail-main grid min-w-0 gap-4">
          <SummarySection
            presentation={presentation}
            metrics={
              summaryMetrics ?? {
                totalTokens: null,
                functionCalls: null,
                functionErrors: null,
                durationSeconds: null,
                runCount: 0,
              }
            }
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
      <footer>
        <span>Harness E2E · public execution report</span>
        <a href={hashForWorkspace()}>Back to all executions</a>
      </footer>
    </>
  )
}
