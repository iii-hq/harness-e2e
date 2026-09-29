import { ArrowDown, ArrowUp, Maximize2, X } from 'lucide-react'
import { DeltaValue, deltaDirection, deltaTone } from '@/design-system'
import { useMeasuredWidth } from '@/hooks/use-measured-width'
import {
  comparedPair,
  counted,
  dayMarks,
  deltaFormat,
  deltaOf,
  domain,
  laneLabel,
  pointTime,
  referenceText,
  roomyMarks,
  segments,
  slotX,
  type TrendChange,
  type TrendMetric,
  type TrendPoint,
} from '@/lib/trends'

/** A difference in its tone (Trends.dc.html): DeltaValue coloured by which
 *  way the measure is better, on a tinted pill; "no change" when it rounds
 *  to nothing. */
export function DeltaPill({
  metric,
  value,
}: {
  metric: TrendMetric
  value: number | null
}) {
  if (value === null) return null
  return (
    <span
      className="tr-delta"
      data-tone={deltaTone(deltaDirection(value), metric.better)}
    >
      {value === 0 ? (
        'no change'
      ) : (
        <DeltaValue
          value={value}
          format={deltaFormat(metric)}
          betterWhen={metric.better}
        />
      )}
    </span>
  )
}

/** Which way the measure is better, with its arrow. */
function Direction({ better }: { better: TrendMetric['better'] }) {
  if (better === 'neither') return null
  const Arrow = better === 'higher' ? ArrowUp : ArrowDown
  return (
    <span className="tr-direction">
      <Arrow size={12} aria-hidden="true" />
      {better} is better
    </span>
  )
}

/** The value of a measure on the execution compared (the picked one, else the
 *  latest) and its delta against the baseline, else the execution before. */
function Latest({
  metric,
  points,
  selected,
  baseline,
}: {
  metric: TrendMetric
  points: TrendPoint[]
  selected: number
  baseline: number
}) {
  const { current, previous } = comparedPair(points, metric, selected, baseline)
  const value = current ? metric.value(current) : null
  return (
    <>
      <span className="tr-latest">
        {current && value !== null ? metric.figure(value, current) : '—'}
      </span>
      <DeltaPill
        metric={metric}
        value={current && previous ? deltaOf(metric, current, previous) : null}
      />
    </>
  )
}

/** Which executions the value and its delta are, said under them. */
function Reference({
  metric,
  points,
  selected,
  baseline,
}: {
  metric: TrendMetric
  points: TrendPoint[]
  selected: number
  baseline: number
}) {
  const text = referenceText(comparedPair(points, metric, selected, baseline))
  return text ? <p className="tr-footnote tr-reference">{text}</p> : null
}

const LARGE = { top: 52, bottom: 22, gutter: 52, right: 10 }
const SMALL = { top: 8, bottom: 20, gutter: 46, right: 8 }
const BASELINE_LABEL = { w: 60, h: 16 }

/** The plot: one step per execution, a line through the counted ones (faint
 *  across those without), dashed where something major changed, a solid
 *  rule on the picked one and a heavier one on the baseline. The large plot
 *  carries the lane of diamonds and its points are buttons. */
