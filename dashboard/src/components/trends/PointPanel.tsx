import { ArrowUpRight, X } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { InvestigationAction } from '@/components/InvestigationAction'
import { buttonClassName } from '@/design-system'
import {
  hashForComparison,
  hashForExecution,
  hashFrom,
} from '@/hooks/use-hash-route'
import type { DashboardDataBridge } from '@/lib/dashboard-data-source'
import { plural } from '@/lib/format'
import type { Investigation } from '@/lib/investigation'
import {
  baselineId,
  CHANGE_KIND_TEXT,
  changesBetween,
  commitsLinkText,
  compareKey,
  counted,
  deltaOf,
  majorsFirst,
  mixesSeries,
  notRun,
  pointTime,
  releaseControlId,
  seriesText,
  sourceText,
  TREND_METRICS,
  type TrendChange,
  type TrendPoint,
  type VersionCompareResponse,
  versionsText,
} from '@/lib/trends'
import { DeltaPill } from './TrendsChart'

type Lookup = VersionCompareResponse | 'pending' | 'failed'

/** The other changes shown before the rest fold behind a button. */
const SHOWN_MINORS = 5

/** version-compare for each change that names a range, asked once the panel
 *  shows it. */
function useCommitLookups(
  bridge: DashboardDataBridge | null,
  changes: TrendChange[],
) {
  const [found, setFound] = useState<Record<string, Lookup>>({})
  // A range is asked once while the panel is open; one that failed is asked
  // again the next time it is shown.
  const asked = useRef(new Set<string>())
  const open = useRef(true)
  useEffect(() => {
    open.current = true
    return () => {
      open.current = false
    }
  }, [])
  useEffect(() => {
    if (!bridge) return
    for (const change of changes) {
      const request = change.compare
      if (!request) continue
      const key = compareKey(request)
      if (asked.current.has(key)) continue
      asked.current.add(key)
      setFound((current) => ({ ...current, [key]: 'pending' }))
      bridge
        .compareVersions(request)
        .then((answer) => {
          if (open.current)
            setFound((current) => ({ ...current, [key]: answer }))
        })
        .catch(() => {
          asked.current.delete(key)
          if (open.current)
            setFound((current) => ({ ...current, [key]: 'failed' }))
        })
    }
  }, [bridge, changes])
  return found
}

function CommitsLink({
  change,
  lookup,
}: {
  change: TrendChange
  lookup: Lookup | undefined
}) {
  if (!change.compare) return null
  if (lookup === undefined || lookup === 'pending')
    return <span className="tr-faint">Looking up the commits…</span>
  if (lookup === 'failed')
    return <span className="tr-faint">The commits could not be looked up.</span>
  return (
    <a
      className="tr-link"
      href={lookup.url}
      target="_blank"
      rel="noreferrer"
      data-commits={lookup.total_commits ?? undefined}
    >
      {commitsLinkText(change, lookup.total_commits)}
      <ArrowUpRight size={12} aria-hidden="true" />
    </a>
  )
}

/** The chat reads the point against its reference, the reference as A,
 *  with what changed between them. */
export function pointInvestigation(
  points: TrendPoint[],
  reference: TrendPoint,
  index: number,
): Investigation {
  const from = points.findIndex(
    (item) => item.execution_id === reference.execution_id,
  )
  return {
    executionId: reference.execution_id,
    comparisonExecutionId: points[index].execution_id,
    changes: changesBetween(points, from, index).map((change) => ({
      what: `${change.name} (${CHANGE_KIND_TEXT[change.kind]})`,
      change: change.text,
      commits: change.compare,
    })),
  }
}

/** One execution of the trend: what changed since the one before it, its
 *  measures against the previous counted one, and where to go from here.
 *  With a baseline pinned, all of it reads against the baseline instead. */
