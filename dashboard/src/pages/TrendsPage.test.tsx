import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { PointPanel } from '@/components/trends/PointPanel'
import { LargeChart, SmallChart } from '@/components/trends/TrendsChart'
import { ByTest, ExecutionsTable } from '@/components/trends/TrendsTables'
import { changesAt, previousCounted, trendMetric } from '@/lib/trends'
import {
  answeredView,
  requestParams,
  requestSeries,
  stackChoosable,
  stackLabel,
  stackNotice,
  stackSub,
  TrendsSkeleton,
} from '@/pages/TrendsPage'
import { seriesPoints } from '@/test-fixtures/trends'

const regression = seriesPoints('regression', 'default')
const changes = regression.map((_, index) => changesAt(regression, index))

describe('trends page state', () => {
  it('keeps a whole series and its stack in the hash, nothing by default', () => {
    const request = {
      suite: 'se',
      provider: 'deepseek',
      model: 'deepseek-flash',
      profile: 'ade-worker-builder',
      stack: 'any',
    }
    expect(requestSeries(request)).toEqual({
      suite: 'se',
      provider: 'deepseek',
      model: 'deepseek-flash',
      profile: 'ade-worker-builder',
    })
    expect(requestParams(request).toString()).toBe(
      'suite=se&provider=deepseek&model=deepseek-flash&profile=ade-worker-builder&stack=any',
    )
    expect(requestParams({}).toString()).toBe('')
    expect(requestParams({ stack: 'default' }).toString()).toBe('stack=default')
    expect(requestSeries({ suite: 'se' })).toBeNull()
  })

  it('pins the view to the series and stack the worker answered', () => {
    const answer = {
      series: [],
      selected: {
        suite: 'regression',
        provider: 'deepseek',
        model: 'deepseek-flash',
        profile: null,
      },
      stack: 'default',
      stacks: [],
      points: [],
    }
    expect(answeredView({}, answer)).toEqual({
      ...answer.selected,
      stack: 'default',
    })
    expect(requestParams(answeredView({}, answer)).toString()).toBe(
      'suite=regression&provider=deepseek&model=deepseek-flash&profile=&stack=default',
    )
    expect(answeredView({ stack: 'x' }, { ...answer, selected: null })).toEqual(
      { stack: 'x' },
    )
  })

  it('says when the stack asked for is not the one shown, with a way out', () => {
    const answer = {
      series: [],
      selected: null,
      stack: 'default',
      stacks: [
        { name: 'default', executions: 11 },
        { name: 'any', executions: 11 },
      ],
      points: [],
    }
    expect(stackNotice({ stack: 'default' }, answer)).toBeNull()
    expect(stackNotice({}, answer)).toBeNull()
    expect(stackNotice({ stack: 'lean' }, answer)).toEqual({
      text: 'No execution of this series ran on lean, so this shows default, the stack of its latest execution.',
      anyStack: false,
    })
    expect(stackChoosable(answer)).toBe(false)
    const unknown = { ...answer, stack: 'lean' }
    expect(stackNotice({ stack: 'lean' }, unknown)).toEqual({
      text: 'No execution of this series ran on lean.',
      anyStack: true,
    })
    expect(stackChoosable(unknown)).toBe(true)
  })

  it('names the stack a series only ran on this harness', () => {
    expect(stackLabel('not_recorded', seriesPoints('local'))).toBe(
      'this harness',
    )
    expect(stackLabel('not_recorded', seriesPoints('se'))).toBe('not recorded')
    expect(stackLabel('default', regression)).toBe('default')
    expect(stackSub('any')).toBe('every execution of the series')
  })
})

