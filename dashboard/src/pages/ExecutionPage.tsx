import { Link2, RotateCcw, Trash2 } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState } from 'react'
import { AssessmentDetailDialog } from '@/components/AssessmentWorkspace'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { DisclosureLayer } from '@/components/DisclosureLayer'
import { ExecutionMetricsPanel } from '@/components/ExecutionMetricsPanel'
import { requestQuickExecution } from '@/components/ExecutionSetup'
import { LiveProgressPanel } from '@/components/LiveProgressPanel'
import { PlanProgress } from '@/components/PlanStatus'
import { PrimaryMetricsView } from '@/components/PrimaryMetricsView'
import {
  contractScent,
  ResultContractStrip,
  ScenarioMatrix,
} from '@/components/ScenarioMatrix'
import type { SystemOutcome } from '@/components/SystemOutcome'
import { TranscriptDialog } from '@/components/TranscriptDialog'
import {
  buttonClassName,
  Callout,
  Dialog,
  EmptyState,
  MetricCard,
  type OperationalStatus,
  PageHeader,
  Panel,
  StatusBadge,
} from '@/design-system'
import {
  hashForExecution,
  hashForPlan,
  hashForPlans,
  hashForWorkspace,
} from '@/hooks/use-hash-route'
import { useLatestRequest } from '@/hooks/use-latest-request'
import {
  type AssessmentRunView,
  buildAssessmentWorkspace,
} from '@/lib/assessment-view'
import {
  type DashboardDataBridge,
  type DashboardExecutionDetail,
  type DashboardExecutionSummary,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import {
  buildExecutionPresentation,
  type ExecutionPresentation,
  executionTitle,
  formatDate,
  formatDuration,
} from '@/lib/execution-view'
import { planAction } from '@/lib/plan-execution'
import { excludeUnsuccessfulTests } from '@/lib/primary-metrics'
import {
  comparisonPrimaryMetrics,
  filterReferenceScenarios,
  type RcReference,
} from '@/lib/release-control-reference'
import { buildScenarioMatrix } from '@/lib/scenario-matrix'
import { watchExecution } from '@/lib/watch-execution'
import '@/design-system/styles.css'

type DetailSection = 'metrics' | 'results' | 'technical'

function sectionFromAnchor(anchor: string | null | undefined): DetailSection {
  if (anchor === 'metrics') return 'metrics'
  if (anchor === 'evidence' || anchor === 'raw-data') return 'technical'
  if (anchor === 'technical' || anchor === 'configuration') return 'technical'
  return 'results'
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
  if (
    presentation.breakdown.inconclusive > 0 &&
    presentation.breakdown.inconclusive === presentation.breakdown.issues
  )
    return { status: 'inconclusive', label: 'Inconclusive' }
  return { status: 'failed', label: 'Failed' }
}

function finiteMetric(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function formatMetricCount(value: number | null) {
  return value === null ? '—' : Math.round(value).toLocaleString('en-US')
}

function formatReportedCost(value: number | null) {
  if (value == null) return '—'
  if (value > 0 && value < 0.0001) return '<$0.0001'
  return `$${value.toFixed(4)}`
}

/* ---------------------------------------------------------------- layers */

/** The one status the result contract publishes, pooled over the retained
 *  runs (audit ED-05). Read by the overview. */
export function executionOutcome(
  presentation: ExecutionPresentation,
  runs: AssessmentRunView[],
): SystemOutcome {
  const values = runs.length
    ? runs.map((run) => run.systemStatus)
    : [presentation.attention]
  const counts = new Map<string, number>()
  for (const value of values) counts.set(value, (counts.get(value) ?? 0) + 1)
  if (counts.size === 1) return { value: values[0] }
  return {
    value: 'partial',
    label: [...counts]
      .map(
        ([value, count]) =>
          `${count} ${value === 'hard_gate_failed' ? 'failed (legacy result)' : value.replaceAll('_', ' ')}`,
      )
      .join(' · '),
  }
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
          ? 'This page follows recorded progress automatically. The final report and decision appear when the execution finishes.'
          : hasProgress
            ? 'The final report is unavailable. Recorded checkpoints remain visible below as partial evidence, not a final verdict.'
            : 'No report or verified progress is available for this execution.'}
      </p>
    </Panel>
  )
}

