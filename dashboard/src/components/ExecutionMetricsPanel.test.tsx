import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { ExecutionMetricsPanel } from '@/components/ExecutionMetricsPanel'
import {
  executionMetricsFixture,
  metricRun,
} from '@/test-fixtures/execution-metrics'

describe('execution summary panel', () => {
  it('shows all-scenario metrics without expanding individual scenarios', () => {
    const detail = executionMetricsFixture([
      { runs: [metricRun('a', 100_000, { quality_score_completed: 60 })] },
      {
        runs: [
          metricRun('b', 20_000, {
            completion: 'task_incomplete',
            quality_score_completed: null,
          }),
        ],
      },
      { runs: [metricRun('c', 120_000, { quality_score_completed: 100 })] },
    ])
    const html = renderToStaticMarkup(<ExecutionMetricsPanel detail={detail} />)
    expect(html).toContain('execution summary')
    expect(html).toContain('id="metrics"')
    expect(html).toContain('3/3 scenarios')
    expect(html).toContain('66.7%')
    expect(html).toContain('2/3 determined runs')
    expect(html).toContain('240,000')
    expect(html).toContain('120,000')
    expect(html).toContain('110,000')
    expect(html).toContain('20,000')
    expect(html).toContain('80/100')
    expect(html).toContain('3/3 runs with telemetry')
    expect(html).not.toContain('<details')
  })

  // Audit ED-26: inside the counts layer the layer row is the heading.
  it('renders headless inside a layer: same numbers, no title, no anchor', () => {
    const detail = executionMetricsFixture([
      { runs: [metricRun('a', 100_000, { quality_score_completed: 60 })] },
    ])
    const html = renderToStaticMarkup(
      <ExecutionMetricsPanel detail={detail} headless />,
    )
    expect(html).toContain('data-execution-metrics="headless"')
    expect(html).not.toContain('execution summary')
    expect(html).not.toContain('id="metrics"')
    expect(html).toContain('100,000')
    expect(html).toContain('1/1 runs with telemetry')
  })

  it('marks observed subtotals and distinguishes missing cost from zero', () => {
    const detail = executionMetricsFixture([
      {
        runs: [
          metricRun('a', 100, { cost: null }),
          metricRun('b', null, { cost: null }),
        ],
      },
    ])
    const html = renderToStaticMarkup(<ExecutionMetricsPanel detail={detail} />)
    expect(html).toContain('observed subtotal')
    expect(html).toContain('1/2 runs with telemetry')
    expect(html).toContain('0/2 runs with telemetry')
    expect(html).not.toContain('$0.0000')
    expect(html).toMatch(/Tokens per completion<\/td><td[^>]*>—<\/td>/)
  })

  it('labels missing scenario scope and never renders fabricated metrics', () => {
    const detail = executionMetricsFixture([{ runs: [metricRun('a', 100)] }])
    detail.reports[0].available = false
    const html = renderToStaticMarkup(<ExecutionMetricsPanel detail={detail} />)
    expect(html).toContain('partial evidence')
    expect(html).toContain('No compatible run evidence')
    expect(html).not.toContain('100%')
    expect(html).not.toContain('$0.0000')
  })
})
