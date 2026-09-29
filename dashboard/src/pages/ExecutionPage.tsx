import { GitCompare, RotateCcw, Square } from 'lucide-react'
import {
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from 'react'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { DisclosureLayer } from '@/components/DisclosureLayer'
import { ExecutionFacts } from '@/components/ExecutionConfiguration'
import { ExecutionNameControl } from '@/components/ExecutionNameControl'
import { EvidenceRecordPage } from '@/components/execution/EvidenceRecord'
import { ExecutionTotals } from '@/components/execution/ExecutionTotals'
import {
  attentionItems,
  ExecutionMoreMenu,
  NeedsAttention,
} from '@/components/execution/NeedsAttention'
import { ScreenshotGallery } from '@/components/execution/screenshots'
import { TranscriptPage } from '@/components/execution/TranscriptPage'
import {
  CancelExecutionDialog,
  HarnessProgress,
  LiveProgress,
  WhereItRan,
} from '@/components/execution/WhereItRan'
import { liveNotes } from '@/components/execution/where-it-ran-model'
import { InvestigationAction } from '@/components/InvestigationAction'
import { LiveProgressPanel } from '@/components/LiveProgressPanel'
import { LocalRunnerDialog } from '@/components/LocalRunnerDialog'
import {
  contractScent,
  ResultContractStrip,
  ScenarioMatrix,
} from '@/components/ScenarioMatrix'
import { ScenarioRerunDialog } from '@/components/ScenarioRerunDialog'
import type { SystemOutcome } from '@/components/SystemOutcome'
import {
  buttonClassName,
  Callout,
  EmptyState,
  MetricCard,
  type OperationalStatus,
  PageHeader,
  Panel,
  StatusBadge,
} from '@/design-system'
import {
  comparisonOrigin,
  hashForComparison,
  hashForExecution,
  hashForWorkspace,
  hashFrom,
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
  type ExecutionParameters,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import {
  buildExecutionPresentation,
  type ExecutionModel,
  type ExecutionPresentation,
  executionTitle,
  formatDate,
  formatDuration,
  providerModel,
  suiteText,
  workerVersion,
} from '@/lib/execution-view'
import { scenarioReruns } from '@/lib/plan-execution'
import { buildPrimaryMetrics } from '@/lib/primary-metrics'
import { buildScenarioMatrix } from '@/lib/scenario-matrix'
import { screenshotsOf } from '@/lib/screenshots'
import { watchExecution } from '@/lib/watch-execution'
import { buildLedgerRows, DeleteDialog } from '@/pages/ExecutionsPage'
import '@/design-system/styles.css'
import { copyText } from '@/lib/clipboard'

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

/** Running one test again is offered once the execution finished: a Docker
 *  execution shows its tests, and what needs attention, while it still runs. */
export function testRerunOffered(
  detail: Pick<DashboardExecutionDetail, 'plan_execution'>,
  ready: boolean,
  live: boolean,
) {
  return ready && !live && Boolean(detail.plan_execution)
}

/** What running an execution again starts from: its recorded parameters (a
 *  native run's are its own request), or when nothing was recorded, the
 *  scenarios and model it reports with the form's defaults. */
export function rerunParameters(
  detail: Pick<DashboardExecutionDetail, 'plan_execution' | 'parameters'>,
  scenarios: string[],
  subject: ExecutionModel | undefined,
): ExecutionParameters {
  return (
    detail.plan_execution?.parameters ??
    detail.parameters ?? {
      scenarios: [...new Set(scenarios)],
      runs: 1,
      technical_retries: 1,
      model: subject?.model ?? '',
      provider: subject?.provider ?? '',
      agent: null,
    }
  )
}

/** The suite an execution ran: its name and digest, or nothing recorded. */
export function executionSuite(detail: DashboardExecutionDetail) {
  return (
    suiteText(
      (detail.plan_execution?.parameters ?? detail.parameters)?.suite,
    ) ?? 'not recorded'
  )
}

/** Where an execution ran: for one in Docker or imported the stack it
 *  records, then the Harness and the E2E runner as its stack recorded them.
 *  What was not recorded is not shown. */
export function stackVersions(
  detail: DashboardExecutionDetail,
): Array<[string, string]> {
  const stack = detail.plan_execution?.stack
  const source = detail.plan_execution?.source
  const recorded = detail.plan_execution?.parameters?.stack?.name
  return (
    [
      [
        'stack',
        recorded ?? (source?.kind === 'github' ? (source.stack ?? null) : null),
      ],
      ['harness', workerVersion(stack, 'harness')],
      ['runner', workerVersion(stack, 'harness-e2e')],
    ] as const
  ).flatMap(([label, version]) =>
    version ? [[label, version] as [string, string]] : [],
  )
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
  // After a scenario ran again, start to finish spans the pause and the
  // replaced attempt: the time is the current runs' own.
  const reran = detail.plan_execution?.slots.some(
    (slot) => (slot.previous_attempts?.length ?? 0) > 0,
  )
  const runTime = finiteMetric(detail.totals?.wall_time_seconds)
  const duration = reran
    ? runTime === null
      ? 'after running a scenario again'
      : `after running a scenario again · ${formatDuration(runTime)} of current runs`
    : Number.isFinite(started) &&
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
            void copyText(raw).then((ok) => {
              if (!ok) return
              setCopied(true)
              window.setTimeout(() => setCopied(false), 1500)
            })
          }}
        >
          {copied ? 'Copied' : 'Copy JSON'}
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
  hasProgress,
}: {
  presentation: ExecutionPresentation
  status: { status: OperationalStatus; label: string }
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
        <StatusBadge status={status.status} label={status.label} />
        <span className="font-mono text-xs text-ink-soft">
          {[scope, elapsed ? `${elapsed} elapsed` : null]
            .filter(Boolean)
            .join(' · ') || 'no progress reported yet'}
        </span>
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

/** What a live execution's page shows, one progress at a time. On GitHub
 *  the steps and group jobs stand in for the results until the import,
 *  never over results already there (a test run again, a reimport). The
 *  legacy live panel is for executions without a plan. */
export function liveView(
  detail: Pick<DashboardExecutionDetail, 'plan_execution'>,
  {
    live,
    importing,
    hasResults,
  }: {
    live: boolean
    importing: boolean
    hasResults: boolean
  },
) {
  const plan = detail.plan_execution
  const moving = live || importing
  return {
    githubLive:
      plan?.source.kind === 'github' && moving && !plan.rerun && !hasResults,
    legacyPanel: !(plan && moving),
  }
}

/** Where the execution ran, as the status line says it. */
function statusWhere(detail: DashboardExecutionDetail) {
  const kind = detail.plan_execution?.source.kind
  if (kind === 'github') return 'on GitHub'
  if (kind === 'docker') return 'in Docker'
  return 'on this harness'
}

export function ExecutionPage({
  executionId,
  anchor,
  runId,
  view = null,
}: {
  executionId: string
  anchor?: string | null
  /** A run shown as its own page: its evidence record or transcript. */
  runId?: string | null
  view?: 'evidence' | 'transcript' | null
}) {
  const [summary, setSummary] = useState<DashboardExecutionSummary | null>(null)
  const [detail, setDetail] = useState<DashboardExecutionDetail | null>(null)
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)
  const [cancelling, setCancelling] = useState(false)
  const [cancelOpen, setCancelOpen] = useState(false)
  const [renameSignal, setRenameSignal] = useState(0)
  const [openScenario, setOpenScenario] = useState<string | null>(null)
  const [deleteOpen, setDeleteOpen] = useState(false)
  const [deleting, setDeleting] = useState(false)
  // A delete the worker refused: said once, apart from refresh errors.
  const [deleteError, setDeleteError] = useState<string | null>(null)
  // The parameters the Run again form opened with; null while it is closed.
  const [rerun, setRerun] = useState<ExecutionParameters | null>(null)
  // Open apart from the parameters, so Run again keeps its title while the
  // dialog animates closed.
  const [rerunOpen, setRerunOpen] = useState(false)
  // The scenario the Run this scenario again dialog is open on.
  const [scenarioRerun, setScenarioRerun] = useState<string | null>(null)
  const anchorSection = anchor ? sectionFromAnchor(anchor) : null
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
  const importing = detail?.status === 'importing'
  // Running a scenario again: the other scenarios keep their results on screen.
  const rerunning = live && Boolean(detail?.plan_execution?.rerun)

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

  const primaryMetrics = useMemo(
    () => (detail ? buildPrimaryMetrics(detail) : null),
    [detail],
  )

  if (error && !detail)
    return (
      <div className="ds-root min-h-dvh text-ink">
        <DashboardPageActions active="executions" />
        <div className="page-shell">
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
                  Retry
                </button>
                <a
                  className={buttonClassName({
                    variant: 'quiet',
                    className: 'no-underline',
                  })}
                  href={hashForWorkspace('executions')}
                >
                  Back to Executions
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
      <div className="ds-root min-h-dvh text-ink">
        <DashboardPageActions active="executions" />
        <div className="page-shell" aria-busy="true" role="status">
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
  // A run's evidence record and transcript are pages of their own (linkable,
  // back to the execution) instead of dialogs over it.
  if (evidenceRun) {
    // Opened from a comparison: its links keep it, and back goes to it.
    const origin =
      typeof window === 'undefined'
        ? null
        : comparisonOrigin(window.location.hash)
    const keep = (hash: string) => (origin ? hashFrom(hash, origin) : hash)
    const backHref = origin ?? hashForExecution(detail.id, 'results')
    const transcriptHref = keep(
      hashForExecution(detail.id, null, evidenceRun.runId, 'transcript'),
    )
    const evidenceHref = keep(
      hashForExecution(detail.id, null, evidenceRun.runId),
    )
    return (
      <div className="harness-e2e-execution-page">
        <DashboardPageActions
          active="executions"
          context={evidenceRun.scenarioId}
        />
        {view === 'transcript' ? (
          <TranscriptPage
            title={evidenceRun.scenarioId}
            runLine={`run ${evidenceRun.runId} · ${evidenceRun.subjectId} · ${formatDuration((evidenceRun.metrics.durationMs ?? 0) / 1000)}`}
            messages={evidenceRun.transcript?.messages}
            backHref={origin ?? evidenceHref}
            backLabel={
              origin ? 'Back to comparison' : 'Back to the evidence record'
            }
            evidenceHref={evidenceHref}
          />
        ) : (
          <EvidenceRecordPage
            run={evidenceRun}
            detail={detail}
            backHref={backHref}
            backLabel={origin ? 'Back to comparison' : undefined}
            transcriptHref={transcriptHref}
            onRerun={
              testRerunOffered(detail, Boolean(bridge), live)
                ? () => setScenarioRerun(evidenceRun.scenarioId)
                : undefined
            }
            onOpenFile={
              bridge
                ? async (path) => {
                    const record = detail.reports.find(
                      (entry) =>
                        entry.scenario_id === evidenceRun.scenarioId &&
                        entry.report?.scenarios.some((scenario) =>
                          scenario.runs.some(
                            (candidate) =>
                              candidate.run_id === evidenceRun.runId,
                          ),
                        ),
                    )
                    const file = await bridge.readEvidence({
                      execution_id:
                        typeof record?.native_execution_id === 'string' &&
                        record.native_execution_id
                          ? record.native_execution_id
                          : detail.id,
                      path,
                    })
                    const raw = atob(file.base64)
                    const bytes = new Uint8Array(raw.length)
                    for (let index = 0; index < raw.length; index += 1)
                      bytes[index] = raw.charCodeAt(index)
                    const url = URL.createObjectURL(
                      new Blob([bytes], { type: file.media_type }),
                    )
                    window.open(url, '_blank', 'noopener')
                    window.setTimeout(() => URL.revokeObjectURL(url), 60_000)
                  }
                : undefined
            }
          >
            <ScreenshotGallery
              bridge={bridge}
              heading={false}
              screenshots={screenshotsOf(detail, evidenceRun.scenarioId).filter(
                (screenshot) => screenshot.runId === evidenceRun.runId,
              )}
            />
          </EvidenceRecordPage>
        )}
        {detail.plan_execution ? (
          <ScenarioRerunDialog
            bridge={bridge}
            execution={detail.plan_execution}
            scenarioId={scenarioRerun}
            onClose={() => setScenarioRerun(null)}
            onStarted={() => {
              setScenarioRerun(null)
              void load()
            }}
          />
        ) : null}
      </div>
    )
  }
  const scenarioSummary = scenarioMatrix?.summary ?? null
  // In Docker a group's tests fill in as it ends: the table shows from the start.
  const docker = detail.plan_execution?.source.kind === 'docker'
  // On GitHub the results arrive with the import: until then the page shows
  // the run's steps and group jobs, not totals of nothing.
  const { githubLive, legacyPanel } = liveView(detail, {
    live,
    importing,
    hasResults: Boolean(
      scenarioMatrix?.items.some((item) => item.runCount > 0),
    ),
  })
  const status = importing
    ? { status: 'running' as const, label: 'Importing' }
    : detail.plan_execution?.state === 'cancelling'
      ? { status: 'cancelling' as const, label: 'Cancelling' }
      : executionStatus(presentation)
  const noRun = !presentation.available || (scenarioSummary?.total ?? 0) === 0
  const rerunScenarios = new Set(
    detail.plan_execution?.slots
      .filter((slot) => scenarioReruns(detail.plan_execution, slot.scenario_id))
      .map((slot) => slot.scenario_id),
  ).size
  const { title } = executionTitle(presentation)
  // Tests · where · when, live or not (canvas: Execution detail): a live
  // execution counts what it plans, GitHub's from when it was dispatched.
  const tests = Math.max(
    scenarioSummary?.total ?? 0,
    live ? new Set(detail.plan_execution?.parameters?.scenarios).size : 0,
  )
  const statusLine = [
    `${tests} ${tests === 1 ? 'test' : 'tests'}`,
    statusWhere(detail),
    rerunning && detail.plan_execution?.rerun
      ? `${detail.plan_execution.rerun.scenarios.join(', ')} running again since ${formatDate(detail.plan_execution.rerun.started_at)}`
      : presentation.startedAt
        ? `${live ? (detail.plan_execution?.source.kind === 'github' ? 'dispatched ' : 'started ') : ''}${formatDate(presentation.startedAt)}`
        : null,
    rerunScenarios > 0
      ? `${rerunScenarios} ${rerunScenarios === 1 ? 'scenario' : 'scenarios'} run again, the last attempt counts`
      : null,
  ]
    .filter(Boolean)
    .join(' · ')
  const loadStacks = bridge ? () => bridge.listStacks() : undefined
  // Facts the band shows beside what the plan execution recorded.
  const identity: Array<[string, ReactNode]> = [
    ...(detail.plan_execution
      ? []
      : ([['Where', 'This harness']] as Array<[string, ReactNode]>)),
    [
      'Model',
      presentation.subjects.map(providerModel).join(', ') || 'not reported',
    ],
    // What the execution ran on, to compare with another one.
    ...stackVersions(detail).map(
      ([label, value]) =>
        [label.charAt(0).toUpperCase() + label.slice(1), value] as [
          string,
          ReactNode,
        ],
    ),
    ['Suite', executionSuite(detail)],
    ['Id', `${detail.id.slice(0, 9)}…${detail.id.slice(-6)}`],
  ]
  const ready = Boolean(bridge)
  const canCancel =
    live &&
    !importing &&
    presentation.attention !== 'cancelling' &&
    detail.plan_execution?.state !== 'cancelling'
  const cancelRun = async () => {
    if (!bridge) return
    setCancelling(true)
    try {
      if (detail.plan_execution) {
        await bridge.cancelExecution(executionId)
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
  const renameExecution = async (id: string, label: string) => {
    if (!bridge) return
    await bridge.renameExecution(id, label)
    await load()
  }
  const deleteExecution = async () => {
    if (!bridge || !detail) return
    setDeleting(true)
    setDeleteError(null)
    try {
      await bridge.deleteExecution(detail.id)
      window.location.hash = hashForWorkspace('executions')
    } catch (cause) {
      // As the list: the dialog closes and the page says why, once.
      setDeleteOpen(false)
      setDeleteError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setDeleting(false)
    }
  }

  return (
    <div className="ds-root execution-page text-ink">
      <DashboardPageActions active="executions" context={title} />
      <div className="page-shell">
        {/* Audit ED-13 / ED-23: the title is the execution, the trail is flat. */}
        <PageHeader
          variant="detail"
          className="pm-page-header execution-header"
          title={title}
          summary={
            <>
              <StatusBadge status={status.status} label={status.label} />{' '}
              <span data-status-line>{statusLine}</span>
            </>
          }
          headingId="execution-title"
          back={{
            label: 'Back to Executions',
            href: hashForWorkspace('executions'),
          }}
          titleAction={
            ready && detail.plan_execution ? (
              <ExecutionNameControl
                executionId={detail.id}
                fallbackLabel={title}
                label={detail.plan_execution.label ?? ''}
                onRename={renameExecution}
                openSignal={renameSignal}
              />
            ) : undefined
          }
          actions={
            <>
              {!live ? (
                <a
                  className={buttonClassName({
                    variant: 'secondary',
                    className: 'no-underline',
                  })}
                  href={hashForComparison(detail.id)}
                  data-compare-with
                >
                  <GitCompare size={15} aria-hidden="true" />
                  Compare with…
                </a>
              ) : null}
              <InvestigationAction
                executionId={executionId}
                evidenceUnavailable={detail.evidence_error}
              />
              {/* While it runs the header cancels it (canvas: Execution
                  detail · running); once cancelling, nothing to do but wait. */}
              {ready && canCancel ? (
                <button
                  className={buttonClassName({ variant: 'secondary' })}
                  type="button"
                  disabled={cancelling}
                  aria-busy={cancelling || undefined}
                  onClick={() =>
                    detail.plan_execution
                      ? setCancelOpen(true)
                      : void cancelRun()
                  }
                  data-cancel-execution
                >
                  <Square size={15} aria-hidden="true" />
                  {cancelling
                    ? 'Cancelling…'
                    : detail.plan_execution?.source.kind === 'github'
                      ? 'Cancel run'
                      : 'Cancel execution'}
                </button>
              ) : null}
              {/* The page's one primary action (canvas: Execution detail). */}
              {ready && !live ? (
                <button
                  className={buttonClassName({ variant: 'primary' })}
                  type="button"
                  onClick={() => {
                    setRerunOpen(true)
                    setRerun(
                      rerunParameters(
                        detail,
                        scenarioMatrix?.items.map((item) => item.scenarioId) ??
                          [],
                        presentation.subjects[0],
                      ),
                    )
                  }}
                >
                  <RotateCcw size={15} aria-hidden="true" />
                  Run again
                </button>
              ) : null}
              {copied ? (
                <span className="ep-faint" role="status">
                  Copied
                </span>
              ) : null}
              <ExecutionMoreMenu
                onRename={
                  ready && detail.plan_execution
                    ? () => setRenameSignal((signal) => signal + 1)
                    : undefined
                }
                onCopyLink={() => {
                  void copyText(window.location.href).then((ok) => {
                    if (!ok) return
                    setCopied(true)
                    window.setTimeout(() => setCopied(false), 1500)
                  })
                }}
                onCopyId={() => {
                  void copyText(detail.id).then((ok) => {
                    if (!ok) return
                    setCopied(true)
                    window.setTimeout(() => setCopied(false), 1500)
                  })
                }}
                githubUrl={
                  detail.plan_execution?.source.kind === 'github'
                    ? detail.plan_execution.source.url
                    : null
                }
                onDelete={ready ? () => setDeleteOpen(true) : undefined}
                deleteDisabled={live}
              />
            </>
          }
        />
        {detail.plan_execution?.source.kind === 'local' && live ? (
          <HarnessProgress execution={detail.plan_execution} />
        ) : null}
        {!noRun || detail.plan_execution ? (
          <ExecutionFacts
            execution={detail.plan_execution ?? null}
            extra={identity}
            loadStacks={loadStacks}
          />
        ) : null}

        {deleteError ? (
          <Callout
            className="mt-4"
            tone="danger"
            title={`Couldn’t delete “${title}”`}
            data-delete-error
          >
            <span className="ex-callout-line">
              {deleteError}
              <button
                className={buttonClassName({
                  variant: 'quiet',
                  size: 'compact',
                })}
                type="button"
                onClick={() => setDeleteError(null)}
              >
                Dismiss
              </button>
            </span>
          </Callout>
        ) : null}
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
        {!detail.evidence_error && (noRun || live) && legacyPanel ? (
          <LiveState
            presentation={presentation}
            status={status}
            hasProgress={Boolean(detail.live_progress || detail.plan_execution)}
          />
        ) : null}
        {/* One representation of progress: numbered steps in Docker and on
            GitHub; this harness's bar sits under the title. */}
        {detail.plan_execution && (live || importing) ? (
          <LiveProgress execution={detail.plan_execution} />
        ) : null}
        {detail.plan_execution ? (
          <CancelExecutionDialog
            bridge={bridge}
            execution={detail.plan_execution}
            open={cancelOpen}
            onClose={() => setCancelOpen(false)}
            onCancelled={() => {
              // Asked: the header's Cancel waits for the reload that says so.
              setCancelling(true)
              void load().finally(() => setCancelling(false))
            }}
          />
        ) : null}
        {detail.live_progress ? (
          <LiveProgressPanel progress={detail.live_progress} running={live} />
        ) : null}
        {(!live || (detail.plan_execution && !githubLive)) && scenarioMatrix ? (
          <NeedsAttention
            executionId={detail.id}
            items={attentionItems(scenarioMatrix.items, [
              ...(detail.plan_execution?.error
                ? [`Execution error: ${detail.plan_execution.error}`]
                : []),
              ...(detail.plan_execution?.warnings ?? []),
            ])}
            onRerun={
              testRerunOffered(detail, ready, live)
                ? setScenarioRerun
                : undefined
            }
            onShow={(key) => {
              setOpenScenario(key)
              window.setTimeout(
                () =>
                  document
                    .querySelector(`[data-scenario-row="${CSS.escape(key)}"]`)
                    ?.scrollIntoView({ block: 'center' }),
                0,
              )
            }}
          />
        ) : null}
        {primaryMetrics &&
        scenarioMatrix &&
        !detail.evidence_error &&
        !noRun &&
        !githubLive ? (
          <section
            id="metrics"
            className="scroll-mt-24"
            aria-label="Execution summary"
          >
            <ExecutionTotals
              metrics={primaryMetrics}
              items={scenarioMatrix.items}
              running={live}
              detail={detail}
            />
          </section>
        ) : null}
        {!noRun && !githubLive && (!live || detail.plan_execution) ? (
          <div className="execution-layers grid min-w-0">
            <section
              id="results"
              className="min-w-0 scroll-mt-24"
              aria-labelledby="execution-results-heading"
            >
              <ScenarioMatrix
                heading="Results by test"
                detail={detail}
                openKey={openScenario}
                running={live}
                {...(live && !rerunning && detail.plan_execution
                  ? {
                      liveNote: docker
                        ? 'A group’s tests fill in as it finishes.'
                        : 'Rows fill in as tests report.',
                      notes: liveNotes(detail.plan_execution),
                    }
                  : {})}
                onTranscript={(run) => {
                  window.location.hash = hashForExecution(
                    detail.id,
                    null,
                    run.runId,
                    'transcript',
                  )
                }}
                showContract={false}
                onRerun={
                  testRerunOffered(detail, ready, live)
                    ? setScenarioRerun
                    : undefined
                }
              />
            </section>
            <DisclosureLayer
              key={anchor}
              id="technical"
              label="Provenance"
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
        {detail.plan_execution &&
        !live &&
        !importing &&
        detail.plan_execution.source.kind !== 'local' ? (
          <WhereItRan execution={detail.plan_execution} />
        ) : null}
      </div>
      {/* The list's delete confirmation, on this execution. */}
      <DeleteDialog
        request={
          deleteOpen && summary
            ? { rows: buildLedgerRows([summary]), kept: [] }
            : null
        }
        deleting={deleting}
        onCancel={() => setDeleteOpen(false)}
        onConfirm={() => void deleteExecution()}
        onClosed={() =>
          document
            .querySelector<HTMLElement>(
              '.execution-header [aria-label="More actions"]',
            )
            ?.focus()
        }
      />
      {detail.plan_execution ? (
        <ScenarioRerunDialog
          bridge={bridge}
          execution={detail.plan_execution}
          scenarioId={scenarioRerun}
          onClose={() => setScenarioRerun(null)}
          onStarted={() => {
            setScenarioRerun(null)
            void load()
          }}
        />
      ) : null}
      <LocalRunnerDialog
        bridge={bridge}
        open={rerunOpen}
        parameters={rerun}
        label={detail.plan_execution?.label ?? detail.label ?? ''}
        onClose={() => setRerunOpen(false)}
      />
    </div>
  )
}
