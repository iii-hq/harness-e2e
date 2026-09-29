import { describe, expect, it } from 'vitest'
import {
  baseFromParams,
  baselineOf,
  changesAt,
  changesBetween,
  commitsLinkText,
  comparedPair,
  customDays,
  DEFAULT_PERIOD,
  dayMarks,
  deltaFormat,
  deltaOf,
  domain,
  emptyText,
  laneLabel,
  majorsFirst,
  modelChoices,
  notRun,
  panelAgainst,
  periodBounds,
  periodError,
  periodFromParams,
  periodLabel,
  periodPhrase,
  pointTime,
  previousCounted,
  profileChoices,
  referenceText,
  roomyMarks,
  segments,
  stackNote,
  suiteChoices,
  summaryText,
  type TrendMetric,
  type TrendPoint,
  testCell,
  testIds,
  trendMetric,
  trendsParams,
  trendsRequestFromParams,
  utcOffsetText,
  versionsText,
  withBase,
  withPeriod,
} from '@/lib/trends'
import { seriesPoints, trendSeries } from '@/test-fixtures/trends'

// The fixture's times are UTC−3 (TZ is pinned in vite.config.ts) and its
// year 2026: dates read without the year against this now.
const NOW = new Date(2026, 8, 28, 12)
const regression = seriesPoints('regression', 'default')
const at = (points: TrendPoint[], id: string) =>
  points.findIndex((point) => point.execution_id === id)
const names = (points: TrendPoint[], index: number) =>
  changesAt(points, index).map((change) => `${change.kind}:${change.name}`)

