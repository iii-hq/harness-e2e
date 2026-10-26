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
  type Aggregate,
  aggregateDelta,
  combinationText,
  type ReleaseOverview as Overview,
  type OverviewPoint,
  overviewMetric,
  stepText,
  type Timeline,
  timeline,
  uncalibratedText,
  type Verdict,
} from '@/lib/release-overview'
import {
  changesAt,
  releaseText,
  TREND_METRICS,
  type TrendMetricId,
  trendMetric,
} from '@/lib/trends'
import { repetitionsText, verdictHeadline, verdictTone } from '@/lib/verdict'
import { PointPanel } from './PointPanel'
import { DeltaPill, Direction, Plot } from './TrendsChart'
import { MAIN_METRICS } from './VerdictHeadline'

const MORE_METRICS = TREND_METRICS.filter((metric) => metric.tier === 'more')
const PANEL_METRICS = TREND_METRICS.map(overviewMetric)

/** The release a point of the line stands for; none for a local run. */
function releaseOf(overview: Overview, point: OverviewPoint | undefined) {
  return overview.releases.find(
    (release) => `release:${release.release}` === point?.execution_id,
  )
}

/** The verdict's combinations one by one: the calibrated verdict of each,
 *  or why it has none, and the tests left out of it. */
function Combinations({ verdict }: { verdict: Verdict }) {
  return (
    <section className="tr-panel-section" aria-labelledby="tr-cb">
      <h3 id="tr-cb" className="tr-h3">
        By suite, model, profile and stack
      </h3>
      <ul className="tr-changes">
        {verdict.compared.map((item) => {
          const summary = item.calibrated
          return (
            <li key={combinationText(item.combination)} data-combination>
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
              {item.tests_left_out.length > 0 ? (
                <span className="tr-faint">
                  {`Left out: ${item.tests_left_out.join(', ')}`}
                </span>
              ) : null}
            </li>
          )
        })}
      </ul>
    </section>
  )
}

/** A point of the release line, opened. A local run reads on its own; a
 *  release reads against the one before it over what both ran, with what
 *  changed between them and, for the verdict's, each combination. */
export function ReleasePanel({
  overview,
  line,
  index,
  bridge,
  onClose,
}: {
  overview: Overview
  line: Timeline
  index: number
  bridge: DashboardDataBridge | null
  onClose: () => void
}) {
  const point = line.points[index]
  const release = releaseOf(overview, point)
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
  const releases = line.points.filter((item) => item.source.kind === 'github')
  const at = releases.indexOf(point)
  const holding = (item: OverviewPoint, aggregate: Aggregate) => ({
    ...item,
    aggregate,
    measures: aggregate.measures,
  })
  // Both sides over the combinations they share, as the step reads them.
  const step =
    point.step?.latest_aggregate && point.step.previous_aggregate
      ? {
          latest: holding(point, point.step.latest_aggregate),
          previous: holding(releases[at - 1], point.step.previous_aggregate),
        }
      : null
  return (
    <PointPanel
      points={
        step
          ? releases.map((item) => (item === point ? step.latest : item))
          : releases
      }
      index={at}
      changes={[...changesAt(releases, at), ...line.changes[index]]}
      previous={step?.previous ?? null}
      baseline={null}
      bridge={bridge}
      here={hashForTrends()}
      onClose={onClose}
      versions={false}
      subtitle={[
        plural(release.executions.length, 'Release Control execution'),
        plural(release.combinations.length, 'combination'),
        release.tests_not_run > 0
          ? `${plural(release.tests_not_run, 'planned test')} did not run`
          : null,
      ]
        .filter(Boolean)
        .join(' · ')}
      metrics={PANEL_METRICS}
    >
      {overview.verdict?.latest === release.release ? (
        <Combinations verdict={overview.verdict} />
      ) : null}
    </PointPanel>
  )
}

/** Trends' first screen under the verdict: the metric tabs, the release line
 *  with the local runs beside it, the latest release's step, the CI runs
 *  outside Release Control, `options`, then the latest release's
 *  executions. */
export function ReleaseOverview({
  overview,
  metric,
  onMetric,
  bridge,
  narrow,
  options,
  empty,
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
  now?: Date
}) {
  const line = useMemo(() => timeline(overview), [overview])
  // The opened point, by id, so a reload keeps it while it is on the line.
  const [picked, setPicked] = useState<string | null>(null)
  const shown = overviewMetric(trendMetric(metric))
  const more = MORE_METRICS.find((item) => item.id === metric)
  const selected = line.points.findIndex(
    (point) => point.execution_id === picked,
  )
  const latest: OverviewPoint | undefined = line.points[line.latest]
  const last = releaseOf(overview, latest)
  // The latest step, when both of its sides were measured.
  const step = latest?.step
  const against =
    step?.latest_aggregate && step.previous_aggregate ? step : null
  const outside = overview.outside_release_control
  const pick = (index: number) => {
    const id = line.points[index]?.execution_id ?? null
    setPicked((current) => (current === id ? null : id))
  }
  return (
    <div className="tr-overview">
      {latest && last ? (
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
                changes={line.changes}
                selected={selected}
                baseline={-1}
                height={narrow ? 270 : 320}
                latest={line.latest}
                step={(index) => stepText(shown, line.points[index])}
                onPick={pick}
              />
              <p className="tr-footnote">
                One step per Harness release with Release Control results,
                oldest first. Each step reads against the release before it,
                over the suites, models, profiles and stacks both ran, each
                weighing the same. A hollow ring ran on this harness or in
                Docker, beside the release current when it started; it never
                counts in the verdict.
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
                {against ? (
                  <dl className="tr-measures">
                    {MAIN_METRICS.map((item) => (
                      <div key={item.id}>
                        <dt>{item.label}</dt>
                        <dd>
                          <DeltaPill
                            metric={item}
                            value={aggregateDelta(
                              item,
                              against.latest_aggregate,
                              against.previous_aggregate,
                            )}
                          />
                        </dd>
                      </div>
                    ))}
                  </dl>
                ) : (
                  <p className="tr-faint">
                    {step
                      ? `Nothing to compare with ${releaseText(step.previous)}.`
                      : 'Nothing earlier to compare with.'}
                  </p>
                )}
                <div className="tr-actions">
                  <button
                    type="button"
                    className={buttonClassName({ variant: 'secondary' })}
                    onClick={() => setPicked(latest.execution_id)}
                  >
                    What changed
                    <ChevronRight size={16} aria-hidden="true" />
                  </button>
                </div>
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
              local run (this harness or Docker)
            </li>
            <li>
              <span
                className="tr-change-dot"
                data-major="true"
                aria-hidden="true"
              />
              the suites, models, profiles or stacks that ran changed
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
