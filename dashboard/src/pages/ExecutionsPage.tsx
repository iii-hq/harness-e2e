import { ArrowRight, Search, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { DashboardPageActions } from '@/components/DashboardPageActions'
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
  StatusBadge,
} from '@/design-system'
import {
  hashForExecution,
  replaceRouteParams,
  routeParams,
} from '@/hooks/use-hash-route'
import { useLatestRequest } from '@/hooks/use-latest-request'
import {
  type DashboardDataBridge,
  type DashboardExecutionSummary,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import {
  buildExecutionPresentation,
  categoryMessage,
  type ExecutionPresentation,
  formatDate,
  formatDuration,
  formatPercent,
} from '@/lib/execution-view'
import { executionTitle, percentPoints } from '@/lib/overview-signal'
import { modelNames, statusCopy } from '@/pages/OverviewPage'
import '@/design-system/styles.css'

const PAGE_SIZE = 50

const triggerLabels: Record<string, string> = {
  schedule: 'scheduled',
  workflow_dispatch: 'manual',
  local: 'local',
}

export function triggerLabel(event: string) {
  return triggerLabels[event] ?? event.replace(/[_-]+/g, ' ')
}

export type LedgerSort = 'newest' | 'oldest' | 'runtime' | 'tokens' | 'result'

export type LedgerFilters = {
  query: string
  status: string
  event: string
  sort: LedgerSort
}

export const LEDGER_DEFAULT_FILTERS: LedgerFilters = {
  query: '',
  status: 'all',
  event: 'all',
  sort: 'newest',
}

const SORTS: LedgerSort[] = ['newest', 'oldest', 'runtime', 'tokens', 'result']

/** Audit E-04: the ledger's filters live in the hash, not only in state. */
export function ledgerFiltersFromParams(
  params: URLSearchParams,
): LedgerFilters {
  const sort = params.get('sort')
  return {
    query: params.get('q') ?? '',
    status: params.get('status') ?? 'all',
    event: params.get('event') ?? 'all',
    sort:
      sort && (SORTS as string[]).includes(sort)
        ? (sort as LedgerSort)
        : 'newest',
  }
}

export function ledgerFiltersToParams(filters: LedgerFilters): URLSearchParams {
  const params = new URLSearchParams()
  if (filters.query.trim()) params.set('q', filters.query.trim())
  if (filters.status !== 'all') params.set('status', filters.status)
  if (filters.event !== 'all') params.set('event', filters.event)
  if (filters.sort !== 'newest') params.set('sort', filters.sort)
  return params
}

export type LedgerRow = {
  execution: DashboardExecutionSummary
  presentation: ExecutionPresentation
  status: ReturnType<typeof statusCopy>
  searchText: string
}

export function buildLedgerRows(
  executions: DashboardExecutionSummary[],
): LedgerRow[] {
  return executions.map((execution) => {
    const presentation = buildExecutionPresentation(execution)
    const { title, detail } = executionTitle(presentation)
    return {
      execution,
      presentation,
      status: statusCopy(presentation),
      searchText: [
        title,
        detail,
        execution.label,
        execution.workflow_name,
        execution.id,
        execution.run_id,
        formatDate(presentation.completedAt),
        execution.source?.sha,
        ...presentation.subjects.flatMap((model) => [
          model.model,
          `${model.provider}/${model.model}`,
        ]),
      ]
        .filter(Boolean)
        .join(' ')
        .toLowerCase(),
    }
  })
}

function tokensOf(row: LedgerRow) {
  const value = row.execution.totals?.total_tokens
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

const RESULT_ORDER = [
  'failed',
  'hard_gate',
  'inconclusive',
  'incomplete',
  'running',
  'cancelling',
  'cancelled',
  'unavailable',
  'passed',
]

export function filterLedgerRows(rows: LedgerRow[], filters: LedgerFilters) {
  const query = filters.query.trim().toLowerCase()
  const matched = rows.filter((row) => {
    if (filters.status !== 'all' && row.status.status !== filters.status)
      return false
    if (filters.event !== 'all' && row.execution.event !== filters.event)
      return false
    return !query || row.searchText.includes(query)
  })
  const byDateDesc = (left: LedgerRow, right: LedgerRow) =>
    Date.parse(right.presentation.completedAt || '') -
    Date.parse(left.presentation.completedAt || '')
  const sorted = [...matched]
  if (filters.sort === 'oldest') sorted.sort((a, b) => byDateDesc(b, a))
  else if (filters.sort === 'runtime')
    sorted.sort(
      (a, b) =>
        (b.presentation.modelRuntimeSeconds ?? -1) -
          (a.presentation.modelRuntimeSeconds ?? -1) || byDateDesc(a, b),
    )
  else if (filters.sort === 'tokens')
    sorted.sort(
      (a, b) => (tokensOf(b) ?? -1) - (tokensOf(a) ?? -1) || byDateDesc(a, b),
    )
  else if (filters.sort === 'result')
    sorted.sort(
      (a, b) =>
        RESULT_ORDER.indexOf(a.status.status) -
          RESULT_ORDER.indexOf(b.status.status) || byDateDesc(a, b),
    )
  else sorted.sort(byDateDesc)
  return sorted
}

function dayKey(value: string) {
  const timestamp = Date.parse(value)
  if (!Number.isFinite(timestamp)) return 'unknown'
  const date = new Date(timestamp)
  return `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`
}

export function dayLabel(value: string, now = Date.now()) {
  const timestamp = Date.parse(value)
  if (!Number.isFinite(timestamp)) return 'date not reported'
  const day = new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
  }).format(new Date(timestamp))
  if (dayKey(value) === dayKey(new Date(now).toISOString()))
    return `today · ${day}`
  if (dayKey(value) === dayKey(new Date(now - 86_400_000).toISOString()))
    return `yesterday · ${day}`
  return day
}

