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
  /** A stack name, 'not_recorded' or 'any'; default: the latest
   *  execution's stack. */
  stack?: string
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
  completed: number
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
  source: {
    kind: 'local' | 'docker' | 'github'
    run_id?: number
    run_attempt?: number
    release_control_execution_id?: string
    url?: string
  }
  stack: { name: string | null; matched_by_workers: boolean }
  engine: string | null
  runner: string | null
  /** Identity per worker, runner and engine left out; null = not recorded. */
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
}

export type TrendsResponse = {
  /** Every series, latest execution first. */
  series: TrendSeries[]
  selected: TrendSeriesKey | null
  stack: string
  /** Of the selected series, then 'not_recorded' (if any) and 'any'. */
  stacks: Array<{ name: string; executions: number }>
  /** Oldest first. */
  points: TrendPoint[]
}

export type VersionCompareRequest = { name: string; base: string; head: string }
export type VersionCompareResponse = {
  url: string
  total_commits: number | null
}

export const ANY_STACK = 'any'
export const NOT_RECORDED_STACK = 'not_recorded'

/* --------------------------------------------------------------- series */

/** `deepseek/deepseek-flash`; a model id may already carry its provider. */
export function seriesModel(key: Pick<TrendSeriesKey, 'provider' | 'model'>) {
  return key.model.startsWith(`${key.provider}/`)
    ? key.model
    : `${key.provider}/${key.model}`
}

export function profileText(profile: string | null) {
  return profile || 'none'
}

export function sameSeries(a: TrendSeriesKey, b: TrendSeriesKey | null) {
  return (
    b !== null &&
    a.suite === b.suite &&
    a.provider === b.provider &&
    a.model === b.model &&
    (a.profile || null) === (b.profile || null)
  )
}

const WHERE: Record<TrendWhere, string> = {
  harness: 'This harness',
  docker: 'Docker',
  github: 'GitHub',
}

