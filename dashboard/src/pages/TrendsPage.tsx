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
import { DisclosureLayer } from '@/components/DisclosureLayer'
import { GithubImportDialog } from '@/components/GithubImportDialog'
import { LocalRunnerDialog } from '@/components/LocalRunnerDialog'
import type { HeaderAction } from '@/components/shell/HeaderActions'
import { BaselineMenu } from '@/components/trends/BaselineMenu'
import { PointPanel } from '@/components/trends/PointPanel'
import { ReleaseOverview } from '@/components/trends/ReleaseOverview'
import { LargeChart, SmallChart } from '@/components/trends/TrendsChart'
import { ByTest, ExecutionsTable } from '@/components/trends/TrendsTables'
import { VerdictHeadline } from '@/components/trends/VerdictHeadline'
import { buttonClassName, EmptyState } from '@/design-system'
import {
  hashForExecution,
  hashForTrends,
  replaceRouteParams,
} from '@/hooks/use-hash-route'
import { useLatestRequest } from '@/hooks/use-latest-request'
import { useRunChanges } from '@/hooks/use-run-changes'
import {
  type DashboardDataBridge,
  type ExecutionParameters,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import { buildExecutionPresentation } from '@/lib/execution-view'
import { plural } from '@/lib/format'
import type { ReleaseOverview as ReleaseOverviewAnswer } from '@/lib/release-overview'
import {
  ANY,
  ANY_STACK,
  baselineOf,
  changesAt,
  counted,
  customDays,
  DEFAULT_PERIOD,
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
  seriesText,
  shownGroup,
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

/** The hash's params for what is on screen: the overview names only its
 *  measure; a series names its request, period, grouping and baseline. */
export function pageParams(view: {
  mode: 'overview' | 'series'
  request: TrendsRequest
  period: TrendPeriod
  group: TrendGroupBy
  /** The release grouping was asked for, not left to its default. */
  groupAsked?: boolean
  base: string | null
  metric: TrendMetricId
}) {
  const params =
    view.mode === 'overview'
      ? new URLSearchParams()
      : withBase(
          withGroup(
            withPeriod(requestParams(view.request), view.period),
            view.group,
            view.groupAsked,
          ),
          view.base,
        )
  if (view.metric !== 'score') params.set('metric', view.metric)
  return params
}

const OPTIONS_SCENT = 'suite · model · profile · stack · period · baseline'

/** More options' closed row: what it holds, or the series it shows. */
export function optionsScent(
  mode: 'overview' | 'series',
  data: TrendsResponse,
  period: TrendPeriod,
) {
  const current = data.selected
  if (mode === 'overview' || !current) return OPTIONS_SCENT
  const suite =
    suiteChoices(data.series).find((choice) => choice.suite === current.suite)
      ?.label ?? current.suite
  return [
    suite,
    seriesText(current),
    `stack ${stackLabel(data.stack, data.points)}`,
    periodLabel(period),
  ].join(' · ')
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

/** The first screen's shape while it loads: the verdict and its scope, the
 *  metric tabs, the release line, then More options and Details. */
export function TrendsSkeleton() {
  return (
    <div className="tr-loading" role="status" aria-busy="true">
      <span className="ds-visually-hidden">Loading the release overview</span>
      <Skeleton className="tr-skel tr-skel-line" />
      <Skeleton className="tr-skel tr-skel-line" />
      <div className="tr-tabs">
        {TREND_METRICS.filter((item) => item.tier === 'main').map((metric) => (
          <Skeleton key={metric.id} className="tr-skel tr-skel-tab" />
        ))}
      </div>
      <Skeleton className="tr-skel tr-skel-big" />
      {['options', 'details'].map((row) => (
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
  group: routeGroup = 'release',
  groupAsked: routeGroupAsked = false,
  mode: routeMode = 'overview',
  metric: routeMetric = 'score',
}: {
  request: TrendsRequest
  period: TrendPeriod
  base: string | null
  group?: TrendGroupBy
  /** The hash names the release grouping, not its default. */
  groupAsked?: boolean
  /** The release overview first; a series once the hash names one. */
  mode?: 'overview' | 'series'
  metric?: TrendMetricId
}) {
  const narrow = useDashboardChrome()?.narrow ?? false
  const [mode, setMode] = useState(routeMode)
  const [optionsOpen, setOptionsOpen] = useState(routeMode === 'series')
  const [detailsOpen, setDetailsOpen] = useState(false)
  const [query, setQuery] = useState<TrendsRequest>(request)
  const [period, setPeriod] = useState<TrendPeriod>(routePeriod)
  // The execution pinned as the baseline; it lives in the hash.
  const [base, setBase] = useState<string | null>(routeBase)
  const [groupBy, setGroupBy] = useState<TrendGroupBy>(routeGroup)
  const [groupAsked, setGroupAsked] = useState(routeGroupAsked)
  // The two days being typed for a custom period, until they make one.
  const [draft, setDraft] = useState<{ since: string; until: string } | null>(
    null,
  )
  const [data, setData] = useState<TrendsResponse | null>(null)
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [overview, setOverview] = useState<ReleaseOverviewAnswer | null>(null)
  const [overviewError, setOverviewError] = useState<string | null>(null)
  // The picked execution, by id, so a reload keeps it while it is listed.
  const [picked, setPicked] = useState<string | null>(null)
  const [focus, setFocus] = useState<TrendMetricId>(routeMetric)
  const [runner, setRunner] = useState<Runner | null>(null)
  const [runnerOpen, setRunnerOpen] = useState(false)
  const [importOpen, setImportOpen] = useState(false)
  const [rerunError, setRerunError] = useState<string | null>(null)
  const beginRequest = useLatestRequest()
  const beginOverview = useLatestRequest()
  // What a reload asks and the hash names: the request, then the view its
  // answer showed.
  const shown = useRef<TrendsRequest>(request)
  const [view, setView] = useState<TrendsRequest>(request)

  // The Trends tab or a link lands here again: start over from its request.
  useEffect(() => {
    setMode(routeMode)
    setOptionsOpen(routeMode === 'series')
    setQuery(request)
    setPeriod(routePeriod)
    setBase(routeBase)
    setGroupBy(routeGroup)
    setGroupAsked(routeGroupAsked)
    setDraft(null)
    setFocus(routeMetric)
  }, [
    request,
    routePeriod,
    routeBase,
    routeGroup,
    routeGroupAsked,
    routeMode,
    routeMetric,
  ])

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

  // The overview fails on its own: the series view and its controls stay.
  const loadOverview = useCallback(async () => {
    const pending = beginOverview()
    try {
      const next = await getDashboardDataBridge()
      if (!pending.isCurrent()) return
      const answer = await next.getReleaseOverview()
      if (!pending.isCurrent()) return
      setOverview(answer)
      setOverviewError(null)
    } catch (cause) {
      if (pending.isCurrent())
        setOverviewError(cause instanceof Error ? cause.message : String(cause))
    }
  }, [beginOverview])

  useEffect(() => {
    void loadOverview()
  }, [loadOverview])

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

  // Both follow new executions quietly, as the executions list does.
  const reload = useCallback(() => {
    void load()
    void loadOverview()
  }, [load, loadOverview])
  useRunChanges(bridge, reload)

  // The hash names what is on screen, once the worker said what that is.
  const viewParams = pageParams({
    mode,
    request: view,
    period,
    group: groupBy,
    groupAsked,
    base,
    metric: focus,
  }).toString()
  useEffect(() => {
    replaceRouteParams(new URLSearchParams(viewParams))
  }, [viewParams])

  const points = data?.points ?? []
  const group = shownGroup(points, groupBy, groupAsked)
  // What the charts draw: each execution, or one point per day or release.
  const drawn = useMemo(() => groupPoints(points, group.by), [points, group.by])
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
    setMode('series')
    setPicked(null)
    setGroupBy(next)
    setGroupAsked(next === 'release')
  }
  const pickBase = (next: string | null) => {
    setMode('series')
    setBase(next)
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
    setMode('series')
    setFocus('score')
    setBase(null)
    setQuery(request)
  }
  const pickStack = (stack: string) => {
    setMode('series')
    setQuery({ ...(data?.selected ?? requestSeries(query) ?? {}), stack })
  }
  // Every series control back to where the first screen starts; the
  // measure stays.
  const backToOverview = () => {
    setMode('overview')
    setQuery({})
    setPeriod(DEFAULT_PERIOD)
    setDraft(null)
    setBase(null)
    setPicked(null)
    setGroupBy('release')
    setGroupAsked(false)
    setOptionsOpen(false)
  }

  const failedFirstLoad = Boolean(error) && data === null
  // Both answered, or failed: until then the first screen's shape.
  const settled =
    (data !== null || error !== null) &&
    (overview !== null || overviewError !== null)
  const noneInPeriod = data !== null && points.length === 0
  const nothingCounted = !noneInPeriod && !points.some(counted)
  const days =
    draft ??
    ('since' in period ? { since: period.since, until: period.until } : null)
  const pickPeriod = (value: string) => {
    setMode('series')
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
    setMode('series')
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

  // Today's controls and the series' summary, closed until asked for.
  const moreOptions =
    data && data.series.length > 0 ? (
      <DisclosureLayer
        id="tr-more-options"
        label="More options"
        scent={optionsScent(mode, data, period)}
        open={optionsOpen}
        onToggle={setOptionsOpen}
      >
        <div className="tr-layer">
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
            <GroupMenu group={group.by} onPick={pickGroup} />
            <BaselineMenu
              points={drawn}
              base={base}
              why={baseline.why}
              onPick={pickBase}
            />
            <span className="tr-spacer" />
            <Legend
              mixed={
                data.selected?.model === ANY || data.selected?.profile === ANY
              }
              unit={
                group.by === 'day'
                  ? 'day'
                  : group.by === 'release'
                    ? 'Harness release'
                    : 'execution'
              }
            />
          </div>
          <div className="tr-summary" aria-busy={loading || undefined}>
            <p className="tr-faint tr-num-text" data-trend-summary>
              {summaryText(points, period)}
            </p>
            {group.note ? (
              <p className="tr-faint" data-group-note>
                {group.note}
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
        </div>
      </DisclosureLayer>
    ) : null

  // Today's series view: the empty states, else the large chart and the
  // small ones.
  const chart =
    data === null ? null : data.series.length === 0 ? (
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
        <div className="tr-top" data-panel={(point && !narrow) || undefined}>
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
              baseline={baseline.index >= 0 ? drawn[baseline.index] : null}
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
          {TREND_METRICS.filter(
            (item) => item.tier === 'main' && item.id !== metric.id,
          ).map((item) => (
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
          ))}
        </section>
      </>
    )
  // The by-test grid and the executions table, once there is a chart.
  const details =
    data && data.series.length > 0 && !noneInPeriod && !nothingCounted ? (
      <DisclosureLayer
        id="tr-details"
        label="Details"
        scent="by test · executions"
        open={detailsOpen}
        onToggle={setDetailsOpen}
      >
        <div className="tr-layer">
          <ByTest
            points={drawn}
            changes={changes}
            selected={selected}
            narrow={narrow}
            onPick={pick}
          />
          <ExecutionsTable
            points={points}
            selected={group.by === 'execution' ? selected : -1}
            narrow={narrow}
          />
        </div>
      </DisclosureLayer>
    ) : null

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
          Whether the latest Harness release did better or worse than the one
          before, from the runs Release Control dispatched on CI. A hollow ring
          is a run on this harness or in Docker, shown beside the release it ran
          with; it never counts. More options follows one suite on one model and
          profile over time.
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
      ) : null}
      {overviewError && overview ? (
        <LedgerLoadFailure
          what="release overview"
          reload
          message={overviewError}
          onRetry={() => void loadOverview()}
        />
      ) : null}

      {!settled ? (
        <TrendsSkeleton />
      ) : (
        <>
          {overview ? (
            <VerdictHeadline overview={overview} />
          ) : (
            <section className="tr-verdict" aria-labelledby="tr-verdict">
              <div className="tr-verdict-head">
                <h2 id="tr-verdict" className="tr-h2">
                  Harness evolution
                </h2>
              </div>
              <LedgerLoadFailure
                what="release overview"
                reload={false}
                message={overviewError ?? ''}
                onRetry={() => void loadOverview()}
              />
            </section>
          )}
          {mode === 'series' ? (
            <>
              <button
                type="button"
                className={buttonClassName({
                  variant: 'quiet',
                  size: 'compact',
                  className: 'tr-back',
                })}
                onClick={backToOverview}
              >
                Back to release overview
              </button>
              {chart}
              {moreOptions}
              {details}
            </>
          ) : overview ? (
            <ReleaseOverview
              overview={overview}
              metric={focus}
              onMetric={setFocus}
              bridge={bridge}
              narrow={narrow}
              options={moreOptions}
              empty={
                <EmptyState
                  icon={<ChartLine size={24} />}
                  title="No release to draw yet"
                  description="The timeline draws the CI runs Release Control dispatched, once this worker has imported them."
                />
              }
            />
          ) : (
            moreOptions
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
        onImported={reload}
      />
    </div>
  )
}
