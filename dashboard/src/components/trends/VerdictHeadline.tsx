import { Fragment, type ReactNode } from 'react'
import { DisclosureLayer } from '@/components/DisclosureLayer'
import { formatDayLabel } from '@/lib/format'
import {
  alsoChangedText,
  COST_METRICS,
  type Combination,
  calibrated,
  combinationKey,
  combinationText,
  comparedDelta,
  coverageText,
  decidingCombination,
  latestStep,
  type ReleaseOverview,
  scopeText,
  stepItem,
  uncalibratedText,
  VERDICT_METRICS,
} from '@/lib/release-overview'
import { releaseText } from '@/lib/trends'
import { verdictHeadline, verdictTone } from '@/lib/verdict'
import { DeltaPill } from './TrendsChart'

/** Whether the latest Harness release did better or worse than the one
 *  before, as the calibrated verdict of each setup compared says; without
 *  one (one run per test), that there is none. Then what moved in the setup
 *  that decides, never called better or worse, and how much compared. */
export function VerdictHeadline({
  overview,
  onOpen,
  status,
  now = new Date(),
}: {
  overview: ReleaseOverview
  /** Opens the latest release's panel on a setup, by its key. */
  onOpen?: (key: string) => void
  /** Beside the heading: the sync status. */
  status?: ReactNode
  now?: Date
}) {
  const { verdict } = overview
  const last = overview.releases.at(-1)
  const step = latestStep(overview)
  const compared = step?.compared ?? []
  const read = step ? calibrated(step) : []
  const deciding = decidingCombination(overview)
  const decided =
    step && deciding ? stepItem(step, combinationKey(deciding)) : null
  const moved =
    decided && 'latest' in decided
      ? [...VERDICT_METRICS, ...COST_METRICS].flatMap((metric) => {
          const delta = comparedDelta(metric, decided)
          return delta ? [{ metric, delta }] : []
        })
      : []
  // A setup by its suite, unless two compared share that name.
  const name = (combination: Combination) =>
    compared.filter(
      (item) => item.combination.suite_label === combination.suite_label,
    ).length > 1
      ? combinationText(combination)
      : combination.suite_label
  const named = (combination: Combination) =>
    onOpen ? (
      <button
        type="button"
        className="tr-name-button"
        onClick={() => onOpen(combinationKey(combination))}
      >
        {name(combination)}
      </button>
    ) : (
      name(combination)
    )
  const coverage = coverageText(overview)
  const also = decided && 'latest' in decided ? decided.also_changed : []
  const scope = last ? scopeText(overview, last) : null
  return (
    <section className="tr-verdict" aria-labelledby="tr-verdict">
      <div className="tr-verdict-head">
        <h2 id="tr-verdict" className="tr-h2">
          Harness evolution
        </h2>
        {status}
      </div>
      {verdict && step ? (
        <>
          <p className="tr-verdict-line" data-verdict-line>
            <strong>{releaseText(verdict.latest)}</strong>
            {` (${formatDayLabel(verdict.latest_started_at, now)}) vs ${releaseText(verdict.previous)}${compared.length > 0 ? ': ' : '.'}`}
            {compared.length === 0
              ? null
              : read.length === 0
                ? `no calibrated verdict. ${uncalibratedText(compared[0])}`
                : read.map((item, index) => (
                    <Fragment key={combinationKey(item.combination)}>
                      {index > 0 ? ' · ' : null}
                      <span
                        className="tr-move"
                        data-verdict={item.summary.verdict}
                        data-tone={verdictTone(item.summary)}
                      >
                        {/* One child, so the gap does not part the name
                            from its colon. */}
                        <span>
                          {named(item.combination)}
                          {`: ${verdictHeadline(item.summary).toLowerCase()}`}
                        </span>
                      </span>
                    </Fragment>
                  ))}
          </p>
          {deciding ? (
            <p className="tr-faint" data-verdict-moves>
              {moved.length === 0
                ? `No measure moved in ${name(deciding)}.`
                : `Moved in ${name(deciding)}, not read as better or worse: `}
              {moved.map((move, index) => (
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
          <p className="tr-faint" data-verdict-coverage>
            {[coverage, alsoChangedText(also)].filter(Boolean).join(' · ')}
          </p>
          {scope ? (
            <DisclosureLayer
              id="tr-scope"
              label="Compared and left out"
              scent={null}
              open={false}
            >
              <p className="tr-faint">{scope}</p>
            </DisclosureLayer>
          ) : null}
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
