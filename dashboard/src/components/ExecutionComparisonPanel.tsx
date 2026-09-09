import { ArrowRight } from 'lucide-react'
import type React from 'react'
import { DeltaValue, Panel } from '@/design-system'
import type { ComparedMetric } from '@/lib/test-history-comparison'

export type { ComparedMetric } from '@/lib/test-history-comparison'

export function finiteMetric(value: number | null | undefined): number | null {
  return value !== null && value !== undefined && Number.isFinite(value)
    ? value
    : null
}

export function formatCost(value: number | null | undefined) {
  const known = finiteMetric(value)
  return known === null ? '—' : `$${known.toFixed(2)}`
}

export function formatDuration(value: number | null | undefined) {
  const known = finiteMetric(value)
  if (known === null) return '—'
  if (known < 60) return `${known.toFixed(1).replace(/\.0$/, '')}s`
  const rounded = Math.max(0, Math.round(known))
  return `${Math.floor(rounded / 60)}m ${String(rounded % 60).padStart(2, '0')}s`
}

export function formatTokens(value: number | null | undefined) {
  const known = finiteMetric(value)
  if (known === null) return '—'
  if (Math.abs(known) >= 1000) {
    return `${(known / 1000).toFixed(1).replace(/\.0$/, '')}k`
  }
  return Math.round(known).toLocaleString()
}

export function formatCount(value: number | null | undefined) {
  const known = finiteMetric(value)
  return known === null ? '—' : Math.round(known).toLocaleString()
}

export function formatScore(value: number | null | undefined) {
  const known = finiteMetric(value)
  return known === null ? '—' : known.toFixed(0)
}

export type ComparisonMetricKey =
  | 'score'
  | 'duration'
  | 'tokens'
  | 'cost'
  | 'functionCalls'
  | 'functionErrors'
  | 'turns'

export const COMPARISON_METRICS: Array<{
  key: ComparisonMetricKey
  label: string
  betterWhen: 'higher' | 'lower' | 'neither'
  format: (value: number | null) => string
}> = [
  { key: 'score', label: 'score', betterWhen: 'higher', format: formatScore },
  {
    key: 'duration',
    label: 'duration',
    betterWhen: 'lower',
    format: formatDuration,
  },
  { key: 'tokens', label: 'tokens', betterWhen: 'lower', format: formatTokens },
  { key: 'cost', label: 'cost', betterWhen: 'lower', format: formatCost },
  {
    key: 'functionCalls',
    label: 'functions',
    betterWhen: 'neither',
    format: formatCount,
  },
  {
    key: 'functionErrors',
    label: 'errors',
    betterWhen: 'lower',
    format: formatCount,
  },
  { key: 'turns', label: 'turns', betterWhen: 'lower', format: formatCount },
]

function deltaMagnitude(
  metric: ComparisonMetricKey,
  value: ComparedMetric,
): { value: number | null; format: (magnitude: number) => string } {
  if (value.delta === null) return { value: null, format: String }
  if (metric === 'score')
    return { value: value.delta, format: (m) => `${m.toFixed(0)} pts` }
  if (
    metric === 'functionCalls' ||
    metric === 'functionErrors' ||
    metric === 'turns'
  )
    return { value: value.delta, format: (m) => Math.round(m).toLocaleString() }
  if (value.relativeDelta === null) {
    const format =
      metric === 'cost'
        ? formatCost
        : metric === 'duration'
          ? formatDuration
          : formatTokens
    return { value: value.delta, format: (m) => format(m) }
  }
  return {
    value: value.relativeDelta * 100,
    format: (m) => `${m.toFixed(m >= 10 ? 0 : 1)}%`,
  }
}

export function SectionPanel({
  title,
  summary,
  headingId,
  actions,
  children,
  ...props
}: {
  title: string
  summary?: React.ReactNode
  headingId: string
  actions?: React.ReactNode
  children: React.ReactNode
} & Omit<React.HTMLAttributes<HTMLElement>, 'title'>) {
  return (
    <Panel aria-labelledby={headingId} {...props}>
      <div className="mb-4 flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h2 className="m-0 text-sm font-semibold text-ink" id={headingId}>
            {title}
          </h2>
          <p className="mt-1 mb-0 font-mono text-label text-ink-muted">
            {summary}
          </p>
        </div>
        {actions ? (
          <div className="flex flex-wrap items-center gap-2">{actions}</div>
        ) : null}
      </div>
      {children}
    </Panel>
  )
}

export function ExecutionComparisonPanel({
  headingId = 'execution-comparison-title',
  title = 'a → b',
  summary,
  metrics,
  controls,
  children,
  actions,
  interpretDeltas = false,
  showDeltas = true,
  metricLabels,
  ...props
}: {
  headingId?: string
  title?: string
  summary?: React.ReactNode
  metrics: Partial<Record<ComparisonMetricKey, ComparedMetric>>
  controls?: React.ReactNode
  children?: React.ReactNode
  actions?: React.ReactNode
  interpretDeltas?: boolean
  showDeltas?: boolean
  metricLabels?: Partial<Record<ComparisonMetricKey, string>>
} & Omit<React.HTMLAttributes<HTMLElement>, 'title' | 'children'>) {
  return (
    <SectionPanel
      title={title}
      summary={summary}
      headingId={headingId}
      actions={controls}
      {...props}
    >
      <div className="grid gap-4">
        {children}
        <div className="grid gap-2 @[560px]:grid-cols-2 @[840px]:grid-cols-4">
          {COMPARISON_METRICS.map(({ key, label, format, betterWhen }) => {
            const value = metrics[key]
            if (!value || (value.baseline === null && value.candidate === null))
              return null
            const delta = deltaMagnitude(key, value)
            return (
              <article
                className="grid gap-1 rounded-[6px] bg-[var(--surface-fill)] p-3"
                key={key}
                data-comparison-metric={key}
              >
                <span className="ds-label">{metricLabels?.[key] ?? label}</span>
                <span className="flex items-center gap-2 font-mono text-sm text-ink">
                  <span>{format(value.baseline)}</span>
                  <ArrowRight size={13} aria-hidden="true" />
                  <span>{format(value.candidate)}</span>
                </span>
                {showDeltas ? (
                  <DeltaValue
                    className="text-label"
                    value={delta.value}
                    format={delta.format}
                    betterWhen={interpretDeltas ? betterWhen : 'neither'}
                    unavailableLabel="—"
                    title="b − a"
                  />
                ) : (
                  <span className="font-mono text-label text-ink-muted">
                    a → b
                  </span>
                )}
              </article>
            )
          })}
        </div>
        {actions ? <div className="flex flex-wrap gap-2">{actions}</div> : null}
      </div>
    </SectionPanel>
  )
}
