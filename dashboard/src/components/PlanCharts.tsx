import type { ReactNode } from 'react'

/**
 * The plan page's three chart forms, drawn as inline SVG on the design
 * system's tokens. Horizontal positions are percentages so every chart fills
 * its column without a resize observer; vertical rhythm is fixed pixels.
 *
 * Marks follow one spec: 2px lines, >= 8px markers with a 2px surface ring,
 * bars 10px thick with a rounded data end, hairline solid grid. Text never
 * wears a series color — identity comes from the mark beside it.
 */

export type SparklinePoint = {
  id: string
  label: string
  value: number | null
  /** baseline: the reference · selected: the candidate being read · other:
   *  earlier candidates · running: an execution still producing its report. */
  role: 'baseline' | 'selected' | 'other' | 'running'
}

const SURFACE = 'var(--panel, #fafafa)'
const HAIRLINE = 'var(--line, rgba(20, 16, 8, 0.16))'
const MUTED = 'var(--text-muted, #6e6b67)'
const ACCENT = 'var(--accent, #b8420f)'
const SUCCESS = 'var(--success, #356f3d)'
const DANGER = 'var(--danger, #c4001d)'

function pct(value: number) {
  return `${value.toFixed(2)}%`
}

/** A stat tile's trend: every completed execution in capture order, the
 *  reference drawn as the hairline every other point is read against. */
export function Sparkline({
  points,
  reference,
  height = 44,
  label,
}: {
  points: SparklinePoint[]
  reference: number | null
  height?: number
  label: string
}) {
  const plotted = points.filter((point) => point.value !== null)
  const values = plotted.map((point) => point.value as number)
  if (reference !== null) values.push(reference)
  const min = values.length ? Math.min(...values) : 0
  const max = values.length ? Math.max(...values) : 0
  const pad = 8
  const y = (value: number) => {
    if (max === min) return height / 2
    return pad + ((max - value) / (max - min)) * (height - pad * 2)
  }
  const slots = Math.max(points.length, 2)
  const x = (index: number) => 6 + (index / (slots - 1)) * 88
  const line = plotted.map((point) => ({
    id: point.id,
    x: x(points.indexOf(point)),
    y: y(point.value as number),
  }))
  const segments = line.slice(1).map((point, index) => ({
    from: line[index],
    to: point,
  }))
  return (
    <svg
      width="100%"
      height={height}
      role="img"
      aria-label={label}
      style={{ display: 'block', overflow: 'visible' }}
      data-sparkline
    >
      {reference !== null ? (
        <line
          x1="0"
          x2="100%"
          y1={y(reference)}
          y2={y(reference)}
          stroke={HAIRLINE}
          strokeWidth="1"
        />
      ) : null}
      {segments.map(({ from, to }) => (
        <line
          key={`${from.id}→${to.id}`}
          x1={pct(from.x)}
          y1={from.y}
          x2={pct(to.x)}
          y2={to.y}
          stroke={ACCENT}
          strokeOpacity={0.55}
          strokeWidth="2"
          strokeLinecap="round"
        />
      ))}
      {points.map((point, index) => {
        const cx = pct(x(index))
        if (point.value === null) {
          return (
            <circle
              key={point.id}
              cx={cx}
              cy={reference === null ? height / 2 : y(reference)}
              r="4"
              fill={SURFACE}
              stroke={MUTED}
              strokeWidth="1.5"
              data-point-role={point.role}
            >
              <title>{point.label}</title>
            </circle>
          )
        }
        const cy = y(point.value)
        const fill = point.role === 'baseline' ? MUTED : ACCENT
        return (
          <g key={point.id} data-point-role={point.role}>
            <circle
              cx={cx}
              cy={cy}
              r={point.role === 'selected' ? 6 : 5}
              fill={SURFACE}
            />
            <circle
              cx={cx}
              cy={cy}
              r={point.role === 'selected' ? 4 : 3.5}
              fill={fill}
              fillOpacity={point.role === 'other' ? 0.45 : 1}
            >
              <title>{point.label}</title>
            </circle>
          </g>
        )
      })}
    </svg>
  )
}

export type DivergingRow = {
  id: string
  label: string
  /** Relative change oriented by improvement: positive is better, whatever
   *  the metric's own direction. */
  improvement: number
  /** The delta in the metric's own unit, shown at the bar's tip. */
  valueLabel: string
  tone: 'positive' | 'negative'
}

