import {
  ArrowLeftRight,
  ArrowRight,
  Braces,
  ChevronDown,
  CircleAlert,
  CircleCheck,
  CircleDollarSign,
  Clock3,
  Layers3,
  MessagesSquare,
  Target,
  TriangleAlert,
} from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { AssessmentWorkspace } from '@/components/AssessmentWorkspace'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { ProviderModelDropdown } from '@/components/ProviderModelDropdown'
import { ScenarioChatAction } from '@/components/ScenarioChatAction'
import { hashForExecution, hashForWorkspace } from '@/hooks/use-hash-route'
import {
  type DashboardExecutionDetail,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import type {
  HistoryModelGroup,
  TestHistoryResponse,
  TestObservation,
} from '@/lib/test-catalog'
import {
  type ComparedMetric,
  compareTestObservations,
  testObservationKey,
} from '@/lib/test-history-comparison'

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

  if (role === 'subject' && groups.size === 0) {
    for (const subject of history.subjects ?? []) {
      const separator = subject.indexOf('/')
      if (separator < 1) continue
      const provider = subject.slice(0, separator)
      const model = subject.slice(separator + 1)
      const models = groups.get(provider) ?? new Set<string>()
      models.add(model)
      groups.set(provider, models)
    }
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

function formatCost(value: number | null | undefined) {
  if (value === null || value === undefined || !Number.isFinite(value))
    return 'Unknown'
  return `$${value.toFixed(2)}`
}

function formatDuration(value: number | null | undefined) {
  if (value === null || value === undefined || !Number.isFinite(value))
    return 'Unknown'
  if (value < 60) return `${value.toFixed(1).replace(/\.0$/, '')}s`
  const rounded = Math.max(0, Math.round(value))
  return (
    String(Math.floor(rounded / 60)) +
    'm ' +
    String(rounded % 60).padStart(2, '0') +
    's'
  )
}

function formatTokens(value: number | null | undefined) {
  if (value === null || value === undefined || !Number.isFinite(value))
    return 'Unknown'
  if (Math.abs(value) >= 1000) {
    return `${(value / 1000).toFixed(1).replace(/\.0$/, '')}k`
  }
  return Math.round(value).toLocaleString()
}

function formatDate(value: string) {
  if (!value) return 'Not completed'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return 'Unknown date'
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  }).format(date)
}

function formatStatus(status: string) {
  const labels: Record<string, string> = {
    passed: 'Passed',
    hard_gate_failed: 'Hard gate failed',
    technical_failed: 'Technical failure',
    infra_failed: 'Infrastructure failure',
  }
  return (
    labels[status] ??
    status
      .replace(/[_-]+/g, ' ')
      .replace(/\b\w/g, (letter) => letter.toUpperCase())
  )
}

function statusClass(status: string) {
  if (status === 'passed') return 'tmh-status-pass'
  if (status === 'hard_gate_failed' || status === 'infra_failed')
    return 'tmh-status-fail'
  return 'tmh-status-tech'
}

function shortHash(value: string | null | undefined) {
  if (!value) return 'Unknown'
  const hash = value.replace(/^sha256:/, '')
  if (hash.length <= 16) return `sha256:${hash}`
  return `sha256:${hash.slice(0, 8)}…${hash.slice(-6)}`
}

function contractSummary(observations: TestObservation[]) {
  const contracts = [
    ...new Set(
      observations
        .map((observation) => observation.contract_sha256)
        .filter(Boolean),
    ),
  ]
  if (contracts.length === 0) return 'Unknown'
  if (contracts.length > 1) return 'Multiple contracts'
  return shortHash(contracts[0])
}

function modelLabel(provider?: string | null, model?: string | null) {
  if (!provider && !model) return 'Unknown model'
  return [provider, model].filter(Boolean).join('/') || 'Unknown model'
}

function systemSummary(observation: TestObservation) {
  if (observation.stack_mode === 'source') return 'Local source'
  if (observation.stack_mode === 'registry') return 'Registry stack'
  return 'Local environment'
}

function visibleLabel(count: number) {
  return `${String(count)} visible ${count === 1 ? 'execution' : 'executions'}`
}

function knownMetricCount(values: Array<number | null | undefined>) {
  return values.filter(
    (value) => value !== null && value !== undefined && Number.isFinite(value),
  ).length
}

function metricCaption(known: number, total: number, metric: string) {
  if (known === 0) return `no ${metric} records`
  if (known === total)
    return `median across ${total} visible ${total === 1 ? 'execution' : 'executions'}`
  return `median across ${known} of ${total} executions`
}

function logicalRunLabel(count: number) {
  return `${String(count)} logical ${count === 1 ? 'run' : 'runs'}`
}

type ComparisonMetricKey =
  | 'score'
  | 'duration'
  | 'tokens'
  | 'cost'
  | 'functionCalls'
  | 'functionErrors'
  | 'turns'

function formatMetricValue(metric: ComparisonMetricKey, value: number | null) {
  if (value === null) return 'Not reported'
  switch (metric) {
    case 'score':
      return value.toFixed(2)
    case 'cost':
      return formatCost(value)
    case 'duration':
      return formatDuration(value)
    case 'tokens':
      return formatTokens(value)
    case 'functionCalls':
    case 'functionErrors':
    case 'turns':
      return Math.round(value).toLocaleString()
  }
}