export function PointPanel({
  points,
  index,
  changes,
  previous,
  baseline,
  bridge,
  here,
  onClose,
  onBaseline,
}: {
  points: TrendPoint[]
  index: number
  changes: TrendChange[]
  /** The previous execution with a counted run. */
  previous: TrendPoint | null
  /** The pinned baseline, when this view shows it. */
  baseline: TrendPoint | null
  bridge: DashboardDataBridge | null
  /** This view's hash, for Compare to come back to. */
  here: string
  onClose: () => void
  onBaseline: (id: string | null) => void
}) {
  const point = points[index]
  const isBaseline = baseline?.execution_id === point.execution_id
  const against = baseline && !isBaseline ? baseline : null
  const againstIndex = against
    ? points.findIndex((item) => item.execution_id === against.execution_id)
    : -1
  const before = index > 0 ? points[index - 1] : null
  // Unfolded only for the reference it was unfolded against.
  const referenceId = against?.execution_id ?? ''
  const [unfolded, setUnfolded] = useState<string | null>(null)
  const showAll = unfolded === referenceId
  const listed = useMemo(() => {
    const all = against ? changesBetween(points, againstIndex, index) : changes
    const ordered = majorsFirst(all)
    const minors = ordered.filter((change) => !change.major)
    const hidden = showAll ? 0 : Math.max(0, minors.length - SHOWN_MINORS)
    return {
      shown: hidden ? ordered.slice(0, ordered.length - hidden) : ordered,
      hidden,
      total: all.length,
    }
  }, [against, againstIndex, changes, index, points, showAll])
  const lookups = useCommitLookups(bridge, listed.shown)
  const missing = notRun(point)
  const rc = releaseControlId(point)
  // A day or a release: several executions read together.
  const group = point.group
  const source = (
    group
      ? [
          plural(group.members.length, 'execution'),
          plural(point.counted, 'counted run'),
        ]
      : [
          sourceText(point),
          rc ? `Release Control ${rc}` : null,
          mixesSeries(points) ? seriesText(point) : null,
          point.label,
        ]
  )
    .filter(Boolean)
    .join(' · ')
  // What the measures and Compare read against. Compare takes two
  // executions, not two groups.
  const reference = against ?? previous
  const canCompare = !group && reference !== null && counted(point)
  // The comparison opens with the earlier execution as A.
  const [earlier, later] =
    againstIndex > index ? [point, reference] : [reference, point]
  return (
    <aside
      className="tr-card tr-panel"
      aria-labelledby="tr-pn"
      data-trend-panel={point.execution_id}
    >
      <div className="tr-panel-head">
        <div className="tr-panel-title">
          <div className="tr-panel-heading">
            <h2 id="tr-pn" className="tr-h2">
              {pointTime(point)}
            </h2>
            {isBaseline ? <span className="tr-tag">baseline</span> : null}
          </div>
          <span className="tr-mono tr-small tr-faint-ink">{source}</span>
        </div>
        <button
          type="button"
          className="tr-icon-button"
          aria-label="Close"
          onClick={onClose}
        >
          <X size={16} aria-hidden="true" />
        </button>
      </div>
      {counted(point) ? null : (
        <p role="status" className="tr-note" data-tone="alert">
          {point.reason ?? 'None of its runs is technically valid.'}
        </p>
      )}
      {group && group.mixed.length > 0 ? (
        <p role="status" className="tr-note" data-tone="warn">
          Its executions did not all run the same versions and tests (
          {group.mixed.map((change) => change.name).join(', ')}), so its
          measures mix them.
        </p>
      ) : null}
      {counted(point) && missing.length > 0 ? (
        <p role="status" className="tr-note" data-tone="warn">
          Planned but not run: {missing.join(', ')}. They count as not
          completed.
        </p>
      ) : null}
      <section className="tr-panel-section" aria-labelledby="tr-ch">
        <h3 id="tr-ch" className="tr-h3">
          {against
            ? `What changed between ${pointTime(against)} (baseline) and ${pointTime(point)}`
            : before
              ? `What changed since ${pointTime(before)}`
              : 'The first execution in this view'}
        </h3>
        {listed.total === 0 ? (
          <p className="tr-faint">
            {against || before
              ? 'Nothing recorded changed: same iii, stack, workers and test definitions.'
              : 'There is nothing before it to compare with.'}
          </p>
        ) : (
          <ul className="tr-changes">
            {listed.shown.map((change) => (
              <li
                key={`${change.kind}:${change.name}`}
                data-change={change.kind}
              >
                <span className="tr-change-head">
                  <span
                    className="tr-change-dot"
                    data-major={change.major}
                    aria-hidden="true"
                  />
                  <span className="tr-mono tr-strong">{change.name}</span>
                  <span className="tr-faint">
                    {CHANGE_KIND_TEXT[change.kind]}
                  </span>
                </span>
                <span className="tr-mono tr-small tr-wrap">{change.text}</span>
                <CommitsLink
                  change={change}
                  lookup={
                    change.compare
                      ? lookups[compareKey(change.compare)]
                      : undefined
                  }
                />
                {change.note ? (
                  <span className="tr-faint">{change.note}</span>
                ) : null}
              </li>
            ))}
          </ul>
        )}
        {listed.hidden > 0 || showAll ? (
          <button
            type="button"
            className={buttonClassName({ variant: 'quiet', size: 'compact' })}
            aria-expanded={showAll}
            onClick={() => setUnfolded(showAll ? null : referenceId)}
          >
            {showAll ? 'Show fewer' : `Show ${listed.hidden} more`}
          </button>
        ) : null}
      </section>
      {group ? (
        <section className="tr-panel-section" aria-labelledby="tr-mb">
          <h3 id="tr-mb" className="tr-h3">
            Its executions
          </h3>
          <ul className="tr-members">
            {[...group.members].reverse().map((member) => {
              const score = TREND_METRICS[0].value(member)
              return (
                <li key={member.execution_id}>
                  <a
                    className="tr-inline-link"
                    href={hashForExecution(member.execution_id)}
                  >
                    {pointTime(member)}
                  </a>
                  <span className="tr-mono tr-small tr-faint-ink">
                    {!counted(member)
                      ? 'no counted run'
                      : [
                          score === null
                            ? null
                            : `score ${TREND_METRICS[0].figure(score, member)}`,
                          plural(member.counted, 'counted run'),
                          group.by === 'day' && member.workers?.harness
                            ? `harness ${member.workers.harness}`
                            : null,
                        ]
                          .filter(Boolean)
                          .join(' · ')}
                  </span>
                </li>
              )
            })}
          </ul>
        </section>
      ) : null}
      {counted(point) ? (
        <section className="tr-panel-section" aria-labelledby="tr-mx">
          <h3 id="tr-mx" className="tr-h3">
            {against
              ? `Against the baseline, ${pointTime(against)}`
              : previous
                ? `Against ${pointTime(previous)}`
                : 'Measures'}
          </h3>
          <dl className="tr-measures">
            {TREND_METRICS.map((metric) => {
              const value = metric.value(point)
              return (
                <div key={metric.id}>
                  <dt>{metric.label}</dt>
                  <dd className="tr-mono">
                    {value === null ? '—' : metric.figure(value, point)}
                  </dd>
                  <dd>
                    <DeltaPill
                      metric={metric}
                      value={
                        reference ? deltaOf(metric, point, reference) : null
                      }
                    />
                  </dd>
                </div>
              )
            })}
          </dl>
        </section>
      ) : null}
      <p className="tr-mono tr-small tr-faint-ink tr-wrap">
        {group ? 'Its last execution: ' : null}
        {versionsText(point)}
      </p>
      {point.workers ? null : (
        <p className="tr-faint">
          Worker versions: not recorded. This execution has no compose lock, and
          the version a worker reports to the engine is not the release that
          ran.
        </p>
      )}
      <div className="tr-actions">
        {group ? null : (
          <a
            className={buttonClassName({
              variant: 'secondary',
              className: 'no-underline',
            })}
            href={hashForExecution(point.execution_id)}
          >
            Open execution
          </a>
        )}
        {canCompare && reference && earlier && later ? (
          <a
            className={buttonClassName({
              variant: 'secondary',
              className: 'no-underline',
            })}
            href={hashFrom(
              hashForComparison(earlier.execution_id, later.execution_id),
              here,
            )}
          >
            {against
              ? 'Compare with the baseline'
              : `Compare with ${pointTime(reference)}`}
          </a>
        ) : null}
        {canCompare && reference ? (
          <InvestigationAction
            {...pointInvestigation(points, reference, index)}
          />
        ) : null}
        {counted(point) ? (
          <button
            type="button"
            className={buttonClassName({ variant: 'secondary' })}
            onClick={() => onBaseline(isBaseline ? null : baselineId(point))}
          >
            {isBaseline ? 'Clear baseline' : 'Set as baseline'}
          </button>
        ) : null}
      </div>
    </aside>
  )
}
