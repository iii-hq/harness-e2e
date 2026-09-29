// How one suite moves over time on one model and profile, as the redesign
// canvas draws it (Trends.dc.html): each point is an execution of the
// series, a diamond marks what changed since the execution before it, and
// every measure is over technically valid (counted) runs only. The worker
// answers e2e::dashboard::trends-get; what changed, the deltas, the axes and
// the grid's cells are worked out here.

import { shortDefinition } from '@/lib/definition-digest'
import { betterWhen } from '@/lib/execution-comparison'
import {
  formatDateTime,
  formatDay,
  formatDuration,
  formatTokens,
  plural,
} from '@/lib/format'

/* ------------------------------------------------------------- contract */

export type TrendsRequest = {
  /** A suite id, or the sha256 of an unsaved suite. */
  suite?: string
  provider?: string
  model?: string
  /** null or '' = no profile. */
  profile?: string | null
  /** A stack name, 'not_recorded' or 'any'; default (and for a stack the
   *  series never ran on): 'any'. */
  stack?: string
  /** RFC 3339 instants, both inclusive, against each execution's
   *  started_at; absent = unbounded. They limit `points` (and the stacks'
   *  counts), never `series`. */
  since?: string
  until?: string
}

export type TrendSeriesKey = {
  suite: string
  provider: string
  model: string
  profile: string | null
}

export type TrendWhere = 'harness' | 'docker' | 'github'

export type TrendSeries = TrendSeriesKey & {
  suite_label: string
  executions: number
  latest_at: string
  where: TrendWhere[]
}

export type TrendMeasures = {
  score_mean: number | null
  /** Counted runs that completed their task. */
  completed: number
  /** Planned tests × runs per test, the same unit as `completed`. */
  planned: number
  duration_ms_mean: number | null
  input_tokens_mean: number | null
  function_calls_mean: number | null
  function_calls: number | null
  function_call_errors: number | null
  turns_mean: number | null
}

export type TrendTestState =
  | 'scored'
  | 'no_score'
  | 'technical_invalid'
  | 'not_run'

export type TrendTest = {
  id: string
  state: TrendTestState
  /** Mean of the counted runs' scores. */
  score: number | null
  behavior_sha256: string | null
}

export type TrendPoint = {
  execution_id: string
  label: string | null
  started_at: string
  /** Its own model and profile: under `any` they differ between points. */
  provider: string
  model: string
  profile: string | null
  source: {
    kind: 'local' | 'docker' | 'github'
    run_id?: number
    run_attempt?: number
    release_control_execution_id?: string
    url?: string
  }
  stack: { name: string | null; matched_by_workers: boolean }
  engine: string | null
  /** Identity per worker, the engine and harness-e2e left out; null = not
   *  recorded. */
  workers: Record<string, string> | null
  planned: string[] | null
  runs: number
  /** Technically valid runs. */
  counted: number
  /** When nothing counted: why. */
  reason: string | null
  measures: TrendMeasures | null
  /** Planned tests first, in plan order, then any other that ran. */
  tests: TrendTest[]
  /** A point that stands for several executions (Group by day or release). */
  group?: TrendGroup
}

export type TrendsResponse = {
  /** Every series, latest execution first. */
  series: TrendSeries[]
  selected: TrendSeriesKey | null
  stack: string
  /** Every stack the selected series ever ran on, latest first, then
   *  'not_recorded' (if any) and 'any'; counts are the period's, so 0 can
   *  be one. */
  stacks: Array<{ name: string; executions: number }>
  /** Oldest first. */
  points: TrendPoint[]
}

export type VersionCompareRequest = { name: string; base: string; head: string }
export type VersionCompareResponse = {
  url: string
  total_commits: number | null
}

/** Every stack, model or profile: one line over all of them. */
export const ANY = 'any'
export const ANY_STACK = ANY
export const NOT_RECORDED_STACK = 'not_recorded'

/* --------------------------------------------------------------- series */

/** `deepseek/deepseek-flash`; a model id may already carry its provider. */
export function seriesModel(key: Pick<TrendSeriesKey, 'provider' | 'model'>) {
  if (key.model === ANY) return ANY
  return key.model.startsWith(`${key.provider}/`)
    ? key.model
    : `${key.provider}/${key.model}`
}

export function profileText(profile: string | null) {
  return profile || 'none'
}

/** Whether a view mixes models or profiles (only under `any`). */
export function mixesSeries(
  points: Pick<TrendPoint, 'provider' | 'model' | 'profile'>[],
) {
  return (
    new Set(
      points.map((point) => `${seriesModel(point)}|${point.profile ?? ''}`),
    ).size > 1
  )
}

/** An execution's model and profile, said where a view mixes them. */
export function seriesText(
  point: Pick<TrendPoint, 'provider' | 'model' | 'profile'>,
) {
  return `${seriesModel(point)} · profile ${profileText(point.profile)}`
}

/** The series and stack in the hash, so a link reopens the same view. */
export function trendsRequestFromParams(params: URLSearchParams) {
  const request: TrendsRequest = {}
  for (const key of ['suite', 'provider', 'model', 'stack'] as const) {
    const value = params.get(key)
    if (value) request[key] = value
  }
  if (params.has('profile')) request.profile = params.get('profile') || null
  return request
}