function metricDeltaText(metric: ComparisonMetricKey, value: ComparedMetric) {
  if (value.delta === null) return 'Delta unavailable'
  if (value.delta === 0) return 'No change'
  const sign = value.delta > 0 ? '+' : '−'
  const absolute = Math.abs(value.delta)
  if (metric === 'score') return `${sign}${absolute.toFixed(1)} points`
  if (metric === 'functionCalls') {
    return `${sign}${Math.round(absolute).toLocaleString()} ${absolute === 1 ? 'call' : 'calls'}`
  }
  if (metric === 'functionErrors') {
    return `${sign}${Math.round(absolute).toLocaleString()} ${absolute === 1 ? 'error' : 'errors'}`
  }
  if (metric === 'turns') {
    return `${sign}${Math.round(absolute).toLocaleString()} ${absolute === 1 ? 'turn' : 'turns'}`
  }
  if (value.relativeDelta === null) {
    if (metric === 'cost') return `${sign}${formatCost(absolute)}`
    if (metric === 'duration') return `${sign}${formatDuration(absolute)}`
    return `${sign}${formatTokens(absolute)}`
  }
  const magnitude = Math.abs(value.relativeDelta * 100)
  const percent = magnitude.toFixed(magnitude >= 10 ? 0 : 1)
  if (metric === 'duration') {
    return `${percent}% ${value.delta < 0 ? 'faster' : 'slower'}`
  }
  return `${percent}% ${value.delta < 0 ? 'lower' : 'higher'}`
}

function deltaTone(metric: ComparisonMetricKey, value: ComparedMetric) {
  if (value.delta === null || value.delta === 0) return 'tmh-delta-neutral'
  if (metric === 'functionCalls') return 'tmh-delta-caution'
  const improved = metric === 'score' ? value.delta > 0 : value.delta < 0
  return improved ? 'tmh-delta-improved' : 'tmh-delta-regressed'
}

function finiteMetric(value: number | null | undefined): number | null {
  return value !== null && value !== undefined && Number.isFinite(value)
    ? value
    : null
}

function formatDay(value: string) {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return ''
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
  }).format(date)
}

function ScoreTrendChart({
  observations,
  selectedKeys,
  onSelect,
}: {
  observations: TestObservation[]
  selectedKeys: string[]
  onSelect: (observation: TestObservation) => void
}) {
  // Observations arrive newest-first; the chart reads left → right in time.
  const plotted = [...observations]
    .reverse()
    .filter((item) => finiteMetric(item.median_score) !== null)
  if (plotted.length === 0) {
    return <p className="tmh-chart-empty">No scored executions to plot yet.</p>
  }
  const left = 60
  const right = 515
  const top = 22
  const bottom = 178
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
  return (
    <svg
      className="tmh-chart"
      viewBox="0 0 560 216"
      role="img"
      aria-label="Median score per retained execution"
    >
      <line className="tmh-chart-grid" x1="36" y1={top} x2="548" y2={top} />
      <line className="tmh-chart-grid" x1="36" y1="100" x2="548" y2="100" />
      <line
        className="tmh-chart-grid"
        x1="36"
        y1={bottom}
        x2="548"
        y2={bottom}
      />
      <text className="tmh-chart-tick" x="28" y={top + 4} textAnchor="end">
        100
      </text>
      <text className="tmh-chart-tick" x="28" y="104" textAnchor="end">
        50
      </text>
      <text className="tmh-chart-tick" x="28" y={bottom + 4} textAnchor="end">
        0
      </text>
      <text className="tmh-chart-tick" x="548" y="96" textAnchor="end">
        gate 50
      </text>
      {plotted.length > 1 && (
        <polyline className="tmh-chart-line" points={line} />
      )}
      {plotted.map((item, index) => {
        const key = testObservationKey(item)
        const selectedIndex = selectedKeys.indexOf(key)
        const failed = item.status !== 'passed'
        const score = item.median_score as number
        const x = xs[index]
        const y = yFor(score)
        const dotClass =
          selectedIndex >= 0
            ? 'tmh-chart-dot-selected'
            : failed
              ? 'tmh-chart-dot-failed'
              : 'tmh-chart-dot-context'
        const slot =
          selectedIndex === 0 ? 'A' : selectedIndex === 1 ? 'B' : null
        const labelBelow = y <= 168
        return (
          // biome-ignore lint/a11y/useSemanticElements: an SVG hit target cannot be a native <button>; the group carries the full button contract (role, tabIndex, keyboard activation).
          <g
            className="tmh-chart-hit"
            key={key}
            role="button"
            tabIndex={0}
            aria-pressed={selectedIndex >= 0}
            aria-label={`${formatDate(item.completed_at)} · score ${score.toFixed(0)} · ${formatStatus(item.status)}${slot ? ` · selected as ${slot}` : ''}`}
            onClick={() => onSelect(item)}
            onKeyDown={(event) => {
              if (event.key === 'Enter' || event.key === ' ') {
                event.preventDefault()
                onSelect(item)
              }
            }}
          >
            <title>
              {`${formatDate(item.completed_at)} · score ${score.toFixed(0)} · ${formatStatus(item.status)} · ${formatDuration(item.median_duration_seconds)} · ${formatTokens(item.median_tokens)} tokens`}
            </title>
            <circle className="tmh-chart-target" cx={x} cy={y} r="12" />
            <circle
              className={`tmh-chart-dot ${dotClass}`}
              cx={x}
              cy={y}
              r={selectedIndex >= 0 ? 4.5 : 4}
            />
            {slot ? (
              <g
                className={
                  slot === 'A' ? 'tmh-chart-chip-a' : 'tmh-chart-chip-b'
                }
              >
                <rect
                  x={x - 9}
                  y={labelBelow ? y + 10 : y - 27}
                  width="18"
                  height="15"
                  rx="4"
                />
                <text
                  x={x}
                  y={labelBelow ? y + 21.5 : y - 15.5}
                  textAnchor="middle"
                >
                  {slot}
                </text>
              </g>
            ) : failed ? (
              <text
                className="tmh-chart-value"
                x={x}
                y={labelBelow ? y + 18 : y - 10}
                textAnchor="middle"
              >
                {score.toFixed(0)}
              </text>
            ) : null}
          </g>
        )
      })}
      <text className="tmh-chart-tick" x={xs[0]} y="206" textAnchor="middle">
        {formatDay(plotted[0].completed_at)}
      </text>
      {plotted.length > 1 && (
        <text
          className="tmh-chart-tick"
          x={xs[xs.length - 1]}
          y="206"
          textAnchor="middle"
        >
          {formatDay(plotted[plotted.length - 1].completed_at)}
        </text>
      )}
    </svg>
  )
}

