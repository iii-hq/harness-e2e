import { Fragment, type ReactNode } from 'react'
import { formatDayLabel } from '@/lib/format'
import {
  overviewMetric,
  type ReleaseOverview,
  scopeText,
  verdictSummary,
} from '@/lib/release-overview'
import { releaseText, TREND_METRICS } from '@/lib/trends'
import { DeltaPill } from './TrendsChart'

/** The main measures as the overview reads them. */
export const MAIN_METRICS = TREND_METRICS.filter(
  (metric) => metric.tier === 'main',
).map(overviewMetric)

/** Whether the latest Harness release did better or worse than the one
 *  before: the measures that moved, in their tone, and what was compared. */
export function VerdictHeadline({
  overview,
  status,
  now = new Date(),
}: {
  overview: ReleaseOverview
  /** Beside the heading: the sync status. */
  status?: ReactNode
  now?: Date
}) {
  const { verdict } = overview
  const last = overview.releases.at(-1)
  const summary = verdict ? verdictSummary(verdict, MAIN_METRICS) : null
  const moved = summary
    ? [...summary.better, ...summary.worse].sort(
        (one, two) =>
          MAIN_METRICS.indexOf(one.metric) - MAIN_METRICS.indexOf(two.metric),
      )
    : []
  // Nothing compared on both sides is no evidence that nothing moved.
  const compared = moved.length + (summary?.same.length ?? 0) > 0
  return (
    <section className="tr-verdict" aria-labelledby="tr-verdict">
      <div className="tr-verdict-head">
        <h2 id="tr-verdict" className="tr-h2">
          Harness evolution
        </h2>
        {status}
      </div>
      {verdict && summary ? (
        <>
          <p className="tr-verdict-line" data-verdict-line>
            <strong>{releaseText(verdict.latest)}</strong>
            {` (${formatDayLabel(verdict.latest_started_at, now)}) vs ${releaseText(verdict.previous)}${compared ? ': ' : '.'}`}
            {compared && moved.length === 0
              ? 'no measure moved'
              : moved.map((move, index) => (
                  <Fragment key={move.metric.id}>
                    {index > 0 ? ' · ' : null}
                    <span
                      className="tr-move"
                      data-move={
                        summary.better.includes(move) ? 'better' : 'worse'
                      }
                    >
                      {move.metric.label}{' '}
                      <DeltaPill metric={move.metric} value={move.delta} />
                    </span>
                  </Fragment>
                ))}
          </p>
          <p className="tr-faint">{scopeText(verdict)}</p>
        </>
      ) : (
        <p className="tr-verdict-line" data-verdict-line>
          {last
            ? `Only ${releaseText(last.release)} has Release Control results so far: a verdict needs two releases.`
            : 'No Release Control results yet.'}
        </p>
      )}
    </section>
  )
}