function Plot({
  metric,
  points,
  changes,
  selected,
  baseline,
  height,
  onPick,
}: {
  metric: TrendMetric
  points: TrendPoint[]
  changes: TrendChange[][]
  selected: number
  baseline: number
  height: number
  /** Only the large chart picks. */
  onPick?: (index: number) => void
}) {
  const large = Boolean(onPick)
  const { ref, width } = useMeasuredWidth(large ? 900 : 360)
  const g = large ? LARGE : SMALL
  const plotH = height - g.top - g.bottom
  const plotW = Math.max(40, width - g.gutter - g.right)
  const bottom = g.top + plotH
  const values = points.map((point) => metric.value(point))
  const known = values.filter((value): value is number => value !== null)
  if (known.length === 0)
    return <div ref={ref} className="tr-plot" style={{ height }} />
  const [lo, hi] = domain(metric, known, points)
  const xAt = (index: number) => g.gutter + slotX(index, points.length) * plotW
  const yAt = (value: number) => g.top + (1 - (value - lo) / (hi - lo)) * plotH
  let labelled = 0
  const lane = large
    ? points.flatMap((point, index) => {
        const list = changes[index]
        if (list.length === 0) return []
        const label = laneLabel(list)
        const row = label ? labelled++ % 2 : 0
        return [{ index, point, list, label, row }]
      })
    : []
  return (
    <div ref={ref} className="tr-plot" style={{ height }}>
      <svg
        className="tr-svg"
        width={width}
        height={height}
        viewBox={`0 0 ${width} ${height}`}
        aria-hidden="true"
      >
        {[hi, (lo + hi) / 2, lo].map((value, row) => {
          const y = g.top + (row * plotH) / 2
          return (
            <g key={value}>
              <line
                className="tr-gridline"
                x1={g.gutter}
                x2={width - g.right}
                y1={y}
                y2={y}
              />
              <text
                className="tr-axis"
                x={g.gutter - 8}
                y={y + 4}
                textAnchor="end"
              >
                {metric.axis(value)}
              </text>
            </g>
          )
        })}
        {points.map((point, index) =>
          changes[index].some((change) => change.major) ? (
            <line
              key={point.execution_id}
              className="tr-major"
              x1={xAt(index)}
              x2={xAt(index)}
              y1={large ? g.top - 12 : g.top}
              y2={bottom}
            />
          ) : null,
        )}
        {selected >= 0 && selected < points.length ? (
          <line
            className="tr-cross"
            x1={xAt(selected)}
            x2={xAt(selected)}
            y1={g.top}
            y2={bottom}
          />
        ) : null}
        {baseline >= 0 && baseline < points.length ? (
          <line
            className="tr-baseline"
            data-large={large || undefined}
            x1={xAt(baseline)}
            x2={xAt(baseline)}
            y1={large ? g.top - 12 : g.top}
            y2={bottom}
          />
        ) : null}
        {segments(values).map((segment) => (
          <line
            key={segment.from}
            className="tr-line"
            data-large={large || undefined}
            data-faded={segment.faded || undefined}
            x1={xAt(segment.from)}
            y1={yAt(values[segment.from] as number)}
            x2={xAt(segment.to)}
            y2={yAt(values[segment.to] as number)}
          />
        ))}
        {large && baseline >= 0 && baseline < points.length ? (
          <g
            className="tr-baseline-label"
            transform={`translate(${xAt(baseline) + (xAt(baseline) > width * 0.78 ? -BASELINE_LABEL.w - 6 : 6)} ${bottom - BASELINE_LABEL.h - 2})`}
          >
            <rect width={BASELINE_LABEL.w} height={BASELINE_LABEL.h} rx={4} />
            <text
              x={BASELINE_LABEL.w / 2}
              y={BASELINE_LABEL.h - 4}
              textAnchor="middle"
            >
              baseline
            </text>
          </g>
        ) : null}
        {roomyMarks(dayMarks(points), (mark) => xAt(mark.index), 56).map(
          (mark) => (
            <text
              key={mark.index}
              className="tr-axis"
              x={xAt(mark.index)}
              y={bottom + 15}
              textAnchor="middle"
            >
              {mark.text}
            </text>
          ),
        )}
        {lane.map((item) =>
          item.label ? (
            <text
              key={item.point.execution_id}
              className="tr-axis"
              x={xAt(item.index) + (xAt(item.index) > width * 0.78 ? -8 : 8)}
              y={item.row ? 26 : 12}
              textAnchor={xAt(item.index) > width * 0.78 ? 'end' : 'start'}
            >
              {item.label}
            </text>
          ) : null,
        )}
        {large
          ? null
          : points.map((point, index) => {
              const value = values[index]
              const picked = index === selected
              // A cross drawn as two strokes, above the floor line.
              const x = xAt(index)
              const y = bottom - 5
              // Counted but not measured: a gap in the line, no mark.
              if (value === null && counted(point)) return null
              return value === null ? (
                <g key={point.execution_id} className="tr-miss-mark">
                  <line x1={x - 3} y1={y - 3} x2={x + 3} y2={y + 3} />
                  <line x1={x - 3} y1={y + 3} x2={x + 3} y2={y - 3} />
                </g>
              ) : (
                <circle
                  key={point.execution_id}
                  className="tr-dot"
                  data-selected={picked || undefined}
                  cx={xAt(index)}
                  cy={yAt(value)}
                  r={picked ? 4.5 : 3}
                />
              )
            })}
      </svg>
      {lane.map((item) => (
        <button
          key={item.point.execution_id}
          type="button"
          className="tr-diamond"
          data-major={item.list.some((change) => change.major)}
          data-selected={item.index === selected || undefined}
          aria-pressed={item.index === selected}
          aria-label={`What changed · ${pointTime(item.point)}: ${item.list.map((change) => change.name).join(', ')}`}
          title={`What changed · ${pointTime(item.point)}: ${item.list.map((change) => change.name).join(', ')}`}
          style={{ left: xAt(item.index), top: g.top - 19 }}
          onClick={() => onPick?.(item.index)}
        />
      ))}
      {large
        ? points.map((point, index) => {
            const value = values[index]
            // Only an execution without a counted run gets the cross; one
            // that counted but did not measure this leaves a gap.
            if (value === null && counted(point)) return null
            const label = `${pointTime(point)} · ${metric.label} ${value === null ? 'no counted run' : metric.figure(value, point)}`
            return (
              <button
                key={point.execution_id}
                type="button"
                className={value === null ? 'tr-missed' : 'tr-point'}
                data-selected={index === selected || undefined}
                aria-pressed={index === selected}
                aria-label={label}
                title={label}
                style={{
                  left: xAt(index),
                  // Above the floor, clear of the day labels under it.
                  top: value === null ? bottom - 11 : yAt(value),
                }}
                onClick={() => onPick?.(index)}
              >
                {value === null ? <X size={12} aria-hidden="true" /> : null}
              </button>
            )
          })
        : null}
    </div>
  )
}

