import { ArrowRight } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  DashboardPageActions,
  dashboardHeaderActionClassName,
} from '@/components/DashboardPageActions'
import { consumeQuickExecutionRequest } from '@/components/ExecutionSetup'
import { LocalRunnerDialog } from '@/components/LocalRunnerDialog'
import {
  buttonClassName,
  Callout,
  EmptyState,
  MetricCard,
  type MetricTone,
  PageHeader,
  Panel,
  StatusBadge,
} from '@/design-system'
import {
  hashForExecution,
  hashForNewPlan,
  hashForWorkspace,
} from '@/hooks/use-hash-route'
import { useLatestRequest } from '@/hooks/use-latest-request'
import {
  type DashboardDataBridge,
  type DashboardExecutionSummary,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import {
  categoryLabel,
  categoryMessage,
  type ExecutionPresentation,
  formatDate,
  formatDuration,
  formatPercent,
} from '@/lib/execution-view'
import {
  buildOverviewSignal,
  executionTitle,
  type SignalMetric,
} from '@/lib/overview-signal'
import '@/design-system/styles.css'

/** Executions the Overview asks for; the ledger pages beyond this. */
const OVERVIEW_LIMIT = 20

export function statusCopy(presentation: ExecutionPresentation) {
  if (presentation.attention === 'passed')
    return { label: 'passed', status: 'passed' as const }
  if (presentation.attention === 'running')
    return { label: 'running', status: 'running' as const }
  if (presentation.attention === 'cancelling')
    return { label: 'cancelling', status: 'cancelling' as const }
  if (presentation.attention === 'cancelled')
    return { label: 'cancelled', status: 'cancelled' as const }
  if (presentation.attention === 'incomplete')
    return { label: 'incomplete', status: 'incomplete' as const }
  if (presentation.attention === 'unavailable')
    return { label: 'no report', status: 'unavailable' as const }
  if (presentation.breakdown.hard_gate > 0)
    return { label: 'hard gate', status: 'hard_gate' as const }
  if (
    presentation.breakdown.inconclusive > 0 &&
    presentation.breakdown.issues === presentation.breakdown.inconclusive
  )
    return { label: 'inconclusive', status: 'inconclusive' as const }
  return { label: 'failed', status: 'failed' as const }
}

export function modelNames(models: ExecutionPresentation['subjects']) {
  if (models.length === 0) return 'not reported'
  return models.map((model) => `${model.provider}/${model.model}`).join(', ')
}

function metricTone(presentation: ExecutionPresentation | null): MetricTone {
  if (!presentation) return 'unavailable'
  if (presentation.attention === 'passed') return 'positive'
  if (presentation.attention === 'needs_attention') return 'negative'
  if (presentation.attention === 'unavailable') return 'unavailable'
  return 'warning'
}

/** Audit O-16: the delta reads as a signed change, the caption as the median. */
export function trendDelta(
  metric: SignalMetric,
  format: (value: number) => string,
): string | undefined {
  if (metric.delta === null || metric.delta === 0) return undefined
  const sign = metric.delta > 0 ? '+' : '−'
  return `${sign}${format(Math.abs(metric.delta))} vs prev`
}

export function trendCaption(
  metric: SignalMetric,
  format: (value: number) => string,
  fallback: string,
) {
  if (metric.median === null || metric.sampleSize === 0) return fallback
  return `median of last ${metric.sampleSize}: ${format(metric.median)}`
}

function tokensLabel(value: number | null | undefined) {
  return typeof value === 'number' && Number.isFinite(value)
    ? Math.round(value).toLocaleString()
    : '—'
}

function finiteNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

/**
 * Workflow executions report step progress; the plain ones report how many
 * scenario reports arrived. The second tile shows whichever exists so the
 * headline never repeats a number the first tile already gave.
 */
export function workflowProgress(presentation: ExecutionPresentation) {
  const workflow = presentation.execution.workflow_metrics
  const steps = finiteNumber(workflow?.step_count) ?? 0
  if (steps === 0) return null
  const succeeded = finiteNumber(workflow?.succeeded_steps) ?? 0
  const skipped = finiteNumber(workflow?.skipped_steps) ?? 0
  const active =
    (finiteNumber(workflow?.running_steps) ?? 0) +
    (finiteNumber(workflow?.pending_steps) ?? 0)
  const attention =
    (finiteNumber(workflow?.failed_steps) ?? 0) +
    (finiteNumber(workflow?.hard_gate_failed_steps) ?? 0) +
    (finiteNumber(workflow?.cancelled_steps) ?? 0)
  const gates = finiteNumber(workflow?.hard_gate_count) ?? 0
  const passedGates = finiteNumber(workflow?.passed_hard_gate_count) ?? 0
  const assets = finiteNumber(workflow?.asset_count) ?? 0
  const evaluations = finiteNumber(workflow?.evaluation_count) ?? 0
  const durationMs = finiteNumber(workflow?.duration_ms)
  const complete = succeeded + skipped === steps
  return {
    steps,
    succeeded,
    value: `${succeeded}/${steps}`,
    detail:
      gates > 0
        ? `${passedGates}/${gates} hard gates passed`
        : `${assets} assets · ${evaluations} evaluations`,
    delta:
      attention > 0
        ? 'needs review'
        : active > 0
          ? 'in progress'
          : complete
            ? 'complete'
            : 'incomplete',
    tone: (attention > 0
      ? 'negative'
      : active > 0
        ? 'warning'
        : complete
          ? 'positive'
          : 'unavailable') as MetricTone,
    runtimeSeconds:
      durationMs === null
        ? presentation.workflowRuntimeSeconds
        : durationMs / 1000,
    tokens: finiteNumber(workflow?.total_tokens),
  }
}

/* ------------------------------------------------------------------ bands */

function LatestExecutionBand({
  presentation,
}: {
  presentation: ExecutionPresentation
}) {
  const status = statusCopy(presentation)
  const { title, detail } = executionTitle(presentation)
  const execution = presentation.execution
  const scope =
    presentation.expectedReports !== null &&
    presentation.receivedReports !== null
      ? `${presentation.receivedReports}/${presentation.expectedReports} reports`
      : null
  return (
    <Panel aria-labelledby="latest-execution-title" data-latest-execution>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="ds-label m-0">
            latest execution · {formatDate(presentation.completedAt)}
          </p>
          <div className="mt-1 flex min-w-0 flex-wrap items-center gap-2.5">
            <h2
              className="m-0 font-mono text-sm leading-5 font-semibold tracking-[-0.01em] text-ink"
              id="latest-execution-title"
            >
              {title}
            </h2>
            <StatusBadge status={status.status} label={status.label} />
          </div>
          <p className="mt-1 mb-0 font-mono text-label text-ink-soft">
            {[
              detail,
              modelNames(presentation.subjects),
              presentation.judges.length > 0
                ? `judge ${modelNames(presentation.judges)}`
                : null,
              scope,
            ]
              .filter(Boolean)
              .join(' · ')}
          </p>
        </div>
        <a
          className={buttonClassName({
            variant: presentation.primaryIssue ? 'primary' : 'secondary',
            className: 'no-underline',
          })}
          href={hashForExecution(execution.id)}
        >
          {presentation.primaryIssue ? 'investigate' : 'open execution'}
          <ArrowRight size={14} aria-hidden="true" />
        </a>
      </div>
    </Panel>
  )
}

function SignalMetrics({
  presentation,
  signal,
}: {
  presentation: ExecutionPresentation
  signal: ReturnType<typeof buildOverviewSignal>
}) {
  const workflow = workflowProgress(presentation)
  const totalTokens = workflow
    ? workflow.tokens
    : (presentation.execution.totals?.total_tokens as number | undefined)
  const runtimeSeconds = workflow
    ? workflow.runtimeSeconds
    : presentation.modelRuntimeSeconds
  return (
    <div
      className="grid gap-3 @[560px]:grid-cols-2 @[960px]:grid-cols-4"
      data-signal-metrics
    >
      <MetricCard
        label="scenario pass rate"
        value={formatPercent(presentation.passRate)}
        detail={
          presentation.breakdown.total > 0
            ? `${presentation.breakdown.passed + presentation.breakdown.passed_with_concerns} of ${presentation.breakdown.total} scenarios passed`
            : trendCaption(
                signal.passRate,
                (value) => `${Math.round(value)}%`,
                'objective scenario outcomes',
              )
        }
        tone={metricTone(presentation)}
        delta={trendDelta(
          signal.passRate,
          (value) => `${Math.round(value)} pts`,
        )}
      />
      {workflow ? (
        <MetricCard
          label="semantic steps"
          value={workflow.value}
          detail={workflow.detail}
          tone={workflow.tone}
          delta={workflow.delta}
        />
      ) : (
        <MetricCard
          label="report coverage"
          value={formatPercent(presentation.coverage)}
          detail={
            presentation.expectedReports !== null &&
            presentation.receivedReports !== null
              ? `${presentation.receivedReports} of ${presentation.expectedReports} reports received`
              : 'completeness was not published'
          }
          tone={
            presentation.coverage === null
              ? 'unavailable'
              : presentation.coverage >= 1
                ? 'positive'
                : 'warning'
          }
          delta={
            presentation.coverage !== null && presentation.coverage >= 1
              ? 'complete'
              : undefined
          }
        />
      )}
      <MetricCard
        label={workflow ? 'workflow runtime' : 'runtime'}
        value={formatDuration(runtimeSeconds)}
        detail={trendCaption(
          signal.runtime,
          (value) => formatDuration(value),
          'wall-clock time of the subject models',
        )}
        tone={runtimeSeconds === null ? 'unavailable' : 'neutral'}
      />
      <MetricCard
        label={workflow ? 'workflow tokens' : 'tokens'}
        value={tokensLabel(totalTokens)}
        detail={trendCaption(
          signal.tokens,
          (value) => tokensLabel(value),
          'subject + judge usage',
        )}
        tone={typeof totalTokens === 'number' ? 'neutral' : 'unavailable'}
      />
    </div>
  )
}

/** Audit O-06: every attention row carries the reason and its own action. */
function AttentionQueue({
  entries,
  total,
}: {
  entries: ReturnType<typeof buildOverviewSignal>['attention']
  total: number
}) {
  return (
    <Panel aria-labelledby="attention-title" data-attention-queue>
      <div className="mb-3 flex flex-wrap items-baseline justify-between gap-2">
        <div>
          <h2
            className="m-0 text-sm font-semibold text-ink"
            id="attention-title"
          >
            needs attention
          </h2>
          <p className="mt-1 mb-0 font-mono text-label text-ink-muted">
            {total} of the loaded executions {total === 1 ? 'is' : 'are'}{' '}
            blocked by gates or errors
            {total > entries.length ? ` · showing ${entries.length}` : ''}
          </p>
        </div>
      </div>
      <ul className="m-0 grid list-none gap-px overflow-hidden rounded-[6px] bg-line p-0">
        {entries.map(({ presentation, category, count }) => {
          const { title, detail } = executionTitle(presentation)
          const status = statusCopy(presentation)
          return (
            <li
              className="grid gap-1 bg-panel p-3 @[720px]:grid-cols-[10rem_minmax(0,1fr)_auto] @[720px]:items-center @[720px]:gap-3"
              key={presentation.execution.id}
              data-attention-row={category}
            >
              <StatusBadge
                status={status.status}
                label={categoryLabel(category as never).toLowerCase()}
              />
              <span className="min-w-0">
                <span
                  className="block truncate font-mono text-xs text-ink"
                  title={title}
                >
                  {title}
                </span>
                <span className="block truncate font-mono text-label text-ink-soft">
                  {categoryMessage(category as never, count)}
                  {detail ? ` · ${detail}` : ''}
                </span>
              </span>
              <span className="flex items-center gap-2 justify-self-start @[720px]:justify-self-end">
                <span className="font-mono text-label text-ink-muted">
                  {formatDate(presentation.completedAt)}
                </span>
                <a
                  className={buttonClassName({
                    variant: 'quiet',
                    size: 'compact',
                    className: 'no-underline',
                  })}
                  href={hashForExecution(presentation.execution.id)}
                >
                  open
                  <ArrowRight size={13} aria-hidden="true" />
                </a>
              </span>
            </li>
          )
        })}
      </ul>
    </Panel>
  )
}

/** Audit O-17: running executions get their own live strip. */
function RunningStrip({
  running,
  onCancel,
  cancelling,
}: {
  running: ExecutionPresentation[]
  onCancel?: () => void
  cancelling: boolean
}) {
  return (
    <Panel aria-labelledby="running-title" data-running-strip>
      <h2 className="ds-label m-0" id="running-title">
        running now
      </h2>
      {running.length === 0 ? (
        <p className="mt-2 mb-0 text-xs text-ink-soft">
          Nothing is running. The binary executes one experiment at a time.
        </p>
      ) : (
        <ul className="m-0 mt-2 grid list-none gap-2 p-0">
          {running.map((presentation) => {
            const { title } = executionTitle(presentation)
            const status = statusCopy(presentation)
            const scope =
              presentation.expectedReports !== null &&
              presentation.receivedReports !== null
                ? `${presentation.receivedReports} of ${presentation.expectedReports} scenarios`
                : null
            return (
              <li
                className="flex flex-wrap items-center gap-2"
                key={presentation.execution.id}
              >
                <StatusBadge status={status.status} label={status.label} />
                <a
                  className="min-w-0 truncate font-mono text-xs text-ink no-underline hover:underline"
                  href={hashForExecution(presentation.execution.id)}
                >
                  {title}
                </a>
                {scope ? (
                  <span className="font-mono text-label text-ink-muted">
                    {scope}
                  </span>
                ) : null}
                {onCancel && presentation.attention === 'running' ? (
                  <button
                    className={buttonClassName({
                      variant: 'quiet',
                      size: 'compact',
                      className: 'ms-auto',
                    })}
                    type="button"
                    onClick={onCancel}
                    disabled={cancelling}
                  >
                    {cancelling ? 'cancelling…' : 'cancel'}
                  </button>
                ) : null}
              </li>
            )
          })}
        </ul>
      )}
    </Panel>
  )
}

/** Audit O-01 / O-03: five compact rows, never the ledger's table. */
function RecentExecutions({
  presentations,
  total,
}: {
  presentations: ExecutionPresentation[]
  total: number
}) {
  return (
    <Panel aria-labelledby="recent-title" data-recent-executions>
      <div className="mb-3 flex flex-wrap items-baseline justify-between gap-2">
        <h2 className="m-0 text-sm font-semibold text-ink" id="recent-title">
          recent executions
        </h2>
        <a
          className="font-mono text-label text-ink-soft no-underline hover:text-ink hover:underline"
          href={hashForWorkspace('executions')}
        >
          view all executions →
        </a>
      </div>
      <ul className="m-0 grid list-none gap-px overflow-hidden rounded-[6px] bg-line p-0">
        {presentations.map((presentation) => {
          const { title, detail } = executionTitle(presentation)
          const status = statusCopy(presentation)
          return (
            <li className="bg-panel" key={presentation.execution.id}>
              <a
                className="grid gap-1 p-3 no-underline transition-colors hover:bg-[var(--surface-fill)] @[720px]:grid-cols-[minmax(0,1fr)_9rem_7rem_5rem] @[720px]:items-center @[720px]:gap-3"
                href={hashForExecution(presentation.execution.id)}
              >
                <span className="min-w-0">
                  <span
                    className="block truncate font-mono text-xs text-ink"
                    title={title}
                  >
                    {title}
                  </span>
                  <span className="block truncate font-mono text-label text-ink-muted">
                    {formatDate(presentation.completedAt)}
                    {detail ? ` · ${detail}` : ''}
                  </span>
                </span>
                <StatusBadge status={status.status} label={status.label} />
                <span className="truncate font-mono text-label text-ink-soft">
                  {presentation.subjects[0]?.model ?? '—'}
                </span>
                <span className="font-mono text-xs text-ink-soft @[720px]:text-right">
                  {formatPercent(presentation.passRate)}
                </span>
              </a>
            </li>
          )
        })}
      </ul>
      <p className="mt-2 mb-0 font-mono text-label text-ink-muted">
        last {presentations.length} of {total}
      </p>
    </Panel>
  )
}

/* ------------------------------------------------------------------- page */

export function OverviewPage() {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [executions, setExecutions] = useState<DashboardExecutionSummary[]>([])
  const [total, setTotal] = useState(0)
  const [catalog, setCatalog] = useState<{
    count: number
    revision: string
  } | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [runnerOpen, setRunnerOpen] = useState(false)
  const [runnerScope, setRunnerScope] = useState<string[]>([])
  const [cancelling, setCancelling] = useState(false)
  const beginRequest = useLatestRequest()

  const load = useCallback(async () => {
    const request = beginRequest()
    setError(null)
    try {
      const nextBridge = bridge ?? (await getDashboardDataBridge())
      if (!request.isCurrent()) return
      setBridge(nextBridge)
      const manifest = await nextBridge.listExecutions({
        limit: OVERVIEW_LIMIT,
      })
      if (!request.isCurrent()) return
      setExecutions(manifest.executions ?? [])
      setTotal(manifest.total ?? manifest.executions?.length ?? 0)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setLoading(false)
    }
  }, [beginRequest, bridge])

  useEffect(() => {
    void load()
  }, [load])

  // Audit O-17 / E-12: a running execution updates without a reload.
  useEffect(() => {
    if (!bridge) return
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
  }, [bridge, load])

  useEffect(() => {
    if (!bridge) return
    let cancelled = false
    void bridge
      .listTests({ limit: 100 })
      .then((response) => {
        if (cancelled) return
        setCatalog({
          count: response.total ?? response.rows?.length ?? 0,
          revision: response.revision ?? '',
        })
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [bridge])

  useEffect(() => {
    const requested = consumeQuickExecutionRequest()
    if (requested) {
      setRunnerScope(requested)
      setRunnerOpen(true)
    }
  }, [])

  const signal = useMemo(() => buildOverviewSignal(executions), [executions])
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

  const summary =
    executions.length === 0
      ? 'no executions retained yet'
      : `signal from the last ${executions.length} execution${executions.length === 1 ? '' : 's'}${
          total > executions.length ? ` of ${total}` : ''
        }`

  return (
    <div className="ds-root min-h-dvh bg-canvas text-ink">
      <DashboardPageActions
        active="overview"
        actionsLabel="Overview actions"
        actions={
          local ? (
            <>
              <a
                className={dashboardHeaderActionClassName()}
                href={hashForNewPlan()}
              >
                new plan
              </a>
              <button
                className={dashboardHeaderActionClassName({ primary: true })}
                type="button"
                onClick={() => setRunnerOpen(true)}
              >
                run suite
              </button>
            </>
          ) : null
        }
      />
      <div className="page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-16 md:w-[calc(100%_-_3rem)]">
        <PageHeader
          title="overview"
          summary={
            loading && executions.length === 0 ? 'loading the ledger…' : summary
          }
          headingId="overview-title"
          actions={
            catalog ? (
              <a
                className="font-mono text-label text-ink-soft no-underline hover:text-ink hover:underline"
                href={hashForWorkspace('tests')}
                title={catalog.revision || undefined}
              >
                {catalog.count} tests · browse →
              </a>
            ) : null
          }
        />

        {error ? (
          <Callout
            tone="danger"
            title="Dashboard data unavailable"
            className="mt-6"
          >
            <span className="flex flex-wrap items-center justify-between gap-3">
              {error}
              <button
                className={buttonClassName({
                  variant: 'secondary',
                  size: 'compact',
                })}
                type="button"
                onClick={() => void load()}
              >
                retry
              </button>
            </span>
          </Callout>
        ) : null}

        {!error && loading && executions.length === 0 ? (
          <div className="mt-6 grid gap-3" aria-busy="true" role="status">
            <span className="ds-visually-hidden">
              Loading execution evidence
            </span>
            {Array.from({ length: 3 }, (_, index) => (
              <div
                // biome-ignore lint/suspicious/noArrayIndexKey: static placeholders
                key={index}
                className="h-24 animate-pulse rounded-[6px] bg-[var(--surface-fill)] motion-reduce:animate-none"
              />
            ))}
          </div>
        ) : null}

        {!error && !loading && executions.length === 0 ? (
          <EmptyState
            className="mt-6"
            title="No executions yet"
            description="Run the suite once to publish outcomes, retained evidence and efficiency for this stack. Results land here as soon as the first scenario finishes."
            actions={
              <>
                {local ? (
                  <button
                    className={buttonClassName({ variant: 'primary' })}
                    type="button"
                    onClick={() => setRunnerOpen(true)}
                  >
                    run suite
                  </button>
                ) : null}
                <a
                  className={buttonClassName({
                    variant: 'secondary',
                    className: 'no-underline',
                  })}
                  href={hashForWorkspace('tests')}
                >
                  browse tests
                </a>
              </>
            }
          />
        ) : null}

        {!error && executions.length > 0 ? (
          <div className="mt-6 grid gap-6" data-overview-signal>
            {signal.latest ? (
              <>
                <LatestExecutionBand presentation={signal.latest} />
                <SignalMetrics presentation={signal.latest} signal={signal} />
              </>
            ) : null}
            {signal.running.length > 0 || local ? (
              <RunningStrip
                running={signal.running}
                cancelling={cancelling}
                onCancel={local ? () => void cancelRun() : undefined}
              />
            ) : null}
            {signal.attention.length > 0 ? (
              <AttentionQueue
                entries={signal.attention}
                total={signal.attentionTotal}
              />
            ) : null}
            <RecentExecutions
              presentations={signal.recent}
              total={total || executions.length}
            />
          </div>
        ) : null}
      </div>
      <LocalRunnerDialog
        bridge={bridge}
        open={runnerOpen}
        initialScenarios={runnerScope}
        onClose={() => setRunnerOpen(false)}
        onCompleted={() => void load()}
      />
    </div>
  )
}
