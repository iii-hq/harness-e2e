import { Search } from 'lucide-react'
import { useEffect, useState } from 'react'
import {
  DashboardPageActions,
  dashboardHeaderActionClassName,
} from '@/components/DashboardPageActions'
import {
  hashForComparison,
  hashForNewPlan,
  hashForTestHistory,
} from '@/hooks/use-hash-route'
import {
  type DashboardDataBridge,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import type { TestCatalogRow, TestsListResponse } from '@/lib/test-catalog'

type LifecycleFilter = 'all' | TestCatalogRow['lifecycle']

function currentVersion(row: TestCatalogRow) {
  return row.available_versions.find(
    (item) => item.version === row.current_version,
  )
}

const lifecyclePresentation: Record<
  TestCatalogRow['lifecycle'],
  { label: string; dotClassName: string; textClassName: string }
> = {
  active: {
    label: 'active',
    dotClassName: 'bg-[var(--success)]',
    textClassName: 'text-ink',
  },
  never_run: {
    label: 'never run',
    dotClassName: 'bg-[var(--color-ink-ghost)]',
    textClassName: 'text-[var(--color-ink-ghost)]',
  },
  retired: {
    label: 'retired',
    dotClassName: 'bg-[var(--warning)]',
    textClassName: 'text-[var(--warning)]',
  },
}

const complexityTierLabels = {
  l0_atomic: 'L0 atomic',
  l1_sequential: 'L1 sequential',
  l2_stateful: 'L2 stateful',
  l3_concurrent: 'L3 concurrent',
  l4_coordinated: 'L4 coordinated',
  l5_adaptive: 'L5 adaptive',
} as const

export function catalogComplexityPresentation(row: TestCatalogRow) {
  if (!row.complexity) return { value: 'Not declared', detail: null }
  return {
    value: complexityTierLabels[row.complexity.tier],
    detail:
      row.complexity.method === 'capability_v2'
        ? 'capability v2'
        : row.complexity.method === 'legacy_v1'
          ? 'legacy v1'
          : null,
  }
}

export function catalogHorizonPresentation(row: TestCatalogRow) {
  const horizon = row.characterization?.human_horizon
  const min = horizon?.min_minutes
  const max = horizon?.max_minutes
  if (min === undefined || max === undefined) {
    return { value: 'Unknown', detail: null }
  }
  const value = min === max ? `${min} min` : `${min}–${max} min`
  return {
    value,
    detail:
      horizon?.basis === 'measured'
        ? 'measured'
        : horizon?.basis === 'author_estimate'
          ? 'author estimate'
          : null,
  }
}

export function catalogRealismPresentation(row: TestCatalogRow) {
  const realism = row.characterization?.realism
  const value =
    realism?.execution === 'realistic_simulator'
      ? 'Realistic simulator'
      : realism?.execution === 'frozen_real_artifact'
        ? 'Frozen real artifact'
        : realism?.execution === 'synthetic'
          ? 'Synthetic'
          : 'Not declared'
  return {
    value,
    detail: realism?.shadow === 'read_only' ? 'read-only shadow' : null,
  }
}

export function catalogCalibrationPresentation(row: TestCatalogRow) {
  const maturity = row.calibration?.maturity
  const sampleCount = row.calibration?.compatible_sample_count
  const value =
    maturity === 'tail_calibrated'
      ? 'Tail calibrated'
      : maturity === 'repeatable'
        ? 'Repeatable'
        : maturity === 'observed'
          ? 'Observed'
          : maturity === 'reference_verified'
            ? 'Reference verified'
            : 'Candidate'
  return {
    value,
    detail:
      sampleCount === undefined
        ? null
        : `${sampleCount} compatible sample${sampleCount === 1 ? '' : 's'}`,
  }
}

function DimensionCell({
  value,
  detail,
}: {
  value: string
  detail: string | null
}) {
  return (
    <>
      <span className="block text-xs text-[var(--color-ink-faint)]">
        {value}
      </span>
      {detail ? (
        <small className="block text-[11px] text-[var(--color-ink-ghost)]">
          {detail}
        </small>
      ) : null}
    </>
  )
}

export function TestsCatalogPage() {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [data, setData] = useState<TestsListResponse | null>(null)
  const [query, setQuery] = useState('')
  const [lifecycle, setLifecycle] = useState<LifecycleFilter>('all')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    void getDashboardDataBridge()
      .then(async (next) => {
        if (cancelled) return
        setBridge(next)
        setData(await next.listTests({ limit: 100 }))
      })
      .catch((cause) => {
        if (!cancelled)
          setError(cause instanceof Error ? cause.message : String(cause))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [])

  const allRows = data?.rows ?? []
  const rows = allRows.filter((row) => {
    if (lifecycle !== 'all' && row.lifecycle !== lifecycle) return false
    return row.test_id.toLowerCase().includes(query.trim().toLowerCase())
  })
  const local = bridge?.mode === 'local'

  return (
    <>
      <a className="skip-link" href="#tests-catalog-main">
        Skip to test catalog
      </a>
      <DashboardPageActions
        active="tests"
        actionsLabel="Test catalog actions"
        actions={
          <>
            {local ? (
              <a
                className={dashboardHeaderActionClassName({ primary: true })}
                href={hashForNewPlan()}
                aria-label="New local plan"
              >
                New plan
              </a>
            ) : null}
            <a
              className={dashboardHeaderActionClassName()}
              href={hashForComparison()}
              aria-label="System comparison"
            >
              Compare
            </a>
          </>
        }
      />
      <main
        id="tests-catalog-main"
        className="page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-16 md:w-[calc(100%_-_3rem)]"
      >
        <section
          className="flex flex-wrap items-center gap-2.5"
          aria-label="Test catalog filters"
        >
          <label className="relative block w-full max-w-xs">
            <span className="visually-hidden">Search tests</span>
            <Search
              className="absolute top-1/2 left-3 -translate-y-1/2 text-[var(--color-ink-ghost)]"
              size={14}
              aria-hidden="true"
            />
            <input
              className="min-h-9 w-full rounded-[6px] border-0 bg-[var(--surface-fill)] pl-9 pr-3 text-[13px] text-ink outline-none placeholder:text-[var(--color-ink-ghost)]"
              type="search"
              placeholder="Filter tests…"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
            />
          </label>
          <label>
            <span className="visually-hidden">Filter by lifecycle</span>
            <select
              className="min-h-9 rounded-[6px] border-0 bg-[var(--surface-fill)] px-3 text-[13px] text-[var(--color-ink-faint)] outline-none"
              value={lifecycle}
              onChange={(event) =>
                setLifecycle(event.target.value as LifecycleFilter)
              }
            >
              <option value="all">All lifecycles</option>
              <option value="active">Active</option>
              <option value="never_run">Never run</option>
              <option value="retired">Retired</option>
            </select>
          </label>
          <span className="ms-auto font-mono text-xs text-[var(--color-ink-ghost)]">
            {allRows.length} tests
            {data?.revision ? (
              <>
                {' · catalog '}
                <span className="text-[var(--color-ink-faint)]">
                  {data.revision.slice(-12)}
                </span>
              </>
            ) : null}
          </span>
        </section>
        {error && (
          <section className="empty-state mt-6" role="alert">
            <h2>Test catalog unavailable</h2>
            <p>{error}</p>
          </section>
        )}
        {!error &&
          (loading ? (
            <div className="mt-4 grid gap-2" aria-busy="true" role="status">
              <span className="visually-hidden">Loading test catalog</span>
              {['first', 'second', 'third', 'fourth', 'fifth', 'sixth'].map(
                (placeholder) => (
                  <div
                    key={placeholder}
                    className="h-10 animate-pulse rounded-[6px] bg-[var(--surface-fill)] motion-reduce:animate-none"
                  />
                ),
              )}
            </div>
          ) : rows.length === 0 ? (
            <p className="m-0 mt-8 text-center text-[13px] text-[var(--color-ink-faint)]">
              {allRows.length === 0
                ? 'No tests are registered yet.'
                : 'No tests match this filter.'}
            </p>
          ) : (
            <div className="mt-3 overflow-x-auto">
              <table className="w-full min-w-[82rem] border-collapse text-left [&_td]:border-0 [&_td]:px-3 [&_td]:py-2.5 [&_th]:border-0 [&_th]:px-3 [&_th]:py-2 [&_th]:font-mono [&_th]:text-[11px] [&_th]:font-medium [&_th]:uppercase [&_th]:tracking-[0.06em] [&_th]:text-[var(--color-ink-ghost)] [&_tbody_tr]:transition-colors [&_tbody_tr:hover]:bg-[var(--surface-soft)]">
                <thead>
                  <tr>
                    <th scope="col">Test</th>
                    <th scope="col">Version</th>
                    <th scope="col">Lifecycle</th>
                    <th scope="col">Complexity</th>
                    <th scope="col">Human horizon</th>
                    <th scope="col">Realism</th>
                    <th scope="col">Calibration</th>
                    <th scope="col">Executions</th>
                    <th scope="col">
                      <span className="visually-hidden">History</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map((row) => {
                    const version = currentVersion(row)
                    const tone = lifecyclePresentation[row.lifecycle]
                    const complexity = catalogComplexityPresentation(row)
                    const horizon = catalogHorizonPresentation(row)
                    const realism = catalogRealismPresentation(row)
                    const calibration = catalogCalibrationPresentation(row)
                    return (
                      <tr key={row.test_id}>
                        <td data-label="Test">
                          <span className="block font-mono text-[13px] leading-5 font-medium text-ink">
                            {row.test_id}
                          </span>
                          <small className="block text-xs text-[var(--color-ink-ghost)]">
                            {row.available_versions.length} contract version
                            {row.available_versions.length === 1 ? '' : 's'}
                          </small>
                        </td>
                        <td data-label="Version">
                          <span className="inline-flex items-center rounded-full bg-[var(--surface-fill)] px-2 py-0.5 font-mono text-[11px] font-medium text-[var(--color-ink-faint)]">
                            {row.current_version
                              ? `v${row.current_version}`
                              : '—'}
                          </span>
                        </td>
                        <td data-label="Lifecycle">
                          <span
                            className={`inline-flex items-center gap-2 text-xs ${tone.textClassName}`}
                          >
                            <span
                              className={`h-1.5 w-1.5 rounded-full ${tone.dotClassName}`}
                              aria-hidden="true"
                            />
                            {tone.label}
                          </span>
                        </td>
                        <td data-label="Complexity">
                          <DimensionCell {...complexity} />
                        </td>
                        <td data-label="Human horizon">
                          <DimensionCell {...horizon} />
                        </td>
                        <td data-label="Realism">
                          <DimensionCell {...realism} />
                        </td>
                        <td data-label="Calibration">
                          <DimensionCell {...calibration} />
                        </td>
                        <td data-label="Executions">
                          <span className="font-mono text-xs text-[var(--color-ink-faint)]">
                            {version?.execution_count ?? 0}
                          </span>
                        </td>
                        <td data-label="History" className="text-right">
                          {local ? (
                            <a
                              className="inline-flex min-h-7 items-center rounded-[6px] bg-[var(--surface-fill)] px-2.5 text-xs font-medium text-[var(--color-ink-faint)] no-underline transition-colors hover:bg-[var(--surface-soft)] hover:text-ink"
                              href={hashForTestHistory(row.test_id)}
                            >
                              View metrics
                            </a>
                          ) : (
                            <span className="text-xs text-[var(--color-ink-ghost)]">
                              Local dashboard only
                            </span>
                          )}
                        </td>
                      </tr>
                    )
                  })}
                </tbody>
              </table>
            </div>
          ))}
      </main>
    </>
  )
}