/** The large chart: one measure, the lane of what changed, pickable. */
export function LargeChart({
  metric,
  points,
  changes,
  selected,
  baseline,
  narrow,
  onPick,
}: {
  metric: TrendMetric
  points: TrendPoint[]
  changes: TrendChange[][]
  selected: number
  baseline: number
  narrow: boolean
  onPick: (index: number) => void
}) {
  return (
    <section
      className="tr-card tr-big"
      aria-labelledby="tr-big"
      data-trend-chart={metric.id}
    >
      <div className="tr-card-head">
        <h2 id="tr-big" className="tr-h2">
          {metric.label}
        </h2>
        <span className="tr-tag">
          <Direction better={metric.better} />
        </span>
        <span className="tr-faint">{metric.note}</span>
        <span className="tr-spacer" />
        <Latest
          metric={metric}
          points={points}
          selected={selected}
          baseline={baseline}
        />
      </div>
      <Reference
        metric={metric}
        points={points}
        selected={selected}
        baseline={baseline}
      />
      <Plot
        metric={metric}
        points={points}
        changes={changes}
        selected={selected}
        baseline={baseline}
        height={narrow ? 270 : 320}
        onPick={onPick}
      />
      <p className="tr-footnote">
        Pick a point or a diamond to see that execution and what changed. One
        step per execution; the axis marks where a day starts.
      </p>
    </section>
  )
}

/** A small chart; its title puts its measure in the large chart. */
export function SmallChart({
  metric,
  points,
  changes,
  selected,
  baseline,
  narrow,
  onFocus,
}: {
  metric: TrendMetric
  points: TrendPoint[]
  changes: TrendChange[][]
  selected: number
  baseline: number
  narrow: boolean
  onFocus: () => void
}) {
  return (
    <div className="tr-card tr-mini" data-trend-mini={metric.id}>
      <div className="tr-card-head">
        <button
          type="button"
          className="tr-mini-title"
          title={`Show ${metric.label} in the large chart`}
          onClick={onFocus}
        >
          {metric.label}
          <Maximize2 size={14} aria-hidden="true" />
        </button>
        <span className="tr-spacer" />
        <Latest
          metric={metric}
          points={points}
          selected={selected}
          baseline={baseline}
        />
      </div>
      <Reference
        metric={metric}
        points={points}
        selected={selected}
        baseline={baseline}
      />
      <p className="tr-footnote">
        <Direction better={metric.better} /> · {metric.note}
      </p>
      <Plot
        metric={metric}
        points={points}
        changes={changes}
        selected={selected}
        baseline={baseline}
        height={narrow ? 150 : 132}
      />
    </div>
  )
}
