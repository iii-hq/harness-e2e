// The Trends first screen: a step per Harness release with Release Control
// results, each combination's first completed execution read against the
// release before it over the tests both counted alike. The worker answers
// e2e::dashboard::release-overview-get with every delta, coverage count and,
// on the latest step, each combination's calibrated verdict; the wording and
// the line the Trends chart draws for one combination are worked out here.
// A measure that moved is only said to have moved: better or worse is the
// calibrated verdict's.

import { plural } from '@/lib/format'
import {
  changesAt,
  deltaFormat,
  deltaOf,
  profileText,
  releaseText,
  seriesText,
  stackOptionText,
  TREND_METRICS,
  type TrendChange,
  type TrendMeasures,
  type TrendMetric,
  type TrendMetricId,
  type TrendPoint,
  type TrendRole,
} from '@/lib/trends'
import type { VerdictSummary } from '@/lib/verdict'

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

/** A combination's first completed execution of a release: what its chart
 *  plots and its step compares. */
export type CombinationPoint = {
  combination: Combination
  point: TrendPoint
  /** Runs that failed in the E2E infrastructure. */
  infra_failed_runs: number
  /** Runs the subject failed: the Harness, its provider request or a
   *  budget. */
  harness_failed_runs: number
  /** Planned runs that never ran. */
  not_run: number
}

/** One side of a combination's step: its first completed execution, over
 *  the tests compared. */
export type Side = {
  execution: string
  /** Over the compared tests' runs; `planned` is their planned runs. */
  measures: TrendMeasures
  /** What Tests completed divides by. */
  counted_runs: number
}

export type VersionChange = {
  /** `iii` or `harness-e2e`. */
  name: string
  from: string
  to: string
}

export type TestLeftOutReason =
  | 'only_latest'
  | 'only_previous'
  | 'not_counted_latest'
  | 'not_counted_previous'
  | 'redefined'

export type LeftOutTest = { test: string; reason: TestLeftOutReason }

export type CombinationDelta = {
  combination: Combination
  latest: Side
  previous: Side
  /** Tests both sides counted under one definition. */
  compared_tests: number
  /** Points, rounded to 0.1; null when a side scored nothing. */
  score_delta: number | null
  /** Completed runs over counted runs, in percentage points, rounded to
   *  0.1. */
  completed_delta: number
  /** The iii engine and the runner, when they changed. */
  also_changed: VersionChange[]
  tests_left_out: LeftOutTest[]
  /** The latest against the previous over the tests compared, as
   *  `harness-e2e compare` reads them; only on the latest step. */
  calibrated?: VerdictSummary | null
}

export type CombinationLeftOutReason =
  | 'only_in_one_release'
  | 'nothing_alike'
  | 'no_completed_execution'
  | 'all_runs_infra_failed'

export type LeftOutCombination = {
  combination: Combination
  reason: CombinationLeftOutReason
  /** The only release of the two that counted a run of it. */
  only_in: string | null
  /** With `all_runs_infra_failed`: the other release's runs of it. */
  infra_failed_runs: number | null
}

/** A release against the one before it, combination by combination, in the
 *  worker's order. */
export type ReleaseStep = {
  previous: string
  compared: CombinationDelta[]
  left_out: LeftOutCombination[]
}

export type ReleasePoint = {
  /** One plain semver, without a leading `v`. */
  release: string
  /** Oldest first. */
  executions: string[]
  combinations: Combination[]
  /** Each combination's first completed execution. */
  firsts: CombinationPoint[]
  /** Against the release before it; null for the first. */
  step: ReleaseStep | null
}