describe('what changed between executions', () => {
  it('marks Sep 26, with no counted run, where iii moved to 0.24.3-rc.1', () => {
    const index = at(regression, 'github-36220337119-1')
    expect(regression[index].counted).toBe(0)
    const changes = changesAt(regression, index)
    expect(changes.map((change) => change.text)).toEqual([
      '0.24.2-rc.2 → 0.24.3-rc.1',
    ])
    expect(changes[0]).toMatchObject({
      kind: 'iii',
      major: true,
      compare: { name: 'iii', base: '0.24.2-rc.2', head: '0.24.3-rc.1' },
    })
    expect(laneLabel(changes)).toBe('iii 0.24.3-rc.1')
    // The runner (harness-e2e) is not part of Trends: Sep 26 11:16 AM only
    // moved it, so nothing changed there.
    expect(changesAt(regression, index + 1)).toEqual([])
  })

  it('labels the Harness 1.8.31 → 1.8.34, and outlines other workers and definitions', () => {
    const index = at(regression, 'github-36010839787-2')
    const changes = changesAt(regression, index)
    expect(names(regression, index)).toEqual([
      'worker:ade',
      'harness:harness',
      'worker:iii-directory',
      'definition:shell_coder_sandbox',
    ])
    const harness = changes.find((change) => change.kind === 'harness')
    expect(harness).toMatchObject({ major: true, text: '1.8.31 → 1.8.34' })
    expect(
      changes.filter((change) => !change.major).map((change) => change.kind),
    ).toEqual(['worker', 'worker', 'definition'])
    expect(changes.at(-1)?.text).toBe('7fbb3d6e → 6f93023f')
    expect(laneLabel(changes)).toBe('harness 1.8.34')
  })

  it('compares workers with the last execution that recorded them', () => {
    // Sep 26 10:23 PM: the two executions before it recorded no workers.
    const index = at(regression, 'github-36285373498-1')
    const changes = changesAt(regression, index)
    expect(changes.find((change) => change.name === 'harness')?.text).toBe(
      '1.8.35 → 1.8.36',
    )
    expect(laneLabel(changes)).toBe('harness 1.8.36')
  })

  it('says a worker joined the stack, with nothing to count', () => {
    const index = at(regression, 'github-36097908502-1')
    const judge = changesAt(regression, index).find(
      (change) => change.name === 'judge',
    )
    expect(judge).toMatchObject({
      major: false,
      text: 'not in the stack → 0.2.1',
      note: 'added to the stack',
      compare: null,
    })
  })

  it('names the tests that left and joined the plan', () => {
    const index = at(regression, 'github-35959254741-1')
    const tests = changesAt(regression, index).find(
      (change) => change.kind === 'tests',
    )
    expect(tests?.text).toBe(
      'left: minimal_path, performance_regression, git_regression_forensics · joined: validation_self_repair, context_pressure, prompt_injection_resilience',
    )
    expect(laneLabel(changesAt(regression, index))).toBe('tests changed')
  })

  it('counts commits between two checkouts of a worker on this harness', () => {
    const local = structuredClone(seriesPoints('local').slice(0, 2))
    expect(changesAt(local, 1)).toEqual([])
    if (local[1].workers) local[1].workers.harness = '@2f8826a*'
    const [harness] = changesAt(local, 1)
    expect(harness).toMatchObject({
      kind: 'harness',
      text: '@852b87e* → @2f8826a*',
      compare: { name: 'harness', base: '@852b87e', head: '@2f8826a' },
      note: '* the checkout also had uncommitted edits',
    })
    expect(laneLabel([harness])).toBe('harness @2f8826a*')
    expect(commitsLinkText(harness, 1)).toBe('1 commit between them on GitHub')
    expect(commitsLinkText(harness, null)).toBe(
      'Commits between them on GitHub',
    )
    expect(changesAt(local, 0)).toEqual([])
  })

  it('finds a changed definition across an execution where the test did not run', () => {
    const three = structuredClone(regression.slice(-3))
    const skipped = three[1].tests.find((test) => test.id === 'timer_wake')
    if (skipped) {
      skipped.state = 'not_run'
      skipped.score = null
      skipped.behavior_sha256 = null
    }
    const moved = three[2].tests.find((test) => test.id === 'timer_wake')
    if (moved) moved.behavior_sha256 = `sha256:${'ab'.repeat(32)}`
    expect(changesAt(three, 2)).toContainEqual(
      expect.objectContaining({
        kind: 'definition',
        name: 'timer_wake',
        text: '1450c4b9 → abababab',
      }),
    )
  })

  it('shows a worker that ran more than one build without a lookup', () => {
    const two = structuredClone(regression.slice(-2))
    if (two[1].workers) two[1].workers.ide = '0.12.20, 0.12.21'
    const ide = changesAt(two, 1).find((change) => change.name === 'ide')
    expect(ide).toMatchObject({
      text: '0.12.20 → 0.12.20, 0.12.21',
      compare: null,
    })
  })

  it('says a stack changed only between two recorded stacks', () => {
    const any = seriesPoints('regression')
    // Sep 22, 11:49 AM ran on a stack the Console did not record.
    expect(names(any, at(any, 'github-35743058842-1'))).not.toContain(
      'stack:stack',
    )
    const moved = structuredClone(regression.slice(-2))
    moved[1].stack = { name: 'lean', matched_by_workers: false }
    expect(changesAt(moved, 1)).toEqual([
      expect.objectContaining({ kind: 'stack', text: 'default → lean' }),
    ])
    expect(laneLabel(changesAt(moved, 1))).toBe('stack changed')
  })

  it('writes the link under a version change from version-compare', () => {
    const tags = { name: 'iii', compare: { name: 'iii', base: 'a', head: 'b' } }
    expect(commitsLinkText(tags, 9)).toBe('9 commits between the tags')
    expect(
      commitsLinkText(
        { name: 'harness', compare: { name: 'harness', base: 'a', head: 'b' } },
        19,
      ),
    ).toBe('19 commits between the tags in iii-hq/workers')
    expect(commitsLinkText(tags, null)).toBe('The commits between the tags')
  })
})

