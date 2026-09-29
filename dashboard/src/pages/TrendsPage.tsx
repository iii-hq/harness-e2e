import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
  Input,
  Skeleton,
} from '@iii-dev/console-ui'
import { ChartLine, ChevronDown, Download, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { useDashboardChrome } from '@/components/DashboardShell'
import { GithubImportDialog } from '@/components/GithubImportDialog'
import { LocalRunnerDialog } from '@/components/LocalRunnerDialog'
import type { HeaderAction } from '@/components/shell/HeaderActions'
import { BaselineMenu } from '@/components/trends/BaselineMenu'
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
import { plural } from '@/lib/format'
import {
  ANY,
  ANY_STACK,
  baselineOf,
  changesAt,
  counted,
  customDays,
  emptyText,
  GROUP_CHOICES,
  groupPoints,
  modelChoices,
  NOT_RECORDED_STACK,
  PERIOD_CHOICES,
  periodBounds,
  periodError,
  periodLabel,
  periodPhrase,
  pointTime,
  previousCounted,
  profileChoices,
  profileText,
  seriesModel,
  stackNote,
  stackOptionText,
  suiteChoices,
  summaryText,
  TREND_METRICS,
  type TrendGroupBy,
  type TrendMetricId,
  type TrendPeriod,
  type TrendPoint,
  type TrendRange,
  type TrendSeriesKey,
  type TrendsRequest,
  type TrendsResponse,
  trendMetric,
  trendsParams,
  unversioned,
  withBase,
  withGroup,
  withPeriod,
} from '@/lib/trends'
import { rerunParameters } from '@/pages/ExecutionPage'
import { LedgerLoadFailure } from '@/pages/ExecutionsPage'
import '@/design-system/styles.css'
import './executions-page.css'
import './trends.css'

/* ---------------------------------------------------------------- state */

/** The view an answer shows: its series and the stack it applied, so a
 *  reload or a link stays on what is on screen even when another series
 *  has a newer execution. */
export function answeredView(
  asked: TrendsRequest,
  answer: TrendsResponse,
): TrendsRequest {
  return answer.selected ? { ...answer.selected, stack: answer.stack } : asked
}

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
  return trendsParams(request)
}

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

/** Whether the stack can be chosen: more than one stack besides "any", or
 *  a stack the series does not list, to get out of. */
export function stackChoosable(data: TrendsResponse) {
  return (
    data.stacks.length > 2 ||
    !data.stacks.some((stack) => stack.name === data.stack)
  )
}

/** The stack asked for is not what is shown: say so, and how to get out
 *  when the stack shown is not one of the series'. */
export function stackNotice(asked: TrendsRequest, data: TrendsResponse) {
  if (!data.stacks.some((stack) => stack.name === data.stack))
    return {
      text: `No execution of this series ran on ${stackOptionText(data.stack)}.`,
      anyStack: true,
    }
  if (asked.stack && asked.stack !== data.stack)
    return {
      text: `No execution of this series ran on ${stackOptionText(asked.stack)}, so this shows ${data.stack === ANY_STACK ? 'every stack' : stackOptionText(data.stack)}.`,
      anyStack: false,
    }
  return null
}

/* ------------------------------------------------------------- controls */

/** One of the three pickers: its value, and the choices that exist under
 *  the ones before it, each with its executions. */
