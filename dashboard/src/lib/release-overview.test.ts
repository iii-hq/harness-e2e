import { describe, expect, it } from 'vitest'
import {
  alsoChangedText,
  type CombinationDelta,
  calibrated,
  combinationKey,
  combinationLine,
  combinationOptions,
  combinationText,
  comparedDelta,
  coverageText,
  decidingCombination,
  defaultCombination,
  latestStep,
  overviewMetric,
  type ReleaseOverview,
  type ReleaseStep,
  scopeText,
  stepText,
  uncalibratedText,
} from '@/lib/release-overview'
import {
  axisMarks,
  releaseText,
  type TrendPoint,
  trendMetric,
} from '@/lib/trends'
import { cannotTell, releaseOverview } from '@/test-fixtures/release-overview'

const step = latestStep(releaseOverview) as ReleaseStep
const [core, applications] = step.compared
const [deepwork, onboarding] = step.left_out
const label = (combination: { suite_label: string } | null) =>
  combination?.suite_label

describe('release verdict', () => {
  it('reads better or worse only from a calibrated verdict, and says why there is none', () => {
    expect(
      calibrated(step).map((item) => [
        item.combination.suite_label,
        item.summary.verdict,
      ]),
    ).toEqual([['Core runtime regression', 'cannot_tell']])
    expect(uncalibratedText(applications)).toMatch(/^One run per test/)
    expect(uncalibratedText({ ...applications, calibrated: undefined })).toBe(
      'The worker sent no calibrated verdict for it.',
    )
    const read = calibrated({
      ...step,
      compared: [{ ...applications, calibrated: cannotTell }],
    })
    expect(read.map((item) => item.summary.verdict)).toEqual(['cannot_tell'])
  })

  it('offers every setup of the latest step, and opens on the one that decides', () => {
    expect(combinationOptions(releaseOverview).map(label)).toEqual([
      'Core runtime regression',
      'Application builds and changes',
      'Application builds with Deepwork',
      'Complete the onboarding tour',
    ])
    // Nothing called worse or better: the first compared.
    expect(label(decidingCombination(releaseOverview))).toEqual(
      'Core runtime regression',
    )
    const worse = structuredClone(releaseOverview)
    const latest = worse.releases[3].step as ReleaseStep
    latest.compared[1].calibrated = { ...cannotTell, verdict: 'worse' }
    expect(label(decidingCombination(worse))).toEqual(
      'Application builds and changes',
    )
    expect(label(defaultCombination(releaseOverview))).toEqual(
      'Core runtime regression',
    )
    // Nothing compared: what the latest release ran, then what was left out.
    const none = structuredClone(releaseOverview)
    ;(none.releases[3].step as ReleaseStep).compared = []
    expect(decidingCombination(none)).toBeNull()
    expect(label(defaultCombination(none))).toEqual('Core runtime regression')
    none.releases[3].firsts = []
    expect(label(defaultCombination(none))).toEqual(
      'Application builds with Deepwork',
    )
  })

  it('says how much compared, what failed and what else changed', () => {
    expect(coverageText(releaseOverview)).toEqual(
      '2 of 4 setups compared · 1 not run on v1.8.36 · 4 runs failed in the E2E infrastructure (Application builds and changes, Application builds with Deepwork)',
    )
    expect(alsoChangedText(core.also_changed)).toEqual(
      'Also changed: iii 0.24.2-rc.2 → 0.24.3-rc.1 · harness-e2e 0.18.0 → 0.18.1',
    )
    expect(alsoChangedText([])).toBeNull()
    // A setup left out still counts the latest release's failed runs.
    const unlike = structuredClone(releaseOverview)
    const latest = unlike.releases[3].step as ReleaseStep
    const [apps] = latest.compared.splice(1, 1)
    latest.left_out.unshift({
      combination: apps.combination,
      reason: 'nothing_alike',
      only_in: null,
      infra_failed_runs: null,
    })
    expect(coverageText(unlike)).toEqual(
      '1 of 4 setups compared · 1 not run on v1.8.36 · 4 runs failed in the E2E infrastructure (Application builds and changes, Application builds with Deepwork)',
    )
    expect(
      coverageText({
        ...releaseOverview,
        releases: [releaseOverview.releases[0]],
      }),
    ).toBeNull()
  })

  it('names what was compared, what was left out and why', () => {
    expect(scopeText(releaseOverview, releaseOverview.releases[3])).toEqual(
      'Compared: Core runtime regression, Application builds and changes. Not compared: Application builds with Deepwork (ran on v1.8.36, 3 runs failed in the E2E infrastructure); Complete the onboarding tour (not run on v1.8.36).',
    )
    const latest = releaseOverview.releases[3]
    const opus = {
      ...core.combination,
      provider: 'anthropic',
      model: 'claude-opus-5-5',
    }
    expect(
      scopeText(releaseOverview, {
        ...latest,
        step: {
          ...step,
          left_out: [
            {
              combination: opus,
              reason: 'only_in_one_release',
              only_in: '1.8.36',
              infra_failed_runs: null,
            },
            { ...onboarding, reason: 'nothing_alike', only_in: null },
            { ...deepwork, reason: 'no_completed_execution', only_in: null },
            // Counted on the latest: the release before failed every run.
            { ...deepwork, only_in: '1.8.36' },
          ],
        },
      }),
    ).toEqual(
      'Compared: Core runtime regression, Application builds and changes. Not compared: Core runtime regression · anthropic/claude-opus-5-5 · profile none · stack default (only in v1.8.36); Complete the onboarding tour (no test comparable); Application builds with Deepwork (no completed execution on v1.8.36); Application builds with Deepwork (ran on v1.8.35, 3 runs failed in the E2E infrastructure).',
    )
    expect(
      scopeText(releaseOverview, {
        ...latest,
        step: { ...step, compared: [], left_out: [] },
      }),
    ).toEqual('No setup compared between v1.8.36 and v1.8.35.')
  })

  it('writes releases with a leading v and a combination in full', () => {
    expect(
      ['1.8.36', 'v1.8.36', '@852b87e*'].map((version) => releaseText(version)),
    ).toEqual(['v1.8.36', 'v1.8.36', '@852b87e*'])
    expect(combinationText(core.combination)).toEqual(
      'Core runtime regression · deepseek/deepseek-flash · profile none · stack default',
    )
    expect(combinationKey(core.combination)).toEqual(
      'regression|deepseek|deepseek-flash||default',
    )
  })

  it('reads Tests completed over the counted runs, and the worker’s deltas', () => {
    const completed = overviewMetric(trendMetric('completed'))
    const point = {
      counted: 27,
      measures: { completed: 24 },
    } as unknown as TrendPoint
    expect(completed.value(point)).toBeCloseTo(88.89, 2)
    expect(completed.value({ ...point, counted: 0 })).toBeNull()
    expect(completed.delta).toEqual('percent_points')
    expect(completed.figure(75, point)).toEqual('75.0%')
    expect(overviewMetric(trendMetric('score')).minSpan).toEqual(20)
    expect(overviewMetric(trendMetric('error_rate'))).toBe(
      trendMetric('error_rate'),
    )
    expect(comparedDelta(trendMetric('score'), core)).toEqual(-3.3)
    expect(comparedDelta(completed, core)).toEqual(-11.1)
    // Read from both sides' measures: every run took 1,000 ms.
    expect(comparedDelta(trendMetric('duration'), core)).toEqual(0)
  })
})

