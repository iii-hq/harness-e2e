import { ChevronRight, Copy, Plus, RefreshCw, Search, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  DashboardPageActions,
  dashboardHeaderActionClassName,
} from '@/components/DashboardPageActions'
import { LocalScenarioEditor } from '@/components/LocalScenarioEditor'
import {
  buttonClassName,
  Callout,
  DataTable,
  DataTableRow,
  EmptyState,
  FilterChip,
  FilterChipGroup,
  Input,
  numericCellClassName,
  PageHeader,
  Select,
} from '@/design-system'
import {
  hashForComparison,
  hashForNewPlan,
  hashForTestHistory,
  replaceRouteParams,
  routeParams,
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

type Lifecycle = TestCatalogRow['lifecycle']
type LifecycleFilter = 'all' | Lifecycle
type SourceFilter = 'all' | 'local' | 'registry'
type SortKey = 'lifecycle' | 'name' | 'runs' | 'last_seen' | 'complexity'

export type CatalogFilters = {
  query: string
  lifecycle: LifecycleFilter
  source: SourceFilter
  complexity: string
  realism: string
  withExecutions: boolean
  sort: SortKey
}

export const CATALOG_DEFAULT_FILTERS: CatalogFilters = {
  query: '',
  lifecycle: 'all',
  source: 'all',
  complexity: 'all',
  realism: 'all',
  withExecutions: false,
  sort: 'lifecycle',
}

const LIFECYCLES: Lifecycle[] = ['active', 'never_run', 'retired']
const SORT_KEYS: SortKey[] = [
  'lifecycle',
  'name',
  'runs',
  'last_seen',
  'complexity',
]
const PAGE_SIZE = 50
/** Rows shown per lifecycle group before "show N more". */
const GROUP_PREVIEW = 10
const CATALOG_SCROLL_KEY = 'harness-e2e:tests-catalog-scroll'

/** Audit T-08: the filters live in the hash so "back" restores them. */
export function catalogFiltersFromParams(
  params: URLSearchParams,
): CatalogFilters {
  const lifecycle = params.get('lifecycle')
  const source = params.get('source')
  const sort = params.get('sort')
  return {
    query: params.get('q') ?? '',
    lifecycle:
      lifecycle && (LIFECYCLES as string[]).includes(lifecycle)
        ? (lifecycle as Lifecycle)
        : 'all',
    source: source === 'local' || source === 'registry' ? source : 'all',
    complexity: params.get('complexity') ?? 'all',
    realism: params.get('realism') ?? 'all',
    withExecutions: params.get('evidence') === '1',
    sort:
      sort && (SORT_KEYS as string[]).includes(sort)
        ? (sort as SortKey)
        : 'lifecycle',
  }
}

export function catalogFiltersToParams(
  filters: CatalogFilters,
): URLSearchParams {
  const params = new URLSearchParams()
  if (filters.query.trim()) params.set('q', filters.query.trim())
  if (filters.lifecycle !== 'all') params.set('lifecycle', filters.lifecycle)
  if (filters.source !== 'all') params.set('source', filters.source)
  if (filters.complexity !== 'all') params.set('complexity', filters.complexity)
  if (filters.realism !== 'all') params.set('realism', filters.realism)
  if (filters.withExecutions) params.set('evidence', '1')
  if (filters.sort !== 'lifecycle') params.set('sort', filters.sort)
  return params
}

export function catalogFiltersActive(filters: CatalogFilters) {
  return catalogFiltersToParams(filters).toString() !== ''
}

const lifecyclePresentation: Record<
  Lifecycle,
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

const complexityRank: Record<string, number> = {
  l0_atomic: 0,
  l1_sequential: 1,
  l2_stateful: 2,
  l3_concurrent: 3,
  l4_coordinated: 4,
  l5_adaptive: 5,
}

const realismLabels = {
  synthetic: 'synthetic',
  realistic_simulator: 'realistic simulator',
  frozen_real_artifact: 'frozen real artifact',
} as const

/**
 * Audit T-12 / T-14: one marker for anything not declared. `value: null`
 * renders as "—" with a "not declared" tooltip instead of three vocabularies.
 */
export type DimensionPresentation = {
  value: string | null
  detail: string | null
}

export function catalogComplexityPresentation(
  row: TestCatalogRow,
): DimensionPresentation {
  if (!row.complexity) return { value: null, detail: null }
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

export function catalogHorizonPresentation(
  row: TestCatalogRow,
): DimensionPresentation {
  const horizon = row.characterization?.human_horizon
  const min = horizon?.min_minutes
  const max = horizon?.max_minutes
  if (min === undefined || max === undefined) {
    return { value: null, detail: null }
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

export function catalogRealismPresentation(
  row: TestCatalogRow,
): DimensionPresentation {
  const realism = row.characterization?.realism
  const execution = realism?.execution
  return {
    value: execution ? realismLabels[execution] : null,
    detail: realism?.shadow === 'read_only' ? 'read-only shadow' : null,
  }
}

/** The calibration column reads as evidence: a word plus the sample count. */
export function catalogCalibrationPresentation(
  row: TestCatalogRow,
): DimensionPresentation {
  const maturity = row.calibration?.maturity
  const sampleCount = row.calibration?.compatible_sample_count
  const samples =
    sampleCount === undefined
      ? null
      : `${sampleCount} compatible sample${sampleCount === 1 ? '' : 's'}`
  if (maturity === 'tail_calibrated')
    return { value: 'tail calibrated', detail: samples }
  if (maturity === 'repeatable') return { value: 'repeatable', detail: samples }
  if (maturity === 'observed') return { value: 'observed', detail: samples }
  if (maturity === 'reference_verified')
    return { value: 'reference verified', detail: samples }
  return {
    value: sampleCount ? 'candidate' : 'no samples',
    detail: sampleCount ? samples : null,
  }
}

/** Audit T-05: one line per cell; the detail lives in the tooltip. */
function DimensionCell({
  value,
  detail,
  extra,
}: DimensionPresentation & { extra?: string | null }) {
  if (value === null) {
    return (
      <span className="text-ink-muted" title="not declared">
        —
      </span>
    )
  }
  const tooltip = [detail, extra].filter(Boolean).join(' · ')
  return (
    <span
      className="block whitespace-nowrap text-xs text-ink-soft"
      title={tooltip || undefined}
    >
      {value}
    </span>
  )
}

function shortDate(value: string) {
  const timestamp = Date.parse(value)
  return Number.isFinite(timestamp)
    ? new Date(timestamp).toLocaleDateString(undefined, {
        month: 'short',
        day: 'numeric',
      })
    : value
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

/** Applies the toolbar filters; sorting happens in sortCatalogDisplayRows. */
export function filterCatalogRows(
  rows: CatalogDisplayRow[],
  filters: CatalogFilters,
): CatalogDisplayRow[] {
  const query = filters.query.trim().toLowerCase()
  return rows.filter(({ row, localScenario }) => {
    if (filters.lifecycle !== 'all' && row.lifecycle !== filters.lifecycle)
      return false
    if (filters.source === 'local' && !localScenario) return false
    if (filters.source === 'registry' && localScenario) return false
    if (
      filters.complexity !== 'all' &&
      (row.complexity?.tier ?? 'none') !== filters.complexity
    )
      return false
    if (
      filters.realism !== 'all' &&
      (row.characterization?.realism?.execution ?? 'none') !== filters.realism
    )
      return false
    if (filters.withExecutions && catalogExecutionSummary(row).total === 0)
      return false
    return (
      !query ||
      `${row.test_id} ${localScenario?.title ?? ''}`
        .toLowerCase()
        .includes(query)
    )
  })
}

export function sortCatalogDisplayRows(
  rows: CatalogDisplayRow[],
  sort: SortKey,
): CatalogDisplayRow[] {
  const byName = (a: CatalogDisplayRow, b: CatalogDisplayRow) =>
    a.row.test_id.localeCompare(b.row.test_id)
  const sorted = [...rows]
  if (sort === 'runs') {
    sorted.sort(
      (a, b) =>
        catalogExecutionSummary(b.row).total -
          catalogExecutionSummary(a.row).total || byName(a, b),
    )
  } else if (sort === 'last_seen') {
    sorted.sort(
      (a, b) =>
        (catalogExecutionSummary(b.row).lastSeen ?? '').localeCompare(
          catalogExecutionSummary(a.row).lastSeen ?? '',
        ) || byName(a, b),
    )
  } else if (sort === 'complexity') {
    sorted.sort(
      (a, b) =>
        (complexityRank[b.row.complexity?.tier ?? ''] ?? -1) -
          (complexityRank[a.row.complexity?.tier ?? ''] ?? -1) || byName(a, b),
    )
  } else {
    sorted.sort(byName)
  }
  return sorted
}

export function groupCatalogRows(rows: CatalogDisplayRow[]) {
  return LIFECYCLES.map((lifecycle) => ({
    lifecycle,
    rows: rows.filter((entry) => entry.row.lifecycle === lifecycle),
  })).filter((group) => group.rows.length > 0)
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
          title={localReady ? undefined : 'loading local tests…'}
          aria-label="Create a new local test"
        >
          <Plus size={13} aria-hidden="true" />
          new test
        </button>
      ) : null}
      {local ? (
        <a
          className={dashboardHeaderActionClassName()}
          href={hashForNewPlan()}
          aria-label="New local plan"
        >
          new plan
        </a>
      ) : null}
      <a
        className={dashboardHeaderActionClassName()}
        href={hashForComparison()}
        aria-label="Compare system versions"
      >
        compare versions
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

function CatalogRevision({ revision }: { revision: string }) {
  const [copied, setCopied] = useState(false)
  return (
    <span className="inline-flex items-center gap-1 font-mono text-label text-ink-muted">
      catalog{' '}
      <span title={revision} className="text-ink-soft">
        {revision.slice(-12)}
      </span>
      <button
        className="inline-grid size-6 place-items-center rounded-[6px] border-0 bg-transparent text-ink-muted hover:bg-[var(--surface-fill)] hover:text-ink"
        type="button"
        title={copied ? 'copied' : 'copy the full catalog revision'}
        aria-label="Copy the full catalog revision"
        onClick={() => {
          void navigator.clipboard?.writeText(revision).then(() => {
            setCopied(true)
            window.setTimeout(() => setCopied(false), 1500)
          })
        }}
      >
        <Copy size={12} aria-hidden="true" />
      </button>
    </span>
  )
}

const COLUMNS = [
  {
    key: 'test',
    label: 'test',
    title: 'Test id and, for local tests, the title',
  },
  { key: 'version', label: 'version', title: 'Current contract version' },
  {
    key: 'complexity',
    label: 'complexity',
    title:
      'Capability tier declared by the scenario; hover for the human horizon',
  },
  {
    key: 'realism',
    label: 'realism',
    title: 'How real the execution environment is',
  },
  {
    key: 'evidence',
    label: 'evidence',
    title: 'Calibration maturity and compatible samples',
  },
  {
    key: 'runs',
    label: 'runs',
    title: 'Executions retained across every contract version',
    numeric: true,
  },
  {
    key: 'last-seen',
    label: 'last seen',
    title: 'Most recent retained execution',
  },
] as const

function CatalogRows({
  rows,
  local,
  highlightId,
  showLifecycle,
}: {
  rows: CatalogDisplayRow[]
  local: boolean
  highlightId: string | null
  /** Off inside lifecycle groups, where the heading already says it. */
  showLifecycle: boolean
}) {
  return (
    <>
      {rows.map(({ row, localScenario }) => {
        const tone = lifecyclePresentation[row.lifecycle]
        const complexity = catalogComplexityPresentation(row)
        const horizon = catalogHorizonPresentation(row)
        const realism = catalogRealismPresentation(row)
        const calibration = catalogCalibrationPresentation(row)
        const executions = catalogExecutionSummary(row)
        const historyHref = hashForTestHistory(row.test_id)
        const versions = row.available_versions.length
        return (
          // Audit T-01: the whole row opens the history; the id link keeps
          // the keyboard and screen-reader path. T-13: the link exists
          // whenever there is evidence to read, in every mode.
          <DataTableRow
            key={row.test_id}
            href={historyHref}
            id={`test-${row.test_id}`}
            data-test-id={row.test_id}
            className={
              highlightId === row.test_id ? 'is-highlighted' : undefined
            }
          >
            <td data-label="Test" className="ds-table-sticky-col">
              <span className="flex items-center gap-2 font-mono text-[13px] leading-5 font-medium text-ink">
                <a
                  className="text-ink no-underline hover:underline"
                  href={historyHref}
                >
                  {row.test_id}
                </a>
                {localScenario ? <LocalTestBadge /> : null}
                {showLifecycle ? (
                  <span
                    className={`ml-auto inline-flex items-center gap-1.5 font-mono text-label font-normal ${tone.textClassName}`}
                  >
                    <span
                      className={`ds-status-dot ${tone.dotClassName}`}
                      aria-hidden="true"
                    />
                    {tone.label}
                  </span>
                ) : null}
              </span>
              {localScenario ? (
                <small className="block text-xs text-ink-muted">
                  {localScenario.title}
                </small>
              ) : null}
            </td>
            <td data-label="Version">
              <span
                className="inline-flex items-center rounded-[6px] bg-[var(--surface-fill)] px-1.5 py-0.5 font-mono text-label leading-4 text-ink-soft"
                title={`${versions} contract version${versions === 1 ? '' : 's'}`}
              >
                {row.current_version ? `v${row.current_version}` : '—'}
              </span>
            </td>
            <td data-label="Complexity">
              <DimensionCell
                {...complexity}
                extra={
                  horizon.value
                    ? `human horizon ${horizon.value}${horizon.detail ? ` (${horizon.detail})` : ''}`
                    : null
                }
              />
            </td>
            <td data-label="Realism">
              <DimensionCell {...realism} />
            </td>
            <td data-label="Evidence">
              <DimensionCell {...calibration} />
            </td>
            <td data-label="Runs" className={numericCellClassName}>
              <span
                className="font-mono text-xs text-ink-soft"
                title={executions.breakdown || undefined}
              >
                {executions.total}
              </span>
            </td>
            <td data-label="Last seen">
              <span
                className="block whitespace-nowrap font-mono text-xs text-ink-soft"
                title={
                  executions.lastSeen
                    ? formatDate(executions.lastSeen)
                    : undefined
                }
              >
                {executions.lastSeen ? shortDate(executions.lastSeen) : '—'}
              </span>
            </td>
            <td data-label="History" className="text-right">
              <a
                className="inline-flex min-h-7 items-center gap-1 rounded-[6px] px-2 font-mono text-label text-ink-soft no-underline transition-colors hover:bg-[var(--surface-soft)] hover:text-ink"
                href={historyHref}
                aria-label={`History for ${row.test_id}`}
              >
                {local ? 'history' : 'evidence'}
                <ChevronRight size={13} aria-hidden="true" />
              </a>
            </td>
          </DataTableRow>
        )
      })}
    </>
  )
}

function CatalogTable({
  caption,
  children,
}: {
  caption: string
  children: React.ReactNode
}) {
  return (
    <DataTable
      caption={caption}
      collapse
      collapseInline
      minWidth="64rem"
      sticky
    >
      <thead>
        <tr>
          {COLUMNS.map((column) => (
            <th
              key={column.key}
              scope="col"
              title={column.title}
              className={
                'numeric' in column && column.numeric
                  ? numericCellClassName
                  : undefined
              }
            >
              {column.label}
            </th>
          ))}
          <th scope="col">
            <span className="ds-visually-hidden">History</span>
          </th>
        </tr>
      </thead>
      <tbody>{children}</tbody>
    </DataTable>
  )
}

function CatalogSkeleton() {
  return (
    <div className="mt-4 grid gap-px" aria-busy="true" role="status">
      <span className="ds-visually-hidden">Loading test catalog</span>
      <div className="h-8 rounded-[6px] bg-[var(--surface-fill)]" />
      {Array.from({ length: 8 }, (_, index) => (
        <div
          // biome-ignore lint/suspicious/noArrayIndexKey: static placeholders
          key={index}
          className="h-11 animate-pulse rounded-[6px] bg-[var(--surface-fill)] motion-reduce:animate-none"
        />
      ))}
    </div>
  )
}

export function TestsCatalogPage() {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [data, setData] = useState<TestsListResponse | null>(null)
  const [filters, setFilters] = useState<CatalogFilters>(() =>
    typeof window === 'undefined'
      ? CATALOG_DEFAULT_FILTERS
      : catalogFiltersFromParams(routeParams(window.location.hash)),
  )
  const [expandedGroups, setExpandedGroups] = useState<Set<Lifecycle>>(
    () => new Set(),
  )
  const [loading, setLoading] = useState(true)
  const [loadingMore, setLoadingMore] = useState(false)
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
  const [highlightId, setHighlightId] = useState<string | null>(() =>
    typeof window === 'undefined'
      ? null
      : routeParams(window.location.hash).get('highlight'),
  )
  const restoredScroll = useRef(false)

  useEffect(() => {
    let cancelled = false
    void getDashboardDataBridge()
      .then(async (next) => {
        if (cancelled) return
        setBridge(next)
        setData(await next.listTests({ limit: PAGE_SIZE }))
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

  // Audit T-07: cursor pagination instead of a silent cap at 100.
  const loadMore = async () => {
    if (!bridge || !data?.next_cursor) return
    setLoadingMore(true)
    try {
      const page = await bridge.listTests({
        limit: PAGE_SIZE,
        cursor: data.next_cursor,
      })
      setData({
        ...page,
        rows: [...data.rows, ...page.rows],
      })
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setLoadingMore(false)
    }
  }

  // Audit T-08: filters in the URL, scroll position kept across "back".
  useEffect(() => {
    const params = catalogFiltersToParams(filters)
    if (highlightId) params.set('highlight', highlightId)
    replaceRouteParams(params)
  }, [filters, highlightId])

  // A plain "#/tests" (the section link) clears the filters; the route
  // identity does not change, so the page listens for the hash itself.
  useEffect(() => {
    const sync = () => {
      const next = catalogFiltersFromParams(routeParams(window.location.hash))
      setFilters((current) =>
        catalogFiltersToParams(current).toString() ===
        catalogFiltersToParams(next).toString()
          ? current
          : next,
      )
    }
    window.addEventListener('hashchange', sync)
    return () => window.removeEventListener('hashchange', sync)
  }, [])

  useEffect(() => {
    const save = () => {
      try {
        window.sessionStorage.setItem(
          CATALOG_SCROLL_KEY,
          String(window.scrollY),
        )
      } catch {
        // storage unavailable
      }
    }
    window.addEventListener('pagehide', save)
    window.addEventListener('hashchange', save)
    return () => {
      window.removeEventListener('pagehide', save)
      window.removeEventListener('hashchange', save)
    }
  }, [])

  useEffect(() => {
    if (loading || restoredScroll.current) return
    restoredScroll.current = true
    if (highlightId) {
      const rowElement = document.getElementById(`test-${highlightId}`)
      rowElement?.scrollIntoView({ block: 'center' })
      const timer = window.setTimeout(() => setHighlightId(null), 2000)
      return () => window.clearTimeout(timer)
    }
    try {
      const saved = window.sessionStorage.getItem(CATALOG_SCROLL_KEY)
      if (saved) window.scrollTo(0, Number(saved))
    } catch {
      // storage unavailable
    }
  }, [loading, highlightId])

  // Audit NT-08: the toast dismisses itself.
  useEffect(() => {
    if (!createdScenarioId) return
    const timer = window.setTimeout(() => setCreatedScenarioId(null), 8000)
    return () => window.clearTimeout(timer)
  }, [createdScenarioId])

  const allRows = useMemo(
    () => mergeLocalScenariosIntoCatalog(data?.rows ?? [], localScenarios),
    [data, localScenarios],
  )
  const visibleRows = sortCatalogDisplayRows(
    filterCatalogRows(allRows, filters),
    filters.sort,
  )
  const filtered = catalogFiltersActive(filters)
  const setFilter = <K extends keyof CatalogFilters>(
    key: K,
    value: CatalogFilters[K],
  ) => setFilters((current) => ({ ...current, [key]: value }))
  const clearFilters = () => setFilters(CATALOG_DEFAULT_FILTERS)
  const localBridge = bridge?.mode === 'local' ? bridge : null
  const local = Boolean(localBridge)
  const suggestedLocalFileName = nextLocalScenarioFileName(localScenarios)
  const counts = {
    total: allRows.length,
    active: allRows.filter((entry) => entry.row.lifecycle === 'active').length,
    never_run: allRows.filter((entry) => entry.row.lifecycle === 'never_run')
      .length,
    retired: allRows.filter((entry) => entry.row.lifecycle === 'retired')
      .length,
    local: allRows.filter((entry) => entry.localScenario).length,
  }
  const complexityOptions = [
    ...new Set(
      allRows
        .map((entry) => entry.row.complexity?.tier)
        .filter((tier): tier is keyof typeof complexityTierLabels =>
          Boolean(tier),
        ),
    ),
  ].sort((a, b) => complexityRank[a] - complexityRank[b])
  const realismOptions = [
    ...new Set(
      allRows
        .map((entry) => entry.row.characterization?.realism?.execution)
        .filter((value): value is keyof typeof realismLabels => Boolean(value)),
    ),
  ]
  const groups =
    filters.sort === 'lifecycle' ? groupCatalogRows(visibleRows) : null
  const summary = [
    `${counts.total} test${counts.total === 1 ? '' : 's'}`,
    `${counts.active} active`,
    `${counts.never_run} never run`,
    counts.retired > 0 ? `${counts.retired} retired` : null,
    counts.local > 0 ? `${counts.local} local` : null,
  ]
    .filter(Boolean)
    .join(' · ')
  const totalKnown = data?.total ?? counts.total

  return (
    <>
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
      <div className="ds-root page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-16 md:w-[calc(100%_-_3rem)]">
        <PageHeader
          title="tests"
          summary={loading ? 'loading the catalog…' : summary}
          headingId="tests-catalog-title"
          actions={
            data?.revision ? <CatalogRevision revision={data.revision} /> : null
          }
        />
        {localBridge && authoringScenario ? (
          <LocalScenarioEditor
            bridge={localBridge}
            initialFileName={suggestedLocalFileName}
            onClose={() => setAuthoringScenario(false)}
            onCreated={(scenarioId) => {
              setCreatedScenarioId(scenarioId)
              setAuthoringScenario(false)
              setHighlightId(scenarioId)
              restoredScroll.current = false
              void loadLocalScenarios(localBridge)
            }}
          />
        ) : null}

        {createdScenarioId ? (
          // Audit NT-08: a toast with the next actions, not a bare status line.
          <Callout
            tone="success"
            title={`Created ${createdScenarioId}`}
            className="mt-4"
            data-created-toast
          >
            <p className="m-0">No execution was started.</p>
            <div className="mt-2 flex flex-wrap gap-2">
              <a
                className={buttonClassName({
                  variant: 'secondary',
                  size: 'compact',
                })}
                href={hashForTestHistory(createdScenarioId)}
              >
                view history
              </a>
              <a
                className={buttonClassName({
                  variant: 'secondary',
                  size: 'compact',
                })}
                href={hashForNewPlan()}
                onClick={() => {
                  try {
                    window.sessionStorage.setItem(
                      'harness-e2e:plan-scope',
                      JSON.stringify([createdScenarioId]),
                    )
                  } catch {
                    // storage unavailable
                  }
                }}
              >
                new plan with this test
              </a>
              <button
                className={buttonClassName({
                  variant: 'quiet',
                  size: 'compact',
                })}
                type="button"
                onClick={() => setCreatedScenarioId(null)}
              >
                dismiss
              </button>
            </div>
          </Callout>
        ) : null}

        {localCatalogError ? (
          <Callout
            tone="danger"
            title="Local tests could not be loaded"
            className="mt-4"
          >
            <p className="m-0">{localCatalogError}</p>
            {localBridge ? (
              <button
                className={buttonClassName({
                  variant: 'secondary',
                  size: 'compact',
                  className: 'mt-2',
                })}
                type="button"
                onClick={() => void loadLocalScenarios(localBridge)}
              >
                retry
              </button>
            ) : null}
          </Callout>
        ) : null}

        {!local && !loading ? (
          // Audit T-13: one note above the table instead of a repeated per-row line.
          <Callout tone="info" className="mt-4">
            New tests and plans are created from the local dashboard. Each row
            still opens the retained evidence for that test.
          </Callout>
        ) : null}

        <section className="mt-5 grid gap-3" aria-label="Test catalog filters">
          <div className="grid gap-3 @[720px]:grid-cols-[minmax(0,1fr)_auto] @[720px]:items-center">
            <div className="relative max-w-[28rem]">
              <Search
                className="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-ink-muted"
                size={14}
                aria-hidden="true"
              />
              <Input
                className="pr-9 pl-9"
                type="text"
                value={filters.query}
                placeholder="Filter by name, id or title…"
                aria-label="Search tests"
                onChange={(event) => setFilter('query', event.target.value)}
              />
              {filters.query ? (
                <button
                  className="absolute top-1/2 right-1 inline-grid size-7 -translate-y-1/2 place-items-center rounded-[6px] border-0 bg-transparent text-ink-muted hover:bg-[var(--surface-soft)] hover:text-ink"
                  type="button"
                  onClick={() => setFilter('query', '')}
                  aria-label="Clear search"
                >
                  <X size={13} aria-hidden="true" />
                </button>
              ) : null}
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <label
                className="ds-visually-hidden"
                htmlFor="tests-catalog-sort"
              >
                Sort tests
              </label>
              <Select
                id="tests-catalog-sort"
                value={filters.sort}
                onChange={(event) =>
                  setFilter('sort', event.target.value as SortKey)
                }
              >
                <option value="lifecycle">sort: lifecycle, name</option>
                <option value="name">sort: name</option>
                <option value="runs">sort: most runs</option>
                <option value="last_seen">sort: last seen</option>
                <option value="complexity">sort: complexity</option>
              </Select>
              {localBridge ? (
                // Audit T-04: a refresh, not a filter — icon only, named by
                // its tooltip.
                <button
                  className={buttonClassName({
                    variant: 'quiet',
                    size: 'compact',
                  })}
                  type="button"
                  onClick={() => void loadLocalScenarios(localBridge)}
                  disabled={localCatalogLoading}
                  title="Refresh local tests"
                  aria-label="Refresh local tests"
                >
                  <RefreshCw
                    className={localCatalogLoading ? 'animate-spin' : ''}
                    size={13}
                    aria-hidden="true"
                  />
                </button>
              ) : null}
            </div>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <FilterChipGroup label="Lifecycle">
              <FilterChip
                active={filters.lifecycle === 'all'}
                count={counts.total}
                onClick={() => setFilter('lifecycle', 'all')}
              >
                all
              </FilterChip>
              <FilterChip
                active={filters.lifecycle === 'active'}
                count={counts.active}
                onClick={() => setFilter('lifecycle', 'active')}
              >
                active
              </FilterChip>
              <FilterChip
                active={filters.lifecycle === 'never_run'}
                count={counts.never_run}
                onClick={() => setFilter('lifecycle', 'never_run')}
              >
                never run
              </FilterChip>
              {counts.retired > 0 ? (
                <FilterChip
                  active={filters.lifecycle === 'retired'}
                  count={counts.retired}
                  onClick={() => setFilter('lifecycle', 'retired')}
                >
                  retired
                </FilterChip>
              ) : null}
            </FilterChipGroup>
            {complexityOptions.length > 0 ? (
              <Select
                aria-label="Filter by complexity"
                className="max-w-[14rem]"
                value={filters.complexity}
                onChange={(event) =>
                  setFilter('complexity', event.target.value)
                }
              >
                <option value="all">complexity: all</option>
                {complexityOptions.map((tier) => (
                  <option key={tier} value={tier}>
                    {complexityTierLabels[tier]}
                  </option>
                ))}
                <option value="none">not declared</option>
              </Select>
            ) : null}
            {realismOptions.length > 0 ? (
              <Select
                aria-label="Filter by realism"
                className="max-w-[14rem]"
                value={filters.realism}
                onChange={(event) => setFilter('realism', event.target.value)}
              >
                <option value="all">realism: all</option>
                {realismOptions.map((value) => (
                  <option key={value} value={value}>
                    {realismLabels[value]}
                  </option>
                ))}
                <option value="none">not declared</option>
              </Select>
            ) : null}
            {counts.local > 0 ? (
              <Select
                aria-label="Filter by source"
                value={filters.source}
                onChange={(event) =>
                  setFilter('source', event.target.value as SourceFilter)
                }
              >
                <option value="all">source: all</option>
                <option value="local">local</option>
                <option value="registry">registry</option>
              </Select>
            ) : null}
            <FilterChipGroup label="Evidence">
              <FilterChip
                active={filters.withExecutions}
                onClick={() =>
                  setFilter('withExecutions', !filters.withExecutions)
                }
              >
                with executions
              </FilterChip>
            </FilterChipGroup>
            <output
              className="ms-auto font-mono text-label text-ink-muted"
              aria-live="polite"
            >
              {filtered
                ? `${visibleRows.length} of ${counts.total} tests`
                : `${counts.total} tests`}
              {data && data.total > data.rows.length
                ? ` · ${data.rows.length} of ${totalKnown} loaded`
                : ''}
            </output>
          </div>
        </section>

        {error ? (
          <EmptyState
            className="mt-6"
            tone="error"
            title="Test catalog unavailable"
            description={error}
          />
        ) : null}
        {!error && loading ? <CatalogSkeleton /> : null}
        {!error && !loading && visibleRows.length === 0 ? (
          <EmptyState
            className="mt-6"
            title={
              counts.total === 0
                ? 'No tests are registered yet'
                : 'No tests match these filters'
            }
            description={
              counts.total === 0
                ? local
                  ? 'Create a local Markdown test to start collecting evidence.'
                  : 'The registry has no scenarios for this workspace.'
                : 'Try a broader lifecycle, complexity or source, or clear the search.'
            }
            actions={
              counts.total === 0 ? (
                local ? (
                  <button
                    className={buttonClassName({ variant: 'primary' })}
                    type="button"
                    onClick={() => setAuthoringScenario(true)}
                    disabled={localCatalogLoading}
                  >
                    new test
                  </button>
                ) : null
              ) : (
                <button
                  className={buttonClassName({ variant: 'secondary' })}
                  type="button"
                  onClick={clearFilters}
                >
                  clear filters
                </button>
              )
            }
          />
        ) : null}
        {!error && !loading && visibleRows.length > 0 ? (
          <div className="mt-4 grid gap-6" data-catalog-rows>
            {groups ? (
              groups.map((group) => {
                const tone = lifecyclePresentation[group.lifecycle]
                const expanded = expandedGroups.has(group.lifecycle)
                const collapsedByDefault = group.lifecycle === 'retired'
                const shown =
                  expanded || filtered
                    ? group.rows
                    : group.rows.slice(0, GROUP_PREVIEW)
                const hidden = group.rows.length - shown.length
                const toggle = () =>
                  setExpandedGroups((current) => {
                    const next = new Set(current)
                    if (next.has(group.lifecycle)) next.delete(group.lifecycle)
                    else next.add(group.lifecycle)
                    return next
                  })
                // Audit T-12: retired tests stay out of the way until asked for.
                if (collapsedByDefault && !expanded && !filtered) {
                  return (
                    <div
                      key={group.lifecycle}
                      data-catalog-group={group.lifecycle}
                    >
                      <button
                        className={buttonClassName({
                          variant: 'quiet',
                          size: 'compact',
                        })}
                        type="button"
                        onClick={toggle}
                        aria-expanded={false}
                      >
                        <span
                          className={`ds-status-dot ${tone.dotClassName}`}
                          aria-hidden="true"
                        />
                        {group.rows.length} retired test
                        {group.rows.length === 1 ? '' : 's'}
                        <ChevronRight size={13} aria-hidden="true" />
                      </button>
                    </div>
                  )
                }
                return (
                  <section
                    key={group.lifecycle}
                    aria-labelledby={`catalog-group-${group.lifecycle}`}
                    data-catalog-group={group.lifecycle}
                  >
                    <h2
                      id={`catalog-group-${group.lifecycle}`}
                      className="ds-label mb-2 flex items-center gap-2"
                    >
                      <span
                        className={`ds-status-dot ${tone.dotClassName}`}
                        aria-hidden="true"
                      />
                      {tone.label} · {group.rows.length}
                    </h2>
                    <CatalogTable
                      caption={`${tone.label} tests, ${group.rows.length}`}
                    >
                      <CatalogRows
                        rows={shown}
                        local={local}
                        highlightId={highlightId}
                        showLifecycle={false}
                      />
                    </CatalogTable>
                    {hidden > 0 ||
                    (expanded && group.rows.length > GROUP_PREVIEW) ? (
                      <button
                        className={buttonClassName({
                          variant: 'quiet',
                          size: 'compact',
                          className: 'mt-1',
                        })}
                        type="button"
                        onClick={toggle}
                        aria-expanded={expanded}
                      >
                        {hidden > 0
                          ? `show ${hidden} more ${tone.label}`
                          : `show fewer ${tone.label}`}
                        <ChevronRight size={13} aria-hidden="true" />
                      </button>
                    ) : null}
                  </section>
                )
              })
            ) : (
              <CatalogTable
                caption={`Test catalog, ${visibleRows.length} of ${counts.total} tests`}
              >
                <CatalogRows
                  rows={visibleRows}
                  local={local}
                  highlightId={highlightId}
                  showLifecycle
                />
              </CatalogTable>
            )}
            {data?.next_cursor ? (
              <div className="flex items-center gap-3">
                <button
                  className={buttonClassName({ variant: 'secondary' })}
                  type="button"
                  onClick={() => void loadMore()}
                  disabled={loadingMore}
                  aria-busy={loadingMore}
                >
                  {loadingMore ? 'loading…' : 'load more tests'}
                </button>
                <span className="font-mono text-label text-ink-muted">
                  {data.rows.length} of {totalKnown} loaded
                </span>
              </div>
            ) : null}
          </div>
        ) : null}
      </div>
    </>
  )
}
