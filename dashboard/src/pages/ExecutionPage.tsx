import {
  Badge,
  Button,
  CollapsibleCard,
  CollapsibleCardContent,
  CollapsibleCardTrigger,
  ConfirmDialog,
  EmptyState,
  Skeleton,
  StatusPanel,
  Table,
  TableBody,
  TableCaption,
  TableCell,
  TableFrame,
  TableHead,
  TableHeader,
  TableRow,
  TableViewport,
} from '@iii-dev/console-ui'
import { ChevronDown, ChevronRight, Link2, Trash2 } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState } from 'react'
import { AssessmentDetailDialog } from '@/components/AssessmentWorkspace'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { requestQuickExecution } from '@/components/ExecutionSetup'
import { LiveProgressPanel } from '@/components/LiveProgressPanel'
import { PlanProgress } from '@/components/PlanStatus'
import { ScenarioChatAction } from '@/components/ScenarioChatAction'
import { SemanticTestFlow } from '@/components/SemanticTestFlow'
import { TranscriptDialog } from '@/components/TranscriptDialog'
import { MetricCard, PageHeader } from '@/design-system'
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
import { shortDefinition } from '@/lib/definition-digest'
import {
  type ExecutionMetricCard,
  executionMetricCards,
  executionStatus,
  executionSummarySentence,
  filterResults,
  formatMetricCount,
  formatReportedCost,
  identityEntries,
  liveStateCopy,
  provenanceEntries,
  type ResultFilter,
  resultFilterCounts,
  retainedArtifacts,
  runCountFromDetail,
  snapshotMetricCards,
  summaryFromDetail,
  verdictVariant,
} from '@/lib/execution-detail'
import { executionVerdict } from '@/lib/execution-verdict'
import {
  buildExecutionPresentation,
  executionTitle,
  formatDuration,
} from '@/lib/execution-view'
import { planAction } from '@/lib/plan-execution'
import {
  type RcReference,
  referencePrimaryMetrics,
} from '@/lib/release-control-reference'
import {
  buildScenarioMatrix,
  detailForScenario,
  formatScenarioDuration,
  type ScenarioMatrixItem,
} from '@/lib/scenario-matrix'
import { badgeVariantForStatus } from '@/lib/status-badge'
import { watchExecution } from '@/lib/watch-execution'
import '@/design-system/styles.css'

/* The execution detail, rebuilt on the Console's components: one verdict,
   five numbers, one results table that opens onto each test's runs, and the
   provenance folded away. The logic lives in `lib/execution-detail`. */

const SHELL =
  'mx-auto w-full max-w-[var(--spacing-content-max)] px-4 pt-5 pb-16 md:px-6'
const NUMERIC = 'whitespace-nowrap text-right font-mono tabular-nums'
const META = 'block font-mono text-xs text-ink-faint'

function MetricStrip({ cards }: { cards: ExecutionMetricCard[] }) {
  return (
    <div
      data-execution-metrics
      className="grid min-w-0 gap-3 @[560px]:grid-cols-2 @[960px]:grid-cols-5"
    >
      {cards.map((card) => (
        <MetricCard
          key={card.label}
          label={card.label}
          value={card.value}
          detail={card.detail}
          tone={card.tone}
        />
      ))}
    </div>
  )
}

function IdentityBand({ entries }: { entries: Array<[string, string]> }) {
  return (
    <dl
      className="mt-4 flex flex-wrap gap-x-6 gap-y-2 font-mono text-xs"
      data-identity-band
    >
      {entries.map(([label, value]) => (
        <div className="flex min-w-0 items-baseline gap-2" key={label}>
          <dt className="text-ink-faint">{label}</dt>
          <dd className="m-0 min-w-0 break-words text-ink">{value}</dd>
        </div>
      ))}
    </dl>
  )
}

