import { describe, expect, it } from 'vitest'
import {
  type Aggregate,
  aggregateOf,
  combinationText,
  type MetricMove,
  type OverviewPoint,
  overviewMetric,
  type ReleaseOverview,
  releaseText,
  scopeText,
  stepText,
  timeline,
  type Verdict,
  verdictSummary,
} from '@/lib/release-overview'
import {
  axisMarks,
  TREND_METRICS,
  type TrendMeasures,
  type TrendPoint,
  trendMetric,
} from '@/lib/trends'
import { releaseOverview } from '@/test-fixtures/release-overview'

const main = TREND_METRICS.filter((metric) => metric.tier === 'main')
const verdict = releaseOverview.verdict as Verdict
const regression = verdict.compared[0].combination
const base = (verdict.previous_aggregate as Aggregate).measures
const pairs = (moves: MetricMove[]) =>
  moves.map(({ metric, delta }) => [metric.id, delta])

describe('release verdict', () => {
  it('reads the latest release against the previous one, measure by measure', () => {
    const summary = verdictSummary(verdict, main)
    expect(pairs(summary.better)).toEqual([['input_tokens', -1.7]])
    expect(pairs(summary.worse)).toEqual([
      ['score', -2.3],
      ['duration', 6],
      ['turns', 8.9],
    ])
    expect(summary.same).toEqual(['completed', 'error_rate'])
  })

  it('says nothing moved when a side has no measures', () => {
    expect(
      verdictSummary({ ...verdict, latest_aggregate: null }, main),
    ).toEqual({ better: [], worse: [], same: [] })
  })

  it('reads tests completed and the error rate from the combinations’ shares, never the sums', () => {
    const point = {
      measures: {
        ...base,
        completed: 6,
        planned: 8,
        function_calls: 10,
        function_call_errors: 5,
      },
      aggregate: { measures: base, completed_share: 0.5, error_rate: 0.1 },
    } as unknown as OverviewPoint
    const completed = overviewMetric(trendMetric('completed'))
    expect(completed.value(point)).toEqual(50)
    expect(overviewMetric(trendMetric('error_rate')).value(point)).toEqual(10)
    expect(completed.delta).toEqual('percent_points')
    expect(completed.figure(75, {} as TrendPoint)).toEqual('75.0%')
    expect(overviewMetric(trendMetric('score'))).toBe(trendMetric('score'))
    expect(
      aggregateOf({
        ...base,
        completed: 6,
        planned: 8,
        function_calls: 0,
        function_call_errors: 0,
      }),
    ).toMatchObject({ completed_share: 0.75, error_rate: null })
  })

  it('names what was compared, what was left out and why', () => {
    expect(scopeText(verdict)).toEqual('Compared: Regression (ran on both).')
    const opus = {
      ...regression,
      provider: 'anthropic',
      model: 'claude-opus-5-5',
    }
    const software = {
      ...regression,
      suite: 'software-engineering',
      suite_label: 'Software engineering',
    }
    const onboarding = {
      ...regression,
      suite: 'onboarding',
      suite_label: 'Onboarding',
    }
    expect(
      scopeText({
        ...verdict,
        left_out: [
          { combination: opus, only_in: '1.8.35' },
          { combination: software, only_in: null },
          { combination: onboarding, only_in: '1.8.36' },
        ],
      }),
    ).toEqual(
      "Compared: Regression (ran on both). Not compared: Onboarding (not run on v1.8.35); Regression · anthropic/claude-opus-5-5 · profile none · stack default (not run on v1.8.36); Software engineering (every test's definition changed).",
    )
    expect(scopeText({ ...verdict, compared: [], left_out: [] })).toEqual(
      'No suite ran on both v1.8.36 and v1.8.35',
    )
  })

  it('writes releases with a leading v and a combination in full', () => {
    expect(
      ['1.8.36', 'v1.8.36', '@852b87e*'].map((version) => releaseText(version)),
    ).toEqual(['v1.8.36', 'v1.8.36', '@852b87e*'])
    expect(combinationText(regression)).toEqual(
      'Regression · deepseek/deepseek-flash · profile none · stack default',
    )
  })
})

describe('release timeline', () => {
  const line = timeline(releaseOverview)
  const { releases, locals } = releaseOverview

  it('puts each local run after the release it ran with, and only measured ones', () => {
    expect(line.points.map((point) => point.execution_id)).toEqual([
      'release:1.8.31',
      'release:1.8.34',
      'local-local-1',
      'local-local-2',
      'local-local-3',
      'local-local-4',
      'local-local-5',
      'release:1.8.35',
      'release:1.8.36',
    ])
    expect(line.latest).toEqual(8)
    expect(line.points[2].source.kind).toEqual('local')
    expect(line.points[2].label).toEqual(
      '2 tests, unsaved · ran harness @852b87e*',
    )
    expect(line.points[2].aggregate).toEqual(
      aggregateOf(locals[0].measures as TrendMeasures),
    )
    expect(line.points[8].aggregate).toEqual(releases[3].aggregate)
    expect(axisMarks(line.points).map((mark) => mark.text)).toEqual([
      'v1.8.31',
      'v1.8.34',
      '',
      '',
      '',
      '',
      '',
      'v1.8.35',
      'v1.8.36',
    ])
    expect(line.changes.every((list) => list.length === 0)).toBe(true)
  })

  it('marks a combination that joined or left, and says when nothing ran', () => {
    const overview: ReleaseOverview = structuredClone(releaseOverview)
    overview.releases[3].joined = [
      { ...regression, suite: 'onboarding', suite_label: 'Onboarding' },
    ]
    overview.locals[0].ran = null
    const marked = timeline(overview)
    expect(marked.changes[8]).toEqual([
      {
        kind: 'tests',
        major: true,
        name: 'Onboarding · deepseek/deepseek-flash · profile none · stack default',
        text: 'joined',
        note: null,
        compare: null,
      },
    ])
    expect(marked.points[2].label).toEqual(
      '2 tests, unsaved · Harness version not recorded',
    )
    expect(
      timeline({ ...releaseOverview, releases: [], verdict: null, locals: [] }),
    ).toEqual({ points: [], changes: [], latest: -1 })
  })

  it('reads each release’s step against the release before it', () => {
    const latest = line.points[8]
    expect(stepText(trendMetric('score'), latest)).toEqual('−2.3 vs v1.8.35')
    expect(stepText(trendMetric('duration'), latest)).toEqual(
      '+6.0% vs v1.8.35',
    )
    expect(stepText(trendMetric('completed'), latest)).toEqual(
      'no change vs v1.8.35',
    )
    expect(line.points[7].step?.previous).toEqual('1.8.34')
    expect(stepText(trendMetric('score'), line.points[0])).toBeNull()
    expect(stepText(trendMetric('score'), line.points[2])).toBeNull()
  })
})
