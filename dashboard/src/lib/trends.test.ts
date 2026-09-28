import { describe, expect, it } from 'vitest'
import {
  changesAt,
  commitsLinkText,
  dayMarks,
  deltaFormat,
  deltaOf,
  domain,
  emptyText,
  laneLabel,
  latestPair,
  notRun,
  previousCounted,
  roomyMarks,
  segments,
  stackNote,
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
} from '@/lib/trends'
import { seriesPoints } from '@/test-fixtures/trends'

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
      '0.14.0 → 0.15.0',
    ])
    expect(changes[0]).toMatchObject({
      kind: 'iii',
      major: true,
      compare: { name: 'iii', base: '0.24.2-rc.2', head: '0.24.3-rc.1' },
    })
    expect(changes[1].compare).toEqual({
      name: 'harness-e2e',
      base: '0.14.0',
      head: '0.15.0',
    })
    expect(laneLabel(changes)).toBe('iii 0.24.3-rc.1 +1')
  })

  it('labels the Harness 1.8.31 → 1.8.34 before the runner, and outlines other workers and definitions', () => {
    const index = at(regression, 'github-36010839787-2')
    const changes = changesAt(regression, index)
    expect(names(regression, index)).toEqual([
      'runner:harness-e2e',
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
    expect(laneLabel(changes)).toBe('harness 1.8.34 +1')
  })

  it('compares workers with the last execution that recorded them', () => {
    // Sep 26 10:23 PM: the two executions before it recorded no workers.
    const index = at(regression, 'github-36285373498-1')
    const changes = changesAt(regression, index)
    expect(changes[0]).toMatchObject({
      kind: 'runner',
      text: '0.15.1 → 0.16.2',
    })
    expect(changes.find((change) => change.name === 'harness')?.text).toBe(
      '1.8.35 → 1.8.36',
    )
    expect(laneLabel(changes)).toBe('harness 1.8.36 +1')
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
    expect(laneLabel(changesAt(regression, index))).toBe('runner 0.12.4 +1')
  })

  it('counts commits between two checkouts of the runner on this harness', () => {
    const local = seriesPoints('local')
    const [runner] = changesAt(local, 2)
    expect(runner).toMatchObject({
      kind: 'runner',
      text: '0.14.0@7a16130* → 0.14.0@2f8826a*',
      compare: { name: 'harness-e2e', base: '@7a16130', head: '@2f8826a' },
      note: '* the checkout also had uncommitted edits',
    })
    expect(laneLabel([runner])).toBe('runner @2f8826a*')
    expect(commitsLinkText(runner, 1)).toBe('1 commit between them on GitHub')
    expect(commitsLinkText(runner, null)).toBe('Commits between them on GitHub')
    expect(changesAt(local, 1)).toEqual([])
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
    moved[1].runner = moved[0].runner
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
    const { current, previous } = latestPair(points, metric(id))
    return current && previous ? deltaOf(metric(id), current, previous) : null
  }

  it('reads the latest counted execution against the previous counted one', () => {
    expect(delta('score', regression)).toBe(-4.4)
    expect(delta('duration', regression)).toBe(-19.7)
    expect(delta('completed', regression)).toBe(0)
    expect(delta('error_rate', regression)).toBe(0)
    expect(delta('input_tokens', regression)).toBe(-9.6)
    const { current, previous } = latestPair(regression, metric('score'))
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
      'iii 0.24.3-rc.1 · harness 1.8.36 · runner 0.17.0 · stack default (22 workers)',
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

  it('marks where each day starts', () => {
    const on = (day: number, hour: number) => ({
      started_at: new Date(2026, 8, day, hour).toISOString(),
    })
    expect(dayMarks([on(22, 9), on(22, 15), on(23, 2), on(26, 2)])).toEqual([
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

describe('the view', () => {
  it('round-trips the series and the stack through the hash', () => {
    const params = trendsParams(
      {
        suite: 'regression',
        provider: 'deepseek',
        model: 'deepseek-flash',
        profile: null,
      },
      'any',
    )
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
  })

  it('sums the executions and counted runs up in one line', () => {
    expect(summaryText(regression, 'UTC−3')).toBe(
      '11 executions · 9 with counted runs, 2 without · 81 counted runs · Sep 22 – Sep 28 · times in UTC−3',
    )
    expect(stackNote(regression)).toBe(
      '4 of these ran before the Console recorded stacks; they are in because they ran the same workers as default.',
    )
    expect(stackNote(seriesPoints('local'))).toBeNull()
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