export type DivergingGroup = {
  id: string
  title: string
  subtitle: string
  rows: DivergingRow[]
  /** Metrics that did not move, named once instead of drawn as empty rows. */
  unchanged: string
}

const ROW = 26
const GROUP_HEAD = 46
const AXIS = 30
const LABEL_COLUMN = 18 // percent reserved for the row label
const HALF = (100 - LABEL_COLUMN - 2) / 2 // percent for ±100%

/** What moved by test: a diverging bar per changed metric, right is better. */
export function DivergingBars({
  groups,
  label,
}: {
  groups: DivergingGroup[]
  label: string
}) {
  const center = LABEL_COLUMN + HALF
  const rowCount = groups.reduce((sum, group) => sum + group.rows.length, 0)
  const height =
    groups.length * GROUP_HEAD + rowCount * ROW + AXIS + groups.length * 8
  let cursor = 0
  const ticks: Array<[number, string]> = [
    [-100, '−100%'],
    [-50, '−50%'],
    [0, 'reference'],
    [50, '+50%'],
    [100, '+100%'],
  ]
  const xOf = (improvement: number) =>
    center + (Math.max(-100, Math.min(100, improvement)) / 100) * HALF
  return (
    <svg
      width="100%"
      height={height}
      role="img"
      aria-label={label}
      style={{ display: 'block', overflow: 'visible' }}
      data-diverging-bars
    >
      {ticks.map(([value, tick]) => (
        <g key={value}>
          <line
            x1={pct(xOf(value))}
            x2={pct(xOf(value))}
            y1="4"
            y2={height - AXIS + 6}
            stroke={value === 0 ? MUTED : HAIRLINE}
            strokeWidth="1"
          />
          <text
            x={pct(xOf(value))}
            y={height - 8}
            textAnchor="middle"
            fontSize="10"
            fill={MUTED}
            fontFamily="var(--font-mono)"
          >
            {tick}
          </text>
        </g>
      ))}
      {groups.map((group) => {
        const top = cursor
        cursor += GROUP_HEAD + group.rows.length * ROW + 8
        return (
          <g key={group.id} data-diverging-group={group.id}>
            <text
              x="0"
              y={top + 18}
              fontSize="13"
              fontWeight="600"
              fill="var(--text, #0a0a0a)"
            >
              {group.title}
            </text>
            <text
              x="0"
              y={top + 34}
              fontSize="11"
              fill={MUTED}
              fontFamily="var(--font-mono)"
            >
              {group.subtitle}
              {group.unchanged ? ` · unchanged: ${group.unchanged}` : ''}
            </text>
            {group.rows.map((row, index) => {
              const yMid = top + GROUP_HEAD + index * ROW + ROW / 2
              const tip = xOf(row.improvement)
              const left = Math.min(center, tip)
              const width = Math.abs(tip - center)
              const better = row.improvement >= 0
              const color = row.tone === 'positive' ? SUCCESS : DANGER
              return (
                <g key={row.id} data-diverging-row={row.id}>
                  <text
                    x={pct(2)}
                    y={yMid + 4}
                    fontSize="12"
                    fill="var(--text, #0a0a0a)"
                    fontFamily="var(--font-mono)"
                  >
                    {row.label}
                  </text>
                  {/* rounded data end, square at the reference */}
                  <rect
                    x={pct(left)}
                    y={yMid - 5}
                    width={pct(width)}
                    height="10"
                    rx="4"
                    fill={color}
                  />
                  <rect
                    x={pct(better ? center : center - 0.4)}
                    y={yMid - 5}
                    width={pct(Math.min(width, 0.4))}
                    height="10"
                    fill={color}
                  />
                  <text
                    x={pct(tip)}
                    dx={better ? 10 : -10}
                    y={yMid + 4}
                    textAnchor={better ? 'start' : 'end'}
                    fontSize="11"
                    fill="var(--text, #0a0a0a)"
                    fontFamily="var(--font-mono)"
                  >
                    {row.valueLabel}
                  </text>
                </g>
              )
            })}
          </g>
        )
      })}
    </svg>
  )
}

export type DumbbellRow = {
  id: string
  label: string
  baseline: number | null
  candidate: number | null
  baselineLabel: string
  candidateLabel: string
}

