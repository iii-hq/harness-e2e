// The Trends first screen: a step per Harness release with Release Control
// results, the latest read against the previous one over what both ran
// alike. The worker answers e2e::dashboard::release-overview-get; the
// measures' direction, the wording and the stand-ins the Trends chart draws
// are worked out here.

import { deltaDirection, deltaTone } from '@/design-system'
import {
  ANY,
  deltaFormat,
  deltaOf,
  profileText,
  releaseText,
  seriesText,
  stackOptionText,
  type TrendChange,
  type TrendMeasures,
  type TrendMetric,
  type TrendMetricId,
  type TrendPoint,
  type TrendRole,
} from '@/lib/trends'

/* ------------------------------------------------------------- contract */

/** What the verdict compares like with like: a series and its stack. */
export type Combination = {
  /** A suite id, or the sha256 of an unsaved suite. */
  suite: string
  suite_label: string
  provider: string
  model: string
  profile: string | null
  /** A stack name, or `not_recorded`. */
  stack: string
}

/** Combinations, each weighing the same however often it ran; shares are
 *  fractions, 0..1. */
export type Aggregate = {
  /** The mean of their means; `completed`, `planned`, `function_calls`
   *  and `function_call_errors` added up. */
  measures: TrendMeasures
  /** The mean of their `completed / planned`. */
  completed_share: number | null
  /** The mean of their `function_call_errors / function_calls`. */
  error_rate: number | null
}

export type ReleaseStep = {
  previous: string
  /** Over what both releases ran alike; null when nothing was. */
  latest_aggregate: Aggregate | null
  previous_aggregate: Aggregate | null
}

export type ReleasePoint = {
  /** One plain semver, without a leading `v`. */
  release: string
  first_started_at: string
  /** Oldest first. */
  executions: string[]
  combinations: Combination[]
  aggregate: Aggregate | null
  /** Combinations the release before it did not run. */
  joined: Combination[]
  /** Combinations the release before it ran and this one did not. */
  left: Combination[]
  /** Planned tests its executions never ran. */
  tests_not_run: number
  /** Of its newest execution. */
  engine: string | null
  workers: Record<string, string> | null
  /** Against the release before it; null for the first. */
  step: ReleaseStep | null
}

export type CombinationDelta = {
  combination: Combination
  latest: TrendMeasures
  previous: TrendMeasures
  /** Tests that ran but do not compare: their definition changed, or one
   *  release counted no run of them. */
  tests_left_out: string[]
}

export type LeftOutCombination = {
  combination: Combination
  /** The only release of the two that ran it; null: ran on both, no test
   *  kept its definition. */
  only_in: string | null
}

export type Verdict = {
  latest: string
  previous: string
  latest_started_at: string
  /** Over `compared`; null when it is empty. */
  latest_aggregate: Aggregate | null
  previous_aggregate: Aggregate | null
  compared: CombinationDelta[]
  left_out: LeftOutCombination[]
}

/** A local or Docker execution, beside a release it never counts in. */
export type LocalMark = {
  execution_id: string
  role: TrendRole
  started_at: string
  release: string
  /** The Harness it recorded (`@sha7`, a version); null: not recorded. */
  ran: string | null
  suite_label: string
  measures: TrendMeasures | null
}

/** GitHub executions without a Release Control id. */
export type OutsideCount = { executions: number; latest_at: string | null }

export type ReleaseOverview = {
  /** In semver order, the latest last. */
  releases: ReleasePoint[]
  /** The latest release against the previous one; null with fewer than two. */
  verdict: Verdict | null
  /** Oldest first. */
  locals: LocalMark[]
  outside_release_control: OutsideCount
  /** Counted Release Control executions whose Harness is not one plain
   *  release. */
  unversioned: number
}

/** What the Trends chart draws: a point that carries the aggregate its
 *  measures were read from, and the step it is read against. */
export type OverviewPoint = TrendPoint & {
  aggregate: Aggregate | null
  step: ReleaseStep | null
}

/* -------------------------------------------------------------- wording */

/** `Regression · deepseek/deepseek-flash · profile none · stack default`. */
export function combinationText(combination: Combination) {
  return `${combination.suite_label} · ${seriesText(combination)} · stack ${stackOptionText(combination.stack)}`
}

