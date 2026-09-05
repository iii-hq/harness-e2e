import { ArrowRight, Link2 } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState } from 'react'
import { AssessmentDetailDialog } from '@/components/AssessmentWorkspace'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { DisclosureLayer } from '@/components/DisclosureLayer'
import { ExecutionMetricsPanel } from '@/components/ExecutionMetricsPanel'
import { ExecutionOverview } from '@/components/ExecutionOverview'
import { requestQuickExecution } from '@/components/ExecutionSetup'
import { LiveProgressPanel } from '@/components/LiveProgressPanel'
import type { OutcomeRow } from '@/components/OutcomeDerivation'
import {
  contractScent,
  ResultContractStrip,
  ScenarioMatrix,
} from '@/components/ScenarioMatrix'
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
import { useLatestRequest } from '@/hooks/use-latest-request'
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
import { buildExecutionMetrics } from '@/lib/execution-metrics'
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
  unsupportedExecutionReason,
} from '@/lib/execution-view'
import { executionTitle } from '@/lib/overview-signal'
import {
  buildScenarioMatrix,
  formatScenarioDuration,
  type ScenarioMatrixItem,
  type ScenarioMatrixSummary,
} from '@/lib/scenario-matrix'
import { watchExecution } from '@/lib/watch-execution'
import {
  type GeneralRunMetrics,
  summedGeneralRunMetricsFromDetail,
  type WorkflowMetricsSummary,
  workflowMetricsFromDetail,
} from '@/lib/workflow-metrics'
import '@/design-system/styles.css'

type DetailSection = 'summary' | 'metrics' | 'results' | 'technical'

