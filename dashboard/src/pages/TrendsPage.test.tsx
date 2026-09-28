import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { PointPanel } from '@/components/trends/PointPanel'
import { LargeChart } from '@/components/trends/TrendsChart'
import { ByTest, ExecutionsTable } from '@/components/trends/TrendsTables'
import { changesAt, previousCounted, trendMetric } from '@/lib/trends'
import {
  requestParams,
  requestSeries,
  stackLabel,
  stackSub,
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
    expect(html.match(/class="tr-diamond"/g)).toHaveLength(10)
    expect(html).toMatch(/aria-label="What changed · [^"]+: iii, harness-e2e"/)
    expect(html.match(/class="tr-missed"/g)).toHaveLength(2)
    expect(html).toMatch(/aria-label="[^"]+ · Score no counted run"/)
    expect(html.match(/class="tr-point"/g)).toHaveLength(9)
    expect(html).toContain('>iii 0.24.3-rc.1 +1</text>')
    expect(html).toContain('>harness 1.8.34 +1</text>')
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
    expect(html).toContain('data-change="runner"')
    expect(html).toContain('0.16.2 → 0.17.0')
    expect(html).toContain('>no change<')
    expect(html).toContain(
      'iii 0.24.3-rc.1 · harness 1.8.36 · runner 0.17.0 · stack default (22 workers)',
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
    const narrow = renderToStaticMarkup(
      <ExecutionsTable points={regression} selected={-1} narrow />,
    )
    expect(narrow).not.toContain('>Runner<')
  })
})