describe('measures', () => {
  const metric = (id: string) => trendMetric(id)
  const delta = (id: string, points: TrendPoint[]) => {
    const { current, previous } = comparedPair(points, metric(id), -1, -1)
    return current && previous ? deltaOf(metric(id), current, previous) : null
  }

  it('reads the latest counted execution against the previous counted one', () => {
    expect(delta('score', regression)).toBe(-4.4)
    expect(delta('duration', regression)).toBe(-19.7)
    expect(delta('completed', regression)).toBe(0)
    expect(delta('error_rate', regression)).toBe(0)
    expect(delta('input_tokens', regression)).toBe(-9.6)
    const { current, previous } = comparedPair(
      regression,
      metric('score'),
      -1,
      -1,
    )
    expect(current?.execution_id).toBe('github-36381232467-1')
    expect(previous?.execution_id).toBe('github-36296751640-1')
  })

  it('skips executions without a counted run', () => {
    const index = at(regression, 'github-36285373498-1')
    expect(previousCounted(regression, index)?.execution_id).toBe(
      'github-36097908502-1',
    )
  })

  it('takes the better direction from the comparison', () => {
    expect(metric('score').better).toBe('higher')
    expect(metric('completed').better).toBe('higher')
    for (const id of [
      'duration',
      'error_rate',
      'input_tokens',
      'function_calls',
      'turns',
    ])
      expect(metric(id).better).toBe('lower')
  })

  it('formats a difference as points, tests, points of a rate or a share', () => {
    expect(deltaFormat(metric('score'))(4.4)).toBe('4.4')
    expect(deltaFormat(metric('completed'))(1)).toBe('1 test')
    expect(deltaFormat(metric('error_rate'))(1.2)).toBe('1.2 pts')
    expect(deltaFormat(metric('turns'))(8.8)).toBe('8.8%')
  })

  it('has no share of a zero', () => {
    const zero = structuredClone(regression.slice(-2))
    if (zero[0].measures) zero[0].measures.turns_mean = 0
    expect(deltaOf(metric('turns'), zero[1], zero[0])).toBeNull()
  })

  it('shows the error call rate and the measures of a point', () => {
    const latest = regression.at(-1) as TrendPoint
    const rate = metric('error_rate')
    expect(rate.figure(rate.value(latest) as number, latest)).toBe('0.0%')
    const done = metric('completed')
    expect(done.figure(9, latest)).toBe('9 of 9 planned')
    expect(versionsText(latest)).toBe(
      'iii 0.24.3-rc.1 · harness 1.8.36 · stack default (21 workers)',
    )
  })
})

