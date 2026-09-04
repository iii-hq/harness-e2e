import { ArrowRight, ChevronDown, Link2 } from 'lucide-react'
import {
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from 'react'
import { AssessmentDetailDialog } from '@/components/AssessmentWorkspace'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { requestQuickExecution } from '@/components/ExecutionSetup'
import {
  OutcomeDerivation,
  type OutcomeRow,
} from '@/components/OutcomeDerivation'
import { ScenarioMatrix } from '@/components/ScenarioMatrix'
import { TranscriptDialog } from '@/components/TranscriptDialog'
import {
  buttonClassName,
  EmptyState,
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
} from '@/lib/assessment-view'
import {
  type DashboardDataBridge,
  type DashboardExecutionDetail,
  type DashboardExecutionSummary,
  type ExecutionTotals,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import {
  type ExecutionVerdict,
  executionVerdict,
} from '@/lib/execution-verdict'
import {
  buildExecutionPresentation,
  type ExecutionPresentation,
  formatDate,
  formatDuration,
  formatPercent,
} from '@/lib/execution-view'
import { executionTitle } from '@/lib/overview-signal'
import {
  buildScenarioMatrix,
  type ScenarioMatrixSummary,
} from '@/lib/scenario-matrix'
import {
  type GeneralRunMetrics,
  summedGeneralRunMetricsFromDetail,
  type WorkflowMetricsSummary,
  workflowMetricsFromDetail,
} from '@/lib/workflow-metrics'
import '@/design-system/styles.css'

type DetailSection = 'summary' | 'results' | 'technical'

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

/* ---------------------------------------------------------------- verdict */

function SectionPanel({
  id,
  title,
  summary,
  actions,
  children,
}: {
  id: string
  title: string
  summary?: string
  actions?: ReactNode
  children: ReactNode
}) {
  return (
    <Panel
      as="section"
      id={id}
      className="scroll-mt-24"
      aria-labelledby={`${id}-heading`}
    >
      <div className="mb-4 flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h2
            className="m-0 text-sm font-semibold text-ink"
            id={`${id}-heading`}
          >
            {title}
          </h2>
          {summary ? (
            <p className="mt-1 mb-0 max-w-[70ch] text-xs leading-5 text-ink-soft">
              {summary}
            </p>
          ) : null}
        </div>
        {actions ? (
          <div className="flex flex-wrap items-center gap-2">{actions}</div>
        ) : null}
      </div>
      {children}
    </Panel>
  )
}

/** Audit ED-03 / ED-05: one verdict for the execution, stated once. */
export function DecisionSection({
  presentation,
  detail,
  verdict,
  primaryRun,
  metrics,
  scenarioSummary,
}: {
  presentation: ExecutionPresentation
  detail: DashboardExecutionDetail
  verdict: ExecutionVerdict
  primaryRun: AssessmentRunView | null
  metrics: SummaryExecutionMetrics | null
  scenarioSummary: ScenarioMatrixSummary | null
}) {
  const systemStatus = primaryRun?.systemStatus ?? presentation.attention
  const advisoryStatus =
    primaryRun?.finalAssessment.result?.verdict ??
    primaryRun?.finalAssessment.availability ??
    'unavailable'
  const effectiveStatus = primaryRun?.effectiveStatus ?? systemStatus
  const boundaries: OutcomeRow[] = [
    { role: 'system', value: systemStatus },
    { role: 'advisory', value: advisoryStatus },
  ]
  // Audit ED-05: the effective status only appears when it differs.
  if (effectiveStatus !== systemStatus)
    boundaries.push({ role: 'effective', value: effectiveStatus })
  const summary = detail.assessment_summary
  const coverageComplete =
    presentation.coverage != null &&
    (presentation.coverage === 1 || presentation.coverage >= 100)
  const scenarioCount = scenarioSummary?.total ?? 0
  return (
    <SectionPanel
      id="summary"
      title="decision"
      summary={verdict.headline}
      actions={
        <a
          className={buttonClassName({
            variant: 'secondary',
            size: 'compact',
            className: 'no-underline',
          })}
          href={hashForExecution(detail.id, 'results')}
        >
          inspect retained evidence
          <ArrowRight size={13} aria-hidden="true" />
        </a>
      }
    >
      <OutcomeDerivation rows={boundaries} />
      {/* Audit ED-22: the narrative used to stop at 80ch and leave the right
          half of a 1400px panel empty. Two columns use the width instead. */}
      <div className="mt-5 grid gap-x-8 gap-y-4 @[900px]:grid-cols-[minmax(0,1.35fr)_minmax(0,1fr)]">
        {verdict.diagnosis ? (
          <div className="grid content-start gap-1">
            <span className="ds-label">what happened</span>
            <p className="m-0 text-sm leading-6 text-pretty text-ink">
              {verdict.diagnosis}
            </p>
          </div>
        ) : null}
        <div className="grid content-start gap-1">
          <span className="ds-label">next step</span>
          <p className="m-0 text-sm leading-6 text-pretty text-ink-soft">
            {verdict.nextStep}
          </p>
        </div>
      </div>
      {/* Audit ED-18: one column below 640px, no delta wrapping into a
          second line beside the label. */}
      <div className="mt-5 grid gap-3 @[560px]:grid-cols-2 @[960px]:grid-cols-4">
        <MetricCard
          label="scenarios"
          value={
            scenarioCount
              ? `${scenarioSummary?.passed ?? 0}/${scenarioCount}`
              : '—'
          }
          detail={
            scenarioCount
              ? `${scenarioSummary?.hardGate ?? 0} hard gate · ${scenarioSummary?.failed ?? 0} failed`
              : 'no scenario report retained'
          }
          tone={
            scenarioCount === 0
              ? 'unavailable'
              : (scenarioSummary?.passed ?? 0) === scenarioCount
                ? 'positive'
                : 'negative'
          }
        />
        <MetricCard
          label="coverage"
          value={formatPercent(presentation.coverage)}
          detail={`${presentation.receivedReports ?? 0} of ${presentation.expectedReports ?? 0} reports`}
          tone={
            presentation.coverage == null
              ? 'unavailable'
              : coverageComplete
                ? 'positive'
                : 'warning'
          }
        />
        <MetricCard
          label="assessments"
          value={
            typeof summary?.assessment_count === 'number'
              ? summary.assessment_count.toLocaleString('en-US')
              : '—'
          }
          // Audit ED-23: "0" beside "N evidence references" read as a
          // contradiction; say the count means nothing was retained.
          detail={[
            summary?.assessment_count ? null : 'none retained',
            typeof summary?.evidence_reference_count === 'number'
              ? `${summary.evidence_reference_count} evidence reference${summary.evidence_reference_count === 1 ? '' : 's'}`
              : 'no evidence references retained',
          ]
            .filter(Boolean)
            .join(' · ')}
          tone={summary?.assessment_count ? 'neutral' : 'unavailable'}
        />
        <MetricCard
          label="reported cost"
          value={
            metrics?.totalCostUsd == null
              ? '—'
              : formatReportedCost(metrics.totalCostUsd)
          }
          detail={
            metrics?.totalCostUsd == null
              ? 'not captured for this run'
              : 'consolidated execution cost'
          }
          tone={metrics?.totalCostUsd == null ? 'unavailable' : 'neutral'}
        />
      </div>
    </SectionPanel>
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
    <SectionPanel
      id="results"
      title="scenario results"
      actions={
        <span className="font-mono text-label text-ink-muted">
          {scenarioCount} {scenarioCount === 1 ? 'scenario' : 'scenarios'} ·{' '}
          {runCount} {runCount === 1 ? 'run' : 'runs'}
        </span>
      }
    >
      <ScenarioMatrix detail={detail} onTranscript={onTranscript} />
    </SectionPanel>
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
  const [copied, setCopied] = useState(false)
  const line = entries.map(([key, value]) => `${key} ${value}`).join(' · ')
  // Audit ED-10 / ED-05: provenance is one collapsed line until asked for.
  return (
    <Panel as="section" id="technical" className="scroll-mt-24">
      <details data-provenance>
        <summary className="flex min-h-9 cursor-pointer list-none items-center gap-3 text-xs marker:hidden">
          <ChevronDown
            className="size-4 shrink-0 -rotate-90 text-ink-muted transition-transform duration-[var(--ds-duration-fast)] group-open:rotate-0 motion-reduce:transition-none"
            aria-hidden="true"
          />
          <span className="font-semibold text-ink">
            raw fields and immutable identity
          </span>
          <span className="ms-auto hidden truncate font-mono text-label text-ink-muted @[720px]:block">
            {line.slice(0, 120)}
          </span>
        </summary>
        <dl className="m-0 mt-4 grid grid-cols-[max-content_minmax(0,1fr)] gap-x-6 gap-y-2 font-mono text-xs">
          {entries.map(([key, value]) => (
            <div key={key} className="contents">
              <dt className="ds-label">{key}</dt>
              <dd className="m-0 break-all text-ink">{value}</dd>
            </div>
          ))}
        </dl>
        <div className="mt-4 flex flex-wrap gap-2">
          <button
            type="button"
            className={buttonClassName({
              variant: 'secondary',
              size: 'compact',
            })}
            onClick={() => {
              void navigator.clipboard?.writeText(raw).then(() => {
                setCopied(true)
                window.setTimeout(() => setCopied(false), 1500)
              })
            }}
          >
            {copied ? 'copied' : 'copy json'}
          </button>
        </div>
        <pre className="mt-3 mb-0 max-h-[480px] overflow-auto rounded-[6px] bg-canvas p-4 font-mono text-xs leading-5 text-ink-soft">
          {raw}
        </pre>
      </details>
    </Panel>
  )
}

/** Audit ED-07: the anchors the router already accepts get a visible bar. */
const SECTIONS: Array<{ id: DetailSection; label: string }> = [
  { id: 'summary', label: 'decision' },
  { id: 'results', label: 'results' },
  { id: 'technical', label: 'provenance' },
]

function SectionBar({
  active,
  executionId,
}: {
  active: DetailSection
  executionId: string
}) {
  return (
    <nav
      className="sticky top-12 z-10 -mx-1 flex flex-wrap gap-1 bg-canvas px-1 py-2"
      aria-label="Execution sections"
      data-section-bar
    >
      {SECTIONS.map((section) => (
        <a
          key={section.id}
          className={buttonClassName({
            variant: active === section.id ? 'secondary' : 'quiet',
            size: 'compact',
            className: 'no-underline',
          })}
          href={hashForExecution(executionId, section.id)}
          aria-current={active === section.id ? 'true' : undefined}
        >
          {section.label}
        </a>
      ))}
    </nav>
  )
}

/** Audit ED-11 / ED-12: running and cancelled are their own screen. */
function LiveState({
  presentation,
  status,
  onCancel,
  cancelling,
}: {
  presentation: ExecutionPresentation
  status: { status: OperationalStatus; label: string }
  onCancel?: () => void
  cancelling: boolean
}) {
  const running =
    presentation.attention === 'running' ||
    presentation.attention === 'cancelling'
  const scope =
    presentation.expectedReports !== null &&
    presentation.receivedReports !== null
      ? `${presentation.receivedReports} of ${presentation.expectedReports} scenarios`
      : null
  const started = presentation.startedAt
    ? Date.parse(presentation.startedAt)
    : Number.NaN
  // Elapsed only means something while the run is live; a finished one
  // reports the time it took, not the time since it started.
  const elapsed =
    running && Number.isFinite(started)
      ? formatDuration((Date.now() - started) / 1000)
      : null
  return (
    <Panel className="mt-5" data-live-state={presentation.attention}>
      <div className="flex flex-wrap items-center gap-3">
        <StatusBadge
          status={status.status}
          label={status.label.toLowerCase()}
        />
        <span className="font-mono text-xs text-ink-soft">
          {[scope, elapsed ? `${elapsed} elapsed` : null]
            .filter(Boolean)
            .join(' · ') || 'no progress reported yet'}
        </span>
        {running && onCancel ? (
          <button
            className={buttonClassName({
              variant: 'secondary',
              size: 'compact',
              className: 'ms-auto',
            })}
            type="button"
            onClick={onCancel}
            disabled={cancelling}
          >
            {cancelling ? 'cancelling…' : 'cancel execution'}
          </button>
        ) : null}
      </div>
      <p className="mt-3 mb-0 max-w-[70ch] text-xs leading-5 text-ink-soft">
        {running
          ? 'The report, the decision and the scenario results appear here as soon as the run finishes. This page follows the run; no reload is needed.'
          : 'No report was retained, so there is nothing to decide. Re-run the same scope to obtain one.'}
      </p>
    </Panel>
  )
}

export function ExecutionPage({
  executionId,
  anchor,
  runId,
}: {
  executionId: string
  anchor?: string | null
  /** Evidence record open on top of the execution (audit AW-09). */
  runId?: string | null
}) {
  const [summary, setSummary] = useState<DashboardExecutionSummary | null>(null)
  const [detail, setDetail] = useState<DashboardExecutionDetail | null>(null)
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)
  const [cancelling, setCancelling] = useState(false)
  const [transcript, setTranscript] = useState<{
    run: AssessmentRunView
    title: string
  } | null>(null)
  const section = sectionFromAnchor(anchor)

  useEffect(() => {
    if (!anchor || !detail) return
    window.requestAnimationFrame(() =>
      document
        .getElementById(sectionFromAnchor(anchor))
        ?.scrollIntoView({ block: 'start' }),
    )
  }, [anchor, detail])

  const load = useCallback(async () => {
    try {
      const nextBridge = await getDashboardDataBridge()
      setBridge(nextBridge)
      const manifest = await nextBridge.listExecutions({
        ids: [executionId],
        limit: 1,
      })
      const nextSummary = manifest.executions[0] ?? null
      const nextDetail = await nextBridge.getExecution(executionId)
      setSummary(nextSummary ?? summaryFromDetail(nextDetail))
      setDetail(nextDetail)
      return nextBridge
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
      return null
    }
  }, [executionId])

  useEffect(() => {
    let active = true
    setError(null)
    setDetail(null)
    void (async () => {
      const nextBridge = await load()
      if (!active || !nextBridge) return
    })()
    return () => {
      active = false
    }
  }, [load])

  const presentation = useMemo(
    () => (summary ? buildExecutionPresentation(summary) : null),
    [summary],
  )
  const live =
    presentation?.attention === 'running' ||
    presentation?.attention === 'cancelling'

  // Audit ED-12: a live execution follows the run instead of waiting for F5.
  useEffect(() => {
    if (!bridge || !live) return
    let cancelled = false
    let dispose: (() => void) | undefined
    let timer: number | undefined
    bridge
      .subscribeRunChanges(() => {
        if (timer) window.clearTimeout(timer)
        timer = window.setTimeout(() => void load(), 400)
      })
      .then((off) => {
        if (cancelled) off()
        else dispose = off
      })
      .catch(() => undefined)
    return () => {
      cancelled = true
      if (timer) window.clearTimeout(timer)
      dispose?.()
    }
  }, [bridge, live, load])

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
      <div className="ds-root min-h-dvh bg-canvas text-ink">
        <DashboardPageActions active="executions" />
        <div className="page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-16 md:w-[calc(100%_-_3rem)]">
          <EmptyState
            tone="error"
            title={
              /not found|unknown|no such|invalid execution|404/i.test(error)
                ? 'Execution not found'
                : 'Execution could not be loaded'
            }
            description={error}
            actions={
              <>
                <button
                  className={buttonClassName({ variant: 'secondary' })}
                  type="button"
                  onClick={() => {
                    setError(null)
                    void load()
                  }}
                >
                  retry
                </button>
                <a
                  className={buttonClassName({
                    variant: 'quiet',
                    className: 'no-underline',
                  })}
                  href={hashForWorkspace('executions')}
                >
                  back to executions
                </a>
              </>
            }
          />
        </div>
      </div>
    )

  // Audit ED-22: the skeleton keeps the chrome, so nothing jumps on arrival.
  if (!detail || !presentation)
    return (
      <div className="ds-root min-h-dvh bg-canvas text-ink">
        <DashboardPageActions active="executions" />
        <div
          className="page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-16 md:w-[calc(100%_-_3rem)]"
          aria-busy="true"
          role="status"
        >
          <span className="ds-visually-hidden">Loading execution report</span>
          <div className="grid gap-4">
            <div className="h-12 w-72 animate-pulse rounded-[6px] bg-[var(--surface-fill)] motion-reduce:animate-none" />
            {['decision', 'results', 'provenance'].map((placeholder) => (
              <div
                key={placeholder}
                className="h-40 animate-pulse rounded-[6px] bg-[var(--surface-fill)] motion-reduce:animate-none"
              />
            ))}
          </div>
        </div>
      </div>
    )

  const primaryRun = assessmentModel.runs[0] ?? null
  const evidenceRun = runId
    ? (assessmentModel.runs.find((run) => run.runId === runId) ?? null)
    : null
  const status = executionStatus(presentation)
  const scenarioSummary = scenarioMatrix?.summary ?? null
  const verdict = executionVerdict(
    presentation,
    scenarioSummary,
    scenarioMatrix?.items ?? [],
    primaryRun,
  )
  const runtimeSeconds =
    presentation.modelRuntimeSeconds ?? summaryMetrics?.durationSeconds ?? null
  const noRun = !presentation.available || (scenarioSummary?.total ?? 0) === 0
  const { title } = executionTitle(presentation)
  const identity: Array<[string, string]> = [
    [
      'subject',
      presentation.subjects
        .map((model) => `${model.provider}/${model.model}`)
        .join(', ') || 'not reported',
    ],
    [
      'judge',
      presentation.judges.map((model) => model.model).join(', ') || 'automatic',
    ],
    [
      'started',
      presentation.startedAt ? formatDate(presentation.startedAt) : '—',
    ],
    ['runtime', runtimeSeconds === null ? '—' : formatDuration(runtimeSeconds)],
    [
      'tokens',
      summaryMetrics?.totalTokens == null
        ? '—'
        : formatMetricCount(summaryMetrics.totalTokens),
    ],
    [
      'trigger',
      [detail.event, detail.actor].filter(Boolean).join(' · ') ||
        'not reported',
    ],
    ['id', `${detail.id.slice(0, 8)}…${detail.id.slice(-6)}`],
  ]
  const local = bridge?.mode === 'local'
  const cancelRun = async () => {
    if (bridge?.mode !== 'local') return
    setCancelling(true)
    try {
      await bridge.cancelRun()
      await load()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setCancelling(false)
    }
  }

  return (
    <div className="ds-root min-h-dvh bg-canvas text-ink">
      <DashboardPageActions active="executions" context={title} />
      <div className="page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-16 md:w-[calc(100%_-_3rem)]">
        {/* Audit ED-13 / ED-23: the title is the execution, the trail is flat. */}
        <PageHeader
          title={title}
          summary={verdict.headline}
          headingId="execution-title"
          breadcrumb={[
            { label: 'executions', href: hashForWorkspace('executions') },
            { label: title },
          ]}
          actions={
            <>
              <StatusBadge
                status={status.status}
                label={status.label.toLowerCase()}
              />
              {/* Audit ED-14: the detail keeps its own actions. */}
              <button
                className={buttonClassName({
                  variant: 'quiet',
                  size: 'compact',
                })}
                type="button"
                onClick={() => {
                  void navigator.clipboard
                    ?.writeText(window.location.href)
                    .then(() => {
                      setCopied(true)
                      window.setTimeout(() => setCopied(false), 1500)
                    })
                }}
              >
                <Link2 size={13} aria-hidden="true" />
                {copied ? 'link copied' : 'copy link'}
              </button>
              {local ? (
                <a
                  className={buttonClassName({
                    variant: 'secondary',
                    size: 'compact',
                    className: 'no-underline',
                  })}
                  href={hashForWorkspace()}
                  onClick={() =>
                    requestQuickExecution(
                      scenarioMatrix?.items.map((item) => item.scenarioId) ??
                        [],
                    )
                  }
                >
                  re-run same scope
                </a>
              ) : null}
            </>
          }
        />

        {/* Audit ED-05: identity is one band of facts, not four cards. */}
        <dl
          className="mt-4 flex flex-wrap gap-x-6 gap-y-2 font-mono text-xs"
          data-identity-band
        >
          {identity.map(([label, value]) => (
            <div className="flex min-w-0 items-baseline gap-2" key={label}>
              <dt className="ds-label">{label}</dt>
              <dd className="m-0 min-w-0 break-words text-ink">{value}</dd>
            </div>
          ))}
        </dl>

        {noRun ? (
          <LiveState
            presentation={presentation}
            status={status}
            cancelling={cancelling}
            onCancel={local ? () => void cancelRun() : undefined}
          />
        ) : (
          <>
            <SectionBar active={section} executionId={detail.id} />
            <div className="grid min-w-0 gap-5">
              <DecisionSection
                presentation={presentation}
                detail={detail}
                verdict={verdict}
                primaryRun={primaryRun}
                metrics={summaryMetrics}
                scenarioSummary={scenarioSummary}
              />
              <ResultsSection
                detail={detail}
                onTranscript={(run, title) => setTranscript({ run, title })}
              />
              <TechnicalSection detail={detail} presentation={presentation} />
            </div>
          </>
        )}
      </div>
      {/* Audit AW-09: the evidence record is a route, so back returns here. */}
      {evidenceRun ? (
        <AssessmentDetailDialog
          run={evidenceRun}
          detail={detail}
          onClose={() => {
            window.location.hash = hashForExecution(detail.id, 'results')
          }}
        />
      ) : null}
      {transcript && (
        <TranscriptDialog
          title={transcript.title}
          messages={transcript.run.transcript?.messages}
          open
          onClose={() => setTranscript(null)}
        />
      )}
    </div>
  )
}
