import { ChevronRight, Info } from 'lucide-react'
import { useId, useMemo, useState } from 'react'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import { buildExecutionMetrics } from '@/lib/execution-metrics'
import type { MetricValue, PrimaryMetrics } from '@/lib/primary-metrics'
import { type ScenarioMatrixItem, unreported } from '@/lib/scenario-matrix'
import './execution-page.css'

const full = new Intl.NumberFormat('en-US')

export function metricNumber(metric: MetricValue | undefined) {
  if (!metric) return null
  return metric.value ?? metric.observed
}

/** Token counts as the canvas writes them: 6K, 35.2K, 118K, 4.28M. */
export function formatTokens(value: number | null) {
  if (value === null) return '—'
  const n = Math.round(value)
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`
  if (n >= 100_000) return `${Math.round(n / 1000)}K`
  if (n >= 1000) return `${(n / 1000).toFixed(1).replace(/\.0$/, '')}K`
  return String(n)
}

export function formatFull(value: number | null) {
  return value === null ? '—' : full.format(Math.round(value))
}

export function formatSpan(ms: number | null) {
  if (ms === null) return '—'
  const seconds = Math.round(ms / 1000)
  const h = Math.floor(seconds / 3600)
  const m = Math.floor((seconds % 3600) / 60)
  const s = seconds % 60
  if (h > 0) return `${h}h ${String(m).padStart(2, '0')}m`
  if (m > 0) return `${m}m ${String(s).padStart(2, '0')}s`
  return `${s}s`
}

export function formatUsd(value: number | null) {
  if (value === null) return '—'
  if (value > 0 && value < 0.0001) return '<$0.0001'
  return `$${value.toFixed(value >= 10 ? 2 : 4)}`
}

function plural(count: number, one: string, many: string) {
  return `${count} ${count === 1 ? one : many}`
}

/** The execution's totals: six figures, the partial note, and "All metrics"
 *  for the full set (canvas: Execution detail · Totals). */
export function ExecutionTotals({
  metrics,
  items,
  running = false,
  detail,
}: {
  metrics: PrimaryMetrics
  items: ScenarioMatrixItem[]
  running?: boolean
  /** The execution, for the failed attempt tokens. */
  detail?: DashboardExecutionDetail
}) {
  const [open, setOpen] = useState(false)
  const failedAttempt = useMemo(() => {
    if (!detail) return null
    const failed = buildExecutionMetrics(detail).failedAttemptTokens
    return failed.total ?? failed.observed
  }, [detail])
  const tableId = useId()
  const m = metrics.metrics
  const total = items.length
  const scores = metrics.tests
    .map((test) => metricNumber(test.metrics.score))
    .filter((value): value is number => value !== null)
  const passed = items.filter((item) => item.objective.status === 'passed')
  const notRun = items.filter((item) => item.runCount === 0).length
  const lost = scores.filter((value) => value < 100).length
  const score = metricNumber(m.score)
  const input = metricNumber(m.inputTokens)
  const output = metricNumber(m.outputTokens)
  const tokens = metricNumber(m.totalTokens)
  const cacheRead = metricNumber(m.cacheRead)
  const cacheWrite = metricNumber(m.cacheWrite)
  const duration = metricNumber(m.durationMs)
  const cost = metricNumber(m.costUsd)
  // Reported: every test but those running or queued; one that did not run
  // reported so.
  const reported = items.filter((item) => !unreported(item)).length
  const partial = scores.length < total
  const kpis: Array<{
    label: string
    value: string
    sub: string
    full?: string
  }> = [
    {
      label: running ? 'Score so far' : 'Score',
      value: score === null ? '—' : String(Math.round(score * 10) / 10),
      sub: `mean of ${plural(scores.length, 'scored test', 'scored tests')}`,
    },
    {
      label: 'Tests passed',
      value: `${passed.length}/${total}`,
      sub: running
        ? `${total - reported} still to report`
        : [
            lost ? `${lost} lost points` : null,
            notRun ? `${notRun} not run` : null,
          ]
            .filter(Boolean)
            .join(' · ') || 'all at 100',
    },
    {
      label: 'Tokens',
      value: formatTokens(tokens),
      sub: `in ${formatTokens(input)} · out ${formatTokens(output)}`,
      full:
        tokens === null ? undefined : `${formatFull(tokens)} input + output`,
    },
    {
      label: 'Cache',
      value: formatTokens(cacheRead),
      sub: `read · written ${formatTokens(cacheWrite)}`,
      full: cacheRead === null ? undefined : `${formatFull(cacheRead)} read`,
    },
    {
      label: 'Duration',
      value: formatSpan(duration),
      sub: 'sum of runs, retries included',
    },
    {
      label: 'Cost',
      value: formatUsd(cost),
      sub: cost === null ? 'not reported by the provider' : 'recorded spend',
    },
  ]
  const all: Array<[string, string]> = [
    ['Input tokens', formatFull(input)],
    ['Output tokens', formatFull(output)],
    ['Cache read', formatFull(cacheRead)],
    ['Cache written', formatFull(cacheWrite)],
    ['Turns', formatFull(metricNumber(m.turns))],
    ['Function calls', formatFull(metricNumber(m.functionCalls))],
    ['Function call errors', formatFull(metricNumber(m.functionErrors))],
    ...(detail
      ? ([['Failed attempt tokens', formatFull(failedAttempt)]] as Array<
          [string, string]
        >)
      : []),
    [
      'Tokens per completion',
      tokens !== null && passed.length > 0
        ? formatFull(tokens / passed.length)
        : '—',
    ],
  ]
  return (
    <section className="ep-totals" aria-label="Totals" data-execution-totals>
      <div className="ep-kpis">
        {kpis.map((kpi) => (
          <div className="ep-kpi" key={kpi.label}>
            <span className="ep-kpi-label">{kpi.label}</span>
            <span className="ep-kpi-value">{kpi.value}</span>
            <span className="ep-kpi-sub" title={kpi.full}>
              {kpi.sub}
            </span>
          </div>
        ))}
      </div>
      <div className="ep-totals-bar">
        {partial ? (
          <span className="ep-faint ep-partial">
            <Info size={14} aria-hidden="true" />
            {running
              ? `Totals so far: ${reported} of ${total} tests reported.`
              : `Partial: ${scores.length} of ${total} tests reported. Totals cover only what was reported.`}
          </span>
        ) : null}
        <button
          type="button"
          className="ds-button ds-button-quiet ds-button-compact ep-all-metrics"
          aria-expanded={open}
          aria-controls={tableId}
          onClick={() => setOpen(!open)}
        >
          <ChevronRight
            size={14}
            aria-hidden="true"
            className={open ? 'ep-rot' : undefined}
          />
          All metrics
        </button>
      </div>
      {open ? (
        <dl
          id={tableId}
          className="ep-kpis ep-all-tiles"
          aria-label="All metrics"
        >
          {all.map(([label, value]) => (
            <div className="ep-kpi" key={label}>
              <dt className="ep-kpi-label">{label}</dt>
              <dd className="ep-kpi-value">{value}</dd>
            </div>
          ))}
        </dl>
      ) : null}
    </section>
  )
}
