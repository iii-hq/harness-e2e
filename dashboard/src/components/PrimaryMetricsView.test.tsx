import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { PrimaryMetricsView } from '@/components/PrimaryMetricsView'
import {
  buildPrimaryMetricsFromValues,
  type PrimaryTestValues,
} from '@/lib/primary-metrics'

function metrics(
  values: Partial<PrimaryTestValues['values']> = {},
  scopeKnown = true,
) {
  return buildPrimaryMetricsFromValues([
    {
      key: 'smoke',
      label: 'Smoke',
      definition: 'sha256:definition',
      expected: 1,
      scopeKnown,
      values: {
        score: [80],
        costUsd: [0.03],
        durationMs: [100_000],
        totalTokens: [1000],
        inputTokens: [800],
        outputTokens: [200],
        cacheRead: [5000],
        cacheWrite: [0],
        turns: [4],
        functionCalls: [3],
        functionErrors: [0],
        ...values,
      },
    },
  ])
}

function comparison(candidate = metrics(), baseline = metrics()) {
  return renderToStaticMarkup(
    <PrimaryMetricsView
      summaryOnly
      baseline={baseline}
      candidate={candidate}
      baselineLabel="Official baseline"
      candidateLabel="Candidate #1"
    />,
  )
}

describe('primary metrics comparison presentation', () => {
  it('distinguishes score and spend direction while keeping usage changes neutral', () => {
    const html = comparison(
      metrics({
        score: [83],
        costUsd: [0.04],
        totalTokens: [900],
        functionErrors: [1],
      }),
    )
    expect(html.match(/data-tone="positive"/g)).toHaveLength(1)
    expect(html.match(/data-tone="negative"/g)).toHaveLength(2)
    expect(html).toContain('data-change="changed" data-tone="neutral"')
    expect(html).toContain('+3.75% · increase')
    expect(html).toContain('+33.33% · increase')
    expect(html).toContain('−10% · decrease')
    expect(html).toContain('A is zero')
    expect(html).not.toMatch(/Infinity|NaN/)
  })

  it('identifies the two executions and keeps unchanged values explicit', () => {
    const html = comparison()
    expect(html).toContain('Official baseline')
    expect(html).toContain('Candidate #1')
    expect(html).toContain('0% · No change')
    expect(html).toContain('Input tokens')
    expect(html).toContain('Cache written')
  })

  it('blocks only missing metrics and compares the available values', () => {
    const missing = comparison(metrics({ cacheWrite: [null] }))
    expect(missing).toContain('Not reported')
    expect(missing.match(/Not comparable/g)).toHaveLength(1)
    const available = comparison(metrics({ score: [90] }))
    expect(available).not.toContain('Not comparable')
    expect(available).toContain('+12.5%')
  })

  it('keeps zero references undefined and small changes visibly nonzero', () => {
    const html = comparison(
      metrics({ score: [10], costUsd: [0.0300001] }),
      metrics({ score: [0] }),
    )
    expect(html).toContain('+10 pts · A is zero')
    expect(html).toContain('+&lt;0.01% · increase')
    expect(html).not.toMatch(/Infinity|NaN/)
  })

  it('counts executed tests separately from scored tests, including zero scores', () => {
    const html = comparison(metrics({ score: [null] }), metrics({ score: [0] }))
    expect(html).toMatch(
      /data-test-coverage="a"[^>]*>.*Tests executed: 1 · Scored: 1/,
    )
    expect(html).toMatch(
      /data-test-coverage="b"[^>]*>.*Tests executed: 1 · Scored: 0/,
    )
    expect(comparison(metrics({ score: [] }))).toMatch(
      /data-test-coverage="b"[^>]*>.*Tests executed: 0 · Scored: 0/,
    )
  })

  it('shows absolute counts when the planned scope is unknown', () => {
    const html = comparison(metrics({}, false))
    expect(html).toMatch(
      /data-test-coverage="b"[^>]*>.*Tests executed: 1 · Scored: 1/,
    )
    const count = html.match(/<p[^>]*data-test-coverage="b"[^>]*>.*?<\/p>/)?.[0]
    expect(count).toBeDefined()
    expect(count).not.toMatch(/1\/1|100%/)
  })

  it('can show the shared per-test table below the summary', () => {
    const html = renderToStaticMarkup(
      <PrimaryMetricsView
        summaryOnly
        showTests
        baseline={metrics()}
        candidate={metrics({ score: [90] })}
      />,
    )
    expect(html).toContain('aria-label="Metrics by test"')
    expect(html).toContain(
      'Overview metrics by test, baseline A and candidate B',
    )
    expect(html).toContain('<span>Smoke</span>')
    expect(html.indexOf('Test results')).toBeGreaterThan(
      html.indexOf('Equal weight per test'),
    )
    expect(comparison()).not.toContain('aria-label="Metrics by test"')
  })

  it('uses displayed partial values for percentages in the summary and each test', () => {
    const html = renderToStaticMarkup(
      <PrimaryMetricsView
        summaryOnly
        showTests
        baseline={metrics({ score: [80] }, false)}
        candidate={metrics({ score: [90] }, false)}
      />,
    )
    expect(html).toContain('Partial mean')
    expect(html.match(/\+12.5%/g)).toHaveLength(2)
    expect(html).not.toContain('Not comparable')
  })
})
