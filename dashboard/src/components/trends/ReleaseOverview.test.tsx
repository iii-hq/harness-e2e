import type { ComponentProps } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  ReleaseOverview,
  ReleasePanel,
} from '@/components/trends/ReleaseOverview'
import { VerdictHeadline } from '@/components/trends/VerdictHeadline'
import {
  type ReleaseOverview as Overview,
  timeline,
  type Verdict,
} from '@/lib/release-overview'
import { trendMetric } from '@/lib/trends'
import { releaseOverview } from '@/test-fixtures/release-overview'

// The fixture's latest release started at 2:17 AM that day.
const now = new Date('2026-09-28T15:00:00-03:00')
const verdict = releaseOverview.verdict as Verdict
const count = (html: string, pattern: RegExp) =>
  html.match(new RegExp(pattern.source, 'g'))?.length ?? 0

const render = (props: Partial<ComponentProps<typeof ReleaseOverview>> = {}) =>
  renderToStaticMarkup(
    <ReleaseOverview
      overview={releaseOverview}
      metric="score"
      onMetric={() => undefined}
      bridge={null}
      narrow={false}
      options={<p data-more-options />}
      now={now}
      {...props}
    />,
  )
const line = timeline(releaseOverview)
const panel = (index: number) =>
  renderToStaticMarkup(
    <ReleasePanel
      overview={releaseOverview}
      line={line}
      index={index}
      bridge={null}
      onClose={() => undefined}
    />,
  )
const none = (overview: Overview) =>
  renderToStaticMarkup(<VerdictHeadline overview={overview} now={now} />)

describe('verdict headline', () => {
  it('names both releases and only the measures that moved, in their tone', () => {
    const html = renderToStaticMarkup(
      <VerdictHeadline
        overview={releaseOverview}
        status={<span data-sync-status />}
        now={now}
      />,
    )
    for (const text of [
      'Harness evolution',
      'data-sync-status',
      'data-verdict-line',
      '<strong>v1.8.36</strong> (Today) vs v1.8.35:',
      '−2.3',
      '+6.0%',
      '−1.7%',
      '+8.9%',
      'Compared: Regression (ran on both).',
    ])
      expect(html).toContain(text)
    expect(count(html, /data-move="better"/)).toBe(1)
    expect(count(html, /data-move="worse"/)).toBe(3)
    expect(html).not.toContain('Tests completed')
  })

  it('says why there is no verdict', () => {
    expect(none({ ...releaseOverview, releases: [], verdict: null })).toContain(
      'No Release Control results yet.',
    )
    expect(
      none({
        ...releaseOverview,
        releases: releaseOverview.releases.slice(-1),
        verdict: null,
      }),
    ).toContain(
      'Only v1.8.36 has Release Control results so far: a verdict needs two releases.',
    )
    const unshared = none({
      ...releaseOverview,
      verdict: {
        ...verdict,
        compared: [],
        latest_aggregate: null,
        previous_aggregate: null,
      },
    })
    expect(unshared).toContain('No suite ran on both v1.8.36 and v1.8.35')
    expect(unshared).toContain('vs v1.8.35.</p>')
    expect(unshared).not.toContain('no measure moved')
    expect(unshared).not.toContain('data-move')
    expect(
      none({
        ...releaseOverview,
        verdict: { ...verdict, latest_aggregate: verdict.previous_aggregate },
      }),
    ).toContain('vs v1.8.35: no measure moved')
  })
})