describe('a setup’s line', () => {
  const key = combinationKey(core.combination)
  const line = combinationLine(releaseOverview, key)

  it('plots each release’s first completed execution, then the setup’s measured local runs', () => {
    expect(line.points.map((point) => point.execution_id)).toEqual([
      'github-35742568444-1',
      'github-36010839787-2',
      'local-local-1',
      'local-local-2',
      'github-36097908502-1',
      'github-36285373498-1',
    ])
    expect(line.latest).toEqual(5)
    expect(axisMarks(line.points).map((mark) => mark.text)).toEqual([
      'v1.8.31',
      'v1.8.34',
      '',
      '',
      'v1.8.35',
      'v1.8.36',
    ])
    const local = line.points[2]
    expect([local.source.kind, local.release, local.group]).toEqual([
      'local',
      null,
      undefined,
    ])
    expect(local.label).toEqual(
      'Core runtime regression · ran harness @852b87e*',
    )
    // A local reads Tests completed over its own counted runs.
    expect(overviewMetric(trendMetric('completed')).value(local)).toEqual(100)
    expect(line.steps[2]).toBeNull()
    expect(line.changes[2]).toEqual([])
  })

  it('reads each release’s step for the setup', () => {
    const score = overviewMetric(trendMetric('score'))
    const completed = overviewMetric(trendMetric('completed'))
    expect(stepText(releaseOverview, score, line.steps[5])).toEqual(
      '−3.3 vs v1.8.35',
    )
    expect(stepText(releaseOverview, completed, line.steps[5])).toEqual(
      '−11.1 pts vs v1.8.35',
    )
    expect(
      stepText(releaseOverview, trendMetric('duration'), line.steps[5]),
    ).toEqual('no change vs v1.8.35')
    expect(stepText(releaseOverview, score, line.steps[0])).toBeNull()
    // The release after a local run reads against the release before.
    expect(line.steps[4]?.previous).toEqual('1.8.34')
    expect(line.changes[4].map((change) => change.kind)).toEqual(['harness'])
    // The runner, from the step, after what the two records show.
    expect(line.changes[5].map((change) => [change.kind, change.name])).toEqual(
      [
        ['iii', 'iii'],
        ['worker', 'context-manager'],
        ['harness', 'harness'],
        ['worker', 'harness-e2e'],
      ],
    )
  })

  it('marks the tests a step did not count, and names the ones left out', () => {
    const apps = combinationLine(
      releaseOverview,
      combinationKey(applications.combination),
    )
    expect(apps.points.map((point) => point.release)).toEqual([
      '1.8.35',
      '1.8.36',
    ])
    const changes = apps.changes[1]
    expect(changes.find((change) => change.kind === 'tests')).toMatchObject({
      major: true,
      text: '1 test not counted on v1.8.36',
    })
    expect(
      changes.find((change) => change.kind === 'definition')?.name,
    ).toEqual('registry_implementation')
    expect(
      stepText(
        releaseOverview,
        overviewMetric(trendMetric('score')),
        apps.steps[1],
      ),
    ).toEqual('no change vs v1.8.35 · over 5 compared tests, 2 left out')
  })

  it('says why a setup was left out, and draws nothing where it did not run', () => {
    const tour = combinationLine(
      releaseOverview,
      combinationKey(onboarding.combination),
    )
    expect(tour.points.map((point) => point.release)).toEqual(['1.8.35'])
    expect(tour.latest).toEqual(-1)
    expect(
      stepText(releaseOverview, trendMetric('score'), tour.steps[0]),
    ).toEqual('Not compared: only in v1.8.35')
    expect(combinationLine(releaseOverview, 'nothing')).toEqual({
      points: [],
      changes: [],
      steps: [],
      latest: -1,
    })
  })

  it('reads a runner change on a step as a worker change', () => {
    const overview: ReleaseOverview = structuredClone(releaseOverview)
    const latest = overview.releases[3].step as ReleaseStep
    ;(latest.compared[0] as CombinationDelta).also_changed = [
      { name: 'harness-e2e', from: '0.18.0', to: '0.18.2' },
    ]
    const changes = combinationLine(overview, key).changes[5]
    expect(changes.find((change) => change.name === 'harness-e2e')).toEqual({
      kind: 'worker',
      major: false,
      name: 'harness-e2e',
      text: '0.18.0 → 0.18.2',
      note: 'the E2E runner',
      compare: null,
    })
  })
})
