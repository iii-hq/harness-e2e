import { ArrowUpRight, X } from 'lucide-react'
import { useEffect, useState } from 'react'
import { buttonClassName } from '@/design-system'
import {
  hashForComparison,
  hashForExecution,
  hashFrom,
} from '@/hooks/use-hash-route'
import type { DashboardDataBridge } from '@/lib/dashboard-data-source'
import {
  CHANGE_KIND_TEXT,
  commitsLinkText,
  compareKey,
  counted,
  deltaOf,
  notRun,
  pointTime,
  releaseControlId,
  sourceText,
  TREND_METRICS,
  type TrendChange,
  type TrendPoint,
  type VersionCompareResponse,
  versionsText,
} from '@/lib/trends'
import { DeltaPill } from './TrendsChart'

type Lookup = VersionCompareResponse | 'pending' | 'failed'

/** version-compare for each change that names a range, asked once the panel
 *  shows it. */
function useCommitLookups(
  bridge: DashboardDataBridge | null,
  changes: TrendChange[],
) {
  const [found, setFound] = useState<Record<string, Lookup>>({})
  useEffect(() => {
    if (!bridge) return
    let cancelled = false
    for (const change of changes) {
      const request = change.compare
      if (!request) continue
      const key = compareKey(request)
      setFound((current) =>
        key in current ? current : { ...current, [key]: 'pending' },
      )
      bridge
        .compareVersions(request)
        .then((answer) => {
          if (!cancelled) setFound((current) => ({ ...current, [key]: answer }))
        })
        .catch(() => {
          if (!cancelled)
            setFound((current) => ({ ...current, [key]: 'failed' }))
        })
    }
    return () => {
      cancelled = true
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

/** One execution of the trend: what changed since the one before it, its
 *  measures against the previous counted one, and where to go from here. */
export function PointPanel({
  points,
  index,
  changes,
  previous,
  bridge,
  here,
  onClose,
}: {
  points: TrendPoint[]
  index: number
  changes: TrendChange[]
  /** The previous execution with a counted run. */
  previous: TrendPoint | null
  bridge: DashboardDataBridge | null
  /** This view's hash, for Compare to come back to. */
  here: string
  onClose: () => void
}) {
  const point = points[index]
  const before = index > 0 ? points[index - 1] : null
  const lookups = useCommitLookups(bridge, changes)
  const missing = notRun(point)
  const rc = releaseControlId(point)
  const source = [
    sourceText(point),
    rc ? `Release Control ${rc}` : null,
    point.label,
  ]
    .filter(Boolean)
    .join(' · ')
  const canCompare = previous !== null && counted(point)
  return (
    <aside
      className="tr-card tr-panel"
      aria-labelledby="tr-pn"
      data-trend-panel={point.execution_id}
    >
      <div className="tr-panel-head">
        <div className="tr-panel-title">
          <h2 id="tr-pn" className="tr-h2">
            {pointTime(point)}
          </h2>
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
      {counted(point) && missing.length > 0 ? (
        <p role="status" className="tr-note" data-tone="warn">
          Planned but not run: {missing.join(', ')}. They count as not
          completed.
        </p>
      ) : null}
      <section className="tr-panel-section" aria-labelledby="tr-ch">
        <h3 id="tr-ch" className="tr-h3">
          {before
            ? `What changed since ${pointTime(before)}`
            : 'The first execution in this view'}
        </h3>
        {changes.length === 0 ? (
          <p className="tr-faint">
            {before
              ? 'Nothing recorded changed: same iii, runner, stack, workers and test definitions.'
              : 'There is nothing before it to compare with.'}
          </p>
        ) : (
          <ul className="tr-changes">
            {changes.map((change) => (
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
      </section>
      {counted(point) ? (
        <section className="tr-panel-section" aria-labelledby="tr-mx">
          <h3 id="tr-mx" className="tr-h3">
            {previous ? `Against ${pointTime(previous)}` : 'Measures'}
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
                      value={previous ? deltaOf(metric, point, previous) : null}
                    />
                  </dd>
                </div>
              )
            })}
          </dl>
        </section>
      ) : null}
      <p className="tr-mono tr-small tr-faint-ink tr-wrap">
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
        <a
          className={buttonClassName({
            variant: 'secondary',
            className: 'no-underline',
          })}
          href={hashForExecution(point.execution_id)}
        >
          Open execution
        </a>
        {canCompare && previous ? (
          <a
            className={buttonClassName({
              variant: 'secondary',
              className: 'no-underline',
            })}
            href={hashFrom(
              hashForComparison(previous.execution_id, point.execution_id),
              here,
            )}
          >
            Compare with {pointTime(previous)}
          </a>
        ) : null}
      </div>
    </aside>
  )
}
