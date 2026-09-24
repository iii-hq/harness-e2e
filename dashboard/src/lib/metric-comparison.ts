import { formatDuration } from '@/lib/execution-view'

export type MetricTone = 'neutral' | 'unavailable'
export type MetricFormat =
  | 'percent_points'
  | 'score'
  | 'count'
  | 'tokens'
  | 'seconds'
  | 'milliseconds'
  | 'usd'

/** One figure on two sides: `baseline` is A, `candidate` is B. */
export type MetricComparison = {
  id: string
  label: string
  baseline: number | null
  candidate: number | null
  delta: number | null
  delta_percent: number | null
  format: MetricFormat
  tone: MetricTone
}

export function comparisonMetric(
  id: string,
  label: string,
  baseline: number | null,
  candidate: number | null,
  format: MetricFormat,
): MetricComparison {
  const delta =
    baseline === null || candidate === null ? null : candidate - baseline
  const baselineMagnitude = baseline === null ? null : Math.abs(baseline)
  return {
    id,
    label,
    baseline,
    candidate,
    delta,
    delta_percent:
      delta === null || !baselineMagnitude
        ? null
        : (delta / baselineMagnitude) * 100,
    format,
    tone: delta === null ? 'unavailable' : 'neutral',
  }
}

function compactNumber(value: number) {
  // Three significant digits in compact notation, so 1,000 and 1,100 do not
  // both read "1K".
  if (Math.abs(value) >= 1000)
    return new Intl.NumberFormat('en-US', {
      notation: 'compact',
      maximumSignificantDigits: 3,
    }).format(value)
  return new Intl.NumberFormat('en-US', {
    maximumFractionDigits: Math.abs(value) >= 100 ? 0 : 1,
  }).format(value)
}

function signed(value: number, formatted: string) {
  if (formatted.startsWith('-') || formatted.startsWith('+')) return formatted
  return `${value > 0 ? '+' : value < 0 ? '-' : ''}${formatted}`
}

export function formatMetricValue(
  metric: MetricComparison,
  side: 'baseline' | 'candidate',
): string {
  const value = metric[side]
  if (value === null) return 'Not reported'
  switch (metric.format) {
    case 'percent_points':
      return `${compactNumber(value)}%`
    case 'score':
      return compactNumber(value)
    case 'tokens':
      return compactNumber(value)
    case 'seconds':
      return formatDuration(value)
    case 'milliseconds':
      return value < 1_000
        ? `${compactNumber(value)} ms`
        : `${(value / 1_000).toFixed(1)}s`
    case 'usd':
      return `$${value.toFixed(value < 1 ? 4 : 2)}`
    case 'count':
      return compactNumber(value)
  }
}

export function formatMetricDelta(metric: MetricComparison): string {
  if (metric.delta === null) return 'Not comparable'
  const value = metric.delta
  if (Math.abs(value) < 1e-9) return 'No change'
  if (metric.format === 'percent_points') {
    return `${signed(value, compactNumber(value))} pp`
  }
  if (metric.format === 'score') {
    return `${signed(value, compactNumber(value))} pts`
  }
  const absolute = (() => {
    if (metric.format === 'seconds')
      return signed(value, `${Math.abs(value).toFixed(1)}s`)
    if (metric.format === 'milliseconds')
      return signed(value, `${compactNumber(Math.abs(value))} ms`)
    if (metric.format === 'usd')
      return signed(value, `$${Math.abs(value).toFixed(4)}`)
    return signed(value, compactNumber(Math.abs(value)))
  })()
  const relative = metric.delta_percent
  return relative === null
    ? absolute
    : `${absolute} · ${signed(relative, `${Math.abs(relative).toFixed(1)}%`)}`
}