/** Audit E-12: a running execution is pinned above the day groups. */
export function groupLedgerRows(rows: LedgerRow[], now = Date.now()) {
  const running = rows.filter(
    (row) =>
      row.status.status === 'running' || row.status.status === 'cancelling',
  )
  const settled = rows.filter((row) => !running.includes(row))
  const groups: Array<{ key: string; label: string; rows: LedgerRow[] }> = []
  for (const row of settled) {
    const key = dayKey(row.presentation.completedAt)
    const last = groups.at(-1)
    if (last?.key === key) last.rows.push(row)
    else
      groups.push({
        key,
        label: dayLabel(row.presentation.completedAt, now),
        rows: [row],
      })
  }
  return { running, groups }
}

function LedgerRowCells({ row }: { row: LedgerRow }) {
  const { presentation, execution, status } = row
  const { title, detail } = executionTitle(presentation)
  const tokens = tokensOf(row)
  const evidenceNote =
    execution.availability === 'aggregate'
      ? 'aggregate report'
      : execution.availability === 'unavailable'
        ? 'no report retained'
        : null
  return (
    <>
      <td data-label="Execution" className="ds-table-sticky-col">
        <a
          className="block truncate font-mono text-xs font-medium text-ink no-underline hover:underline"
          href={hashForExecution(execution.id)}
          title={title}
        >
          {title}
        </a>
        <span className="block truncate font-mono text-label text-ink-muted">
          {formatDate(presentation.completedAt)}
          {detail ? ` · ${detail}` : ''}
          {execution.event ? ` · ${triggerLabel(String(execution.event))}` : ''}
        </span>
      </td>
      <td data-label="Result">
        <StatusBadge status={status.status} label={status.label} />
        {presentation.primaryIssue ? (
          <span className="block font-mono text-label text-ink-soft">
            {categoryMessage(
              presentation.primaryIssue.category,
              presentation.primaryIssue.count,
            )}
          </span>
        ) : evidenceNote ? (
          <span className="block font-mono text-label text-ink-muted">
            {evidenceNote}
          </span>
        ) : null}
      </td>
      <td
        data-label="Subject · judge"
        title={modelNames(presentation.subjects)}
      >
        <span className="block font-mono text-xs text-ink">
          {presentation.subjects[0]?.model ?? '—'}
        </span>
        <span className="block font-mono text-label text-ink-muted">
          {presentation.subjects[0]?.provider ?? ''}
          {presentation.judges.length > 0
            ? ` · judge ${presentation.judges[0].model}`
            : ' · judge automatic'}
        </span>
      </td>
      <td data-label="Scope" className={numericCellClassName}>
        {presentation.receivedReports === null &&
        presentation.expectedReports === null
          ? '—'
          : `${presentation.receivedReports ?? '—'}/${presentation.expectedReports ?? '—'}`}
      </td>
      <td data-label="Pass rate" className={numericCellClassName}>
        {presentation.passRate === null
          ? '—'
          : formatPercent(percentPoints(presentation.passRate), false)}
      </td>
      <td data-label="Duration" className={numericCellClassName}>
        {presentation.modelRuntimeSeconds === null
          ? '—'
          : formatDuration(presentation.modelRuntimeSeconds)}
      </td>
      <td data-label="Tokens" className={numericCellClassName}>
        {tokens === null ? '—' : tokens.toLocaleString()}
      </td>
      <td data-label="Open" className="text-right">
        <a
          className={buttonClassName({
            variant: 'quiet',
            size: 'compact',
            className: 'no-underline',
          })}
          href={hashForExecution(execution.id)}
          aria-label={`Open ${title}`}
        >
          open
          <ArrowRight size={13} aria-hidden="true" />
        </a>
      </td>
    </>
  )
}