/* -------------------------------------------------------------- measures */

/** An aggregate of one combination's measures, its shares worked out the
 *  way the worker does for several. */
export function aggregateOf(measures: TrendMeasures): Aggregate {
  const { completed, planned, function_calls, function_call_errors } = measures
  return {
    measures,
    completed_share: planned > 0 ? completed / planned : null,
    error_rate:
      function_calls && typeof function_call_errors === 'number'
        ? function_call_errors / function_calls
        : null,
  }
}

const aggregateOfPoint = (point: TrendPoint) =>
  (point as Partial<OverviewPoint>).aggregate

/** The metric as an overview point reads it: tests completed and the error
 *  rate come from the combinations' shares, never from the summed counts,
 *  so that each combination weighs the same. */
export function overviewMetric(metric: TrendMetric): TrendMetric {
  const share =
    (key: 'completed_share' | 'error_rate') => (point: TrendPoint) => {
      const value = aggregateOfPoint(point)?.[key]
      return typeof value === 'number' ? value * 100 : null
    }
  if (metric.id === 'completed')
    return {
      ...metric,
      note: 'share of the planned runs that completed',
      value: share('completed_share'),
      figure: (value) => `${value.toFixed(1)}%`,
      axis: (value) => `${Math.round(value)}%`,
      delta: 'percent_points',
    }
  if (metric.id === 'error_rate')
    return { ...metric, value: share('error_rate') }
  return metric
}

/** The signed difference of a measure between two aggregates, rounded like
 *  its text; null when a side is missing or has no value. */
export function aggregateDelta(
  metric: TrendMetric,
  latest: Aggregate | null,
  previous: Aggregate | null,
): number | null {
  if (!latest || !previous) return null
  const standIn = (aggregate: Aggregate) =>
    ({ measures: aggregate.measures, aggregate }) as unknown as TrendPoint
  return deltaOf(overviewMetric(metric), standIn(latest), standIn(previous))
}

export type MetricMove = { metric: TrendMetric; delta: number }
export type VerdictSummary = {
  better: MetricMove[]
  worse: MetricMove[]
  same: TrendMetricId[]
}

/** Which of the metrics moved for the better, for the worse or not at all,
 *  in the order given. One with no direction, or no value on a side, is in
 *  none of them. */
export function moves(
  latest: Aggregate,
  previous: Aggregate,
  metrics: TrendMetric[],
): VerdictSummary {
  const summary: VerdictSummary = { better: [], worse: [], same: [] }
  for (const metric of metrics) {
    if (metric.better === 'neither') continue
    const delta = aggregateDelta(metric, latest, previous)
    if (delta === null) continue
    if (delta === 0) {
      summary.same.push(metric.id)
      continue
    }
    const tone = deltaTone(deltaDirection(delta), metric.better)
    if (tone === 'positive') summary.better.push({ metric, delta })
    else if (tone === 'negative') summary.worse.push({ metric, delta })
  }
  return summary
}

/** `moves` of the verdict's two aggregates; none when a side has none. */
export function verdictSummary(
  verdict: Verdict,
  metrics: TrendMetric[],
): VerdictSummary {
  const { latest_aggregate, previous_aggregate } = verdict
  return latest_aggregate && previous_aggregate
    ? moves(latest_aggregate, previous_aggregate, metrics)
    : { better: [], worse: [], same: [] }
}

/* ------------------------------------------------------------- timeline */

export type Timeline = {
  points: OverviewPoint[]
  /** What joined or left at each point; none at a local run. */
  changes: TrendChange[][]
  /** The latest release's point; -1 without one. */
  latest: number
}

const UNPLACED = {
  provider: ANY,
  model: ANY,
  profile: null,
  stack: { name: null, matched_by_workers: false },
  planned: null,
  reason: null,
  tests: [],
} satisfies Partial<TrendPoint>

