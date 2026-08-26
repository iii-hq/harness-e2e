import {
  AlertTriangle,
  CheckCircle2,
  Plus,
  RefreshCw,
  Search,
} from 'lucide-react'
import { useCallback, useEffect, useState } from 'react'
import {
  DashboardPageActions,
  dashboardHeaderActionClassName,
} from '@/components/DashboardPageActions'
import { LocalScenarioEditor } from '@/components/LocalScenarioEditor'
import {
  hashForComparison,
  hashForNewPlan,
  hashForTestHistory,
} from '@/hooks/use-hash-route'
import {
  type DashboardDataBridge,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import {
  type LocalScenarioSummary,
  localScenariosFromCatalog,
} from '@/lib/local-scenario-catalog'
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

export function nextLocalScenarioFileName(
  scenarios: LocalScenarioSummary[],
): string {
  const existing = new Set(
    scenarios.map((scenario) => scenario.source_path.split('/').at(-1)),
  )
  if (!existing.has('local-scenario.md')) return 'local-scenario.md'
  let suffix = 2
  while (existing.has(`local-scenario-${suffix}.md`)) suffix += 1
  return `local-scenario-${suffix}.md`
}

export type CatalogDisplayRow = {
  row: TestCatalogRow
  localScenario: LocalScenarioSummary | null
}

export function mergeLocalScenariosIntoCatalog(
  rows: TestCatalogRow[],
  localScenarios: LocalScenarioSummary[],
): CatalogDisplayRow[] {
  const localById = new Map(
    localScenarios.map((scenario) => [scenario.id, scenario]),
  )
  const seen = new Set(rows.map((row) => row.test_id))
  return [
    ...rows.map((row) => {
      const localScenario = localById.get(row.test_id) ?? null
      if (!localScenario) return { row, localScenario }

      const currentVersion = row.available_versions.find(
        (version) => version.version === localScenario.version,
      )
      const availableVersions = currentVersion
        ? row.available_versions
        : [
            {
              version: localScenario.version,
              execution_count: 0,
              run_count: 0,
              last_seen: null,
            },
            ...row.available_versions,
          ]
      const observed = availableVersions.some(
        (version) => version.execution_count > 0,
      )

      return {
        localScenario,
        row: {
          ...row,
          lifecycle: observed ? ('active' as const) : ('never_run' as const),
          current_version: localScenario.version,
          available_versions: availableVersions,
          selected_version: localScenario.version,
        },
      }
    }),
    ...localScenarios
      .filter((scenario) => !seen.has(scenario.id))
      .map((scenario) => ({
        localScenario: scenario,
        row: {
          test_id: scenario.id,
          lifecycle: 'never_run' as const,
          current_version: scenario.version,
          available_versions: [
            {
              version: scenario.version,
              execution_count: 0,
              run_count: 0,
              last_seen: null,
            },
          ],
          selected_version: scenario.version,
          result: null,
        },
      })),
  ]
}

export function TestsCatalogActions({
  local,
  localReady,
  onNewTest,
}: {
  local: boolean
  localReady: boolean
  onNewTest: () => void
}) {
  return (
    <>
      {local ? (
        <button
          className={dashboardHeaderActionClassName({ primary: true })}
          type="button"
          onClick={onNewTest}
          disabled={!localReady}
          aria-label="Create a new local test"
        >
          <Plus size={13} aria-hidden="true" />
          New test
        </button>
      ) : null}
      {local ? (
        <a
          className={dashboardHeaderActionClassName()}
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
  )
}

export function LocalTestBadge() {
  return (
    <span className="inline-flex shrink-0 items-center rounded-full border border-brand/25 bg-brand-soft px-1.5 py-0.5 font-sans text-[9px] leading-none font-semibold tracking-[0.05em] text-brand uppercase">
      Local
    </span>
  )
}

export function TestsCatalogPage() {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [data, setData] = useState<TestsListResponse | null>(null)
  const [query, setQuery] = useState('')
  const [lifecycle, setLifecycle] = useState<LifecycleFilter>('all')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [localScenarios, setLocalScenarios] = useState<LocalScenarioSummary[]>(
    [],
  )
  const [localCatalogLoading, setLocalCatalogLoading] = useState(true)
  const [localCatalogError, setLocalCatalogError] = useState<string | null>(
    null,
  )
  const [authoringScenario, setAuthoringScenario] = useState(false)
  const [createdScenarioId, setCreatedScenarioId] = useState<string | null>(
    null,
  )

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

  const loadLocalScenarios = useCallback(
    async (target = bridge) => {
      if (target?.mode !== 'local') {
        setLocalCatalogLoading(false)
        return
      }
      setLocalCatalogLoading(true)
      setLocalCatalogError(null)
      try {
        setLocalScenarios(localScenariosFromCatalog(await target.getCatalog()))
      } catch (cause) {
        setLocalCatalogError(
          cause instanceof Error ? cause.message : String(cause),
        )
      } finally {
        setLocalCatalogLoading(false)
      }
    },
    [bridge],
  )

  useEffect(() => {
    if (bridge) void loadLocalScenarios(bridge)
  }, [bridge, loadLocalScenarios])

  const allRows = mergeLocalScenariosIntoCatalog(
    data?.rows ?? [],
    localScenarios,
  )
  const rows = allRows.filter(({ row, localScenario }) => {
    if (lifecycle !== 'all' && row.lifecycle !== lifecycle) return false
    return `${row.test_id} ${localScenario?.title ?? ''}`
      .toLowerCase()
      .includes(query.trim().toLowerCase())
  })
  const localBridge = bridge?.mode === 'local' ? bridge : null
  const local = Boolean(localBridge)
  const suggestedLocalFileName = nextLocalScenarioFileName(localScenarios)

  return (
    <>
      <a className="skip-link" href="#tests-catalog-main">
        Skip to test catalog
      </a>
      <DashboardPageActions
        active="tests"
        actionsLabel="Test catalog actions"
        actions={
          <TestsCatalogActions
            local={local}
            localReady={local && !localCatalogLoading}
            onNewTest={() => {
              setCreatedScenarioId(null)
              setAuthoringScenario(true)
            }}
          />
        }
      />
      <main
        id="tests-catalog-main"
        className="page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-16 md:w-[calc(100%_-_3rem)]"
      >
        {localBridge && authoringScenario ? (
          <div className="panel mb-5 overflow-hidden">
            <LocalScenarioEditor
              bridge={localBridge}
              initialFileName={suggestedLocalFileName}
              onClose={() => setAuthoringScenario(false)}
              onCreated={(scenarioId) => {
                setCreatedScenarioId(scenarioId)
                setAuthoringScenario(false)
                void loadLocalScenarios(localBridge)
              }}
            />
          </div>
        ) : null}

        {createdScenarioId ? (
          <p
            className="mb-4 flex items-center gap-2 rounded-[6px] bg-success/5 px-3 py-2 text-sm text-success"
            role="status"
          >
            <CheckCircle2 size={15} aria-hidden="true" />
            Created <code className="font-mono">{createdScenarioId}</code>. No
            execution was started.
          </p>
        ) : null}

        {localCatalogError ? (
          <div
            className="mb-4 flex items-start justify-between gap-4 rounded-[6px] border border-danger/30 bg-danger/5 p-3"
            role="alert"
          >
            <div className="flex gap-2.5">
              <AlertTriangle
                className="mt-0.5 text-danger"
                size={16}
                aria-hidden="true"
              />
              <div>
                <strong className="block text-sm text-ink">
                  Local tests could not be loaded
                </strong>
                <span className="text-sm text-ink-muted">
                  {localCatalogError}
                </span>
              </div>
            </div>
            {localBridge ? (
              <button
                className="button"
                type="button"
                onClick={() => void loadLocalScenarios(localBridge)}
              >
                Retry
              </button>
            ) : null}
          </div>
        ) : null}

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
          {localBridge ? (
            <button
              className="inline-flex min-h-9 items-center gap-2 rounded-[6px] bg-[var(--surface-fill)] px-3 text-xs font-medium text-[var(--color-ink-faint)] transition-colors hover:bg-[var(--surface-soft)] hover:text-ink disabled:cursor-wait disabled:opacity-50"
              type="button"
              onClick={() => void loadLocalScenarios(localBridge)}
              disabled={localCatalogLoading}
              aria-label="Refresh local tests"
            >
              <RefreshCw
                className={localCatalogLoading ? 'animate-spin' : ''}
                size={13}
                aria-hidden="true"
              />
              Local tests
            </button>
          ) : null}
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
                  {rows.map(({ row, localScenario }) => {
                    const version = currentVersion(row)
                    const tone = lifecyclePresentation[row.lifecycle]
                    const complexity = catalogComplexityPresentation(row)
                    const horizon = catalogHorizonPresentation(row)
                    const realism = catalogRealismPresentation(row)
                    const calibration = catalogCalibrationPresentation(row)
                    return (
                      <tr key={row.test_id}>
                        <td data-label="Test">
                          <span className="flex items-center gap-2 font-mono text-[13px] leading-5 font-medium text-ink">
                            <span>{row.test_id}</span>
                            {localScenario ? <LocalTestBadge /> : null}
                          </span>
                          <small className="block text-xs text-[var(--color-ink-ghost)]">
                            {localScenario ? (
                              localScenario.title
                            ) : (
                              <>
                                {row.available_versions.length} contract version
                                {row.available_versions.length === 1 ? '' : 's'}
                              </>
                            )}
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
