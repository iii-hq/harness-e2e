import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { ExecutionMetricsPanel } from '@/components/ExecutionMetricsPanel'
import {
  executionMetricsFixture,
  metricRun,
} from '@/test-fixtures/execution-metrics'

describe('execution efficiency', () => {
  it('shows pooled efficiency without repeating the primary score or removed counts and rates', () => {
    const detail = executionMetricsFixture([
      { runs: [metricRun('a', 100_000, { score: 60 })] },
      {
        runs: [
          metricRun('b', 20_000, {
            completion: 'task_incomplete',
            score: null,
          }),
        ],
      },
      { runs: [metricRun('c', 120_000, { score: 100 })] },
    ])
    const html = renderToStaticMarkup(<ExecutionMetricsPanel detail={detail} />)
    expect(html).toContain('Efficiency')
    expect(html).toContain('Failed attempt tokens')
    expect(html).toContain('Tokens per completion')
    expect(html).toContain('Completed p50 tokens')
    expect(html).toContain('120,000')
    expect(html).toContain('110,000')
    expect(html).toContain('20,000')
    for (const removed of [
      'planned',
      'recorded',
      'completion rate',
      'execution reliability',
      'completion evidence',
      'coverage',
      'assessments',
      'score',
    ]) {
      expect(html).not.toContain(removed)
    }
    expect(html).not.toContain('<details')
    expect(html).not.toContain('<table')
  })

  it('distinguishes observed failed-attempt subtotals from missing and zero values', () => {
    const detail = executionMetricsFixture([
      {
        runs: [
          metricRun('a', 100, { completion: 'task_incomplete' }),
          metricRun('b', null, { completion: 'task_incomplete' }),
        ],
      },
    ])
    const html = renderToStaticMarkup(<ExecutionMetricsPanel detail={detail} />)
    expect(html).toContain('Observed subtotal · 1/2')
    expect(html).toMatch(/Tokens per completion<\/dt><dd[^>]*>—<\/dd>/)
    expect(html).toMatch(/Completed p50 tokens<\/dt><dd[^>]*>—<\/dd>/)
    const zero = renderToStaticMarkup(
      <ExecutionMetricsPanel
        detail={executionMetricsFixture([{ runs: [metricRun('c', 100)] }])}
      />,
    )
    expect(zero).toMatch(/Failed attempt tokens<\/dt><dd[^>]*>0<\/dd>/)
    expect(zero).not.toContain('Observed subtotal')
  })

  it('keeps unavailable scenario scope unknown', () => {
    const detail = executionMetricsFixture([{ runs: [metricRun('a', 100)] }])
    detail.reports[0].available = false
    const html = renderToStaticMarkup(<ExecutionMetricsPanel detail={detail} />)
    expect(html).toContain('No compatible run evidence')
    expect(html).not.toContain('100%')
    expect(html).not.toContain('>0<')
  })
})