const comparisonMetrics = [
  { key: 'score', label: 'Score', icon: Target },
  { key: 'duration', label: 'Duration', icon: Clock3 },
  { key: 'tokens', label: 'Tokens', icon: Layers3 },
  { key: 'cost', label: 'Cost', icon: CircleDollarSign },
  { key: 'functionCalls', label: 'Functions', icon: Braces },
  { key: 'functionErrors', label: 'Errors', icon: CircleAlert },
  { key: 'turns', label: 'Turns', icon: MessagesSquare },
] satisfies Array<{
  key: ComparisonMetricKey
  label: string
  icon: typeof Target
}>

type Verdict = {
  tone: 'improved' | 'regressed' | 'mixed' | 'neutral'
  title: string
  detail: string
}

function comparisonVerdict(
  comparison: ReturnType<typeof compareTestObservations>,
): Verdict {
  if (!comparison.compatible) {
    return {
      tone: 'neutral',
      title: 'Not comparable',
      detail: 'the values are shown, but deltas are not interpreted',
    }
  }

  const objectiveRegressed =
    comparison.baseline.status === 'passed' &&
    comparison.candidate.status !== 'passed'
  const objectiveImproved =
    comparison.baseline.status !== 'passed' &&
    comparison.candidate.status === 'passed'
  const interpreted = [
    ['score', comparison.metrics.score],
    ['duration', comparison.metrics.duration],
    ['tokens', comparison.metrics.tokens],
    ['cost', comparison.metrics.cost],
    ['functionErrors', comparison.metrics.functionErrors],
    ['turns', comparison.metrics.turns],
  ] as const
  const tones = interpreted.map(([metric, value]) => deltaTone(metric, value))
  const improved = objectiveImproved || tones.includes('tmh-delta-improved')
  const regressed = objectiveRegressed || tones.includes('tmh-delta-regressed')
  const tone =
    improved && regressed
      ? 'mixed'
      : regressed
        ? 'regressed'
        : improved
          ? 'improved'
          : 'neutral'
  const title =
    tone === 'mixed'
      ? 'Mixed result'
      : tone === 'regressed'
        ? 'Candidate regressed'
        : tone === 'improved'
          ? 'Candidate improved'
          : 'No material change'

  const details: string[] = []
  const score = comparison.metrics.score
  if (score.delta === 0) details.push('score unchanged')
  else if (score.delta !== null)
    details.push(score.delta > 0 ? 'score improved' : 'score declined')
  for (const [key, label] of [
    ['duration', 'duration'],
    ['tokens', 'tokens'],
    ['cost', 'cost'],
  ] as const) {
    const metric = comparison.metrics[key]
    if (metric.delta === null || metric.delta === 0) continue
    details.push(`${label} ${metric.delta < 0 ? 'improved' : 'increased'}`)
  }
  if (comparison.metrics.functionErrors.delta !== null) {
    if (comparison.metrics.functionErrors.delta > 0)
      details.push('more function errors')
    if (comparison.metrics.functionErrors.delta < 0)
      details.push('fewer function errors')
  }
  return {
    tone,
    title,
    detail: details.length
      ? details.join(', ')
      : 'all comparable metrics are unchanged',
  }
}

