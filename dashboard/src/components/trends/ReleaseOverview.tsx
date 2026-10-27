import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
  SegmentedControl,
} from '@iii-dev/console-ui'
import { ChevronDown, ChevronRight } from 'lucide-react'
import { type ReactNode, useMemo, useState } from 'react'
import { DisclosureLayer } from '@/components/DisclosureLayer'
import { buttonClassName } from '@/design-system'
import {
  hashForExecution,
  hashForTrends,
  hashForWorkspace,
  hashWithParams,
} from '@/hooks/use-hash-route'
import type { DashboardDataBridge } from '@/lib/dashboard-data-source'
import { formatDateTime, plural } from '@/lib/format'
import {
  alsoChangedText,
  type CombinationLine,
  type CombinationPoint,
  combinationKey,
  combinationLine,
  combinationOptions,
  combinationText,
  comparedDelta,
  defaultCombination,
  isCompared,
  leftOutText,
  MAIN_METRICS,
  type ReleaseOverview as Overview,
  overviewMetric,
  type ReleasePoint,
  stepItem,
  stepText,
  testReasonText,
  uncalibratedText,
  VERDICT_METRICS,
} from '@/lib/release-overview'
import {
  releaseText,
  TREND_METRICS,
  type TrendMetricId,
  trendMetric,
} from '@/lib/trends'
import { repetitionsText, verdictHeadline, verdictTone } from '@/lib/verdict'
import { PointPanel } from './PointPanel'
import { DeltaPill, Direction, Plot } from './TrendsChart'
import { VerdictHeadline } from './VerdictHeadline'

const MORE_METRICS = TREND_METRICS.filter((metric) => metric.tier === 'more')
const PANEL_METRICS = TREND_METRICS.map(overviewMetric)

/** What a first execution did not measure: `3 runs failed in the E2E
 *  infrastructure · 1 run where the harness failed · …`, each left out at
 *  0. */
function coverageParts(first: CombinationPoint | undefined) {
  if (!first) return []
  return [
    first.infra_failed_runs > 0
      ? `${plural(first.infra_failed_runs, 'run')} failed in the E2E infrastructure`
      : null,
    first.harness_failed_runs > 0
      ? `${plural(first.harness_failed_runs, 'run')} where the harness failed`
      : null,
    first.not_run > 0
      ? `${plural(first.not_run, 'planned run')} not run`
      : null,
  ].filter((part): part is string => part !== null)
}

const firstOf = (release: ReleasePoint, key: string) =>
  release.firsts.find((first) => combinationKey(first.combination) === key)

/** The verdict's step setup by setup: its calibrated verdict or why there
 *  is none, the deltas, what each side counted, what was left out and why,
 *  and both executions; then the setups left out. */