function ChoiceMenu({
  name,
  value,
  text,
  mono = false,
  choices,
  note,
  onPick,
}: {
  name: 'Suite' | 'Model' | 'Profile'
  value: string
  text: string
  mono?: boolean
  choices: Array<{
    value: string
    label: string
    executions: number
    sub?: string
  }>
  note?: string
  onPick: (value: string) => void
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="tr-control"
          data-picker={name.toLowerCase()}
        >
          <span className="tr-faint-ink">{name}</span>
          <span
            className={
              mono ? 'tr-mono tr-small tr-ellipsis' : 'tr-strong tr-ellipsis'
            }
          >
            {text}
          </span>
          <ChevronDown size={16} aria-hidden="true" className="tr-faint-ink" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        aria-label={name}
        className="tr-menu tr-choice-menu"
      >
        <DropdownMenuRadioGroup value={value} onValueChange={onPick}>
          {choices.map((choice) => (
            <DropdownMenuRadioItem
              key={choice.value}
              value={choice.value}
              className="tr-menu-item"
            >
              <span className="tr-menu-text">
                <span className={mono ? 'tr-mono tr-small' : 'tr-strong'}>
                  {choice.label}
                </span>
                {choice.sub ? (
                  <span className="tr-faint">{choice.sub}</span>
                ) : null}
              </span>
              <span className="tr-mono tr-faint">
                {plural(choice.executions, 'execution')}
              </span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        {note ? <p className="tr-menu-note">{note}</p> : null}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

const modelId = (key: Pick<TrendSeriesKey, 'provider' | 'model'>) =>
  JSON.stringify([key.provider, key.model])

/** Suite, then the models that ran it, then the profiles that ran both;
 *  `any` model or profile puts all of them on one line. Picking one asks for
 *  it with the ones before it; the worker answers the latest series that
 *  fits. `any` stays picked across a suite or a model. */
function SeriesPickers({
  data,
  onPick,
}: {
  data: TrendsResponse
  onPick: (request: TrendsRequest) => void
}) {
  const current = data.selected
  if (!current) return null
  const suites = suiteChoices(data.series)
  const models = modelChoices(data.series, current.suite)
  const profiles = profileChoices(data.series, current)
  const anyModel = current.model === ANY
  const anyProfile = current.profile === ANY
  return (
    <>
      <ChoiceMenu
        name="Suite"
        value={current.suite}
        text={
          suites.find((choice) => choice.suite === current.suite)?.label ??
          current.suite
        }
        choices={suites.map((choice) => ({
          value: choice.suite,
          label: choice.label,
          executions: choice.executions,
        }))}
        note="A series is every execution of one suite on one model and profile, wherever it ran. Any model or profile puts them all on one line. Stack, model and profile changes show as diamonds."
        onPick={(suite) =>
          onPick({
            suite,
            ...(anyModel ? { model: ANY } : {}),
            ...(anyProfile ? { profile: ANY } : {}),
          })
        }
      />
      <ChoiceMenu
        name="Model"
        mono
        value={modelId(current)}
        text={seriesModel(current)}
        choices={models.map((choice) => ({
          value: modelId(choice),
          label: seriesModel(choice),
          executions: choice.executions,
          sub: choice.model === ANY ? 'every model, on one line' : undefined,
        }))}
        onPick={(value) => {
          const picked = models.find((choice) => modelId(choice) === value)
          if (!picked) return
          if (picked.model === ANY)
            onPick({
              suite: current.suite,
              model: ANY,
              profile: current.profile,
            })
          else
            onPick({
              suite: current.suite,
              provider: picked.provider,
              model: picked.model,
              ...(anyProfile ? { profile: ANY } : {}),
            })
        }}
      />
      <ChoiceMenu
        name="Profile"
        mono
        value={current.profile ?? ''}
        text={profileText(current.profile)}
        choices={profiles.map((choice) => ({
          value: choice.profile ?? '',
          label: profileText(choice.profile),
          executions: choice.executions,
          sub:
            choice.profile === ANY ? 'every profile, on one line' : undefined,
        }))}
        onPick={(profile) =>
          onPick({
            suite: current.suite,
            provider: current.provider,
            model: current.model,
            profile: profile || null,
          })
        }
      />
    </>
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
  const fixed = !stackChoosable(data)
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

/** The period: a range ending today, or Custom with two days. */
function PeriodMenu({
  period,
  onPick,
}: {
  period: TrendPeriod
  onPick: (value: string) => void
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button type="button" className="tr-control" data-picker="period">
          <span className="tr-faint-ink">Period</span>
          <span>{periodLabel(period)}</span>
          <ChevronDown size={16} aria-hidden="true" className="tr-faint-ink" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        aria-label="Period"
        className="tr-menu tr-period-menu"
      >
        <DropdownMenuRadioGroup
          value={'range' in period ? period.range : 'custom'}
          onValueChange={onPick}
        >
          {[...PERIOD_CHOICES, { value: 'custom', label: 'Custom' }].map(
            (choice) => (
              <DropdownMenuRadioItem
                key={choice.value}
                value={choice.value}
                className="tr-menu-item"
              >
                {choice.label}
              </DropdownMenuRadioItem>
            ),
          )}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/** One point per execution, per day or per Harness release. */
function GroupMenu({
  group,
  onPick,
}: {
  group: TrendGroupBy
  onPick: (group: TrendGroupBy) => void
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button type="button" className="tr-control" data-picker="group">
          <span className="tr-faint-ink">Group by</span>
          <span>
            {GROUP_CHOICES.find((choice) => choice.value === group)?.label}
          </span>
          <ChevronDown size={16} aria-hidden="true" className="tr-faint-ink" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        aria-label="Group by"
        className="tr-menu tr-stack-menu"
      >
        <DropdownMenuRadioGroup
          value={group}
          onValueChange={(value) => onPick(value as TrendGroupBy)}
        >
          {GROUP_CHOICES.map((choice) => (
            <DropdownMenuRadioItem
              key={choice.value}
              value={choice.value}
              className="tr-menu-item"
            >
              <span className="tr-menu-text">
                <span className="tr-strong">{choice.label}</span>
                <span className="tr-faint">{choice.sub}</span>
              </span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <p className="tr-menu-note">
          A day or a release reads its executions together: means weighted by
          counted runs, tests completed per execution.
        </p>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/** A custom period's two days; From after To is said beside them and asks
 *  for nothing. */
function CustomPeriod({
  since,
  until,
  onChange,
}: {
  since: string
  until: string
  onChange: (field: 'since' | 'until', value: string) => void
}) {
  const error = periodError(since, until)
  return (
    // biome-ignore lint/a11y/useSemanticElements: a labelled pair of fields
    <div className="tr-custom" role="group" aria-label="Custom period">
      <label className="tr-day" htmlFor="tr-since">
        <span className="tr-faint-ink">From</span>
        <Input
          id="tr-since"
          type="date"
          value={since}
          aria-invalid={error ? true : undefined}
          aria-describedby={error ? 'tr-period-error' : undefined}
          onChange={(value) => onChange('since', value)}
        />
      </label>
      <label className="tr-day" htmlFor="tr-until">
        <span className="tr-faint-ink">To</span>
        <Input
          id="tr-until"
          type="date"
          value={until}
          aria-invalid={error ? true : undefined}
          aria-describedby={error ? 'tr-period-error' : undefined}
          onChange={(value) => onChange('until', value)}
        />
      </label>
      {error ? (
        <p id="tr-period-error" className="tr-period-error" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  )
}

function Legend({ mixed, unit }: { mixed: boolean; unit: string }) {
  return (
    <ul className="tr-legend" aria-label="Legend">
      <li>
        <span className="tr-legend-dot" aria-hidden="true" />
        {unit}
      </li>
      <li>
        <X size={12} aria-hidden="true" className="tr-danger" />
        no counted run
      </li>
      <li>
        <span className="tr-change-dot" data-major="true" aria-hidden="true" />
        {mixed
          ? 'model, profile, iii, Harness, tests or stack changed'
          : 'iii, Harness, tests or stack changed'}
      </li>
      <li>
        <span className="tr-change-dot" data-major="false" aria-hidden="true" />
        another worker or a test definition changed
      </li>
    </ul>
  )
}

/** The page's shape while the trend loads: the controls, the summary, the
 *  large chart, the six small ones and a few rows. */
export function TrendsSkeleton({ narrow }: { narrow: boolean }) {
  return (
    <div className="tr-loading" role="status" aria-busy="true">
      <span className="ds-visually-hidden">Loading the trend</span>
      <div className="tr-toolbar">
        <Skeleton className="tr-skel tr-skel-series" />
        <Skeleton className="tr-skel tr-skel-stack" />
      </div>
      <Skeleton className="tr-skel tr-skel-line" />
      <Skeleton className="tr-skel tr-skel-big" />
      <div className="tr-minis" data-narrow={narrow || undefined}>
        {TREND_METRICS.slice(1).map((metric) => (
          <Skeleton key={metric.id} className="tr-skel tr-skel-mini" />
        ))}
      </div>
      {['first', 'second', 'third', 'fourth'].map((row) => (
        <Skeleton key={row} className="tr-skel tr-skel-row" />
      ))}
    </div>
  )
}

/* ----------------------------------------------------------------- page */

type Runner = { parameters: ExecutionParameters | null; label: string }

export function TrendsPage({
  request,
  period: routePeriod,
  base: routeBase,
  group: routeGroup = 'execution',
}: {
  request: TrendsRequest
  period: TrendPeriod
  base: string | null
  group?: TrendGroupBy
}) {
  const narrow = useDashboardChrome()?.narrow ?? false
  const [query, setQuery] = useState<TrendsRequest>(request)
  const [period, setPeriod] = useState<TrendPeriod>(routePeriod)
  // The execution pinned as the baseline; it lives in the hash.
  const [base, setBase] = useState<string | null>(routeBase)
  const [groupBy, setGroupBy] = useState<TrendGroupBy>(routeGroup)
  // The two days being typed for a custom period, until they make one.
  const [draft, setDraft] = useState<{ since: string; until: string } | null>(
    null,
  )
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
  // What a reload asks and the hash names: the request, then the view its
  // answer showed.
  const shown = useRef<TrendsRequest>(request)
  const [view, setView] = useState<TrendsRequest>(request)

  // The Trends tab or a link lands here again: start over from its request.
  useEffect(() => {
    setQuery(request)
    setPeriod(routePeriod)
    setBase(routeBase)
    setGroupBy(routeGroup)
    setDraft(null)
    setFocus('score')
  }, [request, routePeriod, routeBase, routeGroup])

  const load = useCallback(async () => {
    const pending = beginRequest()
    setLoading(true)
    try {
      const next = await getDashboardDataBridge()
      if (!pending.isCurrent()) return
      setBridge(next)
      const asked = shown.current
      const answer = await next.getTrends({
        ...asked,
        ...periodBounds(period),
      })
      if (!pending.isCurrent()) return
      shown.current = answeredView(asked, answer)
      setView(shown.current)
      setData(answer)
      setError(null)
    } catch (cause) {
      if (pending.isCurrent())
        setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      if (pending.isCurrent()) setLoading(false)
    }
  }, [beginRequest, period])

  // A new request starts over; a new period keeps the series and stack.
  const loaded = useRef<TrendsRequest | null>(null)
  useEffect(() => {
    if (loaded.current !== query) {
      loaded.current = query
      shown.current = query
      setView(query)
      setPicked(null)
    }
    void load()
  }, [load, query])

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

  // The hash names what is on screen, once the worker said what that is.
  const viewParams = withBase(
    withGroup(withPeriod(requestParams(view), period), groupBy),
    base,
  ).toString()
  useEffect(() => {
    replaceRouteParams(new URLSearchParams(viewParams))
  }, [viewParams])

  const points = data?.points ?? []
  // What the charts draw: each execution, or one point per day or release.
  const drawn = useMemo(() => groupPoints(points, groupBy), [points, groupBy])
  const changes = useMemo(
    () => drawn.map((_, index) => changesAt(drawn, index)),
    [drawn],
  )
  const here = hashForTrends(new URLSearchParams(viewParams))
  const selected = drawn.findIndex((item) => item.execution_id === picked)
  // Where the baseline is in this view, else why it is not used.
  const baseline = baselineOf(drawn, base)
  const pick = (index: number) => {
    const id = drawn[index]?.execution_id ?? null
    setPicked((current) => (current === id ? null : id))
  }
  const pickGroup = (next: TrendGroupBy) => {
    setPicked(null)
    setGroupBy(next)
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

  // Another series starts over: every stack, the score in front, no baseline.
  const pickSeries = (request: TrendsRequest) => {
    setFocus('score')
    setBase(null)
    setQuery(request)
  }
  const pickStack = (stack: string) =>
    setQuery({ ...(data?.selected ?? requestSeries(query) ?? {}), stack })

  const failedFirstLoad = Boolean(error) && data === null
  const noneInPeriod = data !== null && points.length === 0
  const nothingCounted = !noneInPeriod && !points.some(counted)
  const days =
    draft ??
    ('since' in period ? { since: period.since, until: period.until } : null)
  const pickPeriod = (value: string) => {
    if (value === 'custom') {
      const next = customDays(period)
      setDraft(next)
      setPeriod(next)
    } else {
      setDraft(null)
      setPeriod({ range: value as TrendRange })
    }
  }
  const typeDay = (field: 'since' | 'until', value: string) => {
    const next = { ...(days ?? customDays(period)), [field]: value }
    setDraft(next)
    if (periodError(next.since, next.until) === null) setPeriod(next)
  }
  const latest = points.at(-1) ?? null
  const metric = trendMetric(focus)
  const note = data ? stackNote(points, data.stack) : null
  const notice = data ? stackNotice(query, data) : null
  const empty = emptyText(points)
  const point = selected >= 0 ? drawn[selected] : undefined

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
          How one suite moves over time on one model and profile, or on all of
          them. Each point is an execution; a diamond above the chart marks what
          changed since the execution before it.
        </p>
      </header>

      {error && data ? (
        <LedgerLoadFailure
          what="trend"
          reload
          message={error}
          onRetry={() => void load()}
        />
      ) : null}

      {failedFirstLoad ? (
        <LedgerLoadFailure
          what="trend"
          reload={false}
          message={error ?? ''}
          onRetry={() => void load()}
        />
      ) : data === null ? (
        <TrendsSkeleton narrow={narrow} />
      ) : (
        <>
          {data.series.length > 0 ? (
            <>
              <div role="toolbar" aria-label="Series" className="tr-toolbar">
                <SeriesPickers data={data} onPick={pickSeries} />
                <StackMenu data={data} onPick={pickStack} />
                <PeriodMenu period={period} onPick={pickPeriod} />
                {days ? (
                  <CustomPeriod
                    since={days.since}
                    until={days.until}
                    onChange={typeDay}
                  />
                ) : null}
                <GroupMenu group={groupBy} onPick={pickGroup} />
                <BaselineMenu
                  points={drawn}
                  base={base}
                  why={baseline.why}
                  onPick={setBase}
                />
                <span className="tr-spacer" />
                <Legend
                  mixed={
                    data.selected?.model === ANY ||
                    data.selected?.profile === ANY
                  }
                  unit={
                    groupBy === 'day'
                      ? 'day'
                      : groupBy === 'release'
                        ? 'Harness release'
                        : 'execution'
                  }
                />
              </div>
              <div className="tr-summary" aria-busy={loading || undefined}>
                <p className="tr-faint tr-num-text" data-trend-summary>
                  {summaryText(points, period)}
                </p>
                {groupBy === 'release' && unversioned(points).length > 0 ? (
                  <p className="tr-faint" data-group-note>
                    {plural(unversioned(points).length, 'execution')} ran with
                    no recorded Harness version, so no release holds{' '}
                    {unversioned(points).length === 1 ? 'it' : 'them'}.
                  </p>
                ) : null}
                {note ? <p className="tr-faint">{note}</p> : null}
                {notice ? (
                  <p className="tr-faint tr-notice" data-stack-notice>
                    {notice.text}
                    {notice.anyStack ? (
                      <button
                        type="button"
                        className={buttonClassName({
                          variant: 'quiet',
                          size: 'compact',
                        })}
                        onClick={() => pickStack(ANY_STACK)}
                      >
                        Show every stack
                      </button>
                    ) : null}
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
                    className={buttonClassName({ variant: 'secondary' })}
                    onClick={() => openRunner({ parameters: null, label: '' })}
                  >
                    Run tests
                  </button>
                ) : null
              }
            />
          ) : noneInPeriod ? (
            <EmptyState
              icon={<ChartLine size={24} />}
              title={`No execution of this series ${periodPhrase(period)}`}
              description="Its executions ran outside this period."
              actions={
                'range' in period && period.range === 'all' ? null : (
                  <button
                    type="button"
                    className={buttonClassName({ variant: 'secondary' })}
                    onClick={() => pickPeriod('all')}
                  >
                    Show all time
                  </button>
                )
              }
            />
          ) : nothingCounted ? (
            <>
              {rerunError && latest ? (
                <LedgerLoadFailure
                  what="execution to run again"
                  reload={false}
                  message={rerunError}
                  onRetry={() => void runAgain(latest)}
                />
              ) : null}
              <EmptyState
                className="tr-empty"
                icon={<ChartLine size={24} />}
                title="Nothing to draw yet"
                description={
                  empty ? (
                    <>
                      {empty.before}
                      <a
                        className="tr-inline-link"
                        href={hashForExecution(empty.point.execution_id)}
                      >
                        {pointTime(empty.point)}
                      </a>
                      {empty.after}
                    </>
                  ) : null
                }
                actions={
                  latest && bridge ? (
                    <button
                      type="button"
                      className={buttonClassName({ variant: 'secondary' })}
                      onClick={() => void runAgain(latest)}
                    >
                      Run again
                    </button>
                  ) : null
                }
              />
            </>
          ) : (
            <>
              <div
                className="tr-top"
                data-panel={(point && !narrow) || undefined}
              >
                <LargeChart
                  metric={metric}
                  points={drawn}
                  changes={changes}
                  selected={selected}
                  baseline={baseline.index}
                  narrow={narrow}
                  onPick={pick}
                />
                {point ? (
                  <PointPanel
                    key={point.execution_id}
                    points={drawn}
                    index={selected}
                    changes={changes[selected]}
                    previous={previousCounted(drawn, selected)}
                    baseline={
                      baseline.index >= 0 ? drawn[baseline.index] : null
                    }
                    bridge={bridge}
                    here={here}
                    onClose={() => setPicked(null)}
                    onBaseline={setBase}
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
                      points={drawn}
                      changes={changes}
                      selected={selected}
                      baseline={baseline.index}
                      narrow={narrow}
                      onFocus={() => setFocus(item.id)}
                    />
                  ),
                )}
              </section>
              <ByTest
                points={drawn}
                changes={changes}
                selected={selected}
                narrow={narrow}
                onPick={pick}
              />
              <ExecutionsTable
                points={points}
                selected={groupBy === 'execution' ? selected : -1}
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