export function seriesWhere(series: Pick<TrendSeries, 'where'>) {
  return series.where.map((where) => WHERE[where]).join(', ')
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

export function trendsParams(
  key: TrendSeriesKey | null,
  stack: string | null,
): URLSearchParams {
  const params = new URLSearchParams()
  if (key) {
    params.set('suite', key.suite)
    params.set('provider', key.provider)
    params.set('model', key.model)
    params.set('profile', key.profile ?? '')
  }
  if (stack) params.set('stack', stack)
  return params
}

/* -------------------------------------------------------------- a point */

export function pointTime(point: Pick<TrendPoint, 'started_at'>) {
  return formatDateTime(point.started_at)
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
  | 'iii'
  | 'harness'
  | 'runner'
  | 'tests'
  | 'stack'
  | 'worker'
  | 'definition'

export type TrendChange = {
  kind: ChangeKind
  /** A filled diamond: iii, the Harness, the runner, the tests or the
   *  stack; another worker or a definition is outlined. */
  major: boolean
  /** `iii`, `harness-e2e`, a worker, `tests`, `stack` or a test id. */
  name: string
  text: string
  note: string | null
  /** What to ask e2e::dashboard::version-compare for the commits between. */
  compare: VersionCompareRequest | null
}

export const CHANGE_KIND_TEXT: Record<ChangeKind, string> = {
  iii: 'engine',
  harness: 'the Harness under test',
  runner: 'runner',
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
  const base = commitOf(from)
  const head = commitOf(to)
  if (base && head)
    return {
      ...change,
      compare: { name, base, head },
      note: to.includes('*')
        ? '* the checkout also had uncommitted edits'
        : null,
    }
  // A release on one side and a checkout on the other: no range to count.
  if (base || head) return change
  return { ...change, compare: { name, base: from, head: to } }
}

function lastBefore(
  points: TrendPoint[],
  index: number,
  test: (point: TrendPoint) => boolean,
) {
  for (let at = index - 1; at >= 0; at -= 1)
    if (test(points[at])) return points[at]
  return null
}

/** What changed at point `index`: iii, the runner and the stack against the
 *  execution right before it; workers, planned tests and definitions against
 *  the last one that recorded them. */
export function changesAt(points: TrendPoint[], index: number): TrendChange[] {
  if (index <= 0) return []
  const current = points[index]
  const previous = points[index - 1]
  const out: TrendChange[] = []
  if (previous.engine && current.engine && previous.engine !== current.engine)
    out.push(versionChange('iii', 'iii', previous.engine, current.engine))
  if (previous.runner && current.runner && previous.runner !== current.runner)
    out.push(
      versionChange('runner', 'harness-e2e', previous.runner, current.runner),
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
  const withWorkers = lastBefore(points, index, (point) => !!point.workers)
  if (current.workers && withWorkers?.workers) {
    const before = withWorkers.workers
    const after = current.workers
    const names = [...new Set([...Object.keys(before), ...Object.keys(after)])]
      .filter((name) => name !== 'compose')
      .sort()
    for (const name of names) {
      const from = before[name] ?? null
      const to = after[name] ?? null
      if (from !== to)
        out.push(
          versionChange(
            name === 'harness' ? 'harness' : 'worker',
            name,
            from,
            to,
          ),
        )
    }
  }
  const withPlan = lastBefore(points, index, (point) => !!point.planned)
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
  const withCounted = lastBefore(points, index, counted)
  if (counted(current) && withCounted) {
    for (const test of current.tests) {
      const before = withCounted.tests.find((item) => item.id === test.id)
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

const LANE_RANK: ChangeKind[] = ['iii', 'harness', 'runner', 'tests', 'stack']

/** `0.14.0@2f8826a*` → `@2f8826a*`: the lane has no room for both. */
function shortRunner(identity: string) {
  const at = identity.indexOf('@')
  return at > 0 ? identity.slice(at) : identity
}

/** The label over a diamond: the first major change by iii > harness >
 *  runner > tests > stack, and how many more. Minor changes go unlabelled. */
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
    first.kind === 'iii'
      ? `iii ${to}`
      : first.kind === 'harness'
        ? `harness ${to}`
        : first.kind === 'runner'
          ? `runner ${shortRunner(to)}`
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
  const where =
    change.name === 'iii' || change.name === 'harness-e2e'
      ? ''
      : ' in iii-hq/workers'
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
  if (metric.delta === 'tests') return b - a
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

/** The latest point with a value and the one with a value before it. */
export function latestPair(points: TrendPoint[], metric: TrendMetric) {
  const valued = points.filter((point) => metric.value(point) !== null)
  return {
    current: valued.at(-1) ?? null,
    previous: valued.at(-2) ?? null,
  }
}

/** The previous execution with a counted run, what deltas are against. */
export function previousCounted(points: TrendPoint[], index: number) {
  return lastBefore(points, index, counted)
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
    lo = 0
    hi = Math.max(
      hi,
      ...points.map(
        (point) => point.measures?.planned ?? point.planned?.length ?? 0,
      ),
    )
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
export function dayMarks(points: Pick<TrendPoint, 'started_at'>[]) {
  const marks: Array<{ index: number; text: string }> = []
  let last = ''
  points.forEach((point, index) => {
    const text = formatDay(point.started_at)
    if (text !== last) marks.push({ index, text })
    last = text
  })
  return marks
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
export function summaryText(points: TrendPoint[], offset = utcOffsetText()) {
  const withRuns = points.filter(counted).length
  const without = points.length - withRuns
  const runs = points.reduce((total, point) => total + point.counted, 0)
  const first = points[0] ? formatDay(points[0].started_at) : null
  const last = points.at(-1) ? formatDay(points.at(-1)?.started_at) : null
  return [
    plural(points.length, 'execution'),
    `${withRuns} with counted runs${without ? `, ${without} without` : ''}`,
    plural(runs, 'counted run'),
    first ? (last && last !== first ? `${first} – ${last}` : first) : null,
    `times in ${offset}`,
  ]
    .filter(Boolean)
    .join(' · ')
}

/** Executions in a stack only because their workers matched it. */
export function stackNote(points: TrendPoint[]) {
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

/** The versions under the panel: iii, the Harness, the runner, the stack. */
export function versionsText(point: TrendPoint) {
  const workers = point.workers ? Object.keys(point.workers).length + 1 : null
  return [
    `iii ${point.engine ?? 'not recorded'}`,
    `harness ${point.workers?.harness ?? 'not recorded'}`,
    `runner ${point.runner ?? 'not recorded'}`,
    `stack ${stackText(point)}${workers ? ` (${workers} workers)` : ''}`,
  ].join(' · ')
}