function Combinations({
  overview,
  release,
}: {
  overview: Overview
  release: ReleasePoint
}) {
  const step = release.step
  if (!step) return null
  const latest = releaseText(release.release)
  const previous = releaseText(step.previous)
  return (
    <section className="tr-panel-section" aria-labelledby="tr-cb">
      <h3 id="tr-cb" className="tr-h3">
        By suite, model, profile and stack
      </h3>
      <ul className="tr-changes">
        {step.compared.map((item) => {
          const summary = item.calibrated
          const reasons = new Map<string, string[]>()
          for (const test of item.tests_left_out) {
            const reason = testReasonText(
              test.reason,
              release.release,
              step.previous,
            )
            reasons.set(reason, [...(reasons.get(reason) ?? []), test.test])
          }
          const coverage = coverageParts(
            firstOf(release, combinationKey(item.combination)),
          )
          const also = alsoChangedText(item.also_changed)
          return (
            <li key={combinationKey(item.combination)} data-combination>
              <span className="tr-mono tr-small tr-strong tr-wrap">
                {combinationText(item.combination)}
              </span>
              {summary?.verdict ? (
                <>
                  <span
                    className="tr-move tr-strong"
                    data-verdict={summary.verdict}
                    data-tone={verdictTone(summary)}
                  >
                    {verdictHeadline(summary)}
                  </span>
                  {summary.reasons.map((reason) => (
                    <span key={reason} className="tr-faint">
                      {reason}
                    </span>
                  ))}
                  <span className="tr-faint">{repetitionsText(summary)}</span>
                </>
              ) : (
                <span className="tr-faint">
                  {`No calibrated verdict: ${uncalibratedText(item)}`}
                </span>
              )}
              {summary?.warnings.map((warning) => (
                <span
                  key={warning}
                  className="tr-note"
                  data-tone="warn"
                  data-verdict-warning
                >
                  {warning}
                </span>
              ))}
              <span className="tr-move" data-combination-deltas>
                {VERDICT_METRICS.map((metric) => (
                  <span key={metric.id} className="tr-move">
                    {metric.label}{' '}
                    <DeltaPill
                      metric={metric}
                      value={comparedDelta(metric, item)}
                    />
                  </span>
                ))}
              </span>
              <span className="tr-faint">
                {`Counted runs: ${item.latest.counted_runs} on ${latest}, ${item.previous.counted_runs} on ${previous} · ${plural(item.compared_tests, 'compared test')}`}
              </span>
              {coverage.length > 0 ? (
                <span className="tr-faint">{`On ${latest}: ${coverage.join(' · ')}`}</span>
              ) : null}
              {reasons.size > 0 ? (
                <span className="tr-faint" data-tests-left-out>
                  {`Left out: ${[...reasons]
                    .map(([reason, tests]) => `${tests.join(', ')} (${reason})`)
                    .join('; ')}`}
                </span>
              ) : null}
              {also ? <span className="tr-faint">{also}</span> : null}
              <span className="tr-move">
                <a
                  className="tr-inline-link"
                  href={hashForExecution(item.latest.execution)}
                >
                  {`${latest} execution`}
                </a>
                <a
                  className="tr-inline-link"
                  href={hashForExecution(item.previous.execution)}
                >
                  {`${previous} execution`}
                </a>
              </span>
            </li>
          )
        })}
        {step.left_out.map((item) => (
          <li key={combinationKey(item.combination)} data-left-out>
            <span className="tr-mono tr-small tr-strong tr-wrap">
              {combinationText(item.combination)}
            </span>
            <span className="tr-faint">
              {`Not compared: ${leftOutText(overview, item, release.release, step.previous)}`}
            </span>
          </li>
        ))}
      </ul>
    </section>
  )
}

/** A point of a setup's line, opened. A local run reads on its own; a
 *  release reads as its first completed execution, with what changed since
 *  the release before it, its coverage and, for the verdict's, each
 *  setup. */
export function ReleasePanel({
  overview,
  line,
  index,
  setup,
  bridge,
  onClose,
}: {
  overview: Overview
  line: CombinationLine
  index: number
  /** The setup's key. */
  setup: string
  bridge: DashboardDataBridge | null
  onClose: () => void
}) {
  const point = line.points[index]
  const release =
    point.group?.by === 'release'
      ? overview.releases.find((item) => item.release === point.release)
      : undefined
  if (!release)
    return (
      <PointPanel
        points={[point]}
        index={0}
        changes={null}
        previous={null}
        baseline={null}
        bridge={bridge}
        here={hashForTrends()}
        onClose={onClose}
      />
    )
  const releases = line.points.filter((item) => item.group?.by === 'release')
  return (
    <PointPanel
      points={releases}
      index={releases.indexOf(point)}
      changes={line.changes[index]}
      previous={null}
      baseline={null}
      bridge={bridge}
      here={hashForTrends()}
      onClose={onClose}
      versions={false}
      subtitle={[
        plural(release.executions.length, 'Release Control execution'),
        ...coverageParts(firstOf(release, setup)),
      ].join(' · ')}
      metrics={PANEL_METRICS}
      notRunText="They are left out of Tests completed."
    >
      {overview.verdict?.latest === release.release ? (
        <Combinations overview={overview} release={release} />
      ) : null}
    </PointPanel>
  )
}

/** Trends' first screen: the verdict, the metric tabs and the Setup picker,
 *  the chosen setup's line with its local runs beside it, the latest
 *  release's step for it, the CI runs outside Release Control, `options`,
 *  then the latest release's executions. */
