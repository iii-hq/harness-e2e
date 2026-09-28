import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from '@iii-dev/console-ui'
import { ChartLine, ChevronDown, Download, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState } from 'react'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { useDashboardChrome } from '@/components/DashboardShell'
import { GithubImportDialog } from '@/components/GithubImportDialog'
import { LocalRunnerDialog } from '@/components/LocalRunnerDialog'
import type { HeaderAction } from '@/components/shell/HeaderActions'
import { PointPanel } from '@/components/trends/PointPanel'
import { LargeChart, SmallChart } from '@/components/trends/TrendsChart'
import { ByTest, ExecutionsTable } from '@/components/trends/TrendsTables'
import { buttonClassName, EmptyState } from '@/design-system'
import {
  hashForExecution,
  hashForTrends,
  replaceRouteParams,
} from '@/hooks/use-hash-route'
import { useLatestRequest } from '@/hooks/use-latest-request'
import {
  type DashboardDataBridge,
  type ExecutionParameters,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import { buildExecutionPresentation } from '@/lib/execution-view'
import {
  ANY_STACK,
  changesAt,
  counted,
  emptyText,
  NOT_RECORDED_STACK,
  previousCounted,
  profileText,
  sameSeries,
  seriesModel,
  seriesWhere,
  stackNote,
  stackOptionText,
  summaryText,
  TREND_METRICS,
  type TrendMetricId,
  type TrendPoint,
  type TrendSeriesKey,
  type TrendsRequest,
  type TrendsResponse,
  trendMetric,
  trendsParams,
} from '@/lib/trends'
import { rerunParameters } from '@/pages/ExecutionPage'
import '@/design-system/styles.css'
import './executions-page.css'
import './trends.css'

/* ---------------------------------------------------------------- state */

/** The request's series, when it names one whole. */
export function requestSeries(request: TrendsRequest): TrendSeriesKey | null {
  return request.suite && request.provider && request.model
    ? {
        suite: request.suite,
        provider: request.provider,
        model: request.model,
        profile: request.profile || null,
      }
    : null
}

/** The hash's params for a request: nothing for the default view. */
export function requestParams(request: TrendsRequest) {
  return trendsParams(requestSeries(request), request.stack ?? null)
}

const seriesId = (key: TrendSeriesKey) =>
  JSON.stringify([key.suite, key.provider, key.model, key.profile ?? null])

/** What each stack option holds, under its name. */
export function stackSub(name: string) {
  if (name === ANY_STACK) return 'every execution of the series'
  if (name === NOT_RECORDED_STACK)
    return 'executions whose stack was not recorded'
  return 'and older runs with its workers'
}

/** The stack control's value: a series only ever run here reads "this
 *  harness". */
export function stackLabel(stack: string, points: TrendPoint[]) {
  return stack === NOT_RECORDED_STACK &&
    points.length > 0 &&
    points.every((point) => point.source.kind === 'local')
    ? 'this harness'
    : stackOptionText(stack)
}

/* ------------------------------------------------------------- controls */

function SeriesMenu({
  data,
  narrow,
  onPick,
}: {
  data: TrendsResponse
  narrow: boolean
  onPick: (key: TrendSeriesKey) => void
}) {
  const current =
    data.series.find((series) => sameSeries(series, data.selected)) ?? null
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="tr-control"
          data-series-picker
          data-narrow={narrow || undefined}
        >
          <span className="tr-faint-ink">Suite</span>
          <span className="tr-strong tr-ellipsis">
            {current?.suite_label ?? '—'}
          </span>
          <span className="tr-ghost" aria-hidden="true">
            ·
          </span>
          <span className="tr-faint-ink">Model</span>
          <span className="tr-mono tr-small tr-ellipsis">
            {current ? seriesModel(current) : '—'}
          </span>
          <span className="tr-ghost" aria-hidden="true">
            ·
          </span>
          <span className="tr-faint-ink">Profile</span>
          <span className="tr-mono tr-small">
            {current ? profileText(current.profile) : '—'}
          </span>
          <ChevronDown size={16} aria-hidden="true" className="tr-faint-ink" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        aria-label="Series"
        className="tr-menu tr-series-menu"
      >
        <DropdownMenuRadioGroup
          value={current ? seriesId(current) : ''}
          onValueChange={(value) => {
            const picked = data.series.find(
              (series) => seriesId(series) === value,
            )
            if (picked) onPick(picked)
          }}
        >
          {data.series.map((series) => (
            <DropdownMenuRadioItem
              key={seriesId(series)}
              value={seriesId(series)}
              className="tr-menu-item"
            >
              <span className="tr-menu-text">
                <span className="tr-strong tr-ellipsis">
                  {series.suite_label} · {seriesModel(series)} ·{' '}
                  {profileText(series.profile)}
                </span>
                <span className="tr-faint tr-ellipsis">
                  {seriesWhere(series)}
                </span>
              </span>
              <span className="tr-mono tr-faint">
                {series.executions === 1
                  ? '1 execution'
                  : `${series.executions} executions`}
              </span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <p className="tr-menu-note">
          A series is every execution of one suite on one model and profile,
          wherever it ran. Stack changes stay inside the series and show as
          diamonds.
        </p>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

function StackMenu({
  data,
  onPick,
}: {
  data: TrendsResponse
  onPick: (stack: string) => void
}) {
  // One stack and "any" hold the same executions: nothing to choose.
  const fixed = data.stacks.length <= 2
  const label = stackLabel(data.stack, data.points)
  const trigger = (
    <button
      type="button"
      className="tr-control"
      disabled={fixed}
      data-stack-picker
    >
      <span className="tr-faint-ink">Stack</span>
      <span className="tr-mono tr-small">{label}</span>
      {fixed ? null : (
        <ChevronDown size={16} aria-hidden="true" className="tr-faint-ink" />
      )}
    </button>
  )
  if (fixed) return trigger
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>{trigger}</DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        aria-label="Stack"
        className="tr-menu tr-stack-menu"
      >
        <DropdownMenuRadioGroup value={data.stack} onValueChange={onPick}>
          {data.stacks.map((stack) => (
            <DropdownMenuRadioItem
              key={stack.name}
              value={stack.name}
              className="tr-menu-item"
            >
              <span className="tr-menu-text">
                <span className="tr-mono tr-small">
                  {stackOptionText(stack.name)}
                </span>
                <span className="tr-faint">{stackSub(stack.name)}</span>
              </span>
              <span className="tr-mono tr-faint">{stack.executions}</span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

function Legend() {
  return (
    <ul className="tr-legend" aria-label="Legend">
      <li>
        <span className="tr-legend-dot" aria-hidden="true" />
        execution
      </li>
      <li>
        <X size={12} aria-hidden="true" className="tr-danger" />
        no counted run
      </li>
      <li>
        <span className="tr-change-dot" data-major="true" aria-hidden="true" />
        iii, Harness, runner, tests or stack changed
      </li>
      <li>
        <span className="tr-change-dot" data-major="false" aria-hidden="true" />
        another worker or a test definition changed
      </li>
    </ul>
  )
}

/* ----------------------------------------------------------------- page */

type Runner = { parameters: ExecutionParameters | null; label: string }

export function TrendsPage({ request }: { request: TrendsRequest }) {
  const narrow = useDashboardChrome()?.narrow ?? false
  const [query, setQuery] = useState<TrendsRequest>(request)
  const [data, setData] = useState<TrendsResponse | null>(null)
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  // The picked execution, by id, so a reload keeps it while it is listed.
  const [picked, setPicked] = useState<string | null>(null)
  const [focus, setFocus] = useState<TrendMetricId>('score')
  const [runner, setRunner] = useState<Runner | null>(null)
  const [runnerOpen, setRunnerOpen] = useState(false)
  const [importOpen, setImportOpen] = useState(false)
  const [rerunError, setRerunError] = useState<string | null>(null)
  const beginRequest = useLatestRequest()

  const load = useCallback(async () => {
    const pending = beginRequest()
    setLoading(true)
    try {
      const next = await getDashboardDataBridge()
      if (!pending.isCurrent()) return
      setBridge(next)
      const answer = await next.getTrends(query)
      if (!pending.isCurrent()) return
      setData(answer)
      setError(null)
    } catch (cause) {
      if (pending.isCurrent())
        setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      if (pending.isCurrent()) setLoading(false)
    }
  }, [beginRequest, query])

  useEffect(() => {
    setPicked(null)
    void load()
  }, [load])

  // The trend follows new executions quietly, as the executions list does.
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
    replaceRouteParams(requestParams(query))
  }, [query])

  const points = data?.points ?? []
  const changes = useMemo(
    () => points.map((_, index) => changesAt(points, index)),
    [points],
  )
  const here = hashForTrends(requestParams(query))
  const selected = points.findIndex((item) => item.execution_id === picked)
  const pick = (index: number) => {
    const id = points[index]?.execution_id ?? null
    setPicked((current) => (current === id ? null : id))
  }

  const openRunner = useCallback((next: Runner) => {
    setRunner(next)
    setRunnerOpen(true)
  }, [])
  const headerActions = useMemo(
    (): HeaderAction[] | undefined =>
      bridge
        ? [
            {
              id: 'import',
              label: narrow ? 'Import' : 'Import from GitHub',
              icon: Download,
              onSelect: () => setImportOpen(true),
            },
            {
              id: 'run',
              label: 'Run tests',
              primary: true,
              onSelect: () => openRunner({ parameters: null, label: '' }),
            },
          ]
        : undefined,
    [bridge, narrow, openRunner],
  )

  // Run again: the latest execution's parameters, as the Executions list's
  // Run again opens them.
  const runAgain = async (point: TrendPoint) => {
    if (!bridge) return
    setRerunError(null)
    try {
      const detail = await bridge.getExecution(point.execution_id)
      openRunner({
        parameters: rerunParameters(
          detail,
          detail.reports.map((record) => record.scenario_id),
          buildExecutionPresentation(detail).subjects[0],
        ),
        label: detail.plan_execution?.label ?? detail.label ?? '',
      })
    } catch (cause) {
      setRerunError(cause instanceof Error ? cause.message : String(cause))
    }
  }

  // Another series starts over: its latest stack, the score in front.
  const pickSeries = (key: TrendSeriesKey) => {
    setFocus('score')
    setQuery({
      suite: key.suite,
      provider: key.provider,
      model: key.model,
      profile: key.profile,
    })
  }
  const pickStack = (stack: string) =>
    setQuery((current) => ({
      ...(requestSeries(current) ?? data?.selected ?? {}),
      stack,
    }))

  const failedFirstLoad = Boolean(error) && data === null
  const nothingCounted = data !== null && !points.some(counted)
  const latest = points.at(-1) ?? null
  const metric = trendMetric(focus)
  const note = stackNote(points)
  const point = selected >= 0 ? points[selected] : undefined

  return (
    <div className="ds-root ex-page tr-page">
      <DashboardPageActions
        active="trends"
        actionsLabel="Trends actions"
        actions={headerActions}
      />
      <header className="ex-header">
        <h1 id="trends-title">Trends</h1>
        <p>
          How one suite moves over time on one model and profile. Each point is
          an execution; a diamond above the chart marks what changed since the
          execution before it.
        </p>
      </header>

      {failedFirstLoad ? (
        <EmptyState
          tone="error"
          title="The trend could not be loaded"
          description={error}
          actions={
            <button
              type="button"
              className={buttonClassName({ variant: 'secondary' })}
              onClick={() => void load()}
            >
              Try again
            </button>
          }
        />
      ) : data === null ? (
        <div className="tr-skeleton" role="status" aria-busy="true">
          <span className="ds-visually-hidden">Loading the trend</span>
        </div>
      ) : (
        <>
          {data.series.length > 0 ? (
            <>
              <div role="toolbar" aria-label="Series" className="tr-toolbar">
                <SeriesMenu data={data} narrow={narrow} onPick={pickSeries} />
                <StackMenu data={data} onPick={pickStack} />
                <span className="tr-spacer" />
                <Legend />
              </div>
              <div className="tr-summary" aria-busy={loading || undefined}>
                <p className="tr-faint tr-num-text" data-trend-summary>
                  {summaryText(points)}
                </p>
                {note ? <p className="tr-faint">{note}</p> : null}
                {error ? (
                  <p className="tr-faint" role="alert">
                    Could not reload: {error}
                  </p>
                ) : null}
              </div>
            </>
          ) : null}

          {data.series.length === 0 ? (
            <EmptyState
              icon={<ChartLine size={24} />}
              title="Nothing to draw yet"
              description="No execution has a suite and a model yet, so there is no series to follow. Run tests to start one."
              actions={
                bridge ? (
                  <button
                    type="button"
                    className={buttonClassName({ variant: 'primary' })}
                    onClick={() => openRunner({ parameters: null, label: '' })}
                  >
                    Run tests
                  </button>
                ) : null
              }
            />
          ) : nothingCounted ? (
            <EmptyState
              className="tr-empty"
              icon={<ChartLine size={24} />}
              title="Nothing to draw yet"
              description={
                <>
                  {emptyText(points)}
                  {rerunError ? (
                    <span className="tr-empty-error" role="alert">
                      Could not open Run again: {rerunError}
                    </span>
                  ) : null}
                </>
              }
              actions={
                latest ? (
                  <>
                    <a
                      className={buttonClassName({
                        variant: 'secondary',
                        className: 'no-underline',
                      })}
                      href={hashForExecution(latest.execution_id)}
                    >
                      Open the execution
                    </a>
                    {bridge ? (
                      <button
                        type="button"
                        className={buttonClassName({ variant: 'primary' })}
                        onClick={() => void runAgain(latest)}
                      >
                        Run again
                      </button>
                    ) : null}
                  </>
                ) : null
              }
            />
          ) : (
            <>
              <div
                className="tr-top"
                data-panel={(point && !narrow) || undefined}
              >
                <LargeChart
                  metric={metric}
                  points={points}
                  changes={changes}
                  selected={selected}
                  narrow={narrow}
                  onPick={pick}
                />
                {point ? (
                  <PointPanel
                    key={point.execution_id}
                    points={points}
                    index={selected}
                    changes={changes[selected]}
                    previous={previousCounted(points, selected)}
                    bridge={bridge}
                    here={here}
                    onClose={() => setPicked(null)}
                  />
                ) : null}
              </div>
              <section
                className="tr-minis"
                aria-label="Other measures"
                data-narrow={narrow || undefined}
              >
                {TREND_METRICS.filter((item) => item.id !== metric.id).map(
                  (item) => (
                    <SmallChart
                      key={item.id}
                      metric={item}
                      points={points}
                      changes={changes}
                      selected={selected}
                      narrow={narrow}
                      onFocus={() => setFocus(item.id)}
                    />
                  ),
                )}
              </section>
              <ByTest
                points={points}
                changes={changes}
                selected={selected}
                narrow={narrow}
                onPick={pick}
              />
              <ExecutionsTable
                points={points}
                selected={selected}
                narrow={narrow}
              />
            </>
          )}
        </>
      )}
      <LocalRunnerDialog
        bridge={bridge}
        open={runnerOpen}
        parameters={runner?.parameters ?? null}
        label={runner?.label ?? ''}
        onClose={() => setRunnerOpen(false)}
      />
      <GithubImportDialog
        bridge={bridge}
        open={importOpen}
        onClose={() => setImportOpen(false)}
        onImported={() => void load()}
      />
    </div>
  )
}
