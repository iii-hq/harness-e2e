import type { TrendPoint, TrendSeries } from '@/lib/trends'
import fixture from '@/test-fixtures/trends.json'

/** The Trends canvas's series (regression, se, local, opus): real
 *  executions of the dev Console on Sep 28 in trends-get's shape, latest
 *  series first, each series' points oldest first. */
export const trendSeries = fixture.series as unknown as Array<{
  key: 'regression' | 'se' | 'local' | 'opus'
  series: TrendSeries
  points: TrendPoint[]
}>

/** A series' points under a stack filter, as the worker would answer. */
export function seriesPoints(key: string, stack = 'any'): TrendPoint[] {
  const points = trendSeries.find((item) => item.key === key)?.points ?? []
  return stack === 'any'
    ? points
    : points.filter((point) => (point.stack.name ?? 'not_recorded') === stack)
}
