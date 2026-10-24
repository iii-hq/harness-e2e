import { ChevronRight, Info } from 'lucide-react'
import { useId, useMemo, useState } from 'react'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import {
  buildExecutionMetrics,
  causeLabel,
  type UsageCoverage,
} from '@/lib/execution-metrics'
import { formatTokens as compactTokens, formatCost } from '@/lib/format'
import type { MetricValue, PrimaryMetrics } from '@/lib/primary-metrics'
import { type ScenarioMatrixItem, unreported } from '@/lib/scenario-matrix'
import './execution-page.css'

const full = new Intl.NumberFormat('en-US')

export function metricNumber(metric: MetricValue | undefined) {
  if (!metric) return null
  return metric.value ?? metric.observed
}

/** Token counts as the canvas writes them: 6K, 35.2K, 118K, 4.28M, 82.1M. */
export function formatTokens(value: number | null) {
  return compactTokens(value)
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

/** `$0.0048` under a dollar, `$2.57` above. */
export function formatUsd(value: number | null) {
  return formatCost(value)
}

export function plural(count: number, one: string, many: string) {
  return `${count} ${count === 1 ? one : many}`
}

/** One figure card: a label, the figure in mono and a caption under it; the
 *  whole number, when the figure is short, on the caption's hover. */
export type Kpi = { label: string; value: string; sub: string; full?: string }

export function KpiTile({
  kpi,
  as: Tag = 'div',
}: {
  kpi: Kpi
  as?: 'div' | 'li'
}) {
  return (
    <Tag className="ep-kpi">
      <span className="ep-kpi-label">{kpi.label}</span>
      <span className="ep-kpi-value">{kpi.value}</span>
      <span className="ep-kpi-sub" title={kpi.full}>
        {kpi.sub}
      </span>
    </Tag>
  )
}

/** Prompt and context figures, then the runs per cause. A provider whose
 *  prompt does not match the Harness's count is marked not comparable. */
export function contextRows(
  metrics: ReturnType<typeof buildExecutionMetrics>,
): Array<[string, string]> {
  // A sum short of runs is shown as what was observed, and says so.
  const counted = (usage: UsageCoverage): string =>
    usage.total === null && usage.observed !== null
      ? `${formatFull(usage.observed)} (partial: ${usage.samples} of ${usage.expected} runs)`
      : formatFull(usage.total)
  const notComparable =
    metrics.cacheNotComparable > 0
      ? ` (not comparable: ${plural(metrics.cacheNotComparable, 'run', 'runs')})`
      : ''
  return [
    ['Processed prompt', `${counted(metrics.processedPrompt)}${notComparable}`],
    [
      'Cache share',
      metrics.cacheShare === null
        ? '—'
        : `${(metrics.cacheShare * 100).toFixed(1)}%`,
    ],
    ['Context per turn', formatFull(metrics.contextPerTurn)],
    [
      'Peak context',
      metrics.peakContext === null
        ? '—'
        : `${formatFull(metrics.peakContext)}${metrics.usableContext ? ` of ${formatFull(metrics.usableContext)}` : ''}`,
    ],
    ['Compactions', counted(metrics.compactions)],
    ...Object.entries(metrics.causes)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([cause, count]): [string, string] => [
        `Runs: ${causeLabel(cause)}`,
        formatFull(count),
      ]),
  ]
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
  const execution = useMemo(
    () => (detail ? buildExecutionMetrics(detail) : null),
    [detail],
  )
  const failedAttempt = execution
    ? (execution.failedAttemptTokens.total ??
      execution.failedAttemptTokens.observed)
    : null
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
  const kpis: Kpi[] = [
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
    ...(execution ? contextRows(execution) : []),
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
          <KpiTile key={kpi.label} kpi={kpi} />
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
        {execution && execution.earlierRule > 0 ? (
          <span className="ep-faint ep-partial">
            <Info size={14} aria-hidden="true" />
            {`${plural(execution.earlierRule, 'run predates', 'runs predate')} run causes: classified by the earlier rule, where every subject error left the mean.`}
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