describe('a baseline', () => {
  const all = seriesPoints('regression')
  const score = trendMetric('score')
  const BASE = at(all, 'github-35821773226-2')
  const SEP25 = at(all, 'github-36097908502-1')
  const SEP22 = at(all, 'github-35742568444-1')
  const pair = (metricId: string, picked: number, baseline: number) =>
    comparedPair(all, trendMetric(metricId), picked, baseline)
  const readout = (metricId: string, picked: number, baseline: number) => {
    const { current, previous } = pair(metricId, picked, baseline)
    return current && previous
      ? deltaOf(trendMetric(metricId), current, previous)
      : null
  }

  it('lists what differs between two executions, each written baseline → this', () => {
    const list = changesBetween(all, BASE, SEP25)
    const text = (name: string) =>
      list.find((change) => change.name === name)?.text
    expect(text('harness')).toBe('1.8.31 → 1.8.35')
    expect(text('ade')).toBe('1.9.39 → 1.9.43')
    expect(text('judge')).toBe('not in the stack → 0.2.1')
    expect(list.some((change) => change.kind === 'tests')).toBe(true)
    // The Harness is one change over two releases, not the sum of the steps.
    expect(list.filter((change) => change.name === 'harness')).toHaveLength(1)
  })

  it('reads an execution before the baseline the same way, commits still earlier → later', () => {
    const forward = changesBetween(all, BASE, SEP25)
    const back = changesBetween(all, SEP25, BASE)
    const harness = back.find((change) => change.name === 'harness')
    expect(harness?.text).toBe('1.8.35 → 1.8.31')
    expect(harness?.compare).toEqual({
      name: 'harness',
      base: '1.8.31',
      head: '1.8.35',
    })
    expect(harness?.compare).toEqual(
      forward.find((change) => change.name === 'harness')?.compare,
    )
    expect(back.find((change) => change.name === 'judge')?.text).toBe(
      '0.2.1 → left the stack',
    )
  })

  it('takes a worker that went A → B → A as no change', () => {
    const [one, two, three] = [all[1], all[3], all[5]]
    const looped = [
      one,
      { ...two, workers: { ...two.workers, ade: '9.9.9' } },
      { ...three, workers: { ...one.workers } },
    ] as TrendPoint[]
    expect(
      changesBetween(looped, 0, 2).find((change) => change.name === 'ade'),
    ).toBeUndefined()
    expect(
      changesBetween(looped, 0, 1).find((change) => change.name === 'ade')
        ?.text,
    ).toBe(`${one.workers?.ade} → 9.9.9`)
  })

  it('puts iii, the Harness, the tests and the stack before the rest', () => {
    const kinds = majorsFirst(changesBetween(all, BASE, all.length - 1)).map(
      (change) => change.kind,
    )
    expect(kinds.slice(0, 4)).toEqual(['iii', 'harness', 'tests', 'worker'])
    expect(kinds.at(-1)).toBe('definition')
  })

  it('shows the latest against the baseline, and any pick against it too', () => {
    expect(readout('score', -1, BASE)).toBe(2.2)
    expect(readout('score', SEP25, BASE)).toBe(6.1)
    // Before the baseline: how far it was from it.
    expect(readout('score', SEP22, BASE)).toBe(-1.7)
    expect(referenceText(pair('score', -1, BASE))).toBe(
      'Sep 28, 2:17 AM against Sep 23, 2:17 AM (baseline)',
    )
    expect(referenceText(pair('score', SEP22, BASE))).toBe(
      'Sep 22, 11:45 AM against Sep 23, 2:17 AM (baseline)',
    )
  })

  it('reads the baseline itself against the execution before it', () => {
    const own = pair('score', BASE, BASE)
    expect(own.headIsBaseline).toBe(true)
    expect(own.previous?.execution_id).toBe(all[BASE - 1].execution_id)
    expect(referenceText(own)).toBe(
      'Sep 23, 2:17 AM (baseline) against Sep 22, 3:27 PM',
    )
  })

  it('follows the pick without a baseline, and is today’s latestPair with neither', () => {
    expect(referenceText(pair('score', SEP25, -1))).toBe(
      'Sep 25, 2:17 AM against Sep 24, 11:10 AM',
    )
    const none = pair('score', -1, -1)
    expect(none.current?.execution_id).toBe('github-36381232467-1')
    expect(none.previous?.execution_id).toBe('github-36296751640-1')
    expect(none.againstBaseline).toBe(false)
  })

  it('skips a baseline with no value for the measure', () => {
    const bare = all.map((point, index) =>
      index === BASE ? { ...point, measures: null } : point,
    ) as TrendPoint[]
    const { previous } = comparedPair(bare, score, -1, BASE)
    expect(previous?.execution_id).toBe(all.at(-2)?.execution_id)
  })

  it('finds the baseline in the view, or says why not', () => {
    expect(baselineOf(all, 'github-35821773226-2')).toEqual({
      index: BASE,
      why: null,
    })
    expect(baselineOf(all, 'github-elsewhere')).toEqual({
      index: -1,
      why: 'not_in_view',
    })
    expect(baselineOf(all, 'github-36220337119-1')).toEqual({
      index: -1,
      why: 'no_counted_run',
    })
    expect(baselineOf(all, null)).toEqual({ index: -1, why: null })
  })

  it('reads the panel against the baseline, else the previous counted execution', () => {
    expect(panelAgainst(all, SEP25, BASE)).toMatchObject({ isBaseline: true })
    expect(panelAgainst(all, SEP25, BASE).point?.execution_id).toBe(
      all[BASE].execution_id,
    )
    expect(panelAgainst(all, BASE, BASE).isBaseline).toBe(false)
    expect(panelAgainst(all, SEP25, -1).point?.execution_id).toBe(
      previousCounted(all, SEP25)?.execution_id,
    )
  })

  it('keeps the baseline in the hash', () => {
    const params = withBase(new URLSearchParams('stack=any'), 'plan-1')
    expect(params.toString()).toBe('stack=any&base=plan-1')
    expect(baseFromParams(params)).toBe('plan-1')
    expect(baseFromParams(new URLSearchParams('base='))).toBeNull()
    expect(withBase(new URLSearchParams('stack=any'), null).toString()).toBe(
      'stack=any',
    )
  })
})