function releasePoint(release: ReleasePoint): OverviewPoint {
  const text = releaseText(release.release)
  return {
    ...UNPLACED,
    execution_id: `release:${release.release}`,
    label: null,
    started_at: release.first_started_at,
    source: { kind: 'github', role: 'release_control' },
    engine: release.engine,
    workers: release.workers,
    release: release.release,
    runs: release.executions.length,
    counted: release.executions.length,
    measures: release.aggregate?.measures ?? null,
    group: { by: 'release', key: text, label: text, members: [], mixed: [] },
    aggregate: release.aggregate,
    step: release.step,
  }
}

function localPoint(local: LocalMark, measures: TrendMeasures): OverviewPoint {
  return {
    ...UNPLACED,
    execution_id: local.execution_id,
    label: `${local.suite_label} · ${local.ran ? `ran harness ${local.ran}` : 'Harness version not recorded'}`,
    started_at: local.started_at,
    source: {
      kind: local.role === 'docker' ? 'docker' : 'local',
      role: local.role,
    },
    engine: null,
    workers: local.ran ? { harness: local.ran } : null,
    // Beside its release on the line, never grouped into it.
    release: null,
    runs: 1,
    counted: 1,
    measures,
    aggregate: aggregateOf(measures),
    step: null,
  }
}

/** A combination that joined or left at a release, as the chart marks a
 *  change of its tests. */
const mixChange =
  (text: 'joined' | 'left') =>
  (combination: Combination): TrendChange => ({
    kind: 'tests',
    major: true,
    name: combinationText(combination),
    text,
    note: null,
    compare: null,
  })

/** The release line: each release, then the measured local runs that ran
 *  with it, oldest first. */
export function timeline(overview: ReleaseOverview): Timeline {
  const points: OverviewPoint[] = []
  const changes: TrendChange[][] = []
  // shortcut: a local whose release has no Release Control point is left out; place it by semver when that shows up
  for (const release of overview.releases) {
    points.push(releasePoint(release))
    changes.push([
      ...release.joined.map(mixChange('joined')),
      ...release.left.map(mixChange('left')),
    ])
    for (const local of overview.locals)
      if (local.release === release.release && local.measures) {
        points.push(localPoint(local, local.measures))
        changes.push([])
      }
  }
  const latest =
    overview.verdict?.latest ?? overview.releases.at(-1)?.release ?? null
  return {
    points,
    changes,
    latest: points.findIndex(
      (point) => point.execution_id === `release:${latest}`,
    ),
  }
}

/** A release's step on a measure, as its point says it: `−2.3 vs v1.8.35`;
 *  null without a step or a value on either side. */
export function stepText(metric: TrendMetric, point: OverviewPoint) {
  const { step } = point
  if (!step) return null
  const delta = aggregateDelta(
    metric,
    step.latest_aggregate,
    step.previous_aggregate,
  )
  if (delta === null) return null
  const text =
    delta === 0
      ? 'no change'
      : `${delta > 0 ? '+' : '−'}${deltaFormat(overviewMetric(metric))(Math.abs(delta))}`
  return `${text} vs ${releaseText(step.previous)}`
}

/* ---------------------------------------------------------------- scope */

/** What the verdict compared and what it left out, and why. */
export function scopeText(verdict: Verdict) {
  const { latest, previous, compared, left_out } = verdict
  if (compared.length === 0)
    return `No suite ran on both ${releaseText(latest)} and ${releaseText(previous)}`
  const first = compared[0].combination
  const name = (combination: Combination) =>
    combination.provider === first.provider &&
    combination.model === first.model &&
    profileText(combination.profile) === profileText(first.profile) &&
    combination.stack === first.stack
      ? combination.suite_label
      : combinationText(combination)
  const group = (only_in: string | null, reason: string) =>
    left_out
      .filter((item) => item.only_in === only_in)
      .map((item) => `${name(item.combination)} (${reason})`)
  const notCompared = [
    ...group(latest, `not run on ${releaseText(previous)}`),
    ...group(previous, `not run on ${releaseText(latest)}`),
    ...group(null, "every test's definition changed"),
  ]
  const ranOnBoth = `Compared: ${compared.map((item) => name(item.combination)).join(', ')} (ran on both).`
  return notCompared.length === 0
    ? ranOnBoth
    : `${ranOnBoth} Not compared: ${notCompared.join('; ')}.`
}