export function trendsParams(request: TrendsRequest): URLSearchParams {
  const params = new URLSearchParams()
  for (const key of ['suite', 'provider', 'model'] as const) {
    const value = request[key]
    if (value) params.set(key, value)
  }
  if (request.profile !== undefined)
    params.set('profile', request.profile ?? '')
  if (request.stack) params.set('stack', request.stack)
  return params
}

/** The suites of every series, latest first, each with all its executions. */
export function suiteChoices(series: TrendSeries[]) {
  const out: Array<{ suite: string; label: string; executions: number }> = []
  for (const item of series) {
    const found = out.find((choice) => choice.suite === item.suite)
    if (found) found.executions += item.executions
    else
      out.push({
        suite: item.suite,
        label: item.suite_label,
        executions: item.executions,
      })
  }
  return out
}

/** `any`, then the models that ran a suite, latest first. */
export function modelChoices(series: TrendSeries[], suite: string) {
  const ofSuite = series.filter((each) => each.suite === suite)
  const out: Array<{ provider: string; model: string; executions: number }> = [
    {
      provider: ANY,
      model: ANY,
      executions: ofSuite.reduce((sum, item) => sum + item.executions, 0),
    },
  ]
  for (const item of ofSuite) {
    const found = out.find(
      (choice) =>
        choice.provider === item.provider && choice.model === item.model,
    )
    if (found) found.executions += item.executions
    else
      out.push({
        provider: item.provider,
        model: item.model,
        executions: item.executions,
      })
  }
  return out
}

/** `any`, then the profiles (null: none) that ran a suite on a model (on
 *  any model under `any`), latest first. */
export function profileChoices(
  series: TrendSeries[],
  key: Pick<TrendSeriesKey, 'suite' | 'provider' | 'model'>,
) {
  const ran = series.filter(
    (item) =>
      item.suite === key.suite &&
      (key.model === ANY ||
        (item.provider === key.provider && item.model === key.model)),
  )
  const out: Array<{ profile: string | null; executions: number }> = [
    {
      profile: ANY,
      executions: ran.reduce((sum, item) => sum + item.executions, 0),
    },
  ]
  for (const item of ran) {
    const profile = item.profile || null
    const found = out.find((choice) => choice.profile === profile)
    if (found) found.executions += item.executions
    else out.push({ profile, executions: item.executions })
  }
  return out
}

/* --------------------------------------------------------------- period */

export type TrendRange = '7d' | '30d' | '90d' | 'all'

/** A range ending today, or two local days (YYYY-MM-DD), both included. */
export type TrendPeriod =
  | { range: TrendRange }
  | { since: string; until: string }

export const DEFAULT_PERIOD: TrendPeriod = { range: '30d' }

const RANGE_DAYS: Record<Exclude<TrendRange, 'all'>, number> = {
  '7d': 7,
  '30d': 30,
  '90d': 90,
}

export const PERIOD_CHOICES: Array<{ value: TrendRange; label: string }> = [
  { value: '7d', label: 'Last 7 days' },
  { value: '30d', label: 'Last 30 days' },
  { value: '90d', label: 'Last 90 days' },
  { value: 'all', label: 'All time' },
]

function isRange(value: string | null): value is TrendRange {
  return value === '7d' || value === '30d' || value === '90d' || value === 'all'
}

function dayDate(day: string) {
  const [year, month, date] = day.split('-').map(Number)
  return new Date(year, month - 1, date)
}

/** A calendar day as a date input writes it, and one that exists. */
export function validDay(day: string) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(day)) return false
  const date = dayDate(day)
  return localDay(date) === day
}