describe('axes', () => {
  const values = (metric: TrendMetric, points: TrendPoint[]) =>
    points
      .map((point) => metric.value(point))
      .filter((value): value is number => value !== null)

  it('scores around what was scored, tests from 0 to the plan', () => {
    const score = trendMetric('score')
    expect(domain(score, values(score, regression), regression)).toEqual([
      87, 100,
    ])
    const done = trendMetric('completed')
    expect(domain(done, values(done, regression), regression)).toEqual([0, 9])
    const rate = trendMetric('error_rate')
    expect(domain(rate, values(rate, regression), regression)).toEqual([0, 1])
    const turns = trendMetric('turns')
    const [lo, hi] = domain(turns, [8, 10], regression)
    expect(lo).toBeCloseTo(7.4)
    expect(hi).toBeCloseTo(10.6)
  })

  it('scales tests completed to the planned runs, never to the list of tests', () => {
    const twice = structuredClone(regression.slice(-2))
    for (const point of twice)
      if (point.measures) {
        point.measures.completed *= 2
        point.measures.planned *= 2
      }
    const done = trendMetric('completed')
    expect(done.figure(18, twice[1])).toBe('18 of 18 planned')
    expect(domain(done, [18, 18], [regression[6], ...twice])).toEqual([0, 18])
  })

  it('marks where each day starts', () => {
    const on = (day: number, hour: number) => ({
      started_at: new Date(2026, 8, day, hour).toISOString(),
    })
    expect(
      dayMarks([on(22, 9), on(22, 15), on(23, 2), on(26, 2)], NOW),
    ).toEqual([
      { index: 0, text: 'Sep 22' },
      { index: 2, text: 'Sep 23' },
      { index: 3, text: 'Sep 26' },
    ])
  })

  it('labels a day only where it has room, first and last kept', () => {
    const at = (x: number) => x
    expect(roomyMarks([0, 30, 60, 100, 130], at, 56)).toEqual([0, 60, 130])
    expect(roomyMarks([0, 60, 90], at, 56)).toEqual([0, 90])
    expect(roomyMarks([0, 20], at, 56)).toEqual([0])
    expect(roomyMarks([], at, 56)).toEqual([])
  })

  it('draws the line faint across executions without a value', () => {
    expect(segments([1, null, null, 2, 3, null])).toEqual([
      { from: 0, to: 3, faded: true },
      { from: 3, to: 4, faded: false },
    ])
  })
})

