import type { ComponentProps } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  ReleaseOverview,
  ReleasePanel,
} from '@/components/trends/ReleaseOverview'
import { VerdictHeadline } from '@/components/trends/VerdictHeadline'
import {
  combinationKey,
  combinationLine,
  latestStep,
  type ReleaseOverview as Overview,
  type ReleaseStep,
} from '@/lib/release-overview'
import { trendMetric } from '@/lib/trends'
import { cannotTell, releaseOverview } from '@/test-fixtures/release-overview'

// The fixture's latest release started at 2:17 AM that day.
const now = new Date('2026-09-28T15:00:00-03:00')
const step = latestStep(releaseOverview) as ReleaseStep
const core = combinationKey(step.compared[0].combination)
const tour = combinationKey(step.left_out[1].combination)
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
const line = combinationLine(releaseOverview, core)
const panel = (index: number) =>
  renderToStaticMarkup(
    <ReleasePanel
      overview={releaseOverview}
      line={line}
      index={index}
      setup={core}
      bridge={null}
      onClose={() => undefined}
    />,
  )
const headline = (overview: Overview) =>
  renderToStaticMarkup(<VerdictHeadline overview={overview} now={now} />)
/** The fixture with the latest step edited. */
const withStep = (edit: (step: ReleaseStep) => void) => {
  const overview = structuredClone(releaseOverview)
  edit(overview.releases[3].step as ReleaseStep)
  return overview
}

describe('verdict headline', () => {
  it('names both releases and each calibrated verdict, what moved in the deciding setup untoned, and how much compared', () => {
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
      '<strong>v1.8.36</strong> (Today) vs v1.8.35: <span',
      'data-verdict="cannot_tell"',
      'Core runtime regression: cannot tell yet',
      'Moved in Core runtime regression, not read as better or worse: ',
      'Tests completed',
      '11.1 pts',
      '2 of 4 setups compared · 1 not run on v1.8.36 · 4 runs failed in the E2E infrastructure (Application builds and changes, Application builds with Deepwork) · Also changed: iii 0.24.2-rc.2 → 0.24.3-rc.1 · harness-e2e 0.18.0 → 0.18.1',
      'Compared and left out',
      'Compared: Core runtime regression, Application builds and changes. Not compared: Application builds with Deepwork (ran on v1.8.36, 3 runs failed in the E2E infrastructure); Complete the onboarding tour (not run on v1.8.36).',
    ])
      expect(html).toContain(text)
    // Score and Tests completed moved; the costs did not.
    expect(count(html, /data-move="moved"/)).toBe(2)
    expect(html).not.toMatch(/tr-delta" data-tone="(positive|negative)"/)
    // Without a way to open a setup, no button.
    expect(html).not.toContain('tr-name-button')
  })

  it('says there is no calibrated verdict and why, and names a setup that is worse', () => {
    const once = headline(
      withStep((step) => {
        step.compared[0].calibrated = { ...cannotTell, verdict: null }
        step.compared[0].calibrated.unavailable =
          'One run per test on a side: no noise to read the difference against.'
      }),
    )
    expect(once).toContain(
      'vs v1.8.35: no calibrated verdict. One run per test on a side',
    )
    const worse = headline(
      withStep((step) => {
        step.compared[1].calibrated = { ...cannotTell, verdict: 'worse' }
      }),
    )
    expect(worse).toContain('data-tone="negative"')
    expect(worse).toContain('Application builds and changes: worse')
    // The worse setup decides what the second line reads.
    expect(worse).toContain(
      'No measure moved in Application builds and changes.',
    )
  })

  it('says why there is no verdict, or that nothing compared', () => {
    expect(
      headline({ ...releaseOverview, releases: [], verdict: null }),
    ).toContain('No Release Control results yet.')
    expect(
      headline({
        ...releaseOverview,
        releases: releaseOverview.releases.slice(-1).map((release) => ({
          ...release,
          step: null,
        })),
        verdict: null,
      }),
    ).toContain(
      'Only v1.8.36 has Release Control results so far: a verdict needs two releases.',
    )
    const unshared = headline(
      withStep((step) => {
        step.compared = []
      }),
    )
    expect(unshared).toContain('vs v1.8.35.</p>')
    expect(unshared).toContain('0 of 2 setups compared')
    expect(unshared).toContain('No setup compared between v1.8.36 and v1.8.35.')
    expect(unshared).not.toContain('data-verdict-moves')
  })
})