describe('trends page parts', () => {
  it('loads in the shape of the page', () => {
    const html = renderToStaticMarkup(<TrendsSkeleton narrow={false} />)
    expect(html).toContain('role="status" aria-busy="true"')
    expect(html.match(/tr-skel-big/g)).toHaveLength(1)
    expect(html.match(/tr-skel-mini/g)).toHaveLength(6)
    expect(html.match(/tr-skel-row/g)).toHaveLength(4)
  })

  it('draws a labelled diamond for each change and a button per execution', () => {
    const html = renderToStaticMarkup(
      <LargeChart
        metric={trendMetric('score')}
        points={regression}
        changes={changes}
        selected={-1}
        narrow={false}
        onPick={() => undefined}
      />,
    )
    expect(html).toContain('data-trend-chart="score"')
    // Every execution with a change but Sep 26 11:16 AM and Sep 28, which
    // only moved the runner (not part of Trends).
    expect(html.match(/class="tr-diamond"/g)).toHaveLength(8)
    expect(html).toMatch(/aria-label="What changed · [^"]+: iii"/)
    expect(html.match(/class="tr-missed"/g)).toHaveLength(2)
    expect(html).toMatch(/aria-label="[^"]+ · Score no counted run"/)
    expect(html.match(/class="tr-point"/g)).toHaveLength(9)
    expect(html).toContain('>iii 0.24.3-rc.1</text>')
    expect(html).toContain('>harness 1.8.34</text>')
    expect(html).not.toContain('runner')
  })

  it('draws a small chart with its direction under the title and crosses as strokes', () => {
    const html = renderToStaticMarkup(
      <SmallChart
        metric={trendMetric('completed')}
        points={regression}
        changes={changes}
        selected={-1}
        narrow={false}
        onFocus={() => undefined}
      />,
    )
    expect(html).toMatch(
      /<p class="tr-footnote"><span class="tr-direction">.*higher is better<\/span> · counted runs that completed their task<\/p>/,
    )
    expect(html.match(/class="tr-miss-mark"/g)).toHaveLength(2)
    expect(html).not.toContain('×')
  })

  it('crosses an execution without a counted run, and leaves a gap where a counted one did not measure', () => {
    const unmeasured = structuredClone(regression)
    const last = unmeasured.at(-1)
    if (last?.measures) last.measures.duration_ms_mean = null
    const html = renderToStaticMarkup(
      <LargeChart
        metric={trendMetric('duration')}
        points={unmeasured}
        changes={changes}
        selected={-1}
        narrow={false}
        onPick={() => undefined}
      />,
    )
    expect(html.match(/class="tr-missed"/g)).toHaveLength(2)
    expect(html.match(/class="tr-point"/g)).toHaveLength(8)
    const small = renderToStaticMarkup(
      <SmallChart
        metric={trendMetric('duration')}
        points={unmeasured}
        changes={changes}
        selected={-1}
        narrow={false}
        onFocus={() => undefined}
      />,
    )
    expect(small.match(/class="tr-miss-mark"/g)).toHaveLength(2)
    expect(small.match(/class="tr-dot"/g)).toHaveLength(8)
  })

  it('says what changed, the measures and where to go from one execution', () => {
    const index = regression.length - 1
    const html = renderToStaticMarkup(
      <PointPanel
        points={regression}
        index={index}
        changes={changes[index]}
        previous={previousCounted(regression, index)}
        bridge={null}
        here="#/ext/harness-e2e/trends?stack=default"
        onClose={() => undefined}
      />,
    )
    expect(html).toContain('GitHub #36381232467 · Release Control cd674932')
    // Run tests in the header is the view's one primary.
    expect(html).not.toContain('ds-button-primary')
    expect(html).toContain(
      'Nothing recorded changed: same iii, stack, workers and test definitions.',
    )
    expect(html).toContain('>no change<')
    expect(html).toContain(
      'iii 0.24.3-rc.1 · harness 1.8.36 · stack default (21 workers)',
    )
    expect(html).toContain(
      'href="#/ext/harness-e2e/execution/github-36381232467-1"',
    )
    expect(html).toContain(
      `href="#/ext/harness-e2e/compare/github-36296751640-1/github-36381232467-1?from=${encodeURIComponent('#/ext/harness-e2e/trends?stack=default')}"`,
    )
  })

  it('explains an execution without a counted run and offers no comparison', () => {
    const index = regression.findIndex(
      (point) => point.execution_id === 'github-36220337119-1',
    )
    const html = renderToStaticMarkup(
      <PointPanel
        points={regression}
        index={index}
        changes={changes[index]}
        previous={previousCounted(regression, index)}
        bridge={null}
        here="#/ext/harness-e2e/trends"
        onClose={() => undefined}
      />,
    )
    expect(html).toContain('did not register e2e::scenarios-list within 300 s')
    expect(html).toContain('Worker versions: not recorded.')
    expect(html).not.toContain('Compare with')
    expect(html).not.toContain('tr-measures')
  })

  it('scores each test by execution, with not run and a changed definition', () => {
    const se = seriesPoints('se')
    const html = renderToStaticMarkup(
      <ByTest
        points={se}
        changes={se.map((_, index) => changesAt(se, index))}
        selected={1}
        narrow={false}
        onPick={() => undefined}
      />,
    )
    expect(html).toContain('aria-label="Score by test and execution"')
    expect(html.match(/data-kind="not_run"/g)).toHaveLength(2)
    // Marks, never text glyphs: a cross icon said as words.
    expect(html).not.toContain('×')
    expect(html).toContain(
      '<span class="ds-visually-hidden">technically invalid</span>',
    )
    expect(html).toContain('>no score<')
    expect(html).toContain('href="#/ext/harness-e2e/tests/kanban_c7_live"')
    expect(html).toContain('aria-pressed="true"')
  })

  it('lists the executions newest first and tints what is new', () => {
    const html = renderToStaticMarkup(
      <ExecutionsTable points={regression} selected={-1} narrow={false} />,
    )
    const ids = [...html.matchAll(/data-execution-id="([^"]+)"/g)].map(
      (match) => match[1],
    )
    expect(ids[0]).toBe('github-36381232467-1')
    expect(ids.at(-1)).toBe('github-35742568444-1')
    expect(html).toContain('no counted run · GitHub #36220337119 · RC e63de635')
    expect(html).toMatch(/data-new="true">0\.24\.3-rc\.1</)
    expect(html).not.toContain('>Runner<')
    const narrow = renderToStaticMarkup(
      <ExecutionsTable points={regression} selected={-1} narrow />,
    )
    expect(narrow).not.toContain('>Stack<')
  })
})
