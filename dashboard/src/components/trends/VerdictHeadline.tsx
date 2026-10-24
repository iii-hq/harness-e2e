import { Fragment, type ReactNode } from 'react'
import { formatDayLabel } from '@/lib/format'
import {
  calibrated,
  combinationText,
  overviewMetric,
  type ReleaseOverview,
  scopeText,
  uncalibratedText,
  verdictMoves,
} from '@/lib/release-overview'
import { releaseText, TREND_METRICS } from '@/lib/trends'
import { verdictHeadline, verdictTone } from '@/lib/verdict'
import { DeltaPill } from './TrendsChart'

/** The main measures as the overview reads them. */
export const MAIN_METRICS = TREND_METRICS.filter(
  (metric) => metric.tier === 'main',
).map(overviewMetric)

/** Whether the latest Harness release did better or worse than the one
 *  before, as the calibrated verdict of each combination says; without one
 *  (one run per test), that there is none. The measures that moved follow,
 *  never called better or worse. */
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
  const summary = verdict ? verdictMoves(verdict, MAIN_METRICS) : null
  const read = verdict ? calibrated(verdict) : []
  // Nothing compared on both sides is no evidence that nothing moved.
  const compared = verdict ? verdict.compared.length > 0 : false
  const name = (index: number) =>
    verdict?.compared.length === 1
      ? read[index].combination.suite_label
      : combinationText(read[index].combination)
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
            {!compared
              ? null
              : read.length === 0
                ? `no calibrated verdict. ${uncalibratedText(verdict.compared[0])}`
                : read.map((item, index) => (
                    <Fragment key={combinationText(item.combination)}>
                      {index > 0 ? ' · ' : null}
                      <span
                        className="tr-move"
                        data-verdict={item.summary.verdict}
                        data-tone={verdictTone(item.summary)}
                      >
                        {`${name(index)}: ${verdictHeadline(item.summary).toLowerCase()}`}
                      </span>
                    </Fragment>
                  ))}
          </p>
          {compared ? (
            <p className="tr-faint" data-verdict-moves>
              {summary.moved.length === 0
                ? 'No measure moved.'
                : 'Moved, not read as better or worse: '}
              {summary.moved.map((move, index) => (
                <Fragment key={move.metric.id}>
                  {index > 0 ? ' · ' : null}
                  <span className="tr-move" data-move="moved">
                    {move.metric.label}{' '}
                    <DeltaPill metric={move.metric} value={move.delta} />
                  </span>
                </Fragment>
              ))}
            </p>
          ) : null}
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