function MetricComparisonCard({
  metric,
  label,
  value,
  compatible,
  Icon,
}: {
  metric: ComparisonMetricKey
  label: string
  value: ComparedMetric
  compatible: boolean
  Icon: typeof Target
}) {
  return (
    <article className="tmh-comparison-card">
      <div className="tmh-comparison-card-head">
        <Icon size={16} strokeWidth={1.75} aria-hidden="true" />
        <span>{label}</span>
      </div>
      <div className="tmh-comparison-values">
        <span className="tmh-comparison-side-label tmh-comparison-side-a">
          A
        </span>
        <strong>{formatMetricValue(metric, value.baseline)}</strong>
        <ArrowRight aria-hidden="true" size={17} strokeWidth={1.5} />
        <strong>{formatMetricValue(metric, value.candidate)}</strong>
        <span className="tmh-comparison-side-label tmh-comparison-side-b">
          B
        </span>
      </div>
      <p
        className={compatible ? deltaTone(metric, value) : 'tmh-delta-neutral'}
      >
        {compatible ? metricDeltaText(metric, value) : 'Delta not interpreted'}
      </p>
    </article>
  )
}

function MetricPair({
  metric,
  value,
  compatible,
}: {
  metric: ComparisonMetricKey
  value: ComparedMetric
  compatible: boolean
}) {
  const changed = value.baseline !== value.candidate
  const tone =
    changed && compatible && value.delta !== null
      ? deltaTone(metric, value).replace('tmh-delta-', '')
      : changed
        ? 'neutral'
        : null

  return (
    <span className="tmh-impact-pair">
      <span className="tmh-impact-value">
        {formatMetricValue(metric, value.baseline)}
      </span>
      <ArrowRight aria-hidden="true" size={13} />
      <span
        className={`tmh-impact-value${tone ? ` tmh-impact-change tmh-impact-change-${tone}` : ''}`}
      >
        {formatMetricValue(metric, value.candidate)}
      </span>
    </span>
  )
}