/**
 * One table for the whole page: the day groups are separator rows so the
 * header is read once and the rhythm stays (audit E-07 / E-12).
 */
function LedgerTable({
  caption,
  groups,
}: {
  caption: string
  groups: Array<{ key: string; label: string; rows: LedgerRow[] }>
}) {
  return (
    <DataTable
      caption={caption}
      collapse
      collapseInline
      minWidth="58rem"
      sticky
      data-ledger-table
    >
      <thead>
        <tr>
          <th scope="col">execution</th>
          <th scope="col">result</th>
          <th scope="col">subject · judge</th>
          <th scope="col" className={numericCellClassName}>
            scope
          </th>
          <th scope="col" className={numericCellClassName}>
            pass rate
          </th>
          <th scope="col" className={numericCellClassName}>
            duration
          </th>
          <th scope="col" className={numericCellClassName}>
            tokens
          </th>
          <th scope="col">
            <span className="ds-visually-hidden">Open</span>
          </th>
        </tr>
      </thead>
      {groups.map((group) => (
        <tbody key={group.key} data-ledger-group={group.key}>
          <tr data-ledger-day>
            <th className="ds-label" colSpan={8} scope="colgroup">
              {group.label} · {group.rows.length}
            </th>
          </tr>
          {group.rows.map((row) => (
            <DataTableRow
              key={row.execution.id}
              href={hashForExecution(row.execution.id)}
              data-execution-id={row.execution.id}
              data-result={row.status.status}
            >
              <LedgerRowCells row={row} />
            </DataTableRow>
          ))}
        </tbody>
      ))}
    </DataTable>
  )
}