export function ReleaseOverview({
  overview,
  metric,
  onMetric,
  bridge,
  narrow,
  options,
  empty,
  status,
  initialCombination,
  now = new Date(),
}: {
  overview: Overview
  metric: TrendMetricId
  onMetric: (id: TrendMetricId) => void
  bridge: DashboardDataBridge | null
  narrow: boolean
  /** Between the outside strip and Details. */
  options?: ReactNode
  /** In place of the tabs, the line and the callout while no release has
   *  results. */
  empty?: ReactNode
  /** Beside the verdict's heading: the sync status. */
  status?: ReactNode
  /** The setup to open on, by its key; else the one that decides. */
  initialCombination?: string
  now?: Date
}) {
  const last = overview.releases.at(-1)
  const fallback = defaultCombination(overview)
  const fallbackKey = fallback ? combinationKey(fallback) : ''
  // A setup the reader chose holds until another release is the latest;
  // until then the one that decides follows each reload.
  const [chosen, setChosen] = useState<{
    release: string | undefined
    key: string | null
  }>({ release: last?.release, key: initialCombination ?? null })
  const setup =
    chosen.release === last?.release && chosen.key !== null
      ? chosen.key
      : fallbackKey
  const choose = (key: string) => setChosen({ release: last?.release, key })
  const line = useMemo(
    () => combinationLine(overview, setup),
    [overview, setup],
  )
  // Every point is a release: a diamond for the Harness changing would sit
  // on every step. The panel still lists it.
  const marks = useMemo(
    () =>
      line.changes.map((changes) =>
        changes.filter((change) => change.kind !== 'harness'),
      ),
    [line],
  )
  const setups = combinationOptions(overview)
  const current = setups.find((item) => combinationKey(item) === setup)
  // The opened point, by id, so a reload keeps it while it is on the line.
  const [picked, setPicked] = useState<string | null>(null)
  const shown = overviewMetric(trendMetric(metric))
  const more = MORE_METRICS.find((item) => item.id === metric)
  const selected = line.points.findIndex(
    (point) => point.execution_id === picked,
  )
  const item = last?.step ? stepItem(last.step, setup) : null
  const outside = overview.outside_release_control
  const pick = (index: number) => {
    const id = line.points[index]?.execution_id ?? null
    setPicked((was) => (was === id ? null : id))
  }
  const open = (key: string) => {
    choose(key)
    setPicked(last ? (firstOf(last, key)?.point.execution_id ?? null) : null)
  }
  return (
    <div className="tr-overview">
      <VerdictHeadline
        overview={overview}
        onOpen={open}
        status={status}
        now={now}
      />
      {last ? (
        <>
          <div className="tr-tabs">
            <SegmentedControl
              variant="radio"
              aria-label="Metric"
              className="tr-segments"
              value={metric}
              onChange={onMetric}
              options={MAIN_METRICS.map((item) => ({
                value: item.id,
                label: item.label,
              }))}
            />
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  className="tr-control"
                  aria-label={
                    more ? `More metrics: ${more.label}` : 'More metrics'
                  }
                  data-picker="more-metrics"
                  data-active={more ? true : undefined}
                >
                  <span>{more ? more.label : 'More'}</span>
                  <ChevronDown
                    size={16}
                    aria-hidden="true"
                    className="tr-faint-ink"
                  />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent
                align="start"
                aria-label="More metrics"
                className="tr-menu tr-period-menu"
              >
                <DropdownMenuRadioGroup
                  value={metric}
                  onValueChange={(value) => onMetric(value as TrendMetricId)}
                >
                  {MORE_METRICS.map((item) => (
                    <DropdownMenuRadioItem
                      key={item.id}
                      value={item.id}
                      className="tr-menu-item"
                    >
                      {item.label}
                    </DropdownMenuRadioItem>
                  ))}
                </DropdownMenuRadioGroup>
              </DropdownMenuContent>
            </DropdownMenu>
            {setups.length > 0 ? (
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <button
                    type="button"
                    className="tr-control"
                    aria-label={
                      current ? `Setup: ${combinationText(current)}` : 'Setup'
                    }
                    data-picker="setup"
                  >
                    <span>{current ? current.suite_label : 'Setup'}</span>
                    <ChevronDown
                      size={16}
                      aria-hidden="true"
                      className="tr-faint-ink"
                    />
                  </button>
                </DropdownMenuTrigger>
                <DropdownMenuContent
                  align="start"
                  aria-label="Setup"
                  className="tr-menu tr-period-menu"
                >
                  <DropdownMenuRadioGroup
                    value={setup}
                    onValueChange={(key) => {
                      choose(key)
                      setPicked(null)
                    }}
                  >
                    {setups.map((option) => (
                      <DropdownMenuRadioItem
                        key={combinationKey(option)}
                        value={combinationKey(option)}
                        className="tr-menu-item"
                      >
                        {combinationText(option)}
                      </DropdownMenuRadioItem>
                    ))}
                  </DropdownMenuRadioGroup>
                </DropdownMenuContent>
              </DropdownMenu>
            ) : null}
          </div>
          <div className="tr-top" data-panel={!narrow || undefined}>
            <section
              className="tr-card tr-big"
              aria-labelledby="tr-big"
              data-trend-chart={shown.id}
            >
              <div className="tr-card-head">
                <h2 id="tr-big" className="tr-h2">
                  {shown.label}
                </h2>
                <span className="tr-tag">
                  <Direction better={shown.better} />
                </span>
                <span className="tr-faint">{shown.note}</span>
              </div>
              <Plot
                metric={shown}
                points={line.points}
                changes={marks}
                selected={selected}
                baseline={-1}
                height={narrow ? 270 : 320}
                latest={line.latest}
                step={(index) => stepText(overview, shown, line.steps[index])}
                onPick={pick}
              />
              <p className="tr-footnote">
                Each point is this setup's first completed Release Control
                execution of that release, oldest first; a diamond marks a step
                that left tests out or changed what ran.
              </p>
              {overview.unversioned > 0 ? (
                <p className="tr-footnote">
                  {`${plural(overview.unversioned, 'Release Control execution')} recorded no single Harness release and ${overview.unversioned === 1 ? 'is' : 'are'} not on the line.`}
                </p>
              ) : null}
            </section>
            {selected >= 0 ? (
              <ReleasePanel
                key={picked}
                overview={overview}
                line={line}
                index={selected}
                setup={setup}
                bridge={bridge}
                onClose={() => setPicked(null)}
              />
            ) : (
              <aside
                className="tr-card tr-panel tr-callout"
                aria-labelledby="tr-callout"
              >
                <h2 id="tr-callout" className="tr-h2">
                  {`${releaseText(last.release)} · ${plural(last.executions.length, 'Release Control run')}`}
                </h2>
                {item && last.step && isCompared(item) ? (
                  <dl className="tr-measures">
                    {MAIN_METRICS.map((metric) => (
                      <div key={metric.id}>
                        <dt>{metric.label}</dt>
                        <dd>
                          <DeltaPill
                            metric={metric}
                            value={comparedDelta(metric, item)}
                          />
                        </dd>
                      </div>
                    ))}
                  </dl>
                ) : (
                  <p className="tr-faint" data-not-compared>
                    {item && last.step && !isCompared(item)
                      ? `Not compared: ${leftOutText(overview, item, last.release, last.step.previous)}`
                      : 'Nothing earlier to compare with.'}
                  </p>
                )}
                {line.latest >= 0 ? (
                  <div className="tr-actions">
                    <button
                      type="button"
                      className={buttonClassName({ variant: 'secondary' })}
                      onClick={() =>
                        setPicked(line.points[line.latest].execution_id)
                      }
                    >
                      What changed
                      <ChevronRight size={16} aria-hidden="true" />
                    </button>
                  </div>
                ) : null}
              </aside>
            )}
          </div>
          <ul className="tr-legend" aria-label="Legend">
            <li>
              <span className="tr-legend-dot" aria-hidden="true" />
              Release Control release
            </li>
            <li>
              <span className="tr-legend-ring" aria-hidden="true" />
              local run (this harness or Docker); it never counts in the verdict
            </li>
            <li>
              <span
                className="tr-change-dot"
                data-major="true"
                aria-hidden="true"
              />
              tests left out, or what ran changed
            </li>
          </ul>
        </>
      ) : (
        empty
      )}
      {outside.executions > 0 ? (
        <div className="tr-outside" data-outside>
          <span>
            {`CI runs outside Release Control · ${outside.executions}${outside.latest_at ? ` · latest ${formatDateTime(outside.latest_at, now)}` : ''}`}
          </span>
          <a
            className="tr-link"
            href={hashWithParams(
              hashForWorkspace('executions'),
              new URLSearchParams({ origin: 'outside-release-control' }),
            )}
          >
            See them
            <ChevronRight size={12} aria-hidden="true" />
          </a>
        </div>
      ) : null}
      {options}
      {last ? (
        <DisclosureLayer
          id="tr-details"
          label="Details"
          scent={`${plural(last.executions.length, 'Release Control execution')} of ${releaseText(last.release)}`}
          open={false}
        >
          <ul className="tr-members">
            {[...last.executions].reverse().map((id) => (
              <li key={id}>
                <a
                  className="tr-inline-link tr-mono"
                  href={hashForExecution(id)}
                >
                  {id}
                </a>
              </li>
            ))}
          </ul>
        </DisclosureLayer>
      ) : null}
    </div>
  )
}
