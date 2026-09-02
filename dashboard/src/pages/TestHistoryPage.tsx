import {
  ArrowLeftRight,
  ArrowRight,
  ChevronLeft,
  ChevronRight,
  Copy,
} from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { AssessmentWorkspace } from '@/components/AssessmentWorkspace'
import {
  DashboardPageActions,
  dashboardHeaderActionClassName,
} from '@/components/DashboardPageActions'
import {
  requestPlanFromSelection,
  requestQuickExecution,
} from '@/components/ExecutionSetup'
import { ProviderModelDropdown } from '@/components/ProviderModelDropdown'
import { ScenarioChatAction } from '@/components/ScenarioChatAction'
import {
  buttonClassName,
  Callout,
  DataTable,
  DataTableRow,
  DeltaValue,
  Dialog,
  EmptyState,
  FilterChip,
  FilterChipGroup,
  isInteractiveTarget,
  MetricCard,
  numericCellClassName,
  type OperationalStatus,
  PageHeader,
  Panel,
  Select,
  StatusBadge,
} from '@/design-system'
import {
  hashForComparison,
  hashForExecution,
  hashForNewPlan,
  hashForTestHistory,
  hashForTests,
  hashForWorkspace,
  replaceRouteParams,
  routeParams,
} from '@/hooks/use-hash-route'
import {
  type DashboardExecutionDetail,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import type {
  HistoryModelGroup,
  TestCatalogRow,
  TestHistoryResponse,
  TestObservation,
} from '@/lib/test-catalog'
import {
  type ComparedMetric,
  compareTestObservations,
  testObservationKey,
} from '@/lib/test-history-comparison'
import {
  catalogComplexityPresentation,
  catalogRealismPresentation,
} from '@/pages/TestsCatalogPage'

/* ---------------------------------------------------------------- helpers */

function modelSelection(provider: string, model: string) {
  return JSON.stringify([provider, model])
}

function parseModelSelection(value: string) {
  if (!value) return null
  try {
    const parsed = JSON.parse(value) as unknown
    if (
      Array.isArray(parsed) &&
      parsed.length === 2 &&
      typeof parsed[0] === 'string' &&
      typeof parsed[1] === 'string'
    ) {
      return { provider: parsed[0], model: parsed[1] }
    }
  } catch {
    // A malformed selection is treated as no filter.
  }
  return null
}

function modelGroups(
  history: TestHistoryResponse | null,
  role: 'subject' | 'judge',
): HistoryModelGroup[] {
  const configured =
    role === 'subject' ? history?.subject_models : history?.judge_models
  if (configured?.length) return configured
  if (!history) return []

  const groups = new Map<string, Set<string>>()
  for (const observation of history.observations) {
    const provider =
      role === 'subject'
        ? observation.subject_provider
        : observation.judge_provider
    const model =
      role === 'subject' ? observation.subject_model : observation.judge_model
    if (!provider || !model) continue
    const models = groups.get(provider) ?? new Set<string>()
    models.add(model)
    groups.set(provider, models)
  }
  return [...groups.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([provider, models]) => ({
      provider,
      models: [...models].sort((left, right) => left.localeCompare(right)),
    }))
}

function median(values: Array<number | null | undefined>) {
  const known = values
    .filter(
      (value): value is number => value !== null && Number.isFinite(value),
    )
    .sort((left, right) => left - right)
  if (known.length === 0) return null
  const middle = Math.floor(known.length / 2)
  return known.length % 2
    ? known[middle]
    : (known[middle - 1] + known[middle]) / 2
}

function finiteMetric(value: number | null | undefined): number | null {
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

function formatCount(value: number | null | undefined) {
  const known = finiteMetric(value)
  return known === null ? '—' : Math.round(known).toLocaleString()
}

function formatScore(value: number | null | undefined) {
  const known = finiteMetric(value)
  return known === null ? '—' : known.toFixed(0)
}

function formatDate(value: string) {
  if (!value) return 'not completed'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return 'unknown date'
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  }).format(date)
}

function formatDay(value: string) {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return ''
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
  }).format(date)
}

/** Audit TH-05: the DS status vocabulary; "passed" is green, never accent. */
export function statusPresentation(status: string): {
  status: OperationalStatus
  label: string
} {
  if (status === 'passed') return { status: 'passed', label: 'passed' }
  if (status === 'hard_gate_failed')
    return { status: 'hard_gate', label: 'hard gate failed' }
  if (status === 'technical_failed')
    return { status: 'failed', label: 'technical failure' }
  if (status === 'infra_failed')
    return { status: 'failed', label: 'infrastructure failure' }
  return {
    status: 'failed',
    label: status.replace(/[_-]+/g, ' ').toLowerCase(),
  }
}

function shortHash(value: string | null | undefined) {
  if (!value) return null
  return value.replace(/^sha256:/, '').slice(0, 8)
}

function contractSummary(observations: TestObservation[]) {
  const contracts = [
    ...new Set(
      observations
        .map((observation) => observation.contract_sha256)
        .filter(Boolean),
    ),
  ]
  if (contracts.length === 0) return null
  if (contracts.length > 1) return { short: 'multiple contracts', full: null }
  return { short: `sha ${shortHash(contracts[0])}`, full: contracts[0] }
}

function modelLabel(provider?: string | null, model?: string | null) {
  if (!provider && !model) return 'unknown model'
  return [provider, model].filter(Boolean).join('/') || 'unknown model'
}

function systemSummary(observation: TestObservation) {
  const revision = shortHash(observation.system_revision)
  const stack =
    observation.stack_mode === 'source'
      ? 'source'
      : observation.stack_mode === 'registry'
        ? 'registry'
        : 'local'
  return revision ? `${stack} ${revision}` : observation.system_label || stack
}

function knownMetricCount(values: Array<number | null | undefined>) {
  return values.filter((value) => finiteMetric(value) !== null).length
}

export function metricCaption(known: number, total: number) {
  if (known === total)
    return `across ${total} ${total === 1 ? 'execution' : 'executions'}`
  return `across ${known} of ${total} executions`
}

function runLabel(count: number) {
  return `${count} ${count === 1 ? 'run' : 'runs'}`
}