function resultChangeTone(
  baseline: TestObservation['status'],
  candidate: TestObservation['status'],
  compatible: boolean,
) {
  if (baseline === candidate) return null
  if (!compatible) return 'neutral'
  if (candidate === 'passed') return 'improved'
  if (baseline === 'passed') return 'regressed'
  return 'caution'
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
  const verdict = comparison ? comparisonVerdict(comparison) : null
  const resultTone = comparison
    ? resultChangeTone(
        comparison.baseline.status,
        comparison.candidate.status,
        comparison.compatible,
      )
    : null

  return (
    <section
      className="tmh-comparison-workspace"
      aria-labelledby="tmh-comparison-title"
      aria-live="polite"
    >
      <div className="tmh-comparison-picker">
        <div className="tmh-comparison-pick">
          <span className="tmh-ab-chip tmh-ab-chip-a" aria-hidden="true">
            A
          </span>
          <span>
            <strong id="tmh-comparison-title">A · Baseline</strong>
            <small>
              {baseline
                ? `${formatDate(baseline.completed_at)} · ${logicalRunLabel(baseline.run_count)}`
                : 'Choose an execution'}
            </small>
          </span>
          {baseline ? (
            <ScenarioChatAction
              compact
              executionId={baseline.execution_id}
              scenarioId={testId}
            />
          ) : null}
        </div>
        <button
          className="tmh-swap-comparison"
          type="button"
          onClick={onSwap}
          disabled={!candidate}
        >
          <ArrowLeftRight aria-hidden="true" size={15} />
          Swap sides
        </button>
        <div className="tmh-comparison-pick tmh-comparison-pick-b">
          <span className="tmh-ab-chip tmh-ab-chip-b" aria-hidden="true">
            B
          </span>
          <span>
            <strong>B · Candidate</strong>
            <small>
              {candidate
                ? `${formatDate(candidate.completed_at)} · ${logicalRunLabel(candidate.run_count)}`
                : 'Choose an execution'}
            </small>
          </span>
          {candidate ? (
            <ScenarioChatAction
              compact
              executionId={candidate.execution_id}
              scenarioId={testId}
            />
          ) : null}
        </div>
        {baseline && (
          <button
            className="tmh-clear-comparison"
            type="button"
            onClick={onClear}
          >
            Clear
          </button>
        )}
      </div>

      {!baseline ? (
        <p className="tmh-comparison-message">
          Select <strong>Set A</strong> on an execution below to choose the
          baseline. Separate cases and cohorts are never pooled.
        </p>
      ) : !candidate ? (
        <p className="tmh-comparison-message">
          Now select <strong>Set B</strong> on another execution to choose the
          candidate.
        </p>
      ) : comparison ? (
        <>
          <div
            className={`tmh-compatibility${comparison?.compatible ? ' is-compatible' : ' is-incompatible'}`}
            role="status"
          >
            {comparison?.compatible ? (
              <CircleCheck aria-hidden="true" size={17} />
            ) : (
              <TriangleAlert aria-hidden="true" size={17} />
            )}
            <strong>
              {comparison?.compatible
                ? 'Comparable executions'
                : 'Not comparable'}
            </strong>
            <span>
              {comparison?.compatible
                ? 'Same scenario contract, seed, cohort, model, and assessment profile.'
                : comparison?.reasons.join(', ')}
            </span>
          </div>
          {verdict && (
            <div className={`tmh-verdict tmh-verdict-${verdict.tone}`}>
              {verdict.tone === 'regressed' ? (
                <TriangleAlert aria-hidden="true" size={19} />
              ) : verdict.tone === 'improved' ? (
                <CircleCheck aria-hidden="true" size={19} />
              ) : (
                <span className="tmh-verdict-mark" aria-hidden="true" />
              )}
              <strong>{verdict.title}</strong>
              <span>· {verdict.detail}</span>
            </div>
          )}
          <div className="tmh-comparison-card-grid">
            {comparisonMetrics.map(({ key, label, icon }) => (
              <MetricComparisonCard
                key={key}
                Icon={icon}
                label={label}
                metric={key}
                value={comparison.metrics[key]}
                compatible={comparison.compatible}
              />
            ))}
          </div>
          <div className="tmh-impact-table-wrap">
            <table
              className="tmh-impact-table"
              aria-label={`Comparison impact for ${testId}`}
            >
              <caption>Impact by scenario</caption>
              <thead>
                <tr>
                  <th scope="col">Scenario</th>
                  <th scope="col">Result</th>
                  <th scope="col">Score</th>
                  <th scope="col">Duration</th>
                  <th scope="col">Tokens</th>
                  <th scope="col">Cost</th>
                  <th scope="col">Functions</th>
                  <th scope="col">Errors</th>
                  <th scope="col">Turns</th>
                  <th scope="col">Reading</th>
                </tr>
              </thead>
              <tbody>
                <tr className="tmh-impact-row">
                  <td data-label="Scenario">
                    <code>{testId}</code>
                  </td>
                  <td data-label="Result">
                    <span className="tmh-impact-pair">
                      <span className="tmh-impact-value">
                        {formatStatus(comparison.baseline.status)}
                      </span>
                      <ArrowRight aria-hidden="true" size={13} />
                      <span
                        className={`tmh-impact-value${resultTone ? ` tmh-impact-change tmh-impact-change-${resultTone}` : ''}`}
                      >
                        {formatStatus(comparison.candidate.status)}
                      </span>
                    </span>
                  </td>
                  {comparisonMetrics.map(({ key, label }) => (
                    <td key={key} data-label={label}>
                      <MetricPair
                        metric={key}
                        value={comparison.metrics[key]}
                        compatible={comparison.compatible}
                      />
                    </td>
                  ))}
                  <td data-label="Reading">
                    <span
                      className={`tmh-reading tmh-reading-${verdict?.tone ?? 'neutral'}`}
                    >
                      {verdict?.title ?? 'Unknown'}
                    </span>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
        </>
      ) : null}
    </section>
  )
}

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
  const dialogRef = useRef<HTMLDialogElement>(null)
  const [detail, setDetail] = useState<DashboardExecutionDetail | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    const dialog = dialogRef.current
    if (dialog && !dialog.open) dialog.showModal()
  }, [])

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

  return (
    <dialog
      ref={dialogRef}
      className="tmh-execution-dialog"
      onClose={onClose}
      aria-labelledby="tmh-execution-dialog-title"
    >
      <div className="tmh-execution-dialog-shell">
        <header className="tmh-execution-dialog-header">
          <div>
            <span className="tmh-label">Test execution details</span>
            <h2 id="tmh-execution-dialog-title">{testId}</h2>
            <p>
              {formatDate(observation.completed_at)} ·{' '}
              {version ? `Test v${String(version)}` : 'Test version unknown'}
              {' · '}
              {logicalRunLabel(observation.run_count)}
            </p>
          </div>
          <button
            className="tmh-dialog-close"
            type="button"
            onClick={onClose}
            aria-label="Close execution details"
          >
            ×
          </button>
        </header>

        <div className="tmh-execution-dialog-body">
          <section className="tmh-execution-dialog-overview">
            <div>
              <span className="tmh-label">Result</span>
              <strong
                className={`tmh-status ${statusClass(observation.status)}`}
              >
                {formatStatus(observation.status)}
              </strong>
              <small>
                {observation.median_score === null ||
                observation.median_score === undefined
                  ? 'Score unknown'
                  : `Score ${observation.median_score.toFixed(2)}`}
              </small>
            </div>
            <div>
              <span className="tmh-label">Execution model</span>
              <strong>
                {modelLabel(
                  observation.subject_provider,
                  observation.subject_model,
                )}
              </strong>
            </div>
            <div>
              <span className="tmh-label">Judge</span>
              <strong>
                {modelLabel(
                  observation.judge_provider,
                  observation.judge_model,
                )}
              </strong>
              <small>{observation.judge_protocol ?? 'Protocol unknown'}</small>
            </div>
            <div>
              <span className="tmh-label">Environment</span>
              <strong>{systemSummary(observation)}</strong>
            </div>
          </section>

          <section
            className="tmh-execution-dialog-metrics"
            aria-label="Execution metrics"
          >
            <div>
              <span>Cost</span>
              <strong>{formatCost(observation.median_cost_usd)}</strong>
            </div>
            <div>
              <span>Duration</span>
              <strong>
                {formatDuration(observation.median_duration_seconds)}
              </strong>
            </div>
            <div>
              <span>Tokens</span>
              <strong>{formatTokens(observation.median_tokens)}</strong>
            </div>
            <div>
              <span>Turns</span>
              <strong>
                {observation.median_turns === null ||
                observation.median_turns === undefined
                  ? 'Unknown'
                  : Math.round(observation.median_turns).toLocaleString()}
              </strong>
            </div>
          </section>

          <section className="tmh-execution-dialog-report">
            <div className="tmh-execution-dialog-report-heading">
              <div>
                <span className="tmh-label">Test report</span>
                <h3>Assessment details for this test</h3>
              </div>
              {detail && (
                <span className="tmh-visible-count">
                  {String(availableReports ?? 0)} available{' '}
                  {(availableReports ?? 0) === 1 ? 'report' : 'reports'}
                </span>
              )}
            </div>
            {loading ? (
              <p className="tmh-dialog-message" role="status">
                Loading execution report…
              </p>
            ) : error ? (
              <p className="tmh-dialog-message tmh-dialog-error" role="alert">
                {error}
              </p>
            ) : (
              <AssessmentWorkspace detail={scopedDetail} />
            )}
          </section>
        </div>

        <footer className="tmh-execution-dialog-footer">
          <a
            className="tmh-detail-link"
            href={hashForExecution(observation.execution_id)}
          >
            Open full execution report
          </a>
          <ScenarioChatAction
            detail={scopedDetail}
            executionId={observation.execution_id}
            scenarioId={testId}
          />
          <button className="tmh-detail-button" type="button" onClick={onClose}>
            Close
          </button>
        </footer>
      </div>
    </dialog>
  )
}