export function EvidenceBundleUnavailable({
  detail,
}: {
  detail: DashboardExecutionDetail
}) {
  const reports = finiteMetric(detail.totals?.received_reports)
  const tokens = finiteMetric(detail.totals?.total_tokens)
  const cost = finiteMetric(detail.totals?.total_cost_usd)
  const duration = finiteMetric(detail.totals?.wall_time_seconds)
  return (
    <>
      <Callout
        className="mt-4"
        tone="warning"
        title="Evidence bundle unavailable"
      >
        {detail.evidence_error}
      </Callout>
      <Panel className="mt-3">
        <h2 className="m-0 text-sm font-semibold text-ink">
          Retained execution snapshot
        </h2>
        <p className="mt-1 mb-0 text-xs leading-5 text-ink-soft">
          These execution totals remain available. Full per-test metrics and
          evidence require the unavailable bundle.
        </p>
        <div className="mt-3 grid min-w-0 gap-3 @[560px]:grid-cols-2 @[960px]:grid-cols-3">
          <MetricCard
            label="received reports"
            value={formatMetricCount(reports)}
            detail="retained execution total"
            tone={reports === null ? 'unavailable' : 'neutral'}
          />
          <MetricCard
            label="tokens"
            value={formatMetricCount(tokens)}
            detail="retained execution total"
            tone={tokens === null ? 'unavailable' : 'neutral'}
          />
          <MetricCard
            label="reported cost"
            value={formatReportedCost(cost)}
            detail="retained execution total"
            tone={cost === null ? 'unavailable' : 'neutral'}
          />
          <MetricCard
            label="runtime"
            value={duration === null ? '—' : formatDuration(duration)}
            detail="retained execution total"
            tone={duration === null ? 'unavailable' : 'neutral'}
          />
        </div>
      </Panel>
    </>
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
  const [deleteOpen, setDeleteOpen] = useState(false)
  const [deleting, setDeleting] = useState(false)
  const [transcript, setTranscript] = useState<{
    run: AssessmentRunView
    title: string
  } | null>(null)
  const anchorSection = anchor ? sectionFromAnchor(anchor) : null
  const [excludeFailedTests, setExcludeFailedTests] = useState(false)
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
    setExcludeFailedTests(false)
    void load()
  }, [load])

  const presentation = useMemo(
    () => (summary ? buildExecutionPresentation(summary) : null),
    [summary],
  )
  const live =
    detail?.origin !== 'remote' &&
    (presentation?.attention === 'running' ||
      presentation?.attention === 'cancelling')

  // Audit ED-12: a live execution follows the run instead of waiting for F5.
  useEffect(() => {
    if (!bridge || !live) return
    return watchExecution(bridge, executionId, load)
  }, [bridge, executionId, live, load])

  const assessmentModel = useMemo(
    () => buildAssessmentWorkspace(detail),
    [detail],
  )
  const scenarioMatrix = useMemo(
    () => (detail ? buildScenarioMatrix(detail) : null),
    [detail],
  )

  const allPrimaryMetrics = useMemo(
    () => comparisonPrimaryMetrics(detail, null).baseline,
    [detail],
  )
  const excludedScenarioIds = useMemo(() => {
    const excluded = new Set<string>()
    if (!excludeFailedTests || detail?.origin !== 'remote') return excluded
    for (const test of allPrimaryMetrics?.tests ?? []) {
      if (test.metrics.score.value === 0) excluded.add(test.label)
    }
    for (const item of scenarioMatrix?.items ?? []) {
      if (
        item.objective.status === 'failed' ||
        item.runs.some(
          (run) =>
            [
              'hard_gate_failed',
              'subject_error',
              'resource_limit',
              'infrastructure_error',
            ].includes(run.status) || run.technical === 'technical_invalid',
        )
      )
        excluded.add(item.scenarioId)
    }
    return excluded
  }, [allPrimaryMetrics, detail, excludeFailedTests, scenarioMatrix])
  const resultDetail = useMemo(() => {
    if (!detail || !excludeFailedTests) return detail
    if (detail.origin !== 'remote') return excludeUnsuccessfulTests(detail)
    const reference = detail.remote_reference as RcReference | undefined
    return reference
      ? ({
          ...detail,
          remote_reference: filterReferenceScenarios(
            reference,
            new Set(
              (allPrimaryMetrics?.tests ?? [])
                .filter((test) => !excludedScenarioIds.has(test.label))
                .map((test) => test.label),
            ),
          ),
        } as DashboardExecutionDetail)
      : detail
  }, [detail, excludeFailedTests, excludedScenarioIds, allPrimaryMetrics])
  const primaryMetrics = useMemo(
    () => comparisonPrimaryMetrics(resultDetail, null).baseline,
    [resultDetail],
  )
  const excludedTests =
    (allPrimaryMetrics?.tests.length ?? 0) - (primaryMetrics?.tests.length ?? 0)

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

  const evidenceRun = runId
    ? (assessmentModel.runs.find((run) => run.runId === runId) ?? null)
    : null
  const scenarioSummary = scenarioMatrix?.summary ?? null
  const status =
    detail.origin === 'remote' &&
    scenarioSummary &&
    !['running', 'cancelling', 'cancelled', 'incomplete'].includes(
      presentation.attention,
    )
      ? scenarioSummary.failed > 0
        ? { status: 'failed' as const, label: 'Failed' }
        : scenarioSummary.passed > 0 &&
            scenarioSummary.passed === scenarioSummary.total
          ? { status: 'passed' as const, label: 'Passed' }
          : { status: 'inconclusive' as const, label: 'Inconclusive' }
      : executionStatus(presentation)
  const runCount =
    scenarioMatrix?.items.reduce((total, item) => total + item.runCount, 0) ?? 0
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
      'started',
      presentation.startedAt ? formatDate(presentation.startedAt) : '—',
    ],
    [
      'trigger',
      [detail.event, detail.actor].filter(Boolean).join(' · ') ||
        'not reported',
    ],
    ['id', `${detail.id.slice(0, 8)}…${detail.id.slice(-6)}`],
  ]
  const ready = Boolean(bridge)
  const cancelRun = async () => {
    if (!bridge) return
    setCancelling(true)
    try {
      if (detail.plan_execution) {
        await planAction(bridge, {
          action: 'cancel',
          execution_id: executionId,
        })
      } else {
        await bridge.cancelRun()
      }
      await load()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setCancelling(false)
    }
  }
  const deleteExecution = async () => {
    if (!bridge || !detail) return
    setDeleting(true)
    setError(null)
    try {
      await bridge.deleteExecution(detail.id)
      window.location.hash = hashForWorkspace('executions')
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setDeleting(false)
    }
  }

  return (
    <div className="ds-root execution-page bg-canvas text-ink">
      <DashboardPageActions active="executions" context={title} />
      <div className="page-shell max-w-[1420px]">
        {/* Audit ED-13 / ED-23: the title is the execution, the trail is flat. */}
        <PageHeader
          className="pm-page-header execution-header"
          title={title}
          summary={
            <>
              <StatusBadge
                status={status.status}
                label={status.label.toLowerCase()}
              />
              <span>
                {detail.live_progress
                  ? `${detail.live_progress.runs_committed} of ${detail.live_progress.planned_slots} runs recorded · ${live ? 'results are provisional' : 'partial evidence preserved'}`
                  : live
                    ? 'Execution in progress · results are provisional'
                    : `${scenarioSummary?.total ?? 0} ${scenarioSummary?.total === 1 ? 'test' : 'tests'} · ${runCount} ${runCount === 1 ? 'run' : 'runs'}`}
              </span>
            </>
          }
          headingId="execution-title"
          breadcrumb={[
            detail.plan_id
              ? { label: 'plans', href: hashForPlans() }
              : { label: 'executions', href: hashForWorkspace('executions') },
            ...(detail.plan_id
              ? [{ label: 'Plan', href: hashForPlan(detail.plan_id) }]
              : []),
            { label: title },
          ]}
          actions={
            <>
              {ready ? (
                <a
                  className={buttonClassName({
                    variant: 'secondary',
                    className: 'no-underline',
                  })}
                  href={
                    detail.plan_id
                      ? hashForPlan(detail.plan_id)
                      : hashForWorkspace()
                  }
                  onClick={() =>
                    !detail.plan_id &&
                    requestQuickExecution(
                      scenarioMatrix?.items.map((item) => item.scenarioId) ??
                        [],
                    )
                  }
                >
                  {!detail.plan_id ? (
                    <RotateCcw size={15} aria-hidden="true" />
                  ) : null}
                  {detail.plan_id ? 'back to plan' : 're-run same scope'}
                </a>
              ) : null}
              <button
                className={buttonClassName({
                  variant: 'quiet',
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
              {ready &&
              !live &&
              !detail.plan_execution &&
              detail.origin !== 'remote' ? (
                <button
                  className={buttonClassName({
                    variant: 'quiet',
                  })}
                  type="button"
                  aria-label="Delete execution"
                  title="Delete execution"
                  onClick={() => setDeleteOpen(true)}
                >
                  <Trash2 size={15} aria-hidden="true" />
                </button>
              ) : null}
            </>
          }
        />

        {/* Audit ED-05: identity is one band of facts, not four cards. */}
        <dl className="execution-identity" data-identity-band>
          {identity.map(([label, value]) => (
            <div className="grid min-w-0 content-start gap-1" key={label}>
              <dt className="ds-label">{label}</dt>
              <dd className="m-0 min-w-0 break-words text-ink">{value}</dd>
            </div>
          ))}
        </dl>

        {detail.evidence_error ? (
          <EvidenceBundleUnavailable detail={detail} />
        ) : null}
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
        {!detail.evidence_error &&
        (noRun || live) &&
        !(detail.plan_execution && live) ? (
          <LiveState
            presentation={presentation}
            status={status}
            cancelling={cancelling}
            hasProgress={Boolean(detail.live_progress || detail.plan_execution)}
            onCancel={
              ready && detail.origin !== 'remote'
                ? () => void cancelRun()
                : undefined
            }
          />
        ) : null}
        {detail.plan_execution && live ? (
          <PlanProgress
            execution={detail.plan_execution}
            actions={
              ready ? (
                <button
                  type="button"
                  className={buttonClassName({
                    variant: 'secondary',
                    size: 'compact',
                  })}
                  onClick={() => void cancelRun()}
                  disabled={cancelling}
                >
                  {cancelling ? 'cancelling…' : 'cancel execution'}
                </button>
              ) : undefined
            }
          />
        ) : null}
        {detail.live_progress ? (
          <LiveProgressPanel progress={detail.live_progress} running={live} />
        ) : null}
        {primaryMetrics && !detail.evidence_error ? (
          <section
            id="metrics"
            className="mt-6 scroll-mt-24"
            aria-label="Execution summary"
          >
            <PrimaryMetricsView
              key={executionId}
              baseline={primaryMetrics}
              baselineExecutionId={executionId}
              baselineLabel={title}
              summaryOnly
              toolbarActions={
                <div className="pm-execution-filter">
                  <label className="pm-filter">
                    <input
                      type="checkbox"
                      checked={excludeFailedTests}
                      onChange={(event) =>
                        setExcludeFailedTests(event.target.checked)
                      }
                    />
                    Exclude tests with zero score or failures
                  </label>
                  {excludeFailedTests ? (
                    <span className="pm-muted" role="status">
                      {excludedTests} {excludedTests === 1 ? 'test' : 'tests'}{' '}
                      excluded · metrics recalculated
                    </span>
                  ) : null}
                </div>
              }
            />
            {detail.origin !== 'remote' &&
            !noRun &&
            !live &&
            resultDetail &&
            primaryMetrics.tests.length > 0 ? (
              <ExecutionMetricsPanel detail={resultDetail} />
            ) : null}
          </section>
        ) : null}
        {!noRun && !live ? (
          <div className="execution-layers mt-6 grid min-w-0 gap-3">
            <section
              id="results"
              className="min-w-0 scroll-mt-24"
              aria-labelledby="execution-results-heading"
            >
              <h2
                id="execution-results-heading"
                className="m-0 mb-4 text-base font-semibold text-ink"
              >
                Scenario results
              </h2>
              {excludeFailedTests && primaryMetrics?.tests.length === 0 ? (
                <EmptyState
                  title="All scenarios excluded"
                  description="Clear the filter to show the retained test results."
                />
              ) : (
                <ScenarioMatrix
                  detail={resultDetail ?? detail}
                  onTranscript={(run, title) => setTranscript({ run, title })}
                  showContract={false}
                />
              )}
            </section>
            <DisclosureLayer
              key={anchor}
              id="technical"
              label="provenance"
              scent={contractScent(scenarioMatrix?.contracts ?? [])}
              open={anchorSection === 'technical'}
            >
              <ProvenanceSection
                detail={detail}
                presentation={presentation}
                contracts={scenarioMatrix?.contracts ?? []}
              />
            </DisclosureLayer>
          </div>
        ) : null}
      </div>
      <Dialog
        open={deleteOpen}
        onClose={() => !deleting && setDeleteOpen(false)}
        size="sm"
        title="Delete execution?"
        description="This permanently removes the execution and its retained evidence from the Console."
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
              onClick={() => void deleteExecution()}
            >
              {deleting ? 'deleting…' : 'delete execution'}
            </button>
          </div>
        }
      />
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