/** The latest release against the previous one: the latest release's step. */
export type Verdict = {
  latest: string
  previous: string
  latest_started_at: string
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
  /** The combination whose chart it sits on. */
  combination: Combination
  /** `measures` is null when nothing counted. */
  point: TrendPoint
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

/* -------------------------------------------------------------- metrics */

/** The metric as the overview reads it: Tests completed is the share of the
 *  counted runs that completed, and the score axis spans at least 20 points
 *  so that a few points do not fill the chart. */
export function overviewMetric(metric: TrendMetric): TrendMetric {
  if (metric.id === 'completed')
    return {
      ...metric,
      note: 'share of the counted runs that completed',
      value: (point) =>
        point.measures && point.counted > 0
          ? (point.measures.completed / point.counted) * 100
          : null,
      figure: (value) => `${value.toFixed(1)}%`,
      axis: (value) => `${Math.round(value)}%`,
      delta: 'percent_points',
    }
  if (metric.id === 'score') return { ...metric, minSpan: 20 }
  return metric
}

const metrics = (ids: TrendMetricId[]) =>
  TREND_METRICS.filter((metric) => ids.includes(metric.id)).map(overviewMetric)

/** The main measures as the overview reads them. */
export const MAIN_METRICS = TREND_METRICS.filter(
  (metric) => metric.tier === 'main',
).map(overviewMetric)
/** The two the worker reads a step's delta of. */
export const VERDICT_METRICS = metrics(['score', 'completed'])
/** What the step cost, beside them. */
export const COST_METRICS = metrics(['duration', 'input_tokens', 'turns'])

/** A combination's change on a measure: the worker's for score and Tests
 *  completed, else read from both sides' measures; null without a value. */
export function comparedDelta(
  metric: TrendMetric,
  item: CombinationDelta,
): number | null {
  if (metric.id === 'score') return item.score_delta
  if (metric.id === 'completed') return item.completed_delta
  const side = (side: Side) =>
    ({ measures: side.measures, counted: side.counted_runs }) as TrendPoint
  return deltaOf(overviewMetric(metric), side(item.latest), side(item.previous))
}

/** `+2.0 pts`, `−11.1 pts`, `no change`. */
function signedText(metric: TrendMetric, delta: number) {
  return delta === 0
    ? 'no change'
    : `${delta > 0 ? '+' : '−'}${deltaFormat(overviewMetric(metric))(Math.abs(delta))}`
}

/* -------------------------------------------------------------- wording */

/** `Regression · deepseek/deepseek-flash · profile none · stack default`. */
export function combinationText(combination: Combination) {
  return `${combination.suite_label} · ${seriesText(combination)} · stack ${stackOptionText(combination.stack)}`
}

/** A combination's identity, as the worker keys it. */
export function combinationKey(combination: Combination) {
  return [
    combination.suite,
    combination.provider,
    combination.model,
    combination.profile ?? '',
    combination.stack,
  ].join('|')
}

/** The latest release's step; null with fewer than two releases. */
export function latestStep(overview: ReleaseOverview) {
  return overview.releases.at(-1)?.step ?? null
}

/** What the Setup picker offers: the latest step's compared combinations,
 *  then its left-out ones, in the worker's order; with one release, what it
 *  ran. */
export function combinationOptions(overview: ReleaseOverview): Combination[] {
  const step = latestStep(overview)
  if (!step)
    return (overview.releases.at(-1)?.firsts ?? []).map(
      (first) => first.combination,
    )
  return [
    ...step.compared.map((item) => item.combination),
    ...step.left_out.map((item) => item.combination),
  ]
}

/** The combination the headline reads: the first the calibrated verdict
 *  calls worse, else better, else the first compared; null when nothing
 *  was. */
export function decidingCombination(
  overview: ReleaseOverview,
): Combination | null {
  const compared = latestStep(overview)?.compared ?? []
  const called = (verdict: string) =>
    compared.find((item) => item.calibrated?.verdict === verdict)
  return (
    (called('worse') ?? called('better') ?? compared[0])?.combination ?? null
  )
}

/** The combination the overview opens on: the deciding one, else the first
 *  the latest release ran, else the first left out. */
export function defaultCombination(
  overview: ReleaseOverview,
): Combination | null {
  return (
    decidingCombination(overview) ??
    overview.releases.at(-1)?.firsts[0]?.combination ??
    latestStep(overview)?.left_out[0]?.combination ??
    null
  )
}

/** The compared combinations that have a calibrated verdict. */
export function calibrated(step: ReleaseStep) {
  return step.compared.flatMap((item) =>
    item.calibrated?.verdict
      ? [{ combination: item.combination, summary: item.calibrated }]
      : [],
  )
}

/** Why a combination has no calibrated verdict. */
export function uncalibratedText(item: CombinationDelta) {
  return (
    item.calibrated?.unavailable ??
    'The worker sent no calibrated verdict for it.'
  )
}

/** Why a test was left out of a step. */
export function testReasonText(
  reason: TestLeftOutReason,
  latest: string,
  previous: string,
) {
  switch (reason) {
    case 'redefined':
      return 'definition changed'
    case 'not_counted_latest':
      return `no counted run on ${releaseText(latest)}`
    case 'not_counted_previous':
      return `no counted run on ${releaseText(previous)}`
    case 'only_latest':
      return `only in ${releaseText(latest)}`
    case 'only_previous':
      return `only in ${releaseText(previous)}`
  }
}

/** Why a combination was left out of the step of `release`. */
export function leftOutText(
  overview: ReleaseOverview,
  item: LeftOutCombination,
  release: string,
  previous: string,
) {
  switch (item.reason) {
    case 'only_in_one_release':
      return item.only_in === release
        ? `only in ${releaseText(release)}`
        : `not run on ${releaseText(release)}`
    case 'nothing_alike':
      return 'no test comparable'
    case 'no_completed_execution': {
      const key = combinationKey(item.combination)
      const missing = [release, previous].find(
        (name) =>
          !overview.releases
            .find((point) => point.release === name)
            ?.firsts.some((first) => combinationKey(first.combination) === key),
      )
      return `no completed execution on ${releaseText(missing ?? release)}`
    }
    case 'all_runs_infra_failed':
      return `ran, ${plural(item.infra_failed_runs ?? 0, 'run')} failed in the E2E infrastructure`
  }
}

/** `Also changed: iii A → B · harness-e2e C → D`; null when nothing did. */
export function alsoChangedText(changes: VersionChange[]) {
  return changes.length === 0
    ? null
    : `Also changed: ${changes.map((change) => `${change.name} ${change.from} → ${change.to}`).join(' · ')}`
}

/** How much of the latest step compared: `2 of 4 setups compared · 1 not
 *  run on v1.8.36 · 4 runs failed in the E2E infrastructure (…)`; null
 *  without a step. */
export function coverageText(overview: ReleaseOverview) {
  const last = overview.releases.at(-1)
  const step = last?.step
  if (!last || !step) return null
  const total = step.compared.length + step.left_out.length
  const notRun = step.left_out.filter(
    (item) =>
      item.reason === 'only_in_one_release' && item.only_in === step.previous,
  ).length
  // Runs of the latest release that failed in the E2E infrastructure: in
  // each of its first executions, compared or not, and in a setup none of
  // whose runs counted.
  const failed = [
    ...last.firsts.map(
      (first) => [first.combination, first.infra_failed_runs] as const,
    ),
    ...step.left_out
      .filter(
        (item) =>
          item.reason === 'all_runs_infra_failed' &&
          item.only_in === step.previous,
      )
      .map((item) => [item.combination, item.infra_failed_runs ?? 0] as const),
  ].filter(([, runs]) => runs > 0)
  const runs = failed.reduce((sum, [, count]) => sum + count, 0)
  const names = [
    ...new Set(failed.map(([combination]) => combination.suite_label)),
  ]
  return [
    `${step.compared.length} of ${plural(total, 'setup')} compared`,
    notRun > 0 ? `${notRun} not run on ${releaseText(last.release)}` : null,
    runs > 0
      ? `${plural(runs, 'run')} failed in the E2E infrastructure (${names.join(', ')})`
      : null,
  ]
    .filter(Boolean)
    .join(' · ')
}

/** What a step compared and what it left out, and why. */
export function scopeText(overview: ReleaseOverview, release: ReleasePoint) {
  const step = release.step
  if (!step) return null
  const { compared, left_out } = step
  const first = (compared[0] ?? left_out[0])?.combination
  const name = (combination: Combination) =>
    first &&
    combination.provider === first.provider &&
    combination.model === first.model &&
    profileText(combination.profile) === profileText(first.profile) &&
    combination.stack === first.stack
      ? combination.suite_label
      : combinationText(combination)
  const notCompared = left_out.map(
    (item) =>
      `${name(item.combination)} (${leftOutText(overview, item, release.release, step.previous)})`,
  )
  return [
    compared.length > 0
      ? `Compared: ${compared.map((item) => name(item.combination)).join(', ')}.`
      : `No setup compared between ${releaseText(release.release)} and ${releaseText(step.previous)}.`,
    notCompared.length > 0 ? `Not compared: ${notCompared.join('; ')}.` : null,
  ]
    .filter(Boolean)
    .join(' ')
}

/* ----------------------------------------------------------------- line */

/** A release's step for one combination: compared or left out. */
export type CombinationStep = {
  item: CombinationDelta | LeftOutCombination
  release: string
  previous: string
}

export type CombinationLine = {
  /** Each release's first completed execution of the combination, with
   *  the measured local runs of it beside its release, oldest first. */
  points: TrendPoint[]
  /** What changed at each point; none at a local run. */
  changes: TrendChange[][]
  /** Each release point's step; null at a local run and the first release. */
  steps: Array<CombinationStep | null>
  /** The latest release's point; -1 when it did not run the combination. */
  latest: number
}

export function isCompared(
  item: CombinationDelta | LeftOutCombination,
): item is CombinationDelta {
  return 'latest' in item
}

/** The step item of the combination keyed `key` in `step`. */
export function stepItem(step: ReleaseStep, key: string) {
  return (
    step.compared.find((item) => combinationKey(item.combination) === key) ??
    step.left_out.find((item) => combinationKey(item.combination) === key) ??
    null
  )
}

/** What a step changed that the two executions' records do not show: tests
 *  with no counted run, and the runner. */
function stepChanges(step: CombinationStep | null): TrendChange[] {
  if (!step || !isCompared(step.item)) return []
  const { item, release, previous } = step
  const notCounted = (reason: TestLeftOutReason, on: string) => {
    const tests = item.tests_left_out.filter((test) => test.reason === reason)
    return tests.length > 0
      ? `${plural(tests.length, 'test')} not counted on ${releaseText(on)}`
      : null
  }
  const text = [
    notCounted('not_counted_latest', release),
    notCounted('not_counted_previous', previous),
  ].filter(Boolean)
  const runner = item.also_changed.find(
    (change) => change.name === 'harness-e2e',
  )
  return [
    ...(text.length > 0
      ? [
          {
            kind: 'tests',
            major: true,
            name: 'tests',
            text: text.join(' · '),
            note: 'left out of the step: they failed in the E2E infrastructure or never ran',
            compare: null,
          } satisfies TrendChange,
        ]
      : []),
    ...(runner
      ? [
          {
            kind: 'worker',
            major: false,
            name: runner.name,
            text: `${runner.from} → ${runner.to}`,
            note: 'the E2E runner',
            compare: null,
          } satisfies TrendChange,
        ]
      : []),
  ]
}

/** A local run as a ring beside its release, never grouped into it. */
function localPoint(local: LocalMark): TrendPoint {
  return {
    ...local.point,
    label: `${local.suite_label} · ${local.ran ? `ran harness ${local.ran}` : 'Harness version not recorded'}`,
    release: null,
  }
}

/** One combination's line: each release's first completed execution of it,
 *  then its measured local runs; a release that did not run it has no
 *  point. */
export function combinationLine(
  overview: ReleaseOverview,
  key: string,
): CombinationLine {
  const line: CombinationLine = {
    points: [],
    changes: [],
    steps: [],
    latest: -1,
  }
  const releases: TrendPoint[] = []
  const last = overview.releases.at(-1)?.release
  // shortcut: a local whose release has no Release Control point is left out; place it by semver when that shows up
  for (const release of overview.releases) {
    const first = release.firsts.find(
      (item) => combinationKey(item.combination) === key,
    )
    if (first) {
      const text = releaseText(release.release)
      const point: TrendPoint = {
        ...first.point,
        group: {
          by: 'release',
          key: text,
          label: text,
          members: [first.point],
          mixed: [],
        },
      }
      releases.push(point)
      const item = release.step ? stepItem(release.step, key) : null
      const step =
        release.step && item
          ? { item, release: release.release, previous: release.step.previous }
          : null
      if (release.release === last) line.latest = line.points.length
      line.points.push(point)
      line.changes.push([
        ...changesAt(releases, releases.length - 1),
        ...stepChanges(step),
      ])
      line.steps.push(step)
    }
    for (const local of overview.locals)
      if (
        local.release === release.release &&
        local.point.measures &&
        combinationKey(local.combination) === key
      ) {
        line.points.push(localPoint(local))
        line.changes.push([])
        line.steps.push(null)
      }
  }
  return line
}

/** A release's step on a measure, as its point's label says it: `−3.3 pts
 *  vs v1.8.35 · over 9 compared tests, 2 left out`; `Not compared: …` for a
 *  combination left out; null without a step or a value. */
export function stepText(
  overview: ReleaseOverview,
  metric: TrendMetric,
  step: CombinationStep | null,
) {
  if (!step) return null
  const { item, release, previous } = step
  if (!isCompared(item))
    return `Not compared: ${leftOutText(overview, item, release, previous)}`
  const delta = comparedDelta(metric, item)
  if (delta === null) return null
  const left = item.tests_left_out.length
  return `${signedText(metric, delta)} vs ${releaseText(previous)}${left > 0 ? ` · over ${plural(item.compared_tests, 'compared test')}, ${left} left out` : ''}`
}