export function ExecutionsPage() {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [executions, setExecutions] = useState<DashboardExecutionSummary[]>([])
  const [cursor, setCursor] = useState<string | null>(null)
  const [total, setTotal] = useState(0)
  const [filters, setFilters] = useState<LedgerFilters>(() =>
    typeof window === 'undefined'
      ? LEDGER_DEFAULT_FILTERS
      : ledgerFiltersFromParams(routeParams(window.location.hash)),
  )
  const [loading, setLoading] = useState(true)
  const [loadingMore, setLoadingMore] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const beginRequest = useLatestRequest()
  const loaded = useRef(false)

  const load = useCallback(async () => {
    const request = beginRequest()
    setError(null)
    try {
      const nextBridge = bridge ?? (await getDashboardDataBridge())
      if (!request.isCurrent()) return
      setBridge(nextBridge)
      const manifest = await nextBridge.listExecutions({ limit: PAGE_SIZE })
      if (!request.isCurrent()) return
      setExecutions(manifest.executions ?? [])
      setCursor(manifest.next_cursor ?? null)
      setTotal(manifest.total ?? manifest.executions?.length ?? 0)
      loaded.current = true
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setLoading(false)
    }
  }, [beginRequest, bridge])

  useEffect(() => {
    void load()
  }, [load])

  // Audit E-12: the ledger follows run changes instead of waiting for F5.
  useEffect(() => {
    if (!bridge) return
    let cancelled = false
    let dispose: (() => void) | undefined
    let timer: number | undefined
    bridge
      .subscribeRunChanges(() => {
        if (timer) window.clearTimeout(timer)
        timer = window.setTimeout(() => void load(), 400)
      })
      .then((off) => {
        if (cancelled) off()
        else dispose = off
      })
      .catch(() => undefined)
    return () => {
      cancelled = true
      if (timer) window.clearTimeout(timer)
      dispose?.()
    }
  }, [bridge, load])

  useEffect(() => {
    replaceRouteParams(ledgerFiltersToParams(filters))
  }, [filters])

  // Audit E-05: more executions arrive by cursor, never silently truncated.
  const loadMore = async () => {
    if (!bridge || !cursor) return
    setLoadingMore(true)
    try {
      const page = await bridge.listExecutions({ limit: PAGE_SIZE, cursor })
      setExecutions((current) => [...current, ...(page.executions ?? [])])
      setCursor(page.next_cursor ?? null)
      setTotal(page.total ?? total)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setLoadingMore(false)
    }
  }

  const rows = useMemo(() => buildLedgerRows(executions), [executions])
  const visible = useMemo(
    () => filterLedgerRows(rows, filters),
    [rows, filters],
  )
  const { running, groups } = useMemo(() => groupLedgerRows(visible), [visible])
  const setFilter = <K extends keyof LedgerFilters>(
    key: K,
    value: LedgerFilters[K],
  ) => setFilters((current) => ({ ...current, [key]: value }))
  const filtered = ledgerFiltersToParams(filters).toString() !== ''

  const statusCounts = useMemo(() => {
    const counts = new Map<string, { label: string; count: number }>()
    for (const row of rows) {
      const entry = counts.get(row.status.status)
      counts.set(row.status.status, {
        label: row.status.label,
        count: (entry?.count ?? 0) + 1,
      })
    }
    return [...counts.entries()].sort(
      ([left], [right]) =>
        RESULT_ORDER.indexOf(left) - RESULT_ORDER.indexOf(right),
    )
  }, [rows])
  const eventCounts = useMemo(() => {
    const counts = new Map<string, number>()
    for (const row of rows) {
      const event = row.execution.event
      if (typeof event !== 'string' || !event) continue
      counts.set(event, (counts.get(event) ?? 0) + 1)
    }
    return [...counts.entries()]
  }, [rows])

  // Audit E-07: the page says what the ledger holds, in the column vocabulary.
  const summary = [
    `${total || rows.length} execution${(total || rows.length) === 1 ? '' : 's'}`,
    ...statusCounts.map(([, entry]) => `${entry.count} ${entry.label}`),
  ].join(' · ')

  return (
    <div className="ds-root min-h-dvh bg-canvas text-ink">
      <DashboardPageActions
        active="executions"
        actionsLabel="Execution actions"
      />
      <div className="page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-16 md:w-[calc(100%_-_3rem)]">
        <PageHeader
          title="executions"
          summary={
            loading && rows.length === 0 ? 'loading the ledger…' : summary
          }
          headingId="executions-title"
          context="immutable run ledger"
        />

        {error ? (
          <Callout
            tone="danger"
            title="Executions could not be loaded"
            className="mt-6"
          >
            <span className="flex flex-wrap items-center justify-between gap-3">
              {error}
              <button
                className={buttonClassName({
                  variant: 'secondary',
                  size: 'compact',
                })}
                type="button"
                onClick={() => void load()}
              >
                retry
              </button>
            </span>
          </Callout>
        ) : null}

        {/* Audit E-13 / RD-05: one control vocabulary, an explicit grid. */}
        <section className="mt-5 grid gap-3" aria-label="Execution filters">
          <div className="grid gap-3 @[720px]:grid-cols-[minmax(0,1fr)_auto_auto] @[720px]:items-center">
            <div className="relative max-w-[28rem]">
              <Search
                className="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-ink-muted"
                size={14}
                aria-hidden="true"
              />
              <Input
                className="pr-9 pl-9 font-mono"
                type="text"
                value={filters.query}
                placeholder="Search label, model, plan, id or date…"
                aria-label="Search executions"
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
            {eventCounts.length > 1 ? (
              <Select
                aria-label="Filter by trigger"
                className="max-w-[14rem]"
                value={filters.event}
                onChange={(event) => setFilter('event', event.target.value)}
              >
                <option value="all">all triggers · {rows.length}</option>
                {eventCounts.map(([value, count]) => (
                  <option key={value} value={value}>
                    {triggerLabel(value)} · {count}
                  </option>
                ))}
              </Select>
            ) : null}
            <Select
              aria-label="Sort executions"
              className="max-w-[14rem]"
              value={filters.sort}
              onChange={(event) =>
                setFilter('sort', event.target.value as LedgerSort)
              }
            >
              <option value="newest">newest first</option>
              <option value="oldest">oldest first</option>
              <option value="result">result</option>
              <option value="runtime">longest duration</option>
              <option value="tokens">most tokens</option>
            </Select>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <FilterChipGroup label="Result">
              <FilterChip
                active={filters.status === 'all'}
                count={rows.length}
                onClick={() => setFilter('status', 'all')}
              >
                all
              </FilterChip>
              {statusCounts.map(([status, entry]) => (
                <FilterChip
                  key={status}
                  active={filters.status === status}
                  count={entry.count}
                  onClick={() => setFilter('status', status)}
                >
                  {entry.label}
                </FilterChip>
              ))}
            </FilterChipGroup>
            <output
              className="ms-auto font-mono text-label text-ink-muted"
              aria-live="polite"
            >
              showing {visible.length} of {rows.length} loaded
              {total > rows.length ? ` · ${total} retained` : ''}
            </output>
          </div>
        </section>

        {loading && rows.length === 0 ? (
          <div className="mt-4 grid gap-px" aria-busy="true" role="status">
            <span className="ds-visually-hidden">Loading executions</span>
            {Array.from({ length: 6 }, (_, index) => (
              <div
                // biome-ignore lint/suspicious/noArrayIndexKey: static placeholders
                key={index}
                className="h-12 animate-pulse rounded-[6px] bg-[var(--surface-fill)] motion-reduce:animate-none"
              />
            ))}
          </div>
        ) : visible.length === 0 ? (
          <EmptyState
            className="mt-6"
            title={
              rows.length === 0
                ? 'No executions retained yet'
                : 'No executions match these filters'
            }
            description={
              rows.length === 0
                ? 'Run the suite once to publish the first execution.'
                : 'Widen the result or trigger filter, or clear the search.'
            }
            actions={
              filtered ? (
                <button
                  className={buttonClassName({ variant: 'secondary' })}
                  type="button"
                  onClick={() => setFilters(LEDGER_DEFAULT_FILTERS)}
                >
                  clear filters
                </button>
              ) : null
            }
          />
        ) : (
          <div className="mt-4 grid min-w-0 gap-6" data-ledger>
            <LedgerTable
              caption={`Executions, ${visible.length} of ${rows.length} loaded`}
              groups={
                running.length > 0
                  ? [
                      { key: 'running', label: 'running', rows: running },
                      ...groups,
                    ]
                  : groups
              }
            />
            {cursor ? (
              <div className="flex flex-wrap items-center gap-3">
                <button
                  className={buttonClassName({ variant: 'secondary' })}
                  type="button"
                  onClick={() => void loadMore()}
                  disabled={loadingMore}
                  aria-busy={loadingMore}
                >
                  {loadingMore ? 'loading…' : `load ${PAGE_SIZE} more`}
                </button>
                <span className="font-mono text-label text-ink-muted">
                  {rows.length} of {total} loaded
                </span>
              </div>
            ) : null}
          </div>
        )}
      </div>
    </div>
  )
}
