import {
  AlertTriangle,
  CheckCircle2,
  ChevronRight,
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
import { buttonClassName } from '@/design-system'
import {
  hashForComparison,
  hashForNewPlan,
  hashForTestHistory,
} from '@/hooks/use-hash-route'
import {
  type DashboardDataBridge,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import { formatDate } from '@/lib/execution-view'
import {
  type LocalScenarioSummary,
  localScenariosFromCatalog,
} from '@/lib/local-scenario-catalog'
import type { TestCatalogRow, TestsListResponse } from '@/lib/test-catalog'
import { catalogExecutionSummary } from '@/lib/test-catalog-view'

type LifecycleFilter = 'all' | TestCatalogRow['lifecycle']
type SourceFilter = 'all' | 'local' | 'registry'

function isInteractiveTarget(target: EventTarget | null) {
  return (
    target instanceof Element &&
    target.closest('a, button, input, select, textarea, summary') !== null
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
  // Audit T-10: the dot carries the colour; the text stays readable.
  never_run: {
    label: 'never run',
    dotClassName: 'bg-[var(--color-ink-ghost)]',
    textClassName: 'text-ink-soft',
  },
  retired: {
    label: 'retired',
    dotClassName: 'bg-[var(--warning)]',
    textClassName: 'text-ink-soft',
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
      <span className="block text-xs text-ink-soft">{value}</span>
      {detail ? (
        <small className="block text-label text-ink-muted">{detail}</small>
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
  // Audit NT-03: the fallback name no longer repeats the "local_" prefix
  // that the compiler adds ("local_local_scenario").
  if (!existing.has('new-test.md')) return 'new-test.md'
  let suffix = 2
  while (existing.has(`new-test-${suffix}.md`)) suffix += 1
  return `new-test-${suffix}.md`
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

// Audit T-09 / DS-07: same vocabulary as the version pill — mono, lowercase,
// 6px radius, fill, no border.
export function LocalTestBadge() {
  return (
    <span className="inline-flex shrink-0 items-center rounded-[6px] bg-[var(--surface-fill)] px-1.5 py-0.5 font-mono text-label leading-4 text-ink-soft">
      local
    </span>
  )
}

export function TestsCatalogPage() {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [data, setData] = useState<TestsListResponse | null>(null)
  const [query, setQuery] = useState('')
  const [lifecycle, setLifecycle] = useState<LifecycleFilter>('all')
  const [source, setSource] = useState<SourceFilter>('all')
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
    if (source === 'local' && !localScenario) return false
    if (source === 'registry' && localScenario) return false
    return `${row.test_id} ${localScenario?.title ?? ''}`
      .toLowerCase()
      .includes(query.trim().toLowerCase())
  })
  const filtered =
    query.trim() !== '' || lifecycle !== 'all' || source !== 'all'
  const clearFilters = () => {
    setQuery('')
    setLifecycle('all')
    setSource('all')
  }
  const localBridge = bridge?.mode === 'local' ? bridge : null
  const local = Boolean(localBridge)
  const suggestedLocalFileName = nextLocalScenarioFileName(localScenarios)
  const localCount = allRows.filter((entry) => entry.localScenario).length

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
        {/* Audit T-11: the page has a heading even though the console owns
            the visible title. */}
        <h1 className="visually-hidden">Test catalog</h1>
        {localBridge && authoringScenario ? (
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
              className="absolute top-1/2 left-3 -translate-y-1/2 text-ink-muted"
              size={14}
              aria-hidden="true"
            />
            <input
              className="min-h-9 w-full rounded-[6px] border-0 bg-[var(--surface-fill)] pl-9 pr-3 text-[13px] text-ink outline-none placeholder:text-ink-muted"
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
          {localCount > 0 ? (
            <label>
              <span className="visually-hidden">Filter by source</span>
              <select
                className="min-h-9 rounded-[6px] border-0 bg-[var(--surface-fill)] px-3 text-[13px] text-[var(--color-ink-faint)] outline-none"
                value={source}
                onChange={(event) =>
                  setSource(event.target.value as SourceFilter)
                }
              >
                <option value="all">All sources</option>
                <option value="local">Local</option>
                <option value="registry">Registry</option>
              </select>
            </label>
          ) : null}
          {localBridge ? (
            // Audit T-04: a refresh, not a filter — icon only, named by its
            // tooltip.
            <button
              className="inline-grid size-9 place-items-center rounded-[6px] bg-[var(--surface-fill)] text-ink-soft transition-colors hover:bg-[var(--surface-soft)] hover:text-ink disabled:cursor-wait disabled:opacity-50"
              type="button"
              onClick={() => void loadLocalScenarios(localBridge)}
              disabled={localCatalogLoading}
              title="Refresh local tests"
              aria-label="Refresh local tests"
            >
              <RefreshCw
                className={localCatalogLoading ? 'animate-spin' : ''}
                size={14}
                aria-hidden="true"
              />
            </button>
          ) : null}
          <span
            className="ms-auto font-mono text-xs text-ink-muted"
            aria-live="polite"
          >
            {filtered ? `${rows.length} of ${allRows.length}` : allRows.length}{' '}
            tests
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
            <div className="mt-8 grid justify-items-center gap-3 text-center text-[13px] text-ink-soft">
              <p className="m-0">
                {allRows.length === 0
                  ? 'No tests are registered yet.'
                  : 'No tests match these filters.'}
              </p>
              {allRows.length > 0 ? (
                <button
                  className={buttonClassName({
                    variant: 'secondary',
                    size: 'compact',
                  })}
                  type="button"
                  onClick={clearFilters}
                >
                  clear filters
                </button>
              ) : null}
            </div>
          ) : (
            <div className="mt-3 overflow-x-auto">
              <table
                className={`w-full min-w-[82rem] border-collapse text-left [&_td]:border-0 [&_td]:px-3 [&_td]:py-2.5 [&_th]:border-0 [&_th]:px-3 [&_th]:py-2 [&_th]:font-mono [&_th]:text-label [&_th]:font-medium [&_th]:uppercase [&_th]:tracking-[0.06em] [&_th]:text-ink-muted [&_tbody_tr]:transition-colors ${
                  local
                    ? '[&_tbody_tr]:cursor-pointer [&_tbody_tr:hover]:bg-[var(--surface-soft)]'
                    : ''
                }`}
              >
                <caption className="visually-hidden">
                  Test catalog, {rows.length} of {allRows.length} tests
                </caption>
                <thead>
                  <tr>
                    <th scope="col">Test</th>
                    <th scope="col">Version</th>
                    <th scope="col">Lifecycle</th>
                    <th scope="col">Complexity</th>
                    <th scope="col">Human horizon</th>
                    <th scope="col">Realism</th>
                    <th scope="col">Calibration</th>
                    <th scope="col" className="text-right">
                      Executions
                    </th>
                    <th scope="col">
                      <span className="visually-hidden">History</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map(({ row, localScenario }) => {
                    const tone = lifecyclePresentation[row.lifecycle]
                    const complexity = catalogComplexityPresentation(row)
                    const horizon = catalogHorizonPresentation(row)
                    const realism = catalogRealismPresentation(row)
                    const calibration = catalogCalibrationPresentation(row)
                    const executions = catalogExecutionSummary(row)
                    const historyHref = hashForTestHistory(row.test_id)
                    return (
                      // Audit T-01: the whole row opens the history; the id
                      // link keeps the keyboard and screen-reader path.
                      <tr
                        key={row.test_id}
                        onClick={
                          local
                            ? (click) => {
                                if (isInteractiveTarget(click.target)) return
                                window.location.hash = historyHref
                              }
                            : undefined
                        }
                      >
                        <td data-label="Test">
                          <span className="flex items-center gap-2 font-mono text-[13px] leading-5 font-medium text-ink">
                            {local ? (
                              <a
                                className="text-ink no-underline hover:underline"
                                href={historyHref}
                              >
                                {row.test_id}
                              </a>
                            ) : (
                              <span>{row.test_id}</span>
                            )}
                            {localScenario ? <LocalTestBadge /> : null}
                          </span>
                          <small className="block text-xs text-ink-muted">
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
                          <span className="inline-flex items-center rounded-[6px] bg-[var(--surface-fill)] px-1.5 py-0.5 font-mono text-label leading-4 text-ink-soft">
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
                        <td
                          data-label="Executions"
                          className="text-right tabular-nums"
                        >
                          <span
                            className="block font-mono text-xs text-ink-soft"
                            title={executions.breakdown || undefined}
                          >
                            {executions.total}
                          </span>
                          <small className="block text-label text-ink-muted">
                            {executions.lastSeen
                              ? `last seen ${formatDate(executions.lastSeen)}`
                              : 'never run'}
                          </small>
                        </td>
                        <td data-label="History" className="text-right">
                          {local ? (
                            <a
                              className="inline-grid size-7 place-items-center rounded-[6px] text-ink-soft no-underline transition-colors hover:bg-[var(--surface-soft)] hover:text-ink"
                              href={historyHref}
                              aria-label={`History for ${row.test_id}`}
                              title="History"
                            >
                              <ChevronRight size={16} aria-hidden="true" />
                            </a>
                          ) : (
                            <span className="text-xs text-ink-muted">
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