function sectionFromAnchor(anchor: string | null | undefined): DetailSection {
  if (anchor === 'metrics') return 'metrics'
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
  if (presentation.attention === 'unsupported')
    return { status: 'unavailable', label: 'Unsupported' }
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

/* ---------------------------------------------------------------- layers */

/** The two inputs the result contract combines and, only when it differs,
 *  the status it publishes (audit ED-05). Read by the overview's derivation. */
export function executionBoundaries(
  presentation: ExecutionPresentation,
  primaryRun: AssessmentRunView | null,
): OutcomeRow[] {
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
  if (effectiveStatus !== systemStatus)
    boundaries.push({ role: 'effective', value: effectiveStatus })
  return boundaries
}

function firstSentence(value: string | null | undefined): string | null {
  if (!value) return null
  const match = value.match(/^.*?[.!?](?=\s|$)/)
  return (match ? match[0] : value).replace(/\.$/, '')
}

/** Audit ED-26: every closed layer carries a scent — enough of its content to
 *  decide whether to open it. The narrative's is what happened, then what to do. */
export function narrativeScent(verdict: ExecutionVerdict): string {
  return [firstSentence(verdict.diagnosis), firstSentence(verdict.nextStep)]
    .filter(Boolean)
    .join(' · ')
}

/** One line per scenario: name, objective verdict, advisory when it adds
 *  something, runtime. The table behind it has the same order. */
export function resultsScent(items: ScenarioMatrixItem[]): string {
  if (items.length === 0) return 'no scenario report retained'
  return items
    .map((item) => {
      const name = `${item.scenarioId.replace(/_/g, ' ')}${
        item.scenarioVersion == null ? '' : ` v${item.scenarioVersion}`
      }`
      const advisory =
        item.advisory.status === 'passed' ||
        item.advisory.status === 'unavailable'
          ? ''
          : `, ${item.advisory.label.toLowerCase()}`
      const runtime =
        item.durationMs == null
          ? ''
          : ` · ${formatScenarioDuration(item.durationMs)}`
      return `${name} ${item.objective.label.toLowerCase()}${advisory}${runtime}`
    })
    .join(' \u00a0·\u00a0 ')
}

/** The counts the layer opens onto, with the four that are usually zero
 *  folded into one clause when they all are. */
export function countsScent(detail: DashboardExecutionDetail): string {
  const metrics = buildExecutionMetrics(detail)
  const summary = detail.assessment_summary
  const evidence =
    typeof summary?.evidence_reference_count === 'number'
      ? `${summary.evidence_reference_count} evidence reference${summary.evidence_reference_count === 1 ? '' : 's'}`
      : 'no evidence references retained'
  const assessments = `assessments ${
    summary?.assessment_count
      ? summary.assessment_count.toLocaleString('en-US')
      : 'none retained'
  }, ${evidence}`
  if (metrics.includedScenarios === 0)
    return `no compatible run evidence to consolidate · ${assessments}`
  const quiet =
    metrics.incomplete +
      metrics.undetermined +
      metrics.deferred +
      metrics.technicalInvalid ===
    0
  return [
    `planned ${metrics.planned}`,
    `recorded ${metrics.observed}`,
    `completed ${metrics.completed}`,
    quiet
      ? 'no incomplete, undetermined, deferred or technically invalid runs'
      : `${metrics.incomplete} incomplete · ${metrics.undetermined} undetermined · ${metrics.deferred} deferred · ${metrics.technicalInvalid} technically invalid`,
    assessments,
  ].join(' · ')
}

/** Audit ED-03 / ED-05: one verdict for the execution, stated once. The
 *  derivation leads the overview; this layer carries the words. */
export function NarrativeSection({ verdict }: { verdict: ExecutionVerdict }) {
  // Audit ED-22: two columns use the width instead of stopping at 80ch.
  return (
    <div
      className="grid gap-x-8 gap-y-4 @[900px]:grid-cols-[minmax(0,1.35fr)_minmax(0,1fr)]"
      data-narrative
    >
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
  )
}

/** The counts-and-coverage layer: report coverage and retained assessments
 *  first, then the pooled counts and consumption table, headless. */
export function CountsSection({
  presentation,
  detail,
  metrics,
  scenarioSummary,
}: {
  presentation: ExecutionPresentation
  detail: DashboardExecutionDetail
  metrics: SummaryExecutionMetrics | null
  scenarioSummary: ScenarioMatrixSummary | null
}) {
  const summary = detail.assessment_summary
  const coverageComplete =
    presentation.coverage != null &&
    (presentation.coverage === 1 || presentation.coverage >= 100)
  const scenarioCount = scenarioSummary?.total ?? 0
  return (
    <div className="grid gap-5" data-counts>
      {/* Audit ED-18: one column below 640px, no delta wrapping into a
          second line beside the label. */}
      <div className="grid min-w-0 gap-3 @[560px]:grid-cols-2 @[960px]:grid-cols-4">
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
          label="cost"
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
      <ExecutionMetricsPanel detail={detail} headless />
    </div>
  )
}

function runCountFromDetail(detail: DashboardExecutionDetail) {
  return (detail.reports ?? []).reduce(
    (total, record) =>
      total +
      (record.report?.scenarios ?? []).reduce(
        (scenarioTotal, scenario) =>
          scenarioTotal + (scenario.runs?.length ?? 0),
        0,
      ),
    0,
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
    [
      'slot start deadline',
      detail.slot_start_deadline_seconds == null
        ? null
        : `${detail.slot_start_deadline_seconds}s (soft limit for starting new slots)`,
    ],
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

/** Audit ED-10 / ED-29: the results contract is provenance too, so it opens
 *  here with the raw fields instead of sitting above the scenario table. */
function ProvenanceSection({
  detail,
  presentation,
  contracts,
}: {
  detail: DashboardExecutionDetail
  presentation: ExecutionPresentation
  contracts: ReturnType<typeof buildScenarioMatrix>['contracts']
}) {
  const entries = provenanceEntries(detail, presentation)
  const raw = JSON.stringify(detail, null, 2)
  const [copied, setCopied] = useState(false)
  return (
    <div className="grid gap-4" data-provenance>
      <ResultContractStrip contracts={contracts} />
      <dl className="m-0 grid min-w-0 grid-cols-[max-content_minmax(0,1fr)] gap-x-6 gap-y-2 font-mono text-xs">
        {entries.map(([key, value]) => (
          <div key={key} className="contents">
            <dt className="ds-label">{key}</dt>
            <dd className="m-0 break-all text-ink">{value}</dd>
          </div>
        ))}
      </dl>
      <div className="flex flex-wrap gap-2">
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
      <pre className="m-0 min-w-0 max-h-[480px] overflow-auto rounded-[6px] bg-canvas p-4 font-mono text-xs leading-5 text-ink-soft">
        {raw}
      </pre>
    </div>
  )
}

/** Audit ED-07 / ED-26: the anchors the router accepts stay a visible bar;
 *  each one now opens its layer, so deep links keep working. */
const SECTIONS: Array<{ id: DetailSection; label: string }> = [
  { id: 'summary', label: 'what happened' },
  { id: 'results', label: 'results' },
  { id: 'metrics', label: 'counts' },
  { id: 'technical', label: 'provenance' },
]

function SectionBar({
  active,
  executionId,
  onSelect,
}: {
  active: DetailSection | null
  executionId: string
  /** Re-opens a layer the reader closed when its anchor is already current. */
  onSelect: (section: DetailSection) => void
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
          onClick={() => onSelect(section.id)}
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
  hasProgress,
}: {
  presentation: ExecutionPresentation
  status: { status: OperationalStatus; label: string }
  onCancel?: () => void
  cancelling: boolean
  hasProgress: boolean
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
            .join(' · ') ||
            (presentation.attention === 'unsupported'
              ? 'historical result retained'
              : 'no progress reported yet')}
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
        {presentation.attention === 'unsupported'
          ? unsupportedExecutionReason(presentation.execution)
          : running
            ? 'This page follows recorded progress automatically. The final report and decision appear when the execution finishes.'
            : hasProgress
              ? 'The final report is unavailable. Recorded checkpoints remain visible below as partial evidence, not a final verdict.'
              : 'No report or verified progress is available for this execution.'}
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
  // Audit ED-26: the URL opens its layer; toggles the reader makes afterwards
  // hold until the anchor changes, then the URL wins again. No effect needed.
  const anchorSection = anchor ? sectionFromAnchor(anchor) : null
  const [toggles, setToggles] = useState<{
    anchor: string | null | undefined
    layers: Partial<Record<DetailSection, boolean>>
  }>({ anchor, layers: {} })
  const layers = toggles.anchor === anchor ? toggles.layers : {}
  const layerOpen = (id: DetailSection) => layers[id] ?? anchorSection === id
  const setLayer = (id: DetailSection, open: boolean) =>
    setToggles((current) => ({
      anchor,
      layers: {
        ...(current.anchor === anchor ? current.layers : {}),
        [id]: open,
      },
    }))
  const beginRequest = useLatestRequest()
  const loadedExecutionId = detail?.id

  useEffect(() => {
    if (!anchor || !loadedExecutionId) return
    window.requestAnimationFrame(() =>
      document
        .getElementById(sectionFromAnchor(anchor))
        ?.scrollIntoView({ block: 'start' }),
    )
  }, [anchor, loadedExecutionId])

  const load = useCallback(async () => {
    const request = beginRequest()
    try {
      const nextBridge = await getDashboardDataBridge()
      if (!request.isCurrent()) return null
      setBridge(nextBridge)
      const manifest = await nextBridge.listExecutions({
        ids: [executionId],
        limit: 1,
      })
      const nextSummary = manifest.executions[0] ?? null
      const nextDetail = await nextBridge.getExecution(executionId)
      if (!request.isCurrent()) return null
      setSummary(summaryFromDetail(nextDetail, nextSummary ?? undefined))
      setDetail(nextDetail)
      setError(null)
      return nextBridge
    } catch (cause) {
      if (request.isCurrent()) {
        setError(cause instanceof Error ? cause.message : String(cause))
      }
      return null
    }
  }, [beginRequest, executionId])

  useEffect(() => {
    setError(null)
    setDetail(null)
    setSummary(null)
    void load()
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
    return watchExecution(bridge, executionId, load)
  }, [bridge, executionId, live, load])

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

  if (error && !detail)
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
            {['overview', 'results', 'provenance'].map((placeholder) => (
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
  const boundaries = executionBoundaries(presentation, primaryRun)
  const runCount = runCountFromDetail(detail)
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
    [
      'duration',
      runtimeSeconds === null ? '—' : formatDuration(runtimeSeconds),
    ],
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
          summary={
            detail.live_progress
              ? `${detail.live_progress.runs_committed} of ${detail.live_progress.planned_slots} runs recorded · ${live ? 'results are provisional' : 'partial evidence preserved'}`
              : verdict.headline
          }
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
              {local && presentation.attention !== 'unsupported' ? (
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

        {error ? (
          <p className="mt-4 text-sm text-warning" role="status">
            Refresh failed. Showing the last received snapshot; automatic
            updates will retry. {error}
          </p>
        ) : null}
        {detail.live_progress_error ? (
          <p className="mt-4 text-sm text-warning" role="status">
            {detail.live_progress_error}
          </p>
        ) : null}
        {detail.persistence_errors?.length ? (
          <p className="mt-4 text-sm text-warning" role="status">
            Partial result: completed runs were preserved, but persistence
            failed. {detail.persistence_errors.join(' · ')}
          </p>
        ) : null}
        {noRun || live ? (
          <LiveState
            presentation={presentation}
            status={status}
            cancelling={cancelling}
            hasProgress={Boolean(detail.live_progress)}
            onCancel={local ? () => void cancelRun() : undefined}
          />
        ) : null}
        {detail.live_progress ? (
          <LiveProgressPanel progress={detail.live_progress} running={live} />
        ) : null}
        {!noRun && !live ? (
          <>
            <SectionBar
              active={anchorSection}
              executionId={detail.id}
              onSelect={(id) => setLayer(id, true)}
            />
            {/* Audit ED-26: layer 0 is the grouped metrics; everything else
                is a closed row with a scent until the reader needs it. */}
            <div className="grid min-w-0 gap-3">
              <ExecutionOverview detail={detail} boundaries={boundaries} />
              <DisclosureLayer
                id="summary"
                label="what happened and next step"
                scent={narrativeScent(verdict)}
                open={layerOpen('summary')}
                onToggle={(open) => setLayer('summary', open)}
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
                <NarrativeSection verdict={verdict} />
              </DisclosureLayer>
              <DisclosureLayer
                id="results"
                label="scenario results"
                scent={resultsScent(scenarioMatrix?.items ?? [])}
                open={layerOpen('results')}
                onToggle={(open) => setLayer('results', open)}
                actions={
                  <span className="font-mono text-label font-normal text-ink-muted">
                    {scenarioSummary?.total ?? 0}{' '}
                    {scenarioSummary?.total === 1 ? 'scenario' : 'scenarios'} ·{' '}
                    {runCount} {runCount === 1 ? 'run' : 'runs'}
                  </span>
                }
              >
                <ScenarioMatrix
                  detail={detail}
                  onTranscript={(run, title) => setTranscript({ run, title })}
                  showContract={false}
                />
              </DisclosureLayer>
              <DisclosureLayer
                id="metrics"
                label="counts and coverage"
                scent={countsScent(detail)}
                open={layerOpen('metrics')}
                onToggle={(open) => setLayer('metrics', open)}
              >
                <CountsSection
                  presentation={presentation}
                  detail={detail}
                  metrics={summaryMetrics}
                  scenarioSummary={scenarioSummary}
                />
              </DisclosureLayer>
              <DisclosureLayer
                id="technical"
                label="provenance"
                scent={contractScent(scenarioMatrix?.contracts ?? [])}
                open={layerOpen('technical')}
                onToggle={(open) => setLayer('technical', open)}
              >
                <ProvenanceSection
                  detail={detail}
                  presentation={presentation}
                  contracts={scenarioMatrix?.contracts ?? []}
                />
              </DisclosureLayer>
            </div>
          </>
        ) : null}
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