/** The reader's calendar day of a date: YYYY-MM-DD. */
export function localDay(date: Date) {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`
}

/** Why two days are not a period, or null. */
export function periodError(since: string, until: string) {
  if (!validDay(since) || !validDay(until)) return 'Pick a From and a To day.'
  return since <= until ? null : 'From must be on or before To.'
}

/** The period in the hash: range=…, or since=…&until=… as days. */
export function periodFromParams(params: URLSearchParams): TrendPeriod {
  const range = params.get('range')
  if (isRange(range)) return { range }
  const since = params.get('since') ?? ''
  const until = params.get('until') ?? ''
  return periodError(since, until) === null ? { since, until } : DEFAULT_PERIOD
}

export function withPeriod(params: URLSearchParams, period: TrendPeriod) {
  if ('range' in period) params.set('range', period.range)
  else {
    params.set('since', period.since)
    params.set('until', period.until)
  }
  return params
}

/** The instants to ask for: from the start of the first local day to the
 *  end of the last, RFC 3339; nothing for all time. */
export function periodBounds(
  period: TrendPeriod,
  now = new Date(),
): Pick<TrendsRequest, 'since' | 'until'> {
  if ('since' in period) {
    const end = dayDate(period.until)
    end.setHours(23, 59, 59, 999)
    return {
      since: dayDate(period.since).toISOString(),
      until: end.toISOString(),
    }
  }
  if (period.range === 'all') return {}
  const start = new Date(
    now.getFullYear(),
    now.getMonth(),
    now.getDate() - (RANGE_DAYS[period.range] - 1),
  )
  const end = new Date(now.getFullYear(), now.getMonth(), now.getDate())
  end.setHours(23, 59, 59, 999)
  return { since: start.toISOString(), until: end.toISOString() }
}

/** The two days a custom period starts from: the last 30. */
export function customDays(period: TrendPeriod, now = new Date()) {
  if ('since' in period) return { since: period.since, until: period.until }
  const start = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 29)
  return { since: localDay(start), until: localDay(now) }
}

/** `in the last 30 days`, `over all time`, `from Sep 1 to Sep 10`. */
export function periodPhrase(period: TrendPeriod, now = new Date()) {
  if ('since' in period)
    return `from ${formatDay(period.since, now)} to ${formatDay(period.until, now)}`
  return period.range === 'all'
    ? 'over all time'
    : `in the last ${RANGE_DAYS[period.range]} days`
}

/** The period picker's value. */
export function periodLabel(period: TrendPeriod) {
  if ('since' in period) return 'Custom'
  return (
    PERIOD_CHOICES.find((choice) => choice.value === period.range)?.label ??
    'Last 30 days'
  )
}

/* -------------------------------------------------------------- a point */

/** `Sep 28, 2:17 AM`; the year only when it is not `now`'s. */
export function pointTime(
  point: Pick<TrendPoint, 'started_at' | 'group'>,
  now = new Date(),
) {
  return point.group?.label ?? formatDateTime(point.started_at, now)
}

export function counted(point: Pick<TrendPoint, 'counted'>) {
  return point.counted > 0
}

/** `GitHub #36381232467 · attempt 2`, `Docker`, `This harness`. */
export function sourceText(point: Pick<TrendPoint, 'source'>) {
  const { kind, run_id, run_attempt } = point.source
  if (kind === 'github')
    return `GitHub${run_id ? ` #${run_id}` : ''}${run_attempt && run_attempt > 1 ? ` · attempt ${run_attempt}` : ''}`
  return kind === 'docker' ? 'Docker' : 'This harness'
}

export function releaseControlId(point: Pick<TrendPoint, 'source'>) {
  return point.source.release_control_execution_id?.slice(0, 8) ?? null
}

/** The stack as a cell reads it; a local execution runs on this harness. */
export function stackText(point: Pick<TrendPoint, 'stack' | 'source'>) {
  const { name, matched_by_workers } = point.stack
  if (name) return matched_by_workers ? `${name} · by workers` : name
  return point.source.kind === 'local' ? 'this harness' : 'not recorded'
}

export function stackOptionText(name: string) {
  return name === NOT_RECORDED_STACK ? 'not recorded' : name
}

/** The planned tests that did not run. */
export function notRun(point: Pick<TrendPoint, 'tests'>) {
  return point.tests
    .filter((test) => test.state === 'not_run')
    .map((test) => test.id)
}

/* ------------------------------------------------------------- changes */

export type ChangeKind =
  | 'model'
  | 'profile'
  | 'iii'
  | 'harness'
  | 'tests'
  | 'stack'
  | 'worker'
  | 'definition'

export type TrendChange = {
  kind: ChangeKind
  /** A filled diamond: iii, the Harness, the tests or the stack; another
   *  worker or a definition is outlined. */
  major: boolean
  /** `iii`, a worker, `tests`, `stack` or a test id. */
  name: string
  text: string
  note: string | null
  /** What to ask e2e::dashboard::version-compare for the commits between. */
  compare: VersionCompareRequest | null
}

export const CHANGE_KIND_TEXT: Record<ChangeKind, string> = {
  model: 'model',
  profile: 'agent profile',
  iii: 'engine',
  harness: 'the Harness under test',
  tests: 'suite',
  stack: 'stack',
  worker: 'worker',
  definition: 'test definition',
}

/** `@7a16130` from `0.14.0@7a16130*` or `@7a16130`; null for a version. */
function commitOf(identity: string) {
  const at = identity.indexOf('@')
  return at < 0 ? null : `@${identity.slice(at + 1).replace('*', '')}`
}

function versionChange(
  kind: ChangeKind,
  name: string,
  from: string | null,
  to: string | null,
  reversed = false,
): TrendChange {
  const change: TrendChange = {
    kind,
    major: kind !== 'worker',
    name,
    text: `${from ?? 'not in the stack'} → ${to ?? 'left the stack'}`,
    note: null,
    compare: null,
  }
  if (!from || !to)
    return {
      ...change,
      note: from ? 'removed from the stack' : 'added to the stack',
    }
  // More than one build of a worker in one execution ("a, b"): no one range.
  if (from.includes(', ') || to.includes(', ')) return change
  // The text reads from → to; the commits run from the earlier to the later.
  const [earlier, later] = reversed ? [to, from] : [from, to]
  const base = commitOf(earlier)
  const head = commitOf(later)
  if (base && head)
    return {
      ...change,
      compare: { name, base, head },
      note: later.includes('*')
        ? '* the checkout also had uncommitted edits'
        : null,
    }
  // A release on one side and a checkout on the other: no range to count.
  if (base || head) return change
  return { ...change, compare: { name, base: earlier, head: later } }
}

function lastBefore(
  points: TrendPoint[],
  index: number,
  test: (point: TrendPoint) => boolean,
) {
  return lastAtOrBefore(points, index - 1, test)
}

function lastAtOrBefore(
  points: TrendPoint[],
  index: number,
  test: (point: TrendPoint) => boolean,
) {
  for (let at = index; at >= 0; at -= 1) if (test(points[at])) return points[at]
  return null
}

/** What changed at point `index`: iii and the stack against the execution
 *  right before it; workers, planned tests and definitions against
 *  the last one that recorded them. */
export function changesAt(points: TrendPoint[], index: number): TrendChange[] {
  return index <= 0 ? [] : changesBetween(points, index - 1, index)
}

/** What differs from execution `from` to execution `to`, each change
 *  written from → to. iii and the stack are read on those two executions;
 *  workers, planned tests and definitions against the last one at or before
 *  `from` that recorded them. `from` after `to` reads the same way, its
 *  commit ranges still running from the earlier version to the later. */
export function changesBetween(
  points: TrendPoint[],
  from: number,
  to: number,
): TrendChange[] {
  const current = points[to]
  const previous = points[from]
  const reversed = from > to
  const out: TrendChange[] = []
  // Only under `any` do two executions of a view differ in these.
  if (seriesModel(previous) !== seriesModel(current))
    out.push({
      kind: 'model',
      major: true,
      name: 'model',
      text: `${seriesModel(previous)} → ${seriesModel(current)}`,
      note: 'another model, so the measures on either side are not the same agent',
      compare: null,
    })
  if ((previous.profile || null) !== (current.profile || null))
    out.push({
      kind: 'profile',
      major: true,
      name: 'profile',
      text: `${profileText(previous.profile)} → ${profileText(current.profile)}`,
      note: null,
      compare: null,
    })
  if (previous.engine && current.engine && previous.engine !== current.engine)
    out.push(
      versionChange('iii', 'iii', previous.engine, current.engine, reversed),
    )
  // Only two recorded stacks say the stack changed: one not recorded may
  // have run the same workers.
  if (
    previous.stack.name &&
    current.stack.name &&
    previous.stack.name !== current.stack.name
  )
    out.push({
      kind: 'stack',
      major: true,
      name: 'stack',
      text: `${stackText(previous)} → ${stackText(current)}`,
      note: null,
      compare: null,
    })
  const withWorkers = lastAtOrBefore(points, from, (point) => !!point.workers)
  if (current.workers && withWorkers?.workers) {
    const before = withWorkers.workers
    const after = current.workers
    const names = [...new Set([...Object.keys(before), ...Object.keys(after)])]
      .filter((name) => name !== 'compose')
      .sort()
    for (const name of names) {
      if ((before[name] ?? null) !== (after[name] ?? null))
        out.push(
          versionChange(
            name === 'harness' ? 'harness' : 'worker',
            name,
            before[name] ?? null,
            after[name] ?? null,
            reversed,
          ),
        )
    }
  }
  const withPlan = lastAtOrBefore(points, from, (point) => !!point.planned)
  if (current.planned && withPlan?.planned) {
    const before = withPlan.planned
    const after = current.planned
    const gone = before.filter((id) => !after.includes(id))
    const came = after.filter((id) => !before.includes(id))
    if (gone.length || came.length)
      out.push({
        kind: 'tests',
        major: true,
        name: 'tests',
        text: [
          gone.length ? `left: ${gone.join(', ')}` : null,
          came.length ? `joined: ${came.join(', ')}` : null,
        ]
          .filter(Boolean)
          .join(' · '),
        note: 'the suite changed its tests, so means before and after cover different work',
        compare: null,
      })
  }
  // Each test against the last counted execution that ran it: one where it
  // did not run (not_run, no definition) is skipped, not taken as the same.
  if (counted(current)) {
    for (const test of current.tests) {
      const digestOf = (point: TrendPoint) =>
        point.tests.find((item) => item.id === test.id)?.behavior_sha256
      const earlier = test.behavior_sha256
        ? lastAtOrBefore(points, from, (point) =>
            Boolean(counted(point) && digestOf(point)),
          )
        : null
      const before = earlier
        ? earlier.tests.find((item) => item.id === test.id)
        : undefined
      if (
        test.behavior_sha256 &&
        before?.behavior_sha256 &&
        before.behavior_sha256 !== test.behavior_sha256
      )
        out.push({
          kind: 'definition',
          major: false,
          name: test.id,
          text: `${shortDefinition(before.behavior_sha256)} → ${shortDefinition(test.behavior_sha256)}`,
          note: 'its definition changed, so comparisons across this point leave it out',
          compare: null,
        })
    }
  }
  return out
}

const LANE_RANK: ChangeKind[] = [
  'model',
  'profile',
  'iii',
  'harness',
  'tests',
  'stack',
]

/** The label over a diamond: the first major change by model > profile >
 *  iii > harness > tests > stack, and how many more. Minor changes go
 *  unlabelled. */
export function laneLabel(changes: TrendChange[]) {
  const majors = changes
    .filter((change) => change.major)
    .sort(
      (one, two) => LANE_RANK.indexOf(one.kind) - LANE_RANK.indexOf(two.kind),
    )
  const first = majors[0]
  if (!first) return ''
  const to = first.text.split(' → ')[1] ?? ''
  const text =
    first.kind === 'model'
      ? `model ${to.split('/').at(-1)}`
      : first.kind === 'profile'
        ? `profile ${to}`
        : first.kind === 'iii'
          ? `iii ${to}`
          : first.kind === 'harness'
            ? `harness ${to}`
            : first.kind === 'tests'
              ? 'tests changed'
              : 'stack changed'
  return majors.length > 1 ? `${text} +${majors.length - 1}` : text
}

/** The link under a version change, once version-compare answered. */
export function commitsLinkText(
  change: Pick<TrendChange, 'name' | 'compare'>,
  commits: number | null,
) {
  const count = commits === null ? null : plural(commits, 'commit')
  if (change.compare?.base.startsWith('@'))
    return `${count ?? 'Commits'} between them on GitHub`
  const where = change.name === 'iii' ? '' : ' in iii-hq/workers'
  return `${count ? `${count} between the tags` : 'The commits between the tags'}${where}`
}

export function compareKey(request: VersionCompareRequest) {
  return `${request.name}|${request.base}|${request.head}`
}

/* ------------------------------------------------------------- measures */

export type TrendMetricId =
  | 'score'
  | 'completed'
  | 'duration'
  | 'error_rate'
  | 'input_tokens'
  | 'function_calls'
  | 'turns'

export type TrendMetric = {
  id: TrendMetricId
  label: string
  note: string
  better: 'higher' | 'lower' | 'neither'
  value: (point: TrendPoint) => number | null
  figure: (value: number, point: TrendPoint) => string
  axis: (value: number) => string
  /** How a difference reads: score points, tests, percentage points or a
   *  share of the earlier value. */
  delta: 'points' | 'tests' | 'percent_points' | 'percent'
}

const round1 = (value: number) => Math.round(value * 10) / 10
const fixed1 = (value: number) => value.toFixed(1)
const whole = (value: number) => String(Math.round(value))
const measure =
  (key: keyof TrendMeasures) =>
  (point: TrendPoint): number | null => {
    const value = point.measures?.[key]
    return typeof value === 'number' && Number.isFinite(value) ? value : null
  }

/** Score first; the large chart shows one, the six small ones the rest. */
export const TREND_METRICS: TrendMetric[] = [
  {
    id: 'score',
    label: 'Score',
    note: 'mean of the counted runs',
    better: betterWhen('score'),
    value: measure('score_mean'),
    figure: fixed1,
    axis: whole,
    delta: 'points',
  },
  {
    id: 'completed',
    label: 'Tests completed',
    note: 'counted runs that completed their task',
    better: betterWhen('completed'),
    value: measure('completed'),
    figure: (value, point) =>
      `${value} of ${point.measures?.planned ?? value} planned`,
    axis: whole,
    delta: 'tests',
  },
  {
    id: 'duration',
    label: 'Run duration',
    note: 'mean per run',
    better: betterWhen('duration'),
    value: measure('duration_ms_mean'),
    figure: formatDuration,
    axis: formatDuration,
    delta: 'percent',
  },
  {
    id: 'error_rate',
    label: 'Error call rate',
    note: 'failed function calls ÷ calls',
    better: betterWhen('function_errors'),
    value: (point) => {
      const calls = point.measures?.function_calls
      const errors = point.measures?.function_call_errors
      return calls && typeof errors === 'number' ? (errors / calls) * 100 : null
    },
    figure: (value) => `${fixed1(value)}%`,
    axis: (value) => `${round1(value)}%`,
    delta: 'percent_points',
  },
  {
    id: 'input_tokens',
    label: 'Input tokens',
    note: 'mean per run',
    better: betterWhen('tokens'),
    value: measure('input_tokens_mean'),
    figure: formatTokens,
    axis: formatTokens,
    delta: 'percent',
  },
  {
    id: 'function_calls',
    label: 'Function calls',
    note: 'mean per run',
    better: betterWhen('function_calls'),
    value: measure('function_calls_mean'),
    figure: fixed1,
    axis: whole,
    delta: 'percent',
  },
  {
    id: 'turns',
    label: 'Turns',
    note: 'mean per run',
    better: betterWhen('turns'),
    value: measure('turns_mean'),
    figure: fixed1,
    axis: whole,
    delta: 'percent',
  },
]

export function trendMetric(id: string | null | undefined) {
  return TREND_METRICS.find((metric) => metric.id === id) ?? TREND_METRICS[0]
}

/** The signed difference as it is written, rounded like its text: 0 reads
 *  "no change". Null when a side has no value (or a share of zero). */
export function deltaOf(
  metric: TrendMetric,
  current: TrendPoint,
  previous: TrendPoint,
): number | null {
  const a = metric.value(previous)
  const b = metric.value(current)
  if (a === null || b === null) return null
  if (metric.delta === 'tests') return round1(b - a) || 0
  if (metric.delta === 'percent') {
    if (a === 0) return null
    return round1(((b - a) / a) * 100) || 0
  }
  return round1(b - a) || 0
}

/** Formats a difference's magnitude, for DeltaValue. */
export function deltaFormat(metric: TrendMetric) {
  if (metric.delta === 'tests')
    return (magnitude: number) => plural(magnitude, 'test')
  if (metric.delta === 'percent_points')
    return (magnitude: number) => `${fixed1(magnitude)} pts`
  if (metric.delta === 'percent')
    return (magnitude: number) => `${fixed1(magnitude)}%`
  return fixed1
}

/** The execution a card shows and the one its delta is against.
 *
 *  It shows `at` (the picked execution, -1 for none) or else the latest with
 *  a value. The delta is against the baseline (an index, -1 for none) when
 *  it has a value and is not the execution shown; otherwise against the one
 *  with a value before it. */
export function comparedPair(
  points: TrendPoint[],
  metric: TrendMetric,
  at: number,
  baseline: number,
) {
  const has = (index: number) =>
    index >= 0 && index < points.length && metric.value(points[index]) !== null
  let head = at
  if (at < 0) {
    head = points.length - 1
    while (head >= 0 && !has(head)) head -= 1
  }
  if (!has(head))
    return {
      current: null,
      previous: null,
      headIsBaseline: false,
      againstBaseline: false,
    }
  let against = -1
  if (baseline >= 0 && baseline !== head && has(baseline)) against = baseline
  else
    for (let index = head - 1; index >= 0; index -= 1)
      if (has(index)) {
        against = index
        break
      }
  return {
    current: points[head],
    previous: against >= 0 ? points[against] : null,
    headIsBaseline: head === baseline,
    againstBaseline: against >= 0 && against === baseline,
  }
}

/** A card's line under its value: which two executions it compares. */
export function referenceText(pair: ReturnType<typeof comparedPair>) {
  if (!pair.current) return null
  const now = `${pointTime(pair.current)}${pair.headIsBaseline ? ' (baseline)' : ''}`
  if (!pair.previous) return now
  return `${now} against ${pointTime(pair.previous)}${pair.againstBaseline ? ' (baseline)' : ''}`
}

/** The previous execution with a counted run, what deltas are against. */
export function previousCounted(points: TrendPoint[], index: number) {
  return lastBefore(points, index, counted)
}

/* ------------------------------------------------------------- baseline */

/** The baseline execution in the hash, if it names one. */
export function baseFromParams(params: URLSearchParams) {
  return params.get('base') || null
}

export function withBase(params: URLSearchParams, base: string | null) {
  if (base) params.set('base', base)
  return params
}

export type BaselineWhy = 'not_in_view' | 'no_counted_run'

/** Where the baseline is in this view: its index when it is shown and has a
 *  counted run, else why it is not used. */
export function baselineOf(
  points: TrendPoint[],
  base: string | null,
): { index: number; why: BaselineWhy | null } {
  if (!base) return { index: -1, why: null }
  const index = points.findIndex(
    (point) =>
      point.execution_id === base ||
      point.group?.members.some((member) => member.execution_id === base),
  )
  if (index < 0) return { index: -1, why: 'not_in_view' }
  return counted(points[index])
    ? { index, why: null }
    : { index: -1, why: 'no_counted_run' }
}

/** The model, the profile, iii, the Harness, the tests and the stack
 *  first, then the rest as they came. */
export function majorsFirst(changes: TrendChange[]) {
  const majors = changes
    .filter((change) => change.major)
    .sort(
      (one, two) => LANE_RANK.indexOf(one.kind) - LANE_RANK.indexOf(two.kind),
    )
  return [...majors, ...changes.filter((change) => !change.major)]
}

/* ---------------------------------------------------------------- axes */

/** The value axis: scores around what was scored, tests from 0 to the
 *  plan, the error rate from 0, anything else padded by 30% of its span. */
export function domain(
  metric: TrendMetric,
  values: number[],
  points: TrendPoint[],
): [number, number] {
  let lo = Math.min(...values)
  let hi = Math.max(...values)
  if (metric.id === 'score') {
    lo = Math.max(0, Math.floor(lo - 2))
    hi = Math.min(100, Math.ceil(hi + 2))
    if (hi - lo < 4) lo = Math.max(0, hi - 4)
  } else if (metric.id === 'completed') {
    // `planned` counts runs (planned tests × runs per test), as `completed`
    // does; the plan's test list does not.
    lo = 0
    hi = Math.max(hi, ...points.map((point) => point.measures?.planned ?? 0))
  } else if (metric.id === 'error_rate') {
    lo = 0
    hi = Math.max(1, Math.ceil(hi * 1.25))
  } else {
    const span = hi - lo || hi * 0.2 || 1
    lo = Math.max(0, lo - span * 0.3)
    hi += span * 0.3
  }
  if (hi <= lo) hi = lo + 1
  return [lo, hi]
}

/** Where a day starts: the first point of each local day, labelled. */
export function dayMarks(
  points: Pick<TrendPoint, 'started_at'>[],
  now = new Date(),
) {
  const marks: Array<{ index: number; text: string }> = []
  let last = ''
  points.forEach((point, index) => {
    const text = formatDay(point.started_at, now)
    if (text !== last) marks.push({ index, text })
    last = text
  })
  return marks
}

/** The marks that have room: none closer than `gap` to the one kept
 *  before it. The first always stays; the last takes the place of a kept
 *  one it would touch. */
export function roomyMarks<T>(
  marks: T[],
  xOf: (mark: T) => number,
  gap: number,
): T[] {
  const kept: T[] = []
  marks.forEach((mark, index) => {
    const last = kept.at(-1)
    if (last === undefined || xOf(mark) - xOf(last) >= gap) kept.push(mark)
    else if (index === marks.length - 1 && kept.length > 1)
      kept[kept.length - 1] = mark
  })
  return kept
}

/** One step per execution: the centre of its slot, as a fraction. */
export function slotX(index: number, count: number) {
  return (index + 0.5) / count
}

export type ChartSegment = {
  from: number
  to: number
  /** It crosses executions without a value: drawn faint. */
  faded: boolean
}

/** The line between consecutive points with a value. */
export function segments(values: Array<number | null>): ChartSegment[] {
  const out: ChartSegment[] = []
  let last = -1
  let gap = false
  values.forEach((value, index) => {
    if (value === null) {
      gap = true
      return
    }
    if (last >= 0) out.push({ from: last, to: index, faded: gap })
    last = index
    gap = false
  })
  return out
}

/* ------------------------------------------------------------ by test */

/** Every test in the view: each execution's, in order of first sight. */
export function testIds(points: TrendPoint[]) {
  const ids: string[] = []
  for (const point of points)
    for (const test of point.tests)
      if (!ids.includes(test.id)) ids.push(test.id)
  return ids
}

export type TestCell =
  | { kind: 'uncounted' | 'absent' | 'not_run' | 'technical_invalid' }
  | { kind: 'no_score'; redefined: boolean }
  | {
      kind: 'scored'
      score: number
      tone: 'full' | 'lost' | 'low'
      redefined: boolean
    }

/** A test's cell in one execution's column. */
export function testCell(
  point: TrendPoint,
  id: string,
  changes: TrendChange[],
): TestCell {
  if (!counted(point)) return { kind: 'uncounted' }
  const test = point.tests.find((item) => item.id === id)
  if (!test) return { kind: 'absent' }
  const redefined = changes.some(
    (change) => change.kind === 'definition' && change.name === id,
  )
  if (test.state === 'not_run' || test.state === 'technical_invalid')
    return { kind: test.state }
  if (test.state === 'no_score' || test.score === null)
    return { kind: 'no_score', redefined }
  const score = round1(test.score)
  return {
    kind: 'scored',
    score,
    tone: score >= 100 ? 'full' : score >= 85 ? 'lost' : 'low',
    redefined,
  }
}

/* -------------------------------------------------------------- summary */

/** `UTC−3`, `UTC+5:30`, `UTC`: how the page's times read. */
export function utcOffsetText(date = new Date()) {
  const minutes = -date.getTimezoneOffset()
  if (minutes === 0) return 'UTC'
  const size = Math.abs(minutes)
  const rest = size % 60
  return `UTC${minutes < 0 ? '−' : '+'}${Math.floor(size / 60)}${rest ? `:${String(rest).padStart(2, '0')}` : ''}`
}

/** `11 executions · 9 with counted runs, 2 without · 81 counted runs ·
 *  Sep 22 – Sep 28 · times in UTC−3`. */
export function summaryText(
  points: TrendPoint[],
  period: TrendPeriod = DEFAULT_PERIOD,
  offset = utcOffsetText(),
  now = new Date(),
) {
  const withRuns = points.filter(counted).length
  const without = points.length - withRuns
  const runs = points.reduce((total, point) => total + point.counted, 0)
  const first = points[0] ? formatDay(points[0].started_at, now) : null
  const last = points.at(-1) ? formatDay(points.at(-1)?.started_at, now) : null
  return [
    `${plural(points.length, 'execution')} ${periodPhrase(period, now)}`,
    `${withRuns} with counted runs${without ? `, ${without} without` : ''}`,
    plural(runs, 'counted run'),
    first ? (last && last !== first ? `${first} – ${last}` : first) : null,
    `times in ${offset}`,
  ]
    .filter(Boolean)
    .join(' · ')
}

/** Executions in a stack only because their workers matched it. */
export function stackNote(points: TrendPoint[], stack: string) {
  // Only a named stack needs it: `any` shows every execution anyway.
  if (stack === ANY_STACK) return null
  const matched = points.filter((point) => point.stack.matched_by_workers)
  if (matched.length === 0) return null
  const name = matched[0].stack.name
  return `${matched.length} of these ran before the Console recorded stacks; they are in because they ran the same workers as ${name}.`
}

const WHERE_PHRASE: Record<TrendPoint['source']['kind'], string> = {
  local: 'on this harness',
  docker: 'in Docker',
  github: 'on GitHub',
}

/** Why there is nothing to draw: no execution has a counted run. The
 *  latest execution goes between `before` and `after`, as a link. */
export function emptyText(points: TrendPoint[]) {
  const point = points.at(-1)
  if (!point) return null
  const tail = 'A trend starts at two executions with counted runs.'
  if (points.length === 1)
    return {
      before: 'This series has one execution, ',
      point,
      after:
        ` ${WHERE_PHRASE[point.source.kind]}, and none of its ${plural(point.runs, 'run')} counted. ${point.reason ?? ''} ${tail}`.replace(
          /\s+/g,
          ' ',
        ),
    }
  return {
    before: `None of this series’ ${points.length} executions has a counted run yet; the latest is `,
    point,
    after: `. ${tail}`,
  }
}

/** The versions under the panel: iii, the Harness, the stack. */
export function versionsText(point: TrendPoint) {
  const workers = point.workers ? Object.keys(point.workers).length : null
  return [
    `iii ${point.engine ?? 'not recorded'}`,
    `harness ${point.workers?.harness ?? 'not recorded'}`,
    `stack ${stackText(point)}${workers ? ` (${workers} workers)` : ''}`,
  ].join(' · ')
}

/* --------------------------------------------------------------- groups */

export type TrendGroupBy = 'execution' | 'day' | 'release'

export const GROUP_CHOICES: Array<{
  value: TrendGroupBy
  label: string
  sub: string
}> = [
  { value: 'execution', label: 'None', sub: 'one point per execution' },
  { value: 'day', label: 'Day', sub: 'one point per day, on your clock' },
  {
    value: 'release',
    label: 'Harness release',
    sub: 'one point per Harness version, in the order they first ran',
  },
]

export type TrendGroup = {
  by: Exclude<TrendGroupBy, 'execution'>
  /** The day (`2026-09-23`) or the Harness version. */
  key: string
  label: string
  /** Its executions, oldest first. */
  members: TrendPoint[]
  /** What differs between its first and its last execution: its measures
   *  mix those versions. */
  mixed: TrendChange[]
}

export function groupFromParams(params: URLSearchParams): TrendGroupBy {
  const group = params.get('group')
  return group === 'day' || group === 'release' ? group : 'execution'
}

export function withGroup(params: URLSearchParams, group: TrendGroupBy) {
  if (group !== 'execution') params.set('group', group)
  return params
}

/** Executions with no recorded Harness version: no release holds them. */
export function unversioned(points: TrendPoint[]) {
  return points.filter((point) => !point.workers?.harness)
}

/** One point per day or per Harness release (an execution with no recorded
 *  version is in none); each execution as it is when grouping by execution. */
export function groupPoints(
  points: TrendPoint[],
  by: TrendGroupBy,
  now = new Date(),
): TrendPoint[] {
  if (by === 'execution') return points
  const groups = new Map<string, TrendPoint[]>()
  for (const point of points) {
    const key =
      by === 'day'
        ? localDay(new Date(point.started_at))
        : point.workers?.harness
    if (!key) continue
    const members = groups.get(key)
    if (members) members.push(point)
    else groups.set(key, [point])
  }
  return [...groups].map(([key, members]) =>
    consolidate(
      by,
      key,
      by === 'day' ? formatDay(members[0].started_at, now) : `harness ${key}`,
      members,
    ),
  )
}

/** A group's measures over its counted runs: means weighted by each
 *  execution's counted runs, completed and planned runs per execution (a
 *  sum would grow with the executions), the function calls summed. Its
 *  versions, stack and plan are its last execution's. */
function consolidate(
  by: TrendGroup['by'],
  key: string,
  label: string,
  members: TrendPoint[],
): TrendPoint {
  const last = members[members.length - 1]
  const valid = members.filter(
    (member): member is TrendPoint & { measures: TrendMeasures } =>
      counted(member) && member.measures !== null,
  )
  const weighted = (pick: (measures: TrendMeasures) => number | null) => {
    let sum = 0
    let weight = 0
    for (const member of valid) {
      const value = pick(member.measures)
      if (value === null) continue
      sum += value * member.counted
      weight += member.counted
    }
    return weight ? sum / weight : null
  }
  const total = (pick: (measures: TrendMeasures) => number | null) => {
    const values = valid
      .map((member) => pick(member.measures))
      .filter((value): value is number => value !== null)
    return values.length ? values.reduce((sum, value) => sum + value, 0) : null
  }
  const ids = [
    ...new Set([...members].reverse().flatMap((m) => m.tests.map((t) => t.id))),
  ]
  const order = new Map(last.tests.map((test, index) => [test.id, index]))
  ids.sort(
    (one, two) =>
      (order.get(one) ?? Number.MAX_SAFE_INTEGER) -
      (order.get(two) ?? Number.MAX_SAFE_INTEGER),
  )
  const tests = ids.map((id): TrendTest => {
    const entries = members.flatMap((member) =>
      member.tests.filter((test) => test.id === id),
    )
    const scores = entries
      .map((test) => test.score)
      .filter((score): score is number => score !== null)
    const has = (state: TrendTestState) =>
      entries.some((test) => test.state === state)
    return {
      id,
      state: scores.length
        ? 'scored'
        : has('no_score')
          ? 'no_score'
          : has('technical_invalid')
            ? 'technical_invalid'
            : 'not_run',
      score: scores.length
        ? scores.reduce((sum, score) => sum + score, 0) / scores.length
        : null,
      behavior_sha256: entries[entries.length - 1]?.behavior_sha256 ?? null,
    }
  })
  return {
    ...last,
    execution_id: `${by}:${key}`,
    label: null,
    runs: members.reduce((sum, member) => sum + member.runs, 0),
    counted: members.reduce((sum, member) => sum + member.counted, 0),
    reason: valid.length
      ? null
      : members.length === 1
        ? last.reason
        : `None of its ${members.length} executions has a counted run.`,
    measures: valid.length
      ? {
          score_mean: weighted((m) => m.score_mean),
          completed: round1(
            valid.reduce((sum, m) => sum + m.measures.completed, 0) /
              valid.length,
          ),
          planned: round1(
            valid.reduce((sum, m) => sum + m.measures.planned, 0) /
              valid.length,
          ),
          duration_ms_mean: weighted((m) => m.duration_ms_mean),
          input_tokens_mean: weighted((m) => m.input_tokens_mean),
          function_calls_mean: weighted((m) => m.function_calls_mean),
          function_calls: total((m) => m.function_calls),
          function_call_errors: total((m) => m.function_call_errors),
          turns_mean: weighted((m) => m.turns_mean),
        }
      : null,
    tests,
    group: {
      by,
      key,
      label,
      members,
      mixed:
        members.length > 1
          ? changesBetween(members, 0, members.length - 1)
          : [],
    },
  }
}

/** Under the plot: where each day starts, or each release's version. */
export function axisMarks(points: TrendPoint[], now = new Date()) {
  if (points[0]?.group?.by === 'release')
    return points.map((point, index) => ({
      index,
      text: point.group?.key ?? '',
    }))
  return dayMarks(points, now)
}

/** The execution a point stands for when it is set as the baseline: a
 *  group's last one. */
export function baselineId(point: TrendPoint) {
  return point.group
    ? point.group.members[point.group.members.length - 1].execution_id
    : point.execution_id
}