export function TestHistoryPage({ testId }: { testId: string }) {
  const [history, setHistory] = useState<TestHistoryResponse | null>(null)
  const [version, setVersion] = useState<number | undefined>()
  const [executionModel, setExecutionModel] = useState('')
  const [judgeModel, setJudgeModel] = useState('')
  const [system, setSystem] = useState('')
  const [result, setResult] = useState('')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [selectedObservation, setSelectedObservation] =
    useState<TestObservation | null>(null)
  const [comparisonKeys, setComparisonKeys] = useState<string[]>([])

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    setError(null)
    void getDashboardDataBridge()
      .then((next) => {
        if (cancelled) return
        if (next.mode !== 'local') {
          throw new Error(
            'Test metric history is available only in the local dashboard',
          )
        }
        const execution = parseModelSelection(executionModel)
        const judge = parseModelSelection(judgeModel)
        return next.getTestHistory({
          test_id: testId,
          test_version: version,
          subject_provider: execution?.provider,
          subject_model: execution?.model,
          judge_provider: judge?.provider,
          judge_model: judge?.model,
          system_version_id: system || undefined,
          result: result || undefined,
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
  }, [executionModel, judgeModel, result, system, testId, version])

  const executionModelGroups = useMemo(
    () => modelGroups(history, 'subject'),
    [history],
  )
  const judgeModelGroups = useMemo(
    () => modelGroups(history, 'judge'),
    [history],
  )

  const observations = history?.observations ?? []
  const costs = observations.map((item) => item.median_cost_usd)
  const durations = observations.map((item) => item.median_duration_seconds)
  const tokens = observations.map((item) => item.median_tokens)
  const turns = observations.map((item) => item.median_turns)
  const summaryCost = median(costs)
  const summaryDuration = median(durations)
  const summaryTokens = median(tokens)
  const summaryTurns = median(turns)
  const latestVersion = history?.available_versions.find(
    (item) => item.version === history.test_version,
  )
  const knownPassed = observations.filter(
    (item) => item.status === 'passed',
  ).length
  const comparisonSelections = comparisonKeys
    .map((key) => observations.find((item) => testObservationKey(item) === key))
    .filter((item): item is TestObservation => Boolean(item))
  const baseline = comparisonSelections[0] ?? null
  const candidate = comparisonSelections[1] ?? null

  function clearComparison() {
    setComparisonKeys([])
  }

  function swapComparison() {
    setComparisonKeys((current) =>
      current.length === 2 ? [current[1], current[0]] : current,
    )
  }

  function selectForComparison(observation: TestObservation) {
    const key = testObservationKey(observation)
    setComparisonKeys((current) => {
      const selectedIndex = current.indexOf(key)
      if (selectedIndex === 0) return current.slice(1)
      if (selectedIndex === 1) return current.slice(0, 1)
      if (current.length === 0) return [key]
      if (current.length === 1) return [...current, key]
      return [current[0], key]
    })
  }

  function comparisonActionLabel(observation: TestObservation) {
    const selectedIndex = comparisonKeys.indexOf(
      testObservationKey(observation),
    )
    if (selectedIndex === 0) return 'A · baseline'
    if (selectedIndex === 1) return 'B · candidate'
    if (comparisonKeys.length === 0) return 'Set A'
    if (comparisonKeys.length === 1) return 'Set B'
    return 'Replace B'
  }

  return (
    <div id="test-metrics-history-proposal" className="tmh-page">
      <DashboardPageActions active="tests" />

      <main className="tmh-main" id="test-history-main">
        <p className="tmh-breadcrumb">
          <a href={hashForWorkspace('tests')}>Tests</a> / <span>{testId}</span>
        </p>
        <h1>{testId}</h1>
        <p className="tmh-subtitle">
          Inspect how this test&apos;s result, score, duration, tokens, cost,
          function activity, errors, and turns changed across retained local
          executions.
        </p>

        <div className="tmh-identity">
          <div className="tmh-identity-item">
            <span className="tmh-label">Current version</span>
            <strong>
              {history?.test_version ? `v${history.test_version}` : 'Unknown'}
            </strong>
          </div>
          <div className="tmh-identity-item">
            <span className="tmh-label">Complexity</span>
            <strong>Not recorded</strong>
          </div>
          <div className="tmh-identity-item">
            <span className="tmh-label">Contract</span>
            <code>{contractSummary(observations)}</code>
          </div>
          <div className="tmh-identity-item">
            <span className="tmh-label">Retained</span>
            <strong>
              {latestVersion?.execution_count ?? history?.total ?? 0} executions
            </strong>
          </div>
        </div>

        <section className="tmh-panel" aria-labelledby="tmh-history-title">
          <header className="tmh-panel-head">
            <div>
              <h2 id="tmh-history-title">Metric history</h2>
              <p>Filters change only the evidence series shown below.</p>
            </div>
            <span className="tmh-visible-count">
              {loading
                ? 'Loading executions…'
                : visibleLabel(observations.length)}
            </span>
          </header>

          <div className="tmh-filters">
            <label className="tmh-field">
              Test version
              <select
                value={version ?? ''}
                onChange={(event) => {
                  setVersion(
                    event.target.value ? Number(event.target.value) : undefined,
                  )
                  clearComparison()
                }}
              >
                <option value="">Current / latest</option>
                {(history?.available_versions ?? []).map((item) => (
                  <option key={item.version} value={item.version}>
                    v{item.version}
                    {item.version === history?.test_version ? ' · current' : ''}
                  </option>
                ))}
              </select>
            </label>
            <div className="tmh-field">
              Execution model
              <ProviderModelDropdown
                groups={executionModelGroups}
                value={executionModel}
                onChange={(next) => {
                  setExecutionModel(next)
                  clearComparison()
                }}
                optionValue={modelSelection}
                placeholder="All execution models"
                ariaLabel="Execution model"
              />
            </div>
            <div className="tmh-field">
              Judge model
              <ProviderModelDropdown
                groups={judgeModelGroups}
                value={judgeModel}
                onChange={(next) => {
                  setJudgeModel(next)
                  clearComparison()
                }}
                optionValue={modelSelection}
                placeholder="All judge models"
                ariaLabel="Judge model"
              />
            </div>
            <label className="tmh-field">
              System revision
              <select
                value={system}
                onChange={(event) => {
                  setSystem(event.target.value)
                  clearComparison()
                }}
              >
                <option value="">All revisions</option>
                {(history?.systems ?? []).map((item) => (
                  <option key={item.id} value={item.id}>
                    {item.label}
                  </option>
                ))}
              </select>
            </label>
            <label className="tmh-field">
              Result
              <select
                value={result}
                onChange={(event) => {
                  setResult(event.target.value)
                  clearComparison()
                }}
              >
                <option value="">All results</option>
                <option value="passed">Passed</option>
                <option value="hard_gate_failed">Hard gate failed</option>
                <option value="technical_failed">Technical failure</option>
                <option value="infra_failed">Infrastructure failure</option>
              </select>
            </label>
          </div>

          {error ? (
            <div className="tmh-empty tmh-error" role="alert">
              <strong>History unavailable</strong>
              <span>{error}</span>
            </div>
          ) : loading ? (
            <div className="tmh-empty" role="status">
              Loading metric history…
            </div>
          ) : (
            <>
              <div className="tmh-metric-summary">
                <div className="tmh-metric">
                  <span>Successful runs</span>
                  <strong>
                    {observations.length
                      ? `${String(knownPassed)} / ${observations.length}`
                      : '—'}
                  </strong>
                  <small>objective result</small>
                </div>
                <div className="tmh-metric">
                  <span>Median cost</span>
                  <strong>{formatCost(summaryCost)}</strong>
                  <small>
                    {metricCaption(
                      knownMetricCount(costs),
                      observations.length,
                      'cost',
                    )}
                  </small>
                </div>
                <div className="tmh-metric">
                  <span>Median duration</span>
                  <strong>{formatDuration(summaryDuration)}</strong>
                  <small>
                    {metricCaption(
                      knownMetricCount(durations),
                      observations.length,
                      'duration',
                    )}
                  </small>
                </div>
                <div className="tmh-metric">
                  <span>Median tokens</span>
                  <strong>{formatTokens(summaryTokens)}</strong>
                  <small>
                    {metricCaption(
                      knownMetricCount(tokens),
                      observations.length,
                      'token',
                    )}
                  </small>
                </div>
                <div className="tmh-metric">
                  <span>Median turns</span>
                  <strong>
                    {summaryTurns === null
                      ? 'Unknown'
                      : Math.round(summaryTurns).toLocaleString()}
                  </strong>
                  <small>
                    {metricCaption(
                      knownMetricCount(turns),
                      observations.length,
                      'turn',
                    )}
                  </small>
                </div>
              </div>

              {comparisonKeys.length > 0 ? (
                <div className="tmh-comparison-stack">
                  <ObservationComparisonPanel
                    baseline={baseline}
                    candidate={candidate}
                    testId={testId}
                    onClear={clearComparison}
                    onSwap={swapComparison}
                  />
                  <details className="tmh-history-disclosure">
                    <summary>
                      <ChevronDown aria-hidden="true" size={16} />
                      <span>
                        View score history across {observations.length}{' '}
                        {observations.length === 1 ? 'execution' : 'executions'}
                      </span>
                      <small>Points can replace A or B</small>
                    </summary>
                    <section
                      className="tmh-history-chart"
                      aria-label="Score per execution"
                    >
                      <ScoreTrendChart
                        observations={observations}
                        selectedKeys={comparisonKeys}
                        onSelect={selectForComparison}
                      />
                    </section>
                  </details>
                </div>
              ) : (
                <p className="tmh-compare-cta">
                  Select <strong>Set A</strong> on an execution below to open
                  the graphical baseline/candidate comparison.
                </p>
              )}

              <table aria-label={`Metric history for ${testId}`}>
                <thead>
                  <tr>
                    <th scope="col" style={{ width: '16%' }}>
                      Execution
                    </th>
                    <th scope="col" style={{ width: '22%' }}>
                      Model and system
                    </th>
                    <th scope="col" style={{ width: '13%' }}>
                      Result
                    </th>
                    <th scope="col" style={{ width: '9%' }}>
                      Cost
                    </th>
                    <th scope="col" style={{ width: '10%' }}>
                      Duration
                    </th>
                    <th scope="col" style={{ width: '10%' }}>
                      Tokens
                    </th>
                    <th scope="col" style={{ width: '7%' }}>
                      Turns
                    </th>
                    <th scope="col" style={{ width: '13%' }}>
                      Actions
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {observations.map((item) => (
                    <tr
                      className="tmh-metric-row"
                      key={
                        item.execution_id +
                        ':' +
                        item.case_id +
                        ':' +
                        item.contract_sha256
                      }
                    >
                      <td data-label="Execution">
                        <strong>{formatDate(item.completed_at)}</strong>
                        <small>
                          Test v
                          {item.scenario_version ??
                            history?.test_version ??
                            '—'}{' '}
                          · {logicalRunLabel(item.run_count)}
                        </small>
                      </td>
                      <td data-label="Model and system">
                        <strong>
                          {modelLabel(
                            item.subject_provider,
                            item.subject_model,
                          )}
                        </strong>
                        <small>
                          {systemSummary(item)} · judge{' '}
                          {modelLabel(item.judge_provider, item.judge_model)}
                          {' · '}
                          {item.judge_protocol ?? 'protocol unknown'}
                        </small>
                      </td>
                      <td data-label="Result">
                        <span
                          className={`tmh-status ${statusClass(item.status)}`}
                        >
                          {formatStatus(item.status)}
                        </span>
                        <small>
                          {item.median_score === null ||
                          item.median_score === undefined
                            ? 'Score unknown'
                            : `Score ${item.median_score.toFixed(2)}`}
                        </small>
                      </td>
                      <td data-label="Cost">
                        {formatCost(item.median_cost_usd)}
                      </td>
                      <td data-label="Duration">
                        {formatDuration(item.median_duration_seconds)}
                      </td>
                      <td data-label="Tokens">
                        {formatTokens(item.median_tokens)}
                      </td>
                      <td data-label="Turns">
                        {item.median_turns === null ||
                        item.median_turns === undefined
                          ? 'Unknown'
                          : Math.round(item.median_turns).toLocaleString()}
                      </td>
                      <td data-label="Actions">
                        <div className="tmh-row-actions">
                          <button
                            className="tmh-detail-button"
                            type="button"
                            onClick={() => setSelectedObservation(item)}
                            aria-label={`Open details for ${testId} from ${formatDate(item.completed_at)}`}
                          >
                            View details
                          </button>
                          <button
                            className={`tmh-compare-button${comparisonKeys.includes(testObservationKey(item)) ? ' is-selected' : ''}`}
                            type="button"
                            onClick={() => selectForComparison(item)}
                            aria-pressed={comparisonKeys.includes(
                              testObservationKey(item),
                            )}
                          >
                            {comparisonActionLabel(item)}
                          </button>
                          <ScenarioChatAction
                            compact
                            executionId={item.execution_id}
                            scenarioId={testId}
                          />
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
              {!observations.length && (
                <div className="tmh-empty" style={{ display: 'block' }}>
                  No executions match these filters.
                </div>
              )}
            </>
          )}
        </section>
      </main>
      {selectedObservation && (
        <ExecutionDetailsDialog
          observation={selectedObservation}
          testVersion={history?.test_version}
          testId={testId}
          onClose={() => setSelectedObservation(null)}
        />
      )}
    </div>
  )
}