describe('release overview', () => {
  it('draws the releases as a line, the local runs as rings beside it and the latest highlighted', () => {
    const html = render()
    expect(count(html, /role="radio"/)).toBe(6)
    // The tabs scroll sideways at phone width instead of overflowing.
    expect(html).toMatch(
      /role="radiogroup"[^>]*tr-segments|tr-segments[^>]*role="radiogroup"/,
    )
    expect(count(html, /role="menuitemradio"/)).toBe(8)
    expect(count(html, /class="tr-point"/)).toBe(9)
    expect(count(html, /data-source="local"/)).toBe(5)
    expect(count(html, /data-latest="true"/)).toBe(1)
    expect(count(html, /class="tr-line"/)).toBe(3)
    // Three steps, each in its point's aria-label and title.
    expect(count(html, / vs v1\.8\.3\d"/)).toBe(6)
    for (const text of [
      'data-trend-chart="score"',
      'aria-label="More metrics"',
      '>v1.8.31</text>',
      'aria-label="v1.8.36 · Score 94.8 · −2.3 vs v1.8.35"',
      '4 Release Control executions recorded no single Harness release and are not on the line.',
    ])
      expect(html).toContain(text)
    expect(html).toMatch(
      /aria-label="[^"]+ · This harness · 2 tests, unsaved · ran harness @852b87e\* · Score 100\.0"/,
    )
    expect(html).not.toContain('data-faded')
  })

  it('calls out every main measure of the latest release against the previous one', () => {
    const html = render()
    expect(html).toContain('v1.8.36 · 3 Release Control runs')
    expect(html).toContain('What changed')
    expect(count(html, />no change</)).toBe(2)
    expect(html).not.toContain('Nothing to compare')
  })

  it('says there is nothing to compare when the latest release shares nothing with the previous one', () => {
    const releases = releaseOverview.releases.map((release, index, all) =>
      index === all.length - 1 && release.step
        ? {
            ...release,
            step: {
              ...release.step,
              latest_aggregate: null,
              previous_aggregate: null,
            },
          }
        : release,
    )
    const html = render({ overview: { ...releaseOverview, releases } })
    expect(html).toContain('Nothing to compare with v1.8.35.')
    expect(html).not.toMatch(/tr-callout[\s\S]*class="tr-measures"/)
  })

  it('links the CI runs outside Release Control and lists the latest release in Details, after More options', () => {
    const html = render()
    for (const text of [
      'data-outside',
      'CI runs outside Release Control',
      'href="#/ext/harness-e2e/executions?origin=outside-release-control"',
      '3 Release Control executions of v1.8.36',
      'href="#/ext/harness-e2e/execution/github-36285373498-1"',
    ])
      expect(html).toContain(text)
    expect(html.indexOf('CI runs outside')).toBeLessThan(
      html.indexOf('data-more-options'),
    )
    expect(html.indexOf('data-more-options')).toBeLessThan(
      html.indexOf('data-layer="tr-details"'),
    )
    expect(
      render({
        overview: {
          ...releaseOverview,
          outside_release_control: { executions: 0, latest_at: null },
        },
      }),
    ).not.toContain('CI runs outside Release Control')
  })

  it('plots a measure of More, named on its menu', () => {
    const html = render({ metric: 'output_tokens' })
    expect(html).toContain('data-trend-chart="output_tokens"')
    expect(html).toContain(`>${trendMetric('output_tokens').label}<`)
    expect(html).toMatch(
      /data-picker="more-metrics"[^>]*><span>Output tokens<\/span>/,
    )
    expect(html).toMatch(
      /aria-label="More metrics: Output tokens"[^>]*data-picker="more-metrics"/,
    )
    expect(render()).toMatch(
      /aria-label="More metrics"[^>]*data-picker="more-metrics"/,
    )
    expect(render()).toMatch(
      /data-picker="more-metrics"[^>]*><span>More<\/span>/,
    )
  })

  it('shows the page’s empty state while no release has results', () => {
    const html = render({
      overview: { ...releaseOverview, releases: [], verdict: null },
      empty: <p data-empty-ci />,
    })
    expect(html).toContain('data-empty-ci')
    expect(html).not.toContain('data-trend-chart')
  })
})

describe('release panel', () => {
  it('opens what changed at the latest release, combination by combination', () => {
    const html = panel(8)
    for (const text of [
      'data-trend-panel="release:1.8.36"',
      '3 Release Control executions · 1 combination',
      'What changed since v1.8.35',
      'data-change="harness"',
      'data-change="iii"',
      'Against v1.8.35',
      '>100.0%<',
      'Regression · deepseek/deepseek-flash · profile none · stack default',
      'Left out: contention_ledger',
    ])
      expect(html).toContain(text)
    for (const text of [
      'Set as baseline',
      'Open execution',
      'Its executions',
      'definition changed',
      // A release spans executions: no one stack or lock speaks for it.
      'stack not recorded',
      'Its last execution',
      'no compose lock',
    ])
      expect(html).not.toContain(text)
  })

  it('reads an earlier release against the one before it, the first release and a local run on their own', () => {
    const earlier = panel(1)
    expect(earlier).toContain('What changed since v1.8.31')
    expect(earlier).toContain('Against v1.8.31')
    expect(earlier).toContain('class="tr-delta"')
    expect(earlier).not.toContain('By suite, model, profile and stack')
    const first = panel(0)
    expect(first).toContain('>Measures</h3>')
    expect(first).not.toContain('class="tr-delta"')
    const local = panel(2)
    expect(local).toContain(
      'This harness · 2 tests, unsaved · ran harness @852b87e*',
    )
    expect(local).toContain('href="#/ext/harness-e2e/execution/local-local-1"')
    expect(local).not.toContain('What changed')
    expect(local).not.toContain('Set as baseline')
  })
})