describe('by test', () => {
  it('keeps planned tests that did not run, apart from the ones not planned', () => {
    const se = seriesPoints('se')
    const [before, after] = se
    expect(notRun(after)).toEqual(['kanban_c7_live', 'linkly_tutorial'])
    const changes = changesAt(se, 1)
    expect(testCell(after, 'kanban_c7_live', changes)).toEqual({
      kind: 'not_run',
    })
    expect(testCell(after, 'alertmanager_route_match', changes)).toEqual({
      kind: 'technical_invalid',
    })
    expect(testCell(before, 'registry_implementation', [])).toEqual({
      kind: 'no_score',
      redefined: false,
    })
    expect(testCell(before, 'form_flow_build', [])).toEqual({ kind: 'absent' })
    expect(testCell(after, 'kanban_c4_ticket_flow', changes)).toMatchObject({
      kind: 'scored',
      score: 80,
      tone: 'low',
    })
    expect(testIds(se).slice(-2)).toEqual([
      'form_flow_build',
      'state_machine_canvas_build',
    ])
  })

  it('blanks a column with no counted run and marks a changed definition', () => {
    const failed = regression[at(regression, 'github-36220337119-1')]
    expect(testCell(failed, 'timer_wake', [])).toEqual({ kind: 'uncounted' })
    const index = at(regression, 'github-36010839787-2')
    expect(
      testCell(
        regression[index],
        'shell_coder_sandbox',
        changesAt(regression, index),
      ),
    ).toMatchObject({
      kind: 'scored',
      score: 87,
      tone: 'lost',
      redefined: true,
    })
  })
})

describe('suite, model and profile pickers', () => {
  const series = trendSeries.map((item) => item.series)

  it('lists the suites, the models that ran one and the profiles that ran both', () => {
    expect(suiteChoices(series)).toEqual([
      { suite: 'regression', label: 'Regression', executions: 15 },
      {
        suite: expect.stringMatching(/^sha256:/),
        label: '2 tests, unsaved',
        executions: 5,
      },
      {
        suite: 'software-engineering',
        label: 'Software engineering',
        executions: 2,
      },
    ])
    expect(modelChoices(series, 'regression')).toEqual([
      { provider: 'deepseek', model: 'deepseek-flash', executions: 14 },
      { provider: 'anthropic', model: 'claude-opus-5-5', executions: 1 },
    ])
    expect(
      profileChoices(series, {
        suite: 'software-engineering',
        provider: 'deepseek',
        model: 'deepseek-flash',
      }),
    ).toEqual([{ profile: 'ade-worker-builder', executions: 2 }])
    expect(
      profileChoices(series, {
        suite: 'regression',
        provider: 'anthropic',
        model: 'claude-opus-5-5',
      }),
    ).toEqual([{ profile: null, executions: 1 }])
  })
})

describe('the period', () => {
  it('asks from the start of the first local day to the end of the last', () => {
    expect(periodBounds({ range: '30d' }, NOW)).toEqual({
      since: '2026-08-30T03:00:00.000Z',
      until: '2026-09-29T02:59:59.999Z',
    })
    expect(periodBounds({ range: '7d' }, NOW).since).toBe(
      '2026-09-22T03:00:00.000Z',
    )
    expect(periodBounds({ range: 'all' }, NOW)).toEqual({})
    expect(
      periodBounds({ since: '2026-09-26', until: '2026-09-28' }, NOW),
    ).toEqual({
      since: '2026-09-26T03:00:00.000Z',
      until: '2026-09-29T02:59:59.999Z',
    })
  })

  it('keeps the period in the hash, a bad one falling back to 30 days', () => {
    const at = (query: string) => periodFromParams(new URLSearchParams(query))
    expect(at('range=7d')).toEqual({ range: '7d' })
    expect(at('since=2026-09-01&until=2026-09-10')).toEqual({
      since: '2026-09-01',
      until: '2026-09-10',
    })
    for (const bad of [
      '',
      'range=1y',
      'since=2026-09-10&until=2026-09-01',
      'since=2026-02-31&until=2026-03-01',
      'since=yesterday&until=today',
    ])
      expect(at(bad)).toEqual(DEFAULT_PERIOD)
    expect(
      withPeriod(new URLSearchParams('stack=any'), {
        since: '2026-09-01',
        until: '2026-09-10',
      }).toString(),
    ).toBe('stack=any&since=2026-09-01&until=2026-09-10')
    expect(withPeriod(new URLSearchParams(), { range: 'all' }).toString()).toBe(
      'range=all',
    )
  })

  it('names the period and checks a custom one', () => {
    expect(periodPhrase({ range: '90d' }, NOW)).toBe('in the last 90 days')
    expect(periodPhrase({ range: 'all' }, NOW)).toBe('over all time')
    expect(
      periodPhrase({ since: '2026-09-01', until: '2026-09-10' }, NOW),
    ).toBe('from Sep 1 to Sep 10')
    expect(periodLabel({ range: 'all' })).toBe('All time')
    expect(periodLabel({ since: '2026-09-01', until: '2026-09-10' })).toBe(
      'Custom',
    )
    expect(periodError('2026-09-10', '2026-09-01')).toBe(
      'From must be on or before To.',
    )
    expect(periodError('2026-09-01', '')).toBe('Pick a From and a To day.')
    expect(periodError('2026-09-01', '2026-09-01')).toBeNull()
    expect(customDays({ range: '30d' }, NOW)).toEqual({
      since: '2026-08-30',
      until: '2026-09-28',
    })
  })
})