/** Before → after per test on one axis: baseline gray, candidate accent. */
export function Dumbbell({
  rows,
  domain,
  ticks,
  label,
}: {
  rows: DumbbellRow[]
  domain: [number, number]
  ticks: Array<{ value: number; label: string }>
  label: string
}) {
  const ROW_HEIGHT = 36
  const plotStart = 32
  const plotEnd = 96
  const [min, max] = domain
  const x = (value: number) =>
    max === min
      ? (plotStart + plotEnd) / 2
      : plotStart + ((value - min) / (max - min)) * (plotEnd - plotStart)
  const height = rows.length * ROW_HEIGHT + 22
  return (
    <svg
      width="100%"
      height={height}
      role="img"
      aria-label={label}
      style={{ display: 'block', overflow: 'visible' }}
      data-dumbbell
    >
      {ticks.map((tick) => (
        <g key={tick.value}>
          <line
            x1={pct(x(tick.value))}
            x2={pct(x(tick.value))}
            y1="4"
            y2={height - 18}
            stroke={HAIRLINE}
            strokeWidth="1"
          />
          <text
            x={pct(x(tick.value))}
            y={height - 4}
            textAnchor="middle"
            fontSize="10"
            fill={MUTED}
            fontFamily="var(--font-mono)"
          >
            {tick.label}
          </text>
        </g>
      ))}
      {rows.map((row, index) => {
        const yMid = index * ROW_HEIGHT + 22
        const same =
          row.baseline !== null &&
          row.candidate !== null &&
          Math.abs(row.baseline - row.candidate) < 1e-9
        // Ends closer than a label's width would overprint each other above
        // the marks; then the labels step aside, one on each side of the pair.
        const close =
          row.baseline !== null &&
          row.candidate !== null &&
          !same &&
          Math.abs(x(row.baseline) - x(row.candidate)) < 9
        const baselineLeft =
          row.baseline !== null &&
          row.candidate !== null &&
          row.baseline <= row.candidate
        const points: ReactNode[] = []
        if (row.baseline !== null && row.candidate !== null && !same) {
          points.push(
            <line
              key="bar"
              x1={pct(x(row.baseline))}
              x2={pct(x(row.candidate))}
              y1={yMid}
              y2={yMid}
              stroke={HAIRLINE}
              strokeWidth="2"
              strokeLinecap="round"
            />,
          )
        }
        if (row.baseline !== null) {
          points.push(
            <g key="baseline" data-dumbbell-end="baseline">
              <circle
                cx={pct(x(row.baseline))}
                cy={yMid}
                r="6"
                fill={SURFACE}
              />
              <circle cx={pct(x(row.baseline))} cy={yMid} r="4" fill={MUTED} />
              <text
                x={pct(x(row.baseline))}
                dx={close ? (baselineLeft ? -10 : 10) : 0}
                y={close ? yMid + 4 : yMid - 10}
                textAnchor={close ? (baselineLeft ? 'end' : 'start') : 'middle'}
                fontSize="11"
                fill={MUTED}
                fontFamily="var(--font-mono)"
              >
                {same ? '' : row.baselineLabel}
              </text>
            </g>,
          )
        }
        if (row.candidate !== null) {
          points.push(
            <g key="candidate" data-dumbbell-end="candidate">
              <circle
                cx={pct(x(row.candidate))}
                cy={yMid}
                r="6"
                fill={SURFACE}
              />
              <circle
                cx={pct(x(row.candidate))}
                cy={yMid}
                r="4"
                fill={ACCENT}
              />
              {same ? (
                <circle
                  cx={pct(x(row.candidate))}
                  cy={yMid}
                  r="7"
                  fill="none"
                  stroke={MUTED}
                  strokeWidth="1.5"
                />
              ) : null}
              <text
                x={pct(x(row.candidate))}
                dx={close ? (baselineLeft ? 10 : -10) : 0}
                y={close ? yMid + 4 : yMid - 10}
                textAnchor={close ? (baselineLeft ? 'start' : 'end') : 'middle'}
                fontSize="11"
                fill="var(--text, #0a0a0a)"
                fontFamily="var(--font-mono)"
              >
                {same
                  ? `${row.candidateLabel} · unchanged`
                  : row.candidateLabel}
              </text>
            </g>,
          )
        }
        return (
          <g key={row.id} data-dumbbell-row={row.id}>
            <text
              x="0"
              y={yMid + 4}
              fontSize="12"
              fill="var(--text, #0a0a0a)"
              fontFamily="var(--font-mono)"
            >
              {row.label}
            </text>
            {points}
          </g>
        )
      })}
    </svg>
  )
}