type ComparisonMetricKey =
  | 'score'
  | 'duration'
  | 'tokens'
  | 'cost'
  | 'functionCalls'
  | 'functionErrors'
  | 'turns'

const COMPARISON_METRICS: Array<{
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

/* --------------------------------------------------------------- trend */

/** The rendered width of an element, so an SVG viewBox can match its pixels. */
function useMeasuredWidth<T extends HTMLElement>(fallback: number) {
  const ref = useRef<T>(null)
  const [width, setWidth] = useState(fallback)
  useEffect(() => {
    const element = ref.current
    if (!element || typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver((entries) => {
      const next = entries[0]?.contentRect.width
      if (next) setWidth(Math.round(next))
    })
    observer.observe(element)
    return () => observer.disconnect()
  }, [])
  return { ref, width }
}

export function ScoreTrendChart({
  observations,
  selectedKeys,
  onSelect,
}: {
  observations: TestObservation[]
  selectedKeys: string[]
  onSelect: (observation: TestObservation) => void
}) {
  const { ref, width } = useMeasuredWidth<HTMLDivElement>(560)
  // Observations arrive newest-first; the chart reads left → right in time.
  const plotted = [...observations]
    .reverse()
    .filter((item) => finiteMetric(item.median_score) !== null)
  if (plotted.length === 0) {
    return (
      <p className="m-0 text-xs text-ink-soft">
        No scored executions to plot yet.
      </p>
    )
  }
  const left = 44
  const right = Math.max(left + 80, width - 20)
  const top = 16
  const bottom = 132
  const xs = plotted.map((_, index) =>
    plotted.length === 1
      ? (left + right) / 2
      : left + (index * (right - left)) / (plotted.length - 1),
  )
  const yFor = (score: number) =>
    top + ((100 - Math.max(0, Math.min(100, score))) / 100) * (bottom - top)
  const line = xs
    .map(
      (x, index) =>
        `${x.toFixed(1)},${yFor(plotted[index].median_score as number).toFixed(1)}`,
    )
    .join(' ')
  const gate = yFor(50)
  const anchorFor = (index: number) =>
    plotted.length === 1
      ? 'middle'
      : index === 0
        ? 'start'
        : index === plotted.length - 1
          ? 'end'
          : 'middle'
  return (
    <div ref={ref} className="w-full">
      <svg
        className="block h-40 w-full font-mono text-label"
        viewBox={`0 0 ${width} 160`}
        role="img"
        aria-label="Median score per retained execution, oldest on the left"
        data-score-trend
      >
        {[top, gate, bottom].map((y) => (
          <line
            key={y}
            className="stroke-line"
            x1={left - 8}
            y1={y}
            x2={right + 8}
            y2={y}
            strokeDasharray={y === gate ? '3 3' : undefined}
          />
        ))}
        <text
          className="fill-ink-soft"
          x={left - 12}
          y={top + 4}
          textAnchor="end"
        >
          100
        </text>
        <text
          className="fill-ink-soft"
          x={left - 12}
          y={gate + 4}
          textAnchor="end"
        >
          50
        </text>
        <text
          className="fill-ink-soft"
          x={left - 12}
          y={bottom + 4}
          textAnchor="end"
        >
          0
        </text>
        <text
          className="fill-ink-muted"
          x={left - 12}
          y={gate + 14}
          textAnchor="end"
        >
          gate
        </text>
        {plotted.length > 1 ? (
          <polyline
            className="fill-none stroke-ink-muted"
            strokeWidth="1.5"
            points={line}
            vectorEffect="non-scaling-stroke"
          />
        ) : null}
        {plotted.map((item, index) => {
          const key = testObservationKey(item)
          const selectedIndex = selectedKeys.indexOf(key)
          const failed = item.status !== 'passed'
          const score = item.median_score as number
          const x = xs[index]
          const y = yFor(score)
          const slot =
            selectedIndex === 0 ? 'A' : selectedIndex === 1 ? 'B' : null
          const labelBelow = y <= bottom - 24
          return (
            // biome-ignore lint/a11y/useSemanticElements: an SVG hit target cannot be a native <button>; the group carries the full button contract (role, tabIndex, keyboard activation).
            <g
              className="cursor-pointer outline-none focus-visible:[outline:2px_solid_var(--accent)]"
              key={key}
              role="button"
              tabIndex={0}
              aria-pressed={selectedIndex >= 0}
              aria-label={`${formatDate(item.completed_at)} · score ${score.toFixed(0)} · ${statusPresentation(item.status).label}${slot ? ` · selected as ${slot}` : ''}`}
              onClick={() => onSelect(item)}
              onKeyDown={(event) => {
                if (event.key === 'Enter' || event.key === ' ') {
                  event.preventDefault()
                  onSelect(item)
                }
              }}
            >
              <title>
                {`${formatDate(item.completed_at)} · score ${score.toFixed(0)} · ${statusPresentation(item.status).label} · ${formatDuration(item.median_duration_seconds)} · ${formatTokens(item.median_tokens)} tokens`}
              </title>
              <circle className="fill-transparent" cx={x} cy={y} r="12" />
              <circle
                className={
                  selectedIndex >= 0
                    ? 'fill-ink'
                    : failed
                      ? 'fill-danger'
                      : 'fill-success'
                }
                cx={x}
                cy={y}
                r={selectedIndex >= 0 ? 5 : 4}
              />
              {slot ? (
                <g>
                  <rect
                    className="fill-ink"
                    x={x - 8}
                    y={labelBelow ? y + 9 : y - 24}
                    width="16"
                    height="14"
                    rx="3"
                  />
                  <text
                    className="fill-canvas font-semibold"
                    x={x}
                    y={labelBelow ? y + 19.5 : y - 13.5}
                    textAnchor="middle"
                  >
                    {slot}
                  </text>
                </g>
              ) : (
                <text
                  className="fill-ink-soft"
                  x={x}
                  y={labelBelow ? y + 18 : y - 9}
                  textAnchor={anchorFor(index)}
                >
                  {score.toFixed(0)}
                </text>
              )}
            </g>
          )
        })}
        <text className="fill-ink-muted" x={xs[0]} y="154" textAnchor="middle">
          {formatDay(plotted[0].completed_at)}
        </text>
        {plotted.length > 1 ? (
          <text
            className="fill-ink-muted"
            x={xs[xs.length - 1]}
            y="154"
            textAnchor="middle"
          >
            {formatDay(plotted[plotted.length - 1].completed_at)}
          </text>
        ) : null}
      </svg>
    </div>
  )
}

/** A panel with the DS heading pair (title, one-line summary) and actions. */
function SectionPanel({
  title,
  summary,
  headingId,
  actions,
  children,
  ...props
}: {
  title: string
  summary: string
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

/* ---------------------------------------------------------- comparison */

type Verdict = {
  status: OperationalStatus
  title: string
  detail: string
}

export function comparisonVerdict(
  comparison: ReturnType<typeof compareTestObservations>,
): Verdict {
  const objectiveRegressed =
    comparison.baseline.status === 'passed' &&
    comparison.candidate.status !== 'passed'
  const objectiveImproved =
    comparison.baseline.status !== 'passed' &&
    comparison.candidate.status === 'passed'
  let improved = objectiveImproved
  let regressed = objectiveRegressed
  const details: string[] = []
  if (objectiveImproved) details.push('now passes')
  if (objectiveRegressed) details.push('no longer passes')
  for (const metric of COMPARISON_METRICS) {
    if (metric.betterWhen === 'neither') continue
    const value = comparison.metrics[metric.key]
    if (value.delta === null || value.delta === 0) continue
    const better =
      metric.betterWhen === 'higher' ? value.delta > 0 : value.delta < 0
    if (better) improved = true
    else regressed = true
    details.push(
      `${metric.label} ${better ? (metric.key === 'score' ? 'up' : 'down') : metric.key === 'score' ? 'down' : 'up'}`,
    )
  }
  const status: OperationalStatus =
    improved && regressed
      ? 'inconclusive'
      : regressed
        ? 'failed'
        : improved
          ? 'passed'
          : 'inconclusive'
  const title =
    improved && regressed
      ? 'mixed result'
      : regressed
        ? 'b regressed'
        : improved
          ? 'b improved'
          : 'no material change'
  return {
    status,
    title,
    detail: details.length
      ? details.join(' · ')
      : 'all comparable metrics unchanged',
  }
}

export function ObservationComparisonPanel({
  baseline,
  candidate,
  testId,
  onClear,
  onSwap,
}: {
  baseline: TestObservation | null
  candidate: TestObservation | null
  testId: string
  onClear: () => void
  onSwap: () => void
}) {
  const comparison =
    baseline && candidate ? compareTestObservations(baseline, candidate) : null
  const verdict = comparison?.compatible ? comparisonVerdict(comparison) : null
  // Audit CP-19 / TH-08: a metric nobody reports is not a card.
  const metrics = comparison
    ? COMPARISON_METRICS.filter(({ key }) => {
        const value = comparison.metrics[key]
        return value.baseline !== null || value.candidate !== null
      })
    : []

  return (
    <SectionPanel
      title="a → b"
      summary={
        baseline && candidate
          ? `a = ${formatDate(baseline.completed_at)} · b = ${formatDate(candidate.completed_at)}`
          : 'choose two executions in the table'
      }
      headingId="test-comparison-title"
      actions={
        <>
          <button
            className={buttonClassName({
              variant: 'secondary',
              size: 'compact',
            })}
            type="button"
            onClick={onSwap}
            disabled={!candidate}
          >
            <ArrowLeftRight aria-hidden="true" size={13} />
            swap
          </button>
          <button
            className={buttonClassName({ variant: 'quiet', size: 'compact' })}
            type="button"
            onClick={onClear}
            disabled={!baseline}
          >
            clear
          </button>
        </>
      }
      data-test-comparison
      aria-live="polite"
    >
      {!baseline ? (
        <p className="m-0 text-xs text-ink-soft">
          Tick an execution in the table to choose a. Separate cases and cohorts
          are never pooled.
        </p>
      ) : !candidate ? (
        <p className="m-0 text-xs text-ink-soft">
          a = {formatDate(baseline.completed_at)}. Tick another execution to
          choose b.
        </p>
      ) : comparison ? (
        <div className="grid gap-4">
          {/* Audit TH-11: one banner says whether deltas can be read. */}
          {comparison.compatible ? (
            <Callout tone="success">
              <span className="flex flex-wrap items-center gap-2">
                {verdict ? (
                  <StatusBadge status={verdict.status} label={verdict.title} />
                ) : null}
                <span className="text-ink-soft">
                  {verdict?.detail} · same contract, seed, cohort, model and
                  judge
                </span>
              </span>
            </Callout>
          ) : (
            <Callout
              tone="warning"
              title="not comparable · values shown, deltas not interpreted"
            >
              {comparison.reasons.join(' · ')}. Choose two executions of the
              same model, judge and cohort to read deltas.
            </Callout>
          )}
          <div className="grid gap-2 @[560px]:grid-cols-2 @[840px]:grid-cols-4">
            {metrics.map(({ key, label, betterWhen, format }) => {
              const value = comparison.metrics[key]
              const delta = deltaMagnitude(key, value)
              return (
                <article
                  className="grid gap-1 rounded-[6px] bg-[var(--surface-fill)] p-3"
                  key={key}
                  data-comparison-metric={key}
                >
                  <span className="ds-label">{label}</span>
                  <span className="flex items-center gap-2 font-mono text-sm text-ink">
                    <span>{format(value.baseline)}</span>
                    <ArrowRight
                      className="text-ink-muted"
                      aria-hidden="true"
                      size={13}
                    />
                    <span>{format(value.candidate)}</span>
                  </span>
                  {comparison.compatible ? (
                    <DeltaValue
                      className="text-label"
                      value={delta.value}
                      format={delta.format}
                      betterWhen={betterWhen}
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
          <div className="flex flex-wrap gap-2">
            {[baseline, candidate].map((observation, index) => (
              <a
                key={observation.execution_id}
                className={buttonClassName({
                  variant: 'quiet',
                  size: 'compact',
                  className: 'no-underline',
                })}
                href={hashForExecution(observation.execution_id)}
              >
                open {index === 0 ? 'a' : 'b'}
                <ArrowRight aria-hidden="true" size={13} />
              </a>
            ))}
            <ScenarioChatAction
              compact
              executionId={candidate.execution_id}
              scenarioId={testId}
            />
          </div>
        </div>
      ) : null}
    </SectionPanel>
  )
}

/* ------------------------------------------------------------- details */

function ExecutionDetailsDialog({
  observation,
  testVersion,
  testId,
  onClose,
}: {
  observation: TestObservation
  testVersion: number | undefined
  testId: string
  onClose: () => void
}) {
  const [detail, setDetail] = useState<DashboardExecutionDetail | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    setDetail(null)
    setError(null)
    setLoading(true)
    void getDashboardDataBridge()
      .then((bridge) => bridge.getExecution(observation.execution_id))
      .then((next) => {
        if (!cancelled) setDetail(next)
      })
      .catch((cause) => {
        if (!cancelled)
          setError(cause instanceof Error ? cause.message : String(cause))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [observation.execution_id])

  const scopedDetail = useMemo(
    () =>
      detail
        ? {
            ...detail,
            reports: detail.reports.filter(
              (report) => report.scenario_id === testId,
            ),
          }
        : null,
    [detail, testId],
  )
  const version = observation.scenario_version ?? testVersion
  const availableReports = scopedDetail?.reports.filter(
    (report) => report.available,
  ).length
  const result = statusPresentation(observation.status)
  const metrics = [
    ['score', formatScore(observation.median_score)],
    ['duration', formatDuration(observation.median_duration_seconds)],
    ['tokens', formatTokens(observation.median_tokens)],
    ['cost', formatCost(observation.median_cost_usd)],
    ['turns', formatCount(observation.median_turns)],
  ].filter(([, value]) => value !== '—')

  // Audit TH-03: an opaque panel from the design system, no accent bar.
  return (
    <Dialog
      open
      onClose={onClose}
      size="lg"
      tall
      kicker="execution"
      title={testId}
      description={`${formatDate(observation.completed_at)} · ${version ? `test v${version}` : 'test version unknown'} · ${runLabel(observation.run_count)}`}
      closeLabel="Close execution details"
      className="ds-root"
      footer={
        <>
          <a
            className={buttonClassName({
              variant: 'secondary',
              className: 'no-underline',
            })}
            href={hashForExecution(observation.execution_id)}
          >
            open full execution report
          </a>
          <ScenarioChatAction
            detail={scopedDetail}
            executionId={observation.execution_id}
            scenarioId={testId}
          />
          <button
            className={buttonClassName({ variant: 'primary' })}
            type="button"
            onClick={onClose}
          >
            close
          </button>
        </>
      }
    >
      <div className="grid gap-6">
        <dl className="m-0 grid gap-3 text-xs @[560px]:grid-cols-2 @[840px]:grid-cols-4">
          <div className="grid gap-1">
            <dt className="ds-label">result</dt>
            <dd className="m-0">
              <StatusBadge status={result.status} label={result.label} />
            </dd>
          </div>
          <div className="grid gap-1">
            <dt className="ds-label">execution model</dt>
            <dd className="m-0 font-mono text-ink">
              {modelLabel(
                observation.subject_provider,
                observation.subject_model,
              )}
            </dd>
          </div>
          <div className="grid gap-1">
            <dt className="ds-label">judge</dt>
            <dd className="m-0 font-mono text-ink">
              {modelLabel(observation.judge_provider, observation.judge_model)}
              {observation.judge_protocol ? (
                <span className="block text-label text-ink-muted">
                  judge protocol: {observation.judge_protocol}
                </span>
              ) : null}
            </dd>
          </div>
          <div className="grid gap-1">
            <dt className="ds-label">system</dt>
            <dd className="m-0 font-mono text-ink">
              {systemSummary(observation)}
            </dd>
          </div>
        </dl>
        {metrics.length > 0 ? (
          <dl
            className="m-0 flex flex-wrap gap-x-6 gap-y-2 font-mono text-xs"
            aria-label="Execution metrics"
          >
            {metrics.map(([label, value]) => (
              <div className="flex items-baseline gap-2" key={label}>
                <dt className="ds-label">{label}</dt>
                <dd className="m-0 text-ink">{value}</dd>
              </div>
            ))}
          </dl>
        ) : null}
        <section
          className="grid gap-3"
          aria-labelledby="execution-report-title"
        >
          <div className="flex flex-wrap items-baseline justify-between gap-2">
            <h3
              className="m-0 text-sm font-semibold text-ink"
              id="execution-report-title"
            >
              Assessment details for this test
            </h3>
            {detail ? (
              <span className="font-mono text-label text-ink-muted">
                {availableReports ?? 0} available{' '}
                {(availableReports ?? 0) === 1 ? 'report' : 'reports'}
              </span>
            ) : null}
          </div>
          {loading ? (
            <p className="m-0 text-xs text-ink-soft" role="status">
              Loading execution report…
            </p>
          ) : error ? (
            <Callout tone="danger">{error}</Callout>
          ) : (
            <AssessmentWorkspace detail={scopedDetail} />
          )}
        </section>
      </div>
    </Dialog>
  )
}

/* ----------------------------------------------------------------- page */

type HistoryFilters = {
  version: number | undefined
  model: string
  judge: string
  system: string
  result: string
}

const EMPTY_FILTERS: HistoryFilters = {
  version: undefined,
  model: '',
  judge: '',
  system: '',
  result: '',
}

/** Audit TH-19: filters, the a/b selection and the open dialog live in the hash. */
export function historyStateFromParams(params: URLSearchParams) {
  const version = params.get('version')
  return {
    filters: {
      version: version && /^\d+$/.test(version) ? Number(version) : undefined,
      model: params.get('model') ?? '',
      judge: params.get('judge') ?? '',
      system: params.get('system') ?? '',
      result: params.get('result') ?? '',
    } satisfies HistoryFilters,
    comparisonKeys: ['a', 'b']
      .map((slot) => params.get(slot))
      .filter((value): value is string => Boolean(value)),
    open: params.get('open'),
  }
}

export function historyStateToParams(
  filters: HistoryFilters,
  comparisonKeys: string[],
  open: string | null,
) {
  const params = new URLSearchParams()
  if (filters.version !== undefined)
    params.set('version', String(filters.version))
  if (filters.model) params.set('model', filters.model)
  if (filters.judge) params.set('judge', filters.judge)
  if (filters.system) params.set('system', filters.system)
  if (filters.result) params.set('result', filters.result)
  if (comparisonKeys[0]) params.set('a', comparisonKeys[0])
  if (comparisonKeys[1]) params.set('b', comparisonKeys[1])
  if (open) params.set('open', open)
  return params
}

function filtersActive(filters: HistoryFilters) {
  return (
    filters.version !== undefined ||
    Boolean(filters.model || filters.judge || filters.system || filters.result)
  )
}

/** Audit TH-07: says which version is shown and whether the contract moved on. */
export function versionStatement(history: TestHistoryResponse) {
  const current = history.current_version ?? null
  if (current === null || current === history.test_version)
    return `contract v${history.test_version}${current === null ? '' : ' · current'}`
  const currentVersion = history.available_versions.find(
    (item) => item.version === current,
  )
  const currentRuns = currentVersion?.execution_count ?? 0
  return `showing v${history.test_version} (latest with executions) · current contract v${current} has ${currentRuns === 0 ? 'no executions yet' : `${currentRuns} ${currentRuns === 1 ? 'execution' : 'executions'}`}`
}

export function TestHistoryPage({ testId }: { testId: string }) {
  const initial = useMemo(
    () =>
      historyStateFromParams(
        typeof window === 'undefined'
          ? new URLSearchParams()
          : routeParams(window.location.hash),
      ),
    [],
  )
  const [history, setHistory] = useState<TestHistoryResponse | null>(null)
  const [catalogRow, setCatalogRow] = useState<TestCatalogRow | null>(null)
  const [neighbours, setNeighbours] = useState<{
    previous: string | null
    next: string | null
  }>({ previous: null, next: null })
  const [filters, setFilters] = useState<HistoryFilters>(initial.filters)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [local, setLocal] = useState(false)
  const [openKey, setOpenKey] = useState<string | null>(initial.open)
  const [comparisonKeys, setComparisonKeys] = useState<string[]>(
    initial.comparisonKeys,
  )
  const [selectionNotice, setSelectionNotice] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    setError(null)
    void getDashboardDataBridge()
      .then((next) => {
        if (cancelled) return
        setLocal(next.mode === 'local')
        const execution = parseModelSelection(filters.model)
        const judge = parseModelSelection(filters.judge)
        return next.getTestHistory({
          test_id: testId,
          test_version: filters.version,
          subject_provider: execution?.provider,
          subject_model: execution?.model,
          judge_provider: judge?.provider,
          judge_model: judge?.model,
          system_version_id: filters.system || undefined,
          limit: 100,
        })
      })
      .then((data) => {
        if (!cancelled && data) setHistory(data)
      })
      .catch((cause) => {
        if (!cancelled)
          setError(cause instanceof Error ? cause.message : String(cause))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [filters.model, filters.judge, filters.system, filters.version, testId])

  // Identity (complexity, realism, lifecycle) and the previous/next test come
  // from the catalog (audit T-14 / TH-06).
  useEffect(() => {
    let cancelled = false
    void getDashboardDataBridge()
      .then((bridge) => bridge.listTests({ limit: 100 }))
      .then((list) => {
        if (cancelled) return
        const ids = list.rows.map((row) => row.test_id)
        const index = ids.indexOf(testId)
        setCatalogRow(list.rows[index] ?? null)
        setNeighbours({
          previous: index > 0 ? ids[index - 1] : null,
          next: index >= 0 && index < ids.length - 1 ? ids[index + 1] : null,
        })
      })
      .catch(() => undefined)
    return () => {
      cancelled = true
    }
  }, [testId])

  useEffect(() => {
    replaceRouteParams(historyStateToParams(filters, comparisonKeys, openKey))
  }, [filters, comparisonKeys, openKey])

  const executionModelGroups = useMemo(
    () => modelGroups(history, 'subject'),
    [history],
  )
  const judgeModelGroups = useMemo(
    () => modelGroups(history, 'judge'),
    [history],
  )

  const allObservations = history?.observations ?? []
  // Audit TH-21: the result filter is applied here so the chips keep their
  // counts and a selection survives when both sides stay visible.
  const observations = filters.result
    ? allObservations.filter((item) =>
        filters.result === 'passed'
          ? item.status === 'passed'
          : item.status !== 'passed',
      )
    : allObservations
  const passedCount = allObservations.filter(
    (item) => item.status === 'passed',
  ).length
  const failedCount = allObservations.length - passedCount

  useEffect(() => {
    if (loading || comparisonKeys.length === 0) return
    const visible = new Set(observations.map(testObservationKey))
    const kept = comparisonKeys.filter((key) => visible.has(key))
    if (kept.length !== comparisonKeys.length) {
      setComparisonKeys(kept)
      setSelectionNotice(
        kept.length === 0
          ? 'Selection cleared: the chosen executions left the filter.'
          : 'b was cleared because it left the filter.',
      )
    }
  }, [loading, observations, comparisonKeys])

  const scores = allObservations.map((item) => item.median_score)
  const costs = allObservations.map((item) => item.median_cost_usd)
  const durations = allObservations.map((item) => item.median_duration_seconds)
  const tokens = allObservations.map((item) => item.median_tokens)
  const knownCosts = knownMetricCount(costs)
  const comparisonSelections = comparisonKeys
    .map((key) => observations.find((item) => testObservationKey(item) === key))
    .filter((item): item is TestObservation => Boolean(item))
  const baseline = comparisonSelections[0] ?? null
  const candidate = comparisonSelections[1] ?? null
  const openObservation =
    openKey === null
      ? null
      : (allObservations.find((item) => testObservationKey(item) === openKey) ??
        null)
  const contract = contractSummary(allObservations)
  const lastRun = allObservations[0] ?? null
  const complexity = catalogRow
    ? catalogComplexityPresentation(catalogRow)
    : null
  const realism = catalogRow ? catalogRealismPresentation(catalogRow) : null
  const hasEvidence = allObservations.length > 0
  const filtered = filtersActive(filters)
  const isLocalTest = testId.startsWith('local_')

  const setFilter = <K extends keyof HistoryFilters>(
    key: K,
    value: HistoryFilters[K],
  ) => {
    setSelectionNotice(null)
    setFilters((current) => ({ ...current, [key]: value }))
  }

  function selectForComparison(observation: TestObservation) {
    const key = testObservationKey(observation)
    setSelectionNotice(null)
    setComparisonKeys((current) => {
      const selectedIndex = current.indexOf(key)
      if (selectedIndex === 0) return current.slice(1)
      if (selectedIndex === 1) return current.slice(0, 1)
      if (current.length === 0) return [key]
      if (current.length === 1) return [...current, key]
      return [current[0], key]
    })
  }

  const identity = [
    history ? versionStatement(history) : null,
    contract?.short ?? null,
    complexity?.value ? `complexity ${complexity.value}` : null,
    realism?.value ? `realism ${realism.value}` : null,
    history
      ? `${history.total} ${history.total === 1 ? 'execution' : 'executions'} retained`
      : null,
    lastRun
      ? `last run ${formatDate(lastRun.completed_at)} · ${statusPresentation(lastRun.status).label}`
      : null,
  ]
    .filter(Boolean)
    .join(' · ')

  const runThisTest = local ? (
    <a
      className={dashboardHeaderActionClassName({ primary: true })}
      href={hashForWorkspace()}
      onClick={() => requestQuickExecution([testId])}
    >
      run this test
    </a>
  ) : null

  return (
    <>
      <DashboardPageActions
        active="tests"
        context={testId}
        actionsLabel="Test actions"
        actions={
          <>
            <a
              className={dashboardHeaderActionClassName()}
              href={hashForComparison()}
            >
              compare systems
            </a>
            {runThisTest}
          </>
        }
      />
      <div className="ds-root page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-24 md:w-[calc(100%_-_3rem)]">
        <PageHeader
          title={testId}
          summary={
            loading && !history
              ? 'loading the history…'
              : identity || 'no identity recorded'
          }
          headingId="test-history-title"
          breadcrumb={[
            {
              label: 'tests',
              href: hashForTests(new URLSearchParams({ highlight: testId })),
            },
            { label: testId },
          ]}
          actions={
            <>
              {catalogRow ? (
                <StatusBadge
                  status={
                    catalogRow.lifecycle === 'active' ? 'passed' : 'unavailable'
                  }
                  label={
                    catalogRow.lifecycle === 'never_run'
                      ? 'never run'
                      : catalogRow.lifecycle
                  }
                />
              ) : null}
              {isLocalTest ? (
                <span className="inline-flex items-center rounded-[6px] bg-[var(--surface-fill)] px-1.5 py-0.5 font-mono text-label text-ink-soft">
                  local
                </span>
              ) : null}
              {contract?.full ? (
                <button
                  className={buttonClassName({
                    variant: 'quiet',
                    size: 'compact',
                  })}
                  type="button"
                  title={contract.full}
                  aria-label="Copy the full contract hash"
                  onClick={() => {
                    void navigator.clipboard
                      ?.writeText(contract.full ?? '')
                      .then(() => {
                        setCopied(true)
                        window.setTimeout(() => setCopied(false), 1500)
                      })
                  }}
                >
                  <Copy size={13} aria-hidden="true" />
                  {copied ? 'copied' : 'copy sha'}
                </button>
              ) : null}
              <span className="inline-flex gap-1">
                <a
                  className={buttonClassName({
                    variant: 'quiet',
                    size: 'compact',
                    className: `no-underline ${neighbours.previous ? '' : 'pointer-events-none opacity-50'}`,
                  })}
                  href={
                    neighbours.previous
                      ? hashForTestHistory(neighbours.previous)
                      : undefined
                  }
                  aria-disabled={!neighbours.previous}
                  title={neighbours.previous ?? 'first test'}
                >
                  <ChevronLeft size={13} aria-hidden="true" />
                  prev
                </a>
                <a
                  className={buttonClassName({
                    variant: 'quiet',
                    size: 'compact',
                    className: `no-underline ${neighbours.next ? '' : 'pointer-events-none opacity-50'}`,
                  })}
                  href={
                    neighbours.next
                      ? hashForTestHistory(neighbours.next)
                      : undefined
                  }
                  aria-disabled={!neighbours.next}
                  title={neighbours.next ?? 'last test'}
                >
                  next
                  <ChevronRight size={13} aria-hidden="true" />
                </a>
              </span>
            </>
          }
        />

        {error ? (
          <EmptyState
            className="mt-6"
            tone="error"
            title="History unavailable"
            description={error}
          />
        ) : null}

        {!error && !loading && !hasEvidence && !filtered ? (
          // Audit TH-01: an empty history is one message and the next action.
          <EmptyState
            className="mt-6"
            title="no retained executions yet"
            description="This test has never run on this dashboard. Run it once to start the metric history, or add it to a plan to capture a baseline you can compare against later."
            actions={
              local ? (
                <>
                  <a
                    className={buttonClassName({
                      variant: 'primary',
                      className: 'no-underline',
                    })}
                    href={hashForWorkspace()}
                    onClick={() => requestQuickExecution([testId])}
                  >
                    run this test
                  </a>
                  <a
                    className={buttonClassName({
                      variant: 'secondary',
                      className: 'no-underline',
                    })}
                    href={hashForNewPlan()}
                    onClick={() => requestPlanFromSelection([testId])}
                  >
                    add to a new plan
                  </a>
                </>
              ) : (
                <a
                  className={buttonClassName({
                    variant: 'secondary',
                    className: 'no-underline',
                  })}
                  href={hashForTests()}
                >
                  back to the catalog
                </a>
              )
            }
          />
        ) : null}

        {!error && (loading || hasEvidence || filtered) ? (
          <div className="mt-6 grid gap-6">
            {hasEvidence ? (
              // Audit TH-08: tiles only for metrics with data.
              <div
                className="grid gap-3 @[560px]:grid-cols-2 @[960px]:grid-cols-4"
                data-history-tiles
              >
                <MetricCard
                  label="successful runs"
                  value={`${passedCount} / ${allObservations.length}`}
                  detail="objective result"
                  tone={
                    passedCount === allObservations.length
                      ? 'positive'
                      : passedCount === 0
                        ? 'negative'
                        : 'warning'
                  }
                />
                {knownMetricCount(scores) > 0 ? (
                  <MetricCard
                    label="median score"
                    value={formatScore(median(scores))}
                    detail={`judge quality · /100 · ${metricCaption(knownMetricCount(scores), allObservations.length)}`}
                  />
                ) : null}
                {knownMetricCount(durations) > 0 ? (
                  <MetricCard
                    label="median duration"
                    value={formatDuration(median(durations))}
                    detail={metricCaption(
                      knownMetricCount(durations),
                      allObservations.length,
                    )}
                  />
                ) : null}
                {knownMetricCount(tokens) > 0 ? (
                  <MetricCard
                    label="median tokens"
                    value={formatTokens(median(tokens))}
                    detail={`subject + judge · ${metricCaption(knownMetricCount(tokens), allObservations.length)}`}
                  />
                ) : null}
                {knownCosts > 0 ? (
                  <MetricCard
                    label="median cost"
                    value={formatCost(median(costs))}
                    detail={metricCaption(knownCosts, allObservations.length)}
                  />
                ) : null}
              </div>
            ) : null}

            {allObservations.filter(
              (item) => finiteMetric(item.median_score) !== null,
            ).length >= 2 ? (
              // Audit TH-10: the trend is always visible with two scored runs.
              <SectionPanel
                title="score trend"
                summary={`newest right · ${allObservations.length} executions${knownCosts === 0 ? ' · cost not recorded in local runs' : ''}`}
                headingId="score-trend-title"
              >
                <ScoreTrendChart
                  observations={allObservations}
                  selectedKeys={comparisonKeys}
                  onSelect={selectForComparison}
                />
              </SectionPanel>
            ) : null}

            <section className="grid gap-3" aria-label="History filters">
              <div className="flex flex-wrap items-center gap-2">
                <label className="ds-visually-hidden" htmlFor="history-version">
                  Test version
                </label>
                <Select
                  id="history-version"
                  className="max-w-[16rem]"
                  value={filters.version ?? ''}
                  onChange={(event) =>
                    setFilter(
                      'version',
                      event.target.value
                        ? Number(event.target.value)
                        : undefined,
                    )
                  }
                >
                  <option value="">version: latest with executions</option>
                  {(history?.available_versions ?? []).map((item) => (
                    <option key={item.version} value={item.version}>
                      v{item.version}
                      {item.version === history?.current_version
                        ? ' · current'
                        : ''}
                      {` · ${item.execution_count} ${item.execution_count === 1 ? 'execution' : 'executions'}`}
                    </option>
                  ))}
                </Select>
                <div className="w-full max-w-[16rem]">
                  <ProviderModelDropdown
                    groups={executionModelGroups}
                    value={filters.model}
                    onChange={(next) => setFilter('model', next)}
                    optionValue={modelSelection}
                    placeholder="all execution models"
                    ariaLabel="Execution model"
                    clearLabel="all execution models"
                  />
                </div>
                <div className="w-full max-w-[16rem]">
                  <ProviderModelDropdown
                    groups={judgeModelGroups}
                    value={filters.judge}
                    onChange={(next) => setFilter('judge', next)}
                    optionValue={modelSelection}
                    placeholder="all judges"
                    ariaLabel="Judge model"
                    clearLabel="all judges"
                  />
                </div>
                {(history?.systems.length ?? 0) > 0 ? (
                  <Select
                    aria-label="System revision"
                    className="max-w-[16rem]"
                    value={filters.system}
                    onChange={(event) =>
                      setFilter('system', event.target.value)
                    }
                  >
                    <option value="">all system revisions</option>
                    {(history?.systems ?? []).map((item) => (
                      <option key={item.id} value={item.id}>
                        {item.label}
                      </option>
                    ))}
                  </Select>
                ) : null}
              </div>
              <div className="flex flex-wrap items-center gap-2">
                <FilterChipGroup label="Result">
                  <FilterChip
                    active={filters.result === ''}
                    count={allObservations.length}
                    onClick={() => setFilter('result', '')}
                  >
                    all
                  </FilterChip>
                  <FilterChip
                    active={filters.result === 'failed'}
                    count={failedCount}
                    onClick={() => setFilter('result', 'failed')}
                  >
                    failed
                  </FilterChip>
                  <FilterChip
                    active={filters.result === 'passed'}
                    count={passedCount}
                    onClick={() => setFilter('result', 'passed')}
                  >
                    passed
                  </FilterChip>
                </FilterChipGroup>
                <output
                  className="ms-auto font-mono text-label text-ink-muted"
                  aria-live="polite"
                >
                  {loading
                    ? 'loading executions…'
                    : `${observations.length} of ${allObservations.length} executions`}
                </output>
              </div>
            </section>

            {selectionNotice ? (
              <Callout tone="info">{selectionNotice}</Callout>
            ) : null}

            {loading ? (
              <div className="grid gap-px" aria-busy="true" role="status">
                <span className="ds-visually-hidden">
                  Loading metric history
                </span>
                {Array.from({ length: 4 }, (_, index) => (
                  <div
                    // biome-ignore lint/suspicious/noArrayIndexKey: static placeholders
                    key={index}
                    className="h-12 animate-pulse rounded-[6px] bg-[var(--surface-fill)] motion-reduce:animate-none"
                  />
                ))}
              </div>
            ) : observations.length === 0 ? (
              <EmptyState
                title="No executions match these filters"
                description="Widen the version, model, judge or result filter."
                actions={
                  <button
                    className={buttonClassName({ variant: 'secondary' })}
                    type="button"
                    onClick={() => {
                      setSelectionNotice(null)
                      setFilters(EMPTY_FILTERS)
                    }}
                  >
                    clear filters
                  </button>
                }
              />
            ) : (
              <DataTable
                caption={`Metric history for ${testId}, ${observations.length} executions`}
                collapse
                minWidth="56rem"
                sticky
                data-history-table
              >
                <thead>
                  <tr>
                    <th scope="col">
                      <span className="ds-visually-hidden">Compare</span>
                      a/b
                    </th>
                    <th scope="col">execution</th>
                    <th scope="col">model · system</th>
                    <th scope="col">result</th>
                    <th scope="col" className={numericCellClassName}>
                      score
                    </th>
                    <th scope="col" className={numericCellClassName}>
                      duration
                    </th>
                    <th scope="col" className={numericCellClassName}>
                      tokens
                    </th>
                    {knownCosts > 0 ? (
                      <th scope="col" className={numericCellClassName}>
                        cost
                      </th>
                    ) : null}
                    <th scope="col">
                      <span className="ds-visually-hidden">Actions</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {observations.map((item) => {
                    const key = testObservationKey(item)
                    const selectedIndex = comparisonKeys.indexOf(key)
                    const result = statusPresentation(item.status)
                    return (
                      // Audit TH-09: one row of compact controls; the row
                      // itself opens the details.
                      <DataTableRow
                        key={key}
                        className={`cursor-pointer ${selectedIndex >= 0 ? 'is-selected' : ''}`}
                        data-execution-id={item.execution_id}
                        onClick={(event) => {
                          if (isInteractiveTarget(event.target)) return
                          setOpenKey(key)
                        }}
                      >
                        <td data-label="a/b">
                          <label className="inline-flex min-h-7 cursor-pointer items-center gap-2 font-mono text-label text-ink-soft">
                            <input
                              className="size-4 accent-[var(--accent)]"
                              type="checkbox"
                              checked={selectedIndex >= 0}
                              onChange={() => selectForComparison(item)}
                              aria-label={`Select ${formatDate(item.completed_at)} for comparison`}
                            />
                            {selectedIndex === 0
                              ? 'a'
                              : selectedIndex === 1
                                ? 'b'
                                : ''}
                          </label>
                        </td>
                        <td data-label="Execution">
                          <span className="block font-mono text-xs text-ink">
                            {formatDate(item.completed_at)}
                          </span>
                          <span className="block font-mono text-label text-ink-muted">
                            {runLabel(item.run_count)} ·{' '}
                            {item.execution_id.slice(0, 8)}…
                            {item.scenario_version
                              ? ` · v${item.scenario_version}`
                              : ''}
                          </span>
                        </td>
                        <td data-label="Model · system">
                          <span className="block font-mono text-xs text-ink">
                            {modelLabel(
                              item.subject_provider,
                              item.subject_model,
                            )}
                          </span>
                          <span className="block font-mono text-label text-ink-muted">
                            judge{' '}
                            {modelLabel(item.judge_provider, item.judge_model)}{' '}
                            · {systemSummary(item)}
                          </span>
                        </td>
                        <td data-label="Result">
                          <StatusBadge
                            status={result.status}
                            label={result.label}
                          />
                        </td>
                        <td data-label="Score" className={numericCellClassName}>
                          {formatScore(item.median_score)}
                        </td>
                        <td
                          data-label="Duration"
                          className={numericCellClassName}
                        >
                          {formatDuration(item.median_duration_seconds)}
                        </td>
                        <td
                          data-label="Tokens"
                          className={numericCellClassName}
                        >
                          {formatTokens(item.median_tokens)}
                        </td>
                        {knownCosts > 0 ? (
                          <td
                            data-label="Cost"
                            className={numericCellClassName}
                          >
                            {formatCost(item.median_cost_usd)}
                          </td>
                        ) : null}
                        <td data-label="Actions" className="text-right">
                          <span className="inline-flex items-center gap-1">
                            <a
                              className={buttonClassName({
                                variant: 'quiet',
                                size: 'compact',
                                className: 'no-underline',
                              })}
                              href={hashForExecution(item.execution_id)}
                            >
                              open
                              <ArrowRight size={13} aria-hidden="true" />
                            </a>
                            <ScenarioChatAction
                              compact
                              executionId={item.execution_id}
                              scenarioId={testId}
                            />
                          </span>
                        </td>
                      </DataTableRow>
                    )
                  })}
                </tbody>
              </DataTable>
            )}

            {baseline && candidate ? (
              <ObservationComparisonPanel
                baseline={baseline}
                candidate={candidate}
                testId={testId}
                onClear={() => setComparisonKeys([])}
                onSwap={() =>
                  setComparisonKeys((current) =>
                    current.length === 2 ? [current[1], current[0]] : current,
                  )
                }
              />
            ) : null}
          </div>
        ) : null}
      </div>

      {comparisonKeys.length > 0 && !loading ? (
        // Audit TH-09: the selection bar stays in view while rows are ticked.
        <div
          className="sticky bottom-0 z-10 bg-panel px-3 py-3 shadow-[0_-1px_0_0_var(--line)] md:px-6"
          data-selection-bar
        >
          <div className="mx-auto flex max-w-[1420px] flex-wrap items-center gap-3 font-mono text-xs">
            <span className="text-ink">
              {comparisonKeys.length} selected
              {baseline ? ` · a = ${formatDate(baseline.completed_at)}` : ''}
              {candidate ? ` · b = ${formatDate(candidate.completed_at)}` : ''}
            </span>
            <span className="text-ink-muted">
              {!candidate
                ? 'tick another execution as b'
                : compareTestObservations(
                      baseline as TestObservation,
                      candidate,
                    ).compatible
                  ? 'same model, judge and cohort · deltas interpreted'
                  : 'different model, judge or cohort · deltas shown, not interpreted'}
            </span>
            <span className="ms-auto flex gap-2">
              <button
                className={buttonClassName({
                  variant: 'secondary',
                  size: 'compact',
                })}
                type="button"
                onClick={() =>
                  setComparisonKeys((current) =>
                    current.length === 2 ? [current[1], current[0]] : current,
                  )
                }
                disabled={!candidate}
              >
                swap
              </button>
              <button
                className={buttonClassName({
                  variant: 'quiet',
                  size: 'compact',
                })}
                type="button"
                onClick={() => setComparisonKeys([])}
              >
                clear
              </button>
              <button
                className={buttonClassName({
                  variant: 'primary',
                  size: 'compact',
                })}
                type="button"
                disabled={!candidate}
                onClick={() =>
                  document
                    .getElementById('test-comparison-title')
                    ?.scrollIntoView({ block: 'start' })
                }
              >
                compare a → b
              </button>
            </span>
          </div>
        </div>
      ) : null}

      {openObservation ? (
        <ExecutionDetailsDialog
          observation={openObservation}
          testVersion={history?.test_version}
          testId={testId}
          onClose={() => setOpenKey(null)}
        />
      ) : null}
    </>
  )
}