describe('the view', () => {
  it('round-trips the series and the stack through the hash', () => {
    const params = trendsParams({
      suite: 'regression',
      provider: 'deepseek',
      model: 'deepseek-flash',
      profile: null,
      stack: 'any',
    })
    expect(params.toString()).toBe(
      'suite=regression&provider=deepseek&model=deepseek-flash&profile=&stack=any',
    )
    expect(trendsRequestFromParams(params)).toEqual({
      suite: 'regression',
      provider: 'deepseek',
      model: 'deepseek-flash',
      profile: null,
      stack: 'any',
    })
    expect(trendsRequestFromParams(new URLSearchParams())).toEqual({})
    // A suite alone, as its picker asks.
    expect(trendsParams({ suite: 'regression' }).toString()).toBe(
      'suite=regression',
    )
  })

  it('sums the executions and counted runs up in one line', () => {
    expect(summaryText(regression, DEFAULT_PERIOD, 'UTC−3', NOW)).toBe(
      '11 executions in the last 30 days · 9 with counted runs, 2 without · 81 counted runs · Sep 22 – Sep 28 · times in UTC−3',
    )
    expect(stackNote(regression, 'default')).toBe(
      '4 of these ran before the Console recorded stacks; they are in because they ran the same workers as default.',
    )
    // Every stack shown: nothing to explain.
    expect(stackNote(regression, 'any')).toBeNull()
    expect(stackNote(seriesPoints('local'), 'default')).toBeNull()
  })

  it('reads the fixture in the pinned timezone', () => {
    expect(pointTime(regression[0], NOW)).toBe('Sep 22, 11:45 AM')
    expect(pointTime(regression[0], new Date(2027, 0, 1))).toBe(
      'Sep 22, 2026, 11:45 AM',
    )
    expect(utcOffsetText(new Date(2026, 8, 28))).toBe('UTC−3')
  })

  it('names the offset of the reader’s clock', () => {
    const clock = (minutes: number) =>
      ({ getTimezoneOffset: () => minutes }) as Date
    expect(utcOffsetText(clock(180))).toBe('UTC−3')
    expect(utcOffsetText(clock(-330))).toBe('UTC+5:30')
    expect(utcOffsetText(clock(0))).toBe('UTC')
  })

  it('says why there is nothing to draw', () => {
    const opus = seriesPoints('opus')
    expect(emptyText(opus)).toEqual({
      before: 'This series has one execution, ',
      point: opus[0],
      after:
        ' on this harness, and none of its 9 runs counted. Every run is technically invalid, so none of the 9 counts. A trend starts at two executions with counted runs.',
    })
    expect(emptyText([...opus, ...opus])).toMatchObject({
      before:
        'None of this series’ 2 executions has a counted run yet; the latest is ',
      after: '. A trend starts at two executions with counted runs.',
    })
    expect(emptyText([])).toBeNull()
  })
})
