import { describe, expect, it } from 'vitest'
import {
  comparisonMetric,
  formatMetricDelta,
  formatMetricValue,
} from '@/lib/metric-comparison'

describe('metric comparison', () => {
  it('reports a signed difference without declaring a winner', () => {
    const tokens = comparisonMetric(
      'tokens',
      'Total tokens',
      1_000,
      1_100,
      'tokens',
    )
    expect(tokens).toMatchObject({ delta: 100, tone: 'neutral' })
    expect(formatMetricDelta(tokens)).toBe('+100 · +10.0%')
    expect(formatMetricValue(tokens, 'candidate')).toBe('1.1K')
  })

  it('keeps a missing side unavailable instead of zero', () => {
    const cost = comparisonMetric('cost', 'Reported cost', null, 0.5, 'usd')
    expect(cost).toMatchObject({ delta: null, tone: 'unavailable' })
    expect(formatMetricValue(cost, 'baseline')).toBe('Not reported')
    expect(formatMetricDelta(cost)).toBe('Not comparable')
  })

  it('rounds a duration before splitting it into minutes and seconds', () => {
    const duration = (seconds: number) =>
      formatMetricValue(
        comparisonMetric('duration', 'Duration', seconds, null, 'seconds'),
        'baseline',
      )
    expect(duration(119.6)).toBe('2m 00s')
    expect(duration(59.7)).toBe('1m 00s')
    expect(duration(83.2)).toBe('1m 23s')
    expect(duration(9.96)).toBe('10.0s')
  })
})
