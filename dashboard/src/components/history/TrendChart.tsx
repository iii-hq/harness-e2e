import { SegmentedControl } from '@iii-dev/console-ui'
import { useEffect, useRef, useState } from 'react'
import { shortDefinition } from '@/lib/definition-digest'
import { formatDateTime, formatTime } from '@/lib/format'
import {
  axisTop,
  CHART_METRICS,
  type ChartMetric,
  type HistoryObservation,
  type Loss,
  metricFigure,
  metricValue,
  profileText,
  shortModel,
} from '@/lib/test-history'
import '@/pages/test-history.css'

/** The rendered width of an element, so the SVG's units are its pixels. */
function useMeasuredWidth(fallback: number) {
  const ref = useRef<HTMLDivElement>(null)
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

// The first series in ink, the second in the accent, then the host glyphs.
const SERIES_COLORS = [
  'var(--color-ink)',
  'var(--color-accent)',
  'var(--color-glyph-teal)',
  'var(--color-glyph-purple)',
  'var(--color-glyph-amber)',
]

/** The legend: by model when the runs used more than one, else by profile. */
export function seriesOf(observations: HistoryObservation[]) {
  const models = [...new Set(observations.map(shortModel))]
  const byModel =
    models.length > 1 || new Set(observations.map(profileText)).size <= 1
  const key = byModel ? shortModel : profileText
  const keys = [...new Set(observations.map(key))]
  return {
    key,
    legend: keys.map((label, index) => ({
      label,
      color: SERIES_COLORS[index % SERIES_COLORS.length],
    })),
  }
}

const TOP = 16
const BOTTOM = 144
const LEFT = 44

export function TrendChart({
  observations,
  metric,
  onMetric,
}: {
  /** Newest first, as the history lists them. */
  observations: HistoryObservation[]
  metric: ChartMetric
  onMetric: (metric: ChartMetric) => void
}) {
  const { ref, width } = useMeasuredWidth(820)
  const runs = [...observations].reverse()
  const spec =
    CHART_METRICS.find((item) => item.id === metric) ?? CHART_METRICS[0]
  const right = Math.max(LEFT + 80, width - 16)
  const count = runs.length
  const xAt = (index: number) =>
    count === 1
      ? (LEFT + right) / 2
      : LEFT + (index * (right - LEFT)) / (count - 1)
  const values = runs.map((item) => metricValue(item, metric))
  const known = values.filter((value): value is number => value !== null)
  const top = axisTop(metric, known.length ? Math.max(...known) : 1)
  const yAt = (value: number) => BOTTOM - (value / top) * (BOTTOM - TOP)
  const { key, legend } = seriesOf(runs)
  const colorOf = (item: HistoryObservation) =>
    legend.find((entry) => entry.label === key(item))?.color ?? SERIES_COLORS[0]
  // Labels need room: every nth run keeps its value and time.
  const spacing = count > 1 ? (right - LEFT) / (count - 1) : right - LEFT
  const every = Math.max(1, Math.ceil(56 / spacing))
  const labelled = (index: number) => index % every === 0 || index === count - 1
  const definitionLines = runs.flatMap((item, index) =>
    index > 0 && item.behavior_sha256 !== runs[index - 1].behavior_sha256
      ? [
          {
            x: Math.round((xAt(index) + xAt(index - 1)) / 2),
            label: shortDefinition(item.behavior_sha256) ?? '',
          },
        ]
      : [],
  )
  return (
    <section
      className="th-card th-chart"
      aria-labelledby="th-trend"
      data-history-chart={metric}
    >
      <div className="th-card-head">
        <h2 id="th-trend" className="th-h2">
          {spec.label} by run
        </h2>
        <ul className="th-legend" aria-label="Series">
          {legend.map((entry) => (
            <li key={entry.label}>
              <span
                className="th-legend-dot"
                style={{ background: entry.color }}
                aria-hidden="true"
              />
              {entry.label}
            </li>
          ))}
        </ul>
        <SegmentedControl
          variant="radio"
          aria-label="Metric"
          className="th-metrics"
          value={metric}
          onChange={onMetric}
          options={CHART_METRICS.map((item) => ({
            value: item.id,
            label: item.label,
          }))}
        />
      </div>
      <div ref={ref} className="th-chart-frame">
        <svg
          viewBox={`0 0 ${width} 180`}
          width="100%"
          height="180"
          role="img"
          aria-label={`${spec.label} of each run, oldest first`}
        >
          {[TOP, (TOP + BOTTOM) / 2, BOTTOM].map((y) => (
            <line
              key={y}
              className="th-grid"
              x1={36}
              x2={right}
              y1={y}
              y2={y}
            />
          ))}
          <text className="th-axis" x={28} y={TOP + 4} textAnchor="end">
            {metricFigure(metric, top)}
          </text>
          <text
            className="th-axis"
            x={28}
            y={(TOP + BOTTOM) / 2 + 4}
            textAnchor="end"
          >
            {metricFigure(metric, top / 2)}
          </text>
          <text className="th-axis" x={28} y={BOTTOM + 4} textAnchor="end">
            0
          </text>
          {definitionLines.map((line) => (
            <g key={`${line.x}:${line.label}`}>
              <line
                className="th-definition-line"
                x1={line.x}
                x2={line.x}
                y1={8}
                y2={BOTTOM + 16}
              />
              <text className="th-axis" x={line.x + 4} y={BOTTOM + 14}>
                {line.label}
              </text>
            </g>
          ))}
          {legend.map((entry) => {
            const points = runs.flatMap((item, index) =>
              colorOf(item) === entry.color && values[index] !== null
                ? [
                    `${Math.round(xAt(index))},${Math.round(yAt(values[index] as number))}`,
                  ]
                : [],
            )
            return points.length > 1 ? (
              <polyline
                key={entry.label}
                className="th-series"
                points={points.join(' ')}
                style={{ stroke: entry.color }}
              />
            ) : null
          })}
          {runs.map((item, index) => {
            const value = values[index]
            const x = Math.round(xAt(index))
            const y = value === null ? BOTTOM : Math.round(yAt(value))
            const color = colorOf(item)
            return (
              <g key={`${item.execution_id}:${item.case_id}`}>
                <title>
                  {`${formatDateTime(item.completed_at)} · ${value === null ? 'not reported' : metricFigure(metric, value)}`}
                </title>
                <circle
                  cx={x}
                  cy={y}
                  r={4.5}
                  strokeWidth={1.5}
                  style={{
                    stroke: color,
                    fill: value === null ? 'var(--color-panel-raised)' : color,
                  }}
                />
                {labelled(index) ? (
                  <>
                    <text
                      className="th-value"
                      x={x}
                      y={value === null ? 136 : y - 9}
                      textAnchor="middle"
                    >
                      {value === null ? '—' : metricFigure(metric, value)}
                    </text>
                    <text className="th-axis" x={x} y={172} textAnchor="middle">
                      {formatTime(item.completed_at)}
                    </text>
                  </>
                ) : null}
              </g>
            )
          })}
        </svg>
      </div>
      {spec.note ? <p className="th-note">{spec.note}</p> : null}
    </section>
  )
}

export function LossesPanel({ losses }: { losses: Loss[] }) {
  return (
    <section className="th-card th-losses" aria-labelledby="th-lose">
      <div className="th-card-head">
        <h2 id="th-lose" className="th-h2">
          Where it loses points
        </h2>
        <span className="th-faint">
          criteria missed, in scored runs of this scope
        </span>
      </div>
      {losses.length === 0 ? (
        <p className="th-faint th-empty-line">
          Every scored run met every criterion.
        </p>
      ) : (
        <ul className="th-loss-list">
          {losses.map((loss) => (
            <li key={loss.id} data-loss={loss.id}>
              <span className="th-mono th-ellipsis">{loss.id}</span>
              <span className="th-loss-bar" aria-hidden="true">
                <span style={{ width: `${loss.share}%` }} />
              </span>
              <span className="th-mono th-faint">{loss.text}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  )
}