describe('release overview', () => {
  it('draws the deciding setup’s line, its local runs as rings beside it and the latest highlighted', () => {
    const html = render()
    expect(count(html, /role="radio"/)).toBe(6)
    // The tabs scroll sideways at phone width instead of overflowing.
    expect(html).toMatch(
      /role="radiogroup"[^>]*tr-segments|tr-segments[^>]*role="radiogroup"/,
    )
    // 27 measures under More, four setups under Setup.
    expect(count(html, /role="menuitemradio"/)).toBe(31)
    expect(count(html, /class="tr-point"/)).toBe(6)
    expect(count(html, /data-source="local"/)).toBe(2)
    expect(count(html, /data-latest="true"/)).toBe(1)
    // Three steps, each in its point's aria-label and title.
    expect(count(html, / vs v1\.8\.3\d"/)).toBe(6)
    for (const text of [
      'data-trend-chart="score"',
      'aria-label="More metrics"',
      'aria-label="Setup: Core runtime regression · deepseek/deepseek-flash · profile none · stack default"',
      '>v1.8.31</text>',
      'aria-label="v1.8.36 · Score 96.7 · −3.3 vs v1.8.35"',
      'first completed Release Control execution of that release',
      '4 Release Control executions recorded no single Harness release and are not on the line.',
      'class="tr-name-button"',
    ])
      expect(html).toContain(text)
    expect(html).toMatch(
      /aria-label="[^"]+ · This harness · Core runtime regression · ran harness @852b87e\* · Score 85\.0"/,
    )
    expect(html).not.toContain('data-faded')
    // The verdict leads the overview.
    expect(html.indexOf('data-verdict-line')).toBeLessThan(
      html.indexOf('data-trend-chart'),
    )
  })

  it('marks only the steps that left tests out, never what else changed', () => {
    // Core changed iii, the Harness and workers at 1.8.36, and left no test
    // out: no diamond.
    expect(count(render(), /class="tr-diamond"/)).toBe(0)
    const apps = render({
      initialCombination: combinationKey(step.compared[1].combination),
    })
    expect(count(apps, /class="tr-diamond"/)).toBe(1)
    expect(apps).toContain(
      'aria-label="What changed · v1.8.36: registry_implementation, tests"',
    )
  })

  it('calls out every main measure of the setup’s latest step', () => {
    const html = render()
    expect(html).toContain('v1.8.36 · 3 Release Control runs')
    expect(html).toContain('What changed')
    expect(count(html, />no change</)).toBe(4)
    expect(html).not.toContain('data-not-compared')
  })

  it('says why a setup was left out, with the picker and tabs still there', () => {
    const html = render({ initialCombination: tour })
    expect(html).toContain('Not compared: not run on v1.8.36')
    expect(html).toMatch(
      /data-picker="setup"[^>]*><span>Complete the onboarding tour<\/span>/,
    )
    expect(count(html, /role="radio"/)).toBe(6)
    expect(count(html, /class="tr-point"/)).toBe(1)
    expect(html).not.toContain('What changed')
    expect(html).not.toContain('No release to draw yet')
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
  it('shows each setup’s warnings, never hiding the verdict', () => {
    const html = panel(5)
    expect(html).toContain('data-verdict-warning')
    expect(html).toContain('54 runs carry no cause')
  })

  it('opens the setup’s first execution of the latest release, then every setup of the step', () => {
    const html = panel(5)
    for (const text of [
      'data-trend-panel="github-36285373498-1"',
      '3 Release Control executions',
      'What changed since v1.8.35',
      'data-change="harness"',
      'data-change="iii"',
      'Its executions',
      '>Measures</h3>',
      'Core runtime regression · deepseek/deepseek-flash · profile none · stack default',
      'Counted runs: 27 on v1.8.36, 27 on v1.8.35 · 9 compared tests',
      'href="#/ext/harness-e2e/execution/github-36285373498-1"',
      'href="#/ext/harness-e2e/execution/github-36097908502-1"',
      'Left out: registry_implementation (definition changed); registry_verification (no counted run on v1.8.36)',
      'On v1.8.36: 1 run failed in the E2E infrastructure',
      'No calibrated verdict: One run per test on a side',
      'Not compared: ran on v1.8.36, 3 runs failed in the E2E infrastructure',
      'Not compared: not run on v1.8.36',
    ])
      expect(html).toContain(text)
    for (const text of [
      'Set as baseline',
      'Open execution',
      'Against v1.8.35',
      // A release's panel reads no one execution's stack or lock.
      'stack not recorded',
      'Its last execution',
      'no compose lock',
    ])
      expect(html).not.toContain(text)
  })

  it('reads an earlier release without the setups, and a local run on its own', () => {
    const earlier = panel(1)
    expect(earlier).toContain('What changed since v1.8.31')
    expect(earlier).not.toContain('By suite, model, profile and stack')
    const local = panel(2)
    expect(local).toContain(
      'This harness · Core runtime regression · ran harness @852b87e*',
    )
    expect(local).toContain('href="#/ext/harness-e2e/execution/local-local-1"')
    expect(local).not.toContain('What changed')
    expect(local).not.toContain('Set as baseline')
  })
})