function RunRows({
  runs,
  executionId,
  detail,
  onTranscript,
}: {
  runs: AssessmentRunView[]
  executionId: string
  detail: DashboardExecutionDetail
  onTranscript: (run: AssessmentRunView, title: string) => void
}) {
  if (runs.length === 0)
    return (
      <p className="m-0 text-sm text-ink-faint">
        No run was retained for this test.
      </p>
    )
  return (
    <TableViewport>
      <TableFrame>
        <Table density="compact" data-run-table>
          <TableCaption className="sr-only">Runs of this test</TableCaption>
          <TableHeader>
            <TableRow>
              <TableHead scope="col">run</TableHead>
              <TableHead scope="col">outcome</TableHead>
              <TableHead scope="col" className="text-right">
                score
              </TableHead>
              <TableHead scope="col" className="text-right">
                runtime
              </TableHead>
              <TableHead scope="col" className="text-right">
                tokens
              </TableHead>
              <TableHead scope="col">
                <span className="sr-only">evidence</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {runs.map((run, index) => (
              <TableRow key={run.key} data-run-id={run.runId}>
                <TableCell>
                  <a
                    className="font-mono text-sm text-ink no-underline hover:underline"
                    href={hashForExecution(executionId, null, run.runId)}
                  >
                    run {index + 1}
                  </a>
                  <span className={META}>{run.runId.slice(0, 8)}</span>
                </TableCell>
                <TableCell>
                  <Badge
                    className="whitespace-nowrap"
                    variant={badgeVariantForStatus(run.systemStatus)}
                  >
                    {run.systemStatus === 'hard_gate_failed'
                      ? 'failed'
                      : run.systemStatus.replaceAll('_', ' ')}
                  </Badge>
                  {run.assessments.length === 0 ? (
                    <span className={META}>nothing scored</span>
                  ) : null}
                </TableCell>
                <TableCell className={NUMERIC}>
                  {run.score === null ? '—' : run.score}
                </TableCell>
                <TableCell className={NUMERIC}>
                  {run.metrics.durationMs === null
                    ? '—'
                    : formatDuration(run.metrics.durationMs / 1000)}
                </TableCell>
                <TableCell className={NUMERIC}>
                  {formatMetricCount(run.metrics.totalTokens)}
                </TableCell>
                <TableCell>
                  <span className="flex flex-wrap items-center justify-end gap-1">
                    {run.transcript ? (
                      <Button
                        variant="ghost"
                        size="sm"
                        type="button"
                        onClick={() =>
                          onTranscript(
                            run,
                            `${run.scenarioId.replaceAll('_', ' ')} · run ${index + 1}`,
                          )
                        }
                      >
                        transcript
                      </Button>
                    ) : null}
                    <Button variant="ghost" size="sm" asChild>
                      <a href={hashForExecution(executionId, null, run.runId)}>
                        evidence
                      </a>
                    </Button>
                    <ScenarioChatAction
                      compact
                      detail={detail}
                      scenarioId={run.scenarioId}
                      subjectId={run.subjectId}
                      runId={run.runId}
                    />
                  </span>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </TableFrame>
    </TableViewport>
  )
}

const RESULT_COLUMNS = 7

/** The results: one row per test, opening onto its runs and evidence. */
export function ResultsTable({
  items,
  runs,
  detail,
  expanded,
  onToggle,
  onTranscript,
}: {
  items: ScenarioMatrixItem[]
  runs: AssessmentRunView[]
  detail: DashboardExecutionDetail
  expanded: ReadonlySet<string>
  onToggle: (key: string) => void
  onTranscript: (run: AssessmentRunView, title: string) => void
}) {
  return (
    <TableViewport>
      <TableFrame>
        <Table density="compact" data-results-table>
          <TableCaption className="sr-only">Test results</TableCaption>
          <TableHeader>
            <TableRow>
              <TableHead scope="col">test</TableHead>
              <TableHead scope="col">result</TableHead>
              <TableHead scope="col" className="text-right">
                score
              </TableHead>
              <TableHead scope="col" className="text-right">
                runs
              </TableHead>
              <TableHead scope="col" className="text-right">
                runtime
              </TableHead>
              <TableHead scope="col" className="text-right">
                tokens
              </TableHead>
              <TableHead scope="col">
                <span className="sr-only">details</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          {items.map((item) => {
            const open = expanded.has(item.key)
            const itemRuns = runs.filter(
              (run) =>
                run.scenarioId === item.scenarioId &&
                run.subjectId === item.subjectId,
            )
            const definition = shortDefinition(item.behaviorSha256)
            return (
              <TableBody key={item.key} data-result={item.objective.status}>
                <TableRow
                  interactive
                  data-scenario-id={item.scenarioId}
                  onClick={(event) => {
                    if (
                      event.target instanceof Element &&
                      event.target.closest('a, button')
                    )
                      return
                    onToggle(item.key)
                  }}
                >
                  <TableCell>
                    <span className="block whitespace-nowrap font-mono text-sm font-medium text-ink">
                      {item.scenarioId}
                    </span>
                    {definition ? (
                      <span className={META}>definition {definition}</span>
                    ) : null}
                  </TableCell>
                  <TableCell>
                    <Badge
                      className="whitespace-nowrap"
                      variant={badgeVariantForStatus(item.objective.status)}
                    >
                      {item.objective.label.toLowerCase()}
                    </Badge>
                    {item.reason ? (
                      <span className={META}>{item.reason}</span>
                    ) : null}
                  </TableCell>
                  <TableCell className={NUMERIC}>
                    {item.aggregate?.mean_score == null
                      ? '—'
                      : Math.round(item.aggregate.mean_score)}
                  </TableCell>
                  <TableCell className={NUMERIC}>
                    {item.runCount || '—'}
                  </TableCell>
                  <TableCell className={NUMERIC}>
                    {item.durationMs === null
                      ? '—'
                      : formatScenarioDuration(item.durationMs)}
                  </TableCell>
                  <TableCell className={NUMERIC}>
                    {formatMetricCount(
                      item.aggregate?.total_tokens_consumed ?? null,
                    )}
                  </TableCell>
                  <TableCell className="text-right">
                    <Button
                      variant="icon"
                      size="icon"
                      type="button"
                      aria-expanded={open}
                      aria-label={`${open ? 'Hide' : 'Show'} the runs of ${item.scenarioId}`}
                      onClick={() => onToggle(item.key)}
                    >
                      {open ? (
                        <ChevronDown aria-hidden="true" />
                      ) : (
                        <ChevronRight aria-hidden="true" />
                      )}
                    </Button>
                  </TableCell>
                </TableRow>
                {open ? (
                  <TableRow data-scenario-detail={item.scenarioId}>
                    <TableCell
                      colSpan={RESULT_COLUMNS}
                      className="bg-card-highlight"
                    >
                      <div className="grid gap-4 py-2">
                        <RunRows
                          runs={itemRuns}
                          executionId={detail.id}
                          detail={detail}
                          onTranscript={onTranscript}
                        />
                        <SemanticTestFlow
                          detail={detailForScenario(detail, item)}
                        />
                      </div>
                    </TableCell>
                  </TableRow>
                ) : null}
              </TableBody>
            )
          })}
        </Table>
      </TableFrame>
    </TableViewport>
  )
}

function ResultFilters({
  items,
  filter,
  onChange,
}: {
  items: ScenarioMatrixItem[]
  filter: ResultFilter
  onChange: (next: ResultFilter) => void
}) {
  const counts = resultFilterCounts(items)
  if (counts.length < 2) return null
  const pill = (value: ResultFilter, label: string, count: number) => (
    <Button
      key={value}
      variant="pill"
      size="sm"
      type="button"
      aria-pressed={filter === value}
      className={filter === value ? 'bg-surface-selected text-ink' : undefined}
      onClick={() => onChange(value)}
    >
      {label}
      <span className={filter === value ? 'text-ink' : 'text-ink-faint'}>
        {count}
      </span>
    </Button>
  )
  return (
    <fieldset className="m-0 flex flex-wrap items-center gap-2 border-0 p-0">
      <legend className="sr-only">Filter tests by result</legend>
      {pill('all', 'all', items.length)}
      {counts.map(([status, count]) => pill(status, status, count))}
    </fieldset>
  )
}

function Provenance({
  detail,
  entries,
  contracts,
}: {
  detail: DashboardExecutionDetail
  entries: Array<[string, string]>
  contracts: ReturnType<typeof buildScenarioMatrix>['contracts']
}) {
  const [copied, setCopied] = useState(false)
  const raw = JSON.stringify(detail, null, 2)
  return (
    <CollapsibleCard className="mt-8" data-provenance>
      <CollapsibleCardTrigger>
        <span className="flex flex-wrap items-center justify-between gap-3 px-4 py-3">
          <span className="text-sm font-semibold text-ink">provenance</span>
          <span className="font-mono text-xs text-ink-faint">
            {entries.length} facts · {contracts.length} results contract
            {contracts.length === 1 ? '' : 's'}
          </span>
        </span>
      </CollapsibleCardTrigger>
      <CollapsibleCardContent>
        <div className="grid gap-4 px-4 pt-1 pb-4">
          {contracts.length > 0 ? (
            <ul className="m-0 grid list-none gap-1 p-0 font-mono text-xs">
              {contracts.map((contract) => (
                <li
                  key={contract.key}
                  className="flex flex-wrap items-center gap-2"
                >
                  <span className="text-ink">
                    results contract {contract.reportState ?? 'unavailable'} ·{' '}
                    {contract.objectiveOutcome ?? 'unavailable'}
                    {contract.resultContractSha256
                      ? ` · ${contract.resultContractSha256.replace(/^sha256:/, '').slice(0, 12)}`
                      : ''}
                  </span>
                  {contract.resultContractCurrent ? null : (
                    <Badge variant="warn">written under another contract</Badge>
                  )}
                </li>
              ))}
            </ul>
          ) : null}
          <dl className="m-0 grid min-w-0 grid-cols-[max-content_minmax(0,1fr)] gap-x-6 gap-y-2 font-mono text-xs">
            {entries.map(([key, value]) => (
              <div key={key} className="contents">
                <dt className="text-ink-faint">{key}</dt>
                <dd className="m-0 break-all text-ink">{value}</dd>
              </div>
            ))}
          </dl>
          <div>
            <Button
              variant="pill"
              size="sm"
              type="button"
              onClick={() => {
                void navigator.clipboard?.writeText(raw).then(() => {
                  setCopied(true)
                  window.setTimeout(() => setCopied(false), 1500)
                })
              }}
            >
              {copied ? 'copied' : 'copy json'}
            </Button>
          </div>
          <pre className="m-0 max-h-[480px] min-w-0 overflow-auto rounded-md bg-surface p-4 font-mono text-xs leading-5 text-ink-faint">
            {raw}
          </pre>
        </div>
      </CollapsibleCardContent>
    </CollapsibleCard>
  )
}

function Notice({
  variant,
  headline,
  detail,
}: {
  variant: 'warn' | 'alert'
  headline: string
  detail?: string | null
}) {
  return (
    <div className="mt-4" role="status">
      <StatusPanel
        variant={variant}
        headline={headline}
        detail={detail ?? undefined}
      />
    </div>
  )
}

function ImportedExecution({
  detail,
  bridge,
}: {
  detail: DashboardExecutionDetail
  bridge: DashboardDataBridge | null
}) {
  const reference = detail.remote_reference as unknown as RcReference
  const metrics = referencePrimaryMetrics(reference).metrics
  const [evidenceMessage, setEvidenceMessage] = useState<string | null>(null)
  const cards: ExecutionMetricCard[] = [
    {
      label: 'score',
      value:
        metrics.score.value === null
          ? '—'
          : String(Math.round(metrics.score.value)),
      detail: `${metrics.score.samples} of ${metrics.score.expected} runs scored`,
      tone: metrics.score.value === null ? 'unavailable' : 'neutral',
    },
    {
      label: 'runtime',
      value:
        metrics.durationMs.value === null
          ? '—'
          : formatDuration(metrics.durationMs.value / 1000),
      detail: `${metrics.durationMs.samples} of ${metrics.durationMs.expected} runs reported`,
      tone: metrics.durationMs.value === null ? 'unavailable' : 'neutral',
    },
    {
      label: 'tokens',
      value: formatMetricCount(metrics.totalTokens.value),
      detail: `${metrics.totalTokens.samples} of ${metrics.totalTokens.expected} runs reported`,
      tone: metrics.totalTokens.value === null ? 'unavailable' : 'neutral',
    },
    {
      label: 'reported cost',
      value: formatReportedCost(metrics.costUsd.value),
      detail: `${metrics.costUsd.samples} of ${metrics.costUsd.expected} runs reported`,
      tone: metrics.costUsd.value === null ? 'unavailable' : 'neutral',
    },
  ]
  const artifacts = retainedArtifacts(detail)
  const download = async (reportId: string, path: string) => {
    if (!bridge) return
    const result = await bridge.openEvidence({
      execution_id: detail.id,
      report_id: reportId,
      path,
    })
    if (result.availability !== 'available' || !result.content_base64) {
      setEvidenceMessage(result.reason ?? result.availability)
      return
    }
    const binary = Uint8Array.from(atob(result.content_base64), (char) =>
      char.charCodeAt(0),
    )
    const url = URL.createObjectURL(
      new Blob([binary], {
        type: result.mime_type ?? 'application/octet-stream',
      }),
    )
    const link = document.createElement('a')
    link.href = url
    link.download = path.split('/').at(-1) ?? 'evidence'
    link.click()
    URL.revokeObjectURL(url)
  }
  return (
    <div className="min-h-dvh bg-panel text-ink">
      <DashboardPageActions active="executions" />
      <div className={SHELL}>
        <PageHeader
          title={detail.label ?? 'imported execution'}
          summary="Historical evidence imported locally; metrics use the retained run ledger."
          breadcrumb={[
            { label: 'plans', href: hashForPlans() },
            ...(detail.plan_id
              ? [{ label: 'Plan', href: hashForPlan(detail.plan_id) }]
              : []),
          ]}
        />
        <div className="mt-6">
          <MetricStrip cards={cards} />
        </div>
        <section className="mt-8" aria-labelledby="imported-runs-title">
          <h2
            id="imported-runs-title"
            className="m-0 mb-3 text-sm font-semibold text-ink"
          >
            retained runs
          </h2>
          <TableViewport>
            <TableFrame>
              <Table density="compact">
                <TableCaption className="sr-only">
                  Historical run ledger
                </TableCaption>
                <TableHeader>
                  <TableRow>
                    <TableHead scope="col">test</TableHead>
                    <TableHead scope="col">attempt</TableHead>
                    <TableHead scope="col">status</TableHead>
                    <TableHead scope="col">evidence</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {reference.runs.map((run, index) => (
                    <TableRow key={run.id ?? `${run.scenarioId}-${index}`}>
                      <TableCell className="font-mono text-sm">
                        {run.scenarioId}
                      </TableCell>
                      <TableCell>{run.repetition ?? '—'}</TableCell>
                      <TableCell>{run.status ?? '—'}</TableCell>
                      <TableCell>
                        {run.attemptsComplete ? 'retained' : 'partial'}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </TableFrame>
          </TableViewport>
        </section>
        {artifacts.length > 0 ? (
          <section className="mt-8" aria-labelledby="imported-artifacts-title">
            <h2
              id="imported-artifacts-title"
              className="m-0 mb-3 text-sm font-semibold text-ink"
            >
              retained artifacts
            </h2>
            <div className="flex flex-wrap gap-2">
              {artifacts.map(({ reportId, path }) => (
                <Button
                  key={`${reportId}:${path}`}
                  variant="pill"
                  size="sm"
                  type="button"
                  onClick={() => void download(reportId, path)}
                >
                  {path}
                </Button>
              ))}
            </div>
            {evidenceMessage ? (
              <p className="mt-3 text-sm text-warn">{evidenceMessage}</p>
            ) : null}
          </section>
        ) : null}
      </div>
    </div>
  )
}

export function ExecutionPage({
  executionId,
  anchor,
  runId,
}: {
  executionId: string
  anchor?: string | null
  /** Evidence record open on top of the execution. */
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
  const [filter, setFilter] = useState<ResultFilter>('all')
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set())
  const [transcript, setTranscript] = useState<{
    run: AssessmentRunView
    title: string
  } | null>(null)
  const beginRequest = useLatestRequest()
  const loadedExecutionId = detail?.id

  useEffect(() => {
    if (!anchor || !loadedExecutionId) return
    window.requestAnimationFrame(() =>
      document
        .getElementById('execution-results')
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
    setExpanded(new Set())
    void load()
  }, [load])

  const presentation = useMemo(
    () => (summary ? buildExecutionPresentation(summary) : null),
    [summary],
  )
  const live =
    presentation?.attention === 'running' ||
    presentation?.attention === 'cancelling'

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

  if (error && !detail) {
    const missing = /not found|unknown|no such|invalid execution|404/i.test(
      error,
    )
    return (
      <div className="min-h-dvh bg-panel text-ink">
        <DashboardPageActions active="executions" />
        <div className={SHELL}>
          <EmptyState
            title={
              missing ? 'Execution not found' : 'Execution could not be loaded'
            }
            description={error}
            action={{
              label: 'retry',
              onClick: () => {
                setError(null)
                void load()
              },
            }}
          />
          <div className="mt-4 flex justify-center">
            <Button variant="pill" size="sm" asChild>
              <a href={hashForWorkspace('executions')}>back to executions</a>
            </Button>
          </div>
        </div>
      </div>
    )
  }

  if (!detail || !presentation) {
    return (
      <div className="min-h-dvh bg-panel text-ink">
        <DashboardPageActions active="executions" />
        <div className={SHELL} aria-busy="true" role="status">
          <span className="sr-only">Loading execution report</span>
          <div className="grid gap-4">
            <Skeleton className="block h-12 w-72" />
            <Skeleton className="block h-24 w-full" />
            <Skeleton className="block h-40 w-full" />
          </div>
        </div>
      </div>
    )
  }

  if (detail.origin === 'remote' && detail.remote_reference) {
    return <ImportedExecution detail={detail} bridge={bridge} />
  }

  const status = executionStatus(presentation)
  const scenarioSummary = scenarioMatrix?.summary ?? null
  const items = scenarioMatrix?.items ?? []
  const verdict = executionVerdict(presentation, scenarioSummary, items)
  const runCount = runCountFromDetail(detail)
  const hasReport = presentation.available && (scenarioSummary?.total ?? 0) > 0
  const { title } = executionTitle(presentation)
  const ready = Boolean(bridge)
  const evidenceRun = runId
    ? (assessmentModel.runs.find((run) => run.runId === runId) ?? null)
    : null
  const liveCopy = liveStateCopy(
    presentation,
    Boolean(detail.live_progress || detail.plan_execution),
  )
  const visibleItems = filterResults(items, filter)

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
    if (!bridge) return
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
  const toggle = (key: string) =>
    setExpanded((current) => {
      const next = new Set(current)
      if (next.has(key)) next.delete(key)
      else next.add(key)
      return next
    })

  const actions = ready ? (
    <>
      <Button
        variant="pill"
        size="sm"
        type="button"
        onClick={() => {
          void navigator.clipboard?.writeText(window.location.href).then(() => {
            setCopied(true)
            window.setTimeout(() => setCopied(false), 1500)
          })
        }}
      >
        <Link2 aria-hidden="true" />
        {copied ? 'link copied' : 'copy link'}
      </Button>
      {!live && !detail.plan_execution ? (
        <Button
          variant="ghost"
          size="sm"
          type="button"
          onClick={() => setDeleteOpen(true)}
        >
          <Trash2 aria-hidden="true" />
          delete
        </Button>
      ) : null}
      {live && detail.plan_id ? (
        <Button variant="pill" size="sm" asChild>
          <a href={hashForPlan(detail.plan_id)}>back to plan</a>
        </Button>
      ) : null}
      {live ? (
        <Button
          variant="primary"
          size="sm"
          type="button"
          onClick={() => void cancelRun()}
          disabled={cancelling}
          aria-busy={cancelling}
        >
          {cancelling ? 'cancelling…' : 'cancel execution'}
        </Button>
      ) : (
        <Button variant="primary" size="sm" asChild>
          <a
            href={
              detail.plan_id ? hashForPlan(detail.plan_id) : hashForWorkspace()
            }
            onClick={() =>
              !detail.plan_id &&
              requestQuickExecution(items.map((item) => item.scenarioId))
            }
          >
            {detail.plan_id ? 'back to plan' : 're-run same scope'}
          </a>
        </Button>
      )}
    </>
  ) : null

  return (
    <div className="min-h-dvh bg-panel text-ink">
      <DashboardPageActions
        active="executions"
        context={title}
        actionsLabel="Execution actions"
        actions={actions}
      />
      <div className={SHELL}>
        <PageHeader
          title={title}
          summary={executionSummarySentence({
            detail,
            live,
            scenarioSummary,
            runCount,
          })}
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
            <Badge variant={badgeVariantForStatus(status.status)}>
              {status.label}
            </Badge>
          }
        />
        <IdentityBand entries={identityEntries(detail, presentation)} />

        {detail.evidence_error ? (
          <>
            <Notice
              variant="warn"
              headline="Evidence bundle unavailable"
              detail={detail.evidence_error}
            />
            <div className="mt-4">
              <MetricStrip cards={snapshotMetricCards(detail)} />
            </div>
          </>
        ) : null}
        {error ? (
          <Notice
            variant="warn"
            headline="Refresh failed; showing the last received snapshot"
            detail={`Automatic updates will retry. ${error}`}
          />
        ) : null}
        {detail.live_progress_error ? (
          <Notice
            variant="warn"
            headline="Live progress unavailable"
            detail={detail.live_progress_error}
          />
        ) : null}
        {detail.persistence_errors?.length ? (
          <Notice
            variant="warn"
            headline="Partial result: completed runs were preserved, persistence failed"
            detail={detail.persistence_errors.join(' · ')}
          />
        ) : null}

        {!detail.evidence_error &&
        (live || !hasReport) &&
        !(detail.plan_execution && live) ? (
          <div className="mt-5" data-live-state={presentation.attention}>
            <StatusPanel
              variant={live ? 'info' : 'warn'}
              headline={liveCopy.headline}
              detail={liveCopy.note}
            />
          </div>
        ) : null}
        {detail.plan_execution && live ? (
          <PlanProgress execution={detail.plan_execution} />
        ) : null}
        {detail.live_progress ? (
          <LiveProgressPanel progress={detail.live_progress} running={live} />
        ) : null}

        {hasReport && !live ? (
          <>
            <div className="mt-5" data-verdict>
              <StatusPanel
                variant={verdictVariant(scenarioSummary)}
                headline={verdict.headline}
                detail={verdict.nextStep}
              />
            </div>
            <div className="mt-4">
              <MetricStrip
                cards={executionMetricCards(detail, scenarioSummary)}
              />
            </div>
            <section
              id="execution-results"
              className="mt-8 grid gap-3"
              aria-labelledby="execution-results-title"
            >
              <div className="flex flex-wrap items-center justify-between gap-3">
                <h2
                  id="execution-results-title"
                  className="m-0 text-sm font-semibold text-ink"
                >
                  results
                </h2>
                <ResultFilters
                  items={items}
                  filter={filter}
                  onChange={setFilter}
                />
              </div>
              {visibleItems.length === 0 ? (
                <EmptyState
                  title="No test matches this filter"
                  description="Every retained test is hidden by the result filter."
                  action={{
                    label: 'show all',
                    onClick: () => setFilter('all'),
                  }}
                />
              ) : (
                <ResultsTable
                  items={visibleItems}
                  runs={assessmentModel.runs}
                  detail={detail}
                  expanded={expanded}
                  onToggle={toggle}
                  onTranscript={(run, runTitle) =>
                    setTranscript({ run, title: runTitle })
                  }
                />
              )}
            </section>
            <Provenance
              detail={detail}
              entries={provenanceEntries(detail, presentation)}
              contracts={scenarioMatrix?.contracts ?? []}
            />
          </>
        ) : null}
      </div>
      <ConfirmDialog
        open={deleteOpen}
        onOpenChange={(open) => !deleting && setDeleteOpen(open)}
        title="Delete execution?"
        description="This permanently removes the execution and its retained evidence from the Console."
        confirmLabel={deleting ? 'deleting…' : 'delete execution'}
        cancelLabel="cancel"
        onConfirm={() => void deleteExecution()}
        onCancel={() => setDeleteOpen(false)}
      />
      {evidenceRun ? (
        <AssessmentDetailDialog
          run={evidenceRun}
          detail={detail}
          onClose={() => {
            window.location.hash = hashForExecution(detail.id, 'results')
          }}
        />
      ) : null}
      {transcript ? (
        <TranscriptDialog
          title={transcript.title}
          messages={transcript.run.transcript?.messages}
          open
          onClose={() => setTranscript(null)}
        />
      ) : null}
    </div>
  )
}
