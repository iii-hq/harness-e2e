import {
  Badge,
  type BadgeVariant,
  Button,
  EmptyState,
  Input,
  Select,
  Skeleton,
  StatusPanel,
  Table,
  TableBody,
  TableCaption,
  TableCell,
  TableFrame,
  TableHead,
  TableHeader,
  TableRow,
  TableViewport,
} from '@iii-dev/console-ui'
import { ArrowRight, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { consumeQuickExecutionRequest } from '@/components/ExecutionSetup'
import { LocalRunnerDialog } from '@/components/LocalRunnerDialog'
import { PageHeader } from '@/design-system'
import {
  hashForExecution,
  hashForNewPlan,
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
  categoryMessage,
  executionTitle,
  formatDate,
  formatDuration,
  formatPercent,
  modelNames,
  percentPoints,
} from '@/lib/execution-view'
import {
  buildLedgerRows,
  filterLedgerRows,
  groupHeading,
  groupLedgerRows,
  LEDGER_DEFAULT_FILTERS,
  type LedgerFilters,
  type LedgerGroup,
  type LedgerRow,
  type LedgerSort,
  ledgerFiltersFromParams,
  ledgerFiltersToParams,
  PAGE_SIZE,
  RESULT_ORDER,
  tokensOf,
  triggerLabel,
} from '@/lib/executions-ledger'
import '@/design-system/styles.css'

/* The pilot screen of the redesign: it renders with the Console's own
   components and tokens, keeps the ledger logic in `lib/executions-ledger`,
   and carries no page-specific CSS. */

/** The Console's status vocabulary for the ledger's result column. */
export function resultBadgeVariant(
  status: LedgerRow['status']['status'],
): BadgeVariant {
  switch (status) {
    case 'passed':
      return 'ok'
    case 'failed':
      return 'alert'
    case 'inconclusive':
    case 'incomplete':
    case 'cancelling':
      return 'warn'
    case 'running':
      return 'accent'
    default:
      return 'default'
  }
}

const NUMERIC = 'text-right font-mono tabular-nums'
const META = 'block truncate font-mono text-xs text-ink-faint'
const SORT_OPTIONS: Array<{ value: LedgerSort; label: string }> = [
  { value: 'newest', label: 'newest first' },
  { value: 'oldest', label: 'oldest first' },
  { value: 'result', label: 'by result' },
  { value: 'runtime', label: 'longest runtime' },
  { value: 'tokens', label: 'most tokens' },
]

function placeholder(value: string | null) {
  return value ?? '—'
}

function LedgerCells({ row }: { row: LedgerRow }) {
  const { presentation, execution, status } = row
  const { title, detail } = executionTitle(presentation)
  const tokens = tokensOf(row)
  // Where it ran and how it started, without repeating the default case:
  // a local, locally triggered run states only its date and system.
  const origin = execution.id.startsWith('rc:') ? 'team · RC' : null
  const trigger =
    execution.event && String(execution.event) !== 'local'
      ? triggerLabel(String(execution.event))
      : null
  const note = presentation.primaryIssue
    ? categoryMessage(
        presentation.primaryIssue.category,
        presentation.primaryIssue.count,
      )
    : execution.availability === 'aggregate'
      ? 'aggregate report'
      : execution.availability === 'unavailable'
        ? 'no report retained'
        : null
  return (
    <>
      <TableCell>
        <a
          className="block truncate font-mono text-sm font-medium text-ink no-underline hover:underline"
          href={hashForExecution(execution.id)}
          title={title}
        >
          {title}
        </a>
        <span className={META}>
          {[origin, trigger, formatDate(presentation.completedAt), detail]
            .filter(Boolean)
            .join(' · ')}
        </span>
      </TableCell>
      <TableCell>
        <Badge variant={resultBadgeVariant(status.status)}>
          {status.label}
        </Badge>
        {note ? <span className={META}>{note}</span> : null}
      </TableCell>
      <TableCell title={modelNames(presentation.subjects)}>
        <span className="block truncate font-mono text-sm text-ink">
          {presentation.subjects[0]?.model ?? '—'}
        </span>
        {presentation.subjects[0]?.provider ? (
          <span className={META}>{presentation.subjects[0].provider}</span>
        ) : null}
      </TableCell>
      <TableCell
        className={NUMERIC}
        title={
          presentation.expectedReports === null
            ? undefined
            : `${presentation.receivedReports ?? 0} of ${presentation.expectedReports} test reports received`
        }
      >
        {presentation.receivedReports === null &&
        presentation.expectedReports === null
          ? '—'
          : `${presentation.receivedReports ?? '—'}/${presentation.expectedReports ?? '—'}`}
      </TableCell>
      <TableCell className={NUMERIC}>
        {placeholder(
          presentation.passRate === null
            ? null
            : formatPercent(percentPoints(presentation.passRate), false),
        )}
      </TableCell>
      <TableCell className={NUMERIC}>
        {placeholder(
          presentation.modelRuntimeSeconds === null
            ? null
            : formatDuration(presentation.modelRuntimeSeconds),
        )}
      </TableCell>
      <TableCell className={NUMERIC}>
        {placeholder(tokens === null ? null : tokens.toLocaleString())}
      </TableCell>
    </>
  )
}

const COLUMNS = 7

/** One table for the whole ledger; running runs first, then plan and day groups. */
export function LedgerTable({
  caption,
  groups,
}: {
  caption: string
  groups: LedgerGroup[]
}) {
  return (
    <TableViewport>
      <TableFrame>
        <Table density="compact" data-ledger-table>
          <TableCaption className="sr-only">{caption}</TableCaption>
          <TableHeader>
            <TableRow>
              <TableHead scope="col">execution</TableHead>
              <TableHead scope="col">result</TableHead>
              <TableHead scope="col">subject</TableHead>
              <TableHead
                scope="col"
                className="text-right"
                title="test reports received / expected"
              >
                tests
              </TableHead>
              <TableHead scope="col" className="text-right">
                pass rate
              </TableHead>
              <TableHead scope="col" className="text-right">
                runtime
              </TableHead>
              <TableHead scope="col" className="text-right">
                tokens
              </TableHead>
            </TableRow>
          </TableHeader>
          {groups.map((group) => (
            <TableBody key={group.key} data-ledger-group={group.key}>
              <TableRow
                data-ledger-day
                data-ledger-plan={group.plan?.execution_id}
              >
                <TableHead
                  scope="colgroup"
                  colSpan={COLUMNS}
                  className="pt-4 text-xs font-semibold text-ink-faint"
                >
                  {group.plan ? (
                    <a
                      className="text-ink-faint no-underline hover:text-ink hover:underline"
                      href={`#/ext/harness-e2e/execution/${group.plan.execution_id}`}
                    >
                      {groupHeading(group)}
                    </a>
                  ) : (
                    groupHeading(group)
                  )}
                </TableHead>
              </TableRow>
              {group.rows.map((row) => (
                <TableRow
                  key={row.execution.id}
                  interactive
                  data-execution-id={row.execution.id}
                  data-result={row.status.status}
                  onClick={(event) => {
                    if (
                      event.defaultPrevented ||
                      (event.target instanceof Element &&
                        event.target.closest('a, button'))
                    )
                      return
                    window.location.hash = hashForExecution(row.execution.id)
                  }}
                >
                  <LedgerCells row={row} />
                </TableRow>
              ))}
            </TableBody>
          ))}
        </Table>
      </TableFrame>
    </TableViewport>
  )
}

function LedgerSkeleton() {
  return (
    <div className="mt-4 grid gap-2" aria-busy="true" role="status">
      <span className="sr-only">Loading executions</span>
      {Array.from({ length: 6 }, (_, index) => (
        <Skeleton
          // biome-ignore lint/suspicious/noArrayIndexKey: static placeholders
          key={index}
          className="block h-11 w-full"
        />
      ))}
    </div>
  )
}

export function ExecutionsPage() {
  const [runnerOpen, setRunnerOpen] = useState(false)
  const [runnerScope, setRunnerScope] = useState<string[]>([])
  useEffect(() => {
    const requested = consumeQuickExecutionRequest()
    if (requested) {
      setRunnerScope(requested)
      setRunnerOpen(true)
    }
  }, [])
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

  // The ledger follows run changes instead of waiting for a reload.
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

  const runningCount = rows.filter(
    (row) =>
      row.status.status === 'running' || row.status.status === 'cancelling',
  ).length
  // One sentence with one denominator: what is retained, and what is live.
  const summary = [
    `${total} execution${total === 1 ? '' : 's'} retained`,
    runningCount > 0 ? `${runningCount} running` : null,
    total > rows.length ? `${rows.length} loaded` : null,
  ]
    .filter(Boolean)
    .join(' · ')

  const openRunner = () => {
    setRunnerScope([])
    setRunnerOpen(true)
  }

  return (
    <div className="min-h-dvh bg-panel text-ink">
      <DashboardPageActions
        active="executions"
        actionsLabel="Execution actions"
        actions={
          bridge ? (
            <>
              <Button variant="pill" size="sm" asChild>
                <a href={hashForNewPlan()}>New plan</a>
              </Button>
              <Button
                variant="primary"
                size="sm"
                type="button"
                onClick={openRunner}
              >
                Run tests
              </Button>
            </>
          ) : null
        }
      />
      <div className="mx-auto w-full max-w-[var(--spacing-content-max)] px-4 pt-5 pb-16 md:px-6">
        <PageHeader
          title="executions"
          summary={
            loading && rows.length === 0 ? 'loading the ledger…' : summary
          }
          headingId="executions-title"
          context="Recent activity and retained evidence"
        />

        {error ? (
          <div className="mt-6">
            <StatusPanel
              variant="alert"
              headline="Executions could not be loaded"
              detail={
                <span className="flex flex-wrap items-center gap-3">
                  <span>{error}</span>
                  <Button
                    variant="pill"
                    size="sm"
                    type="button"
                    onClick={() => void load()}
                  >
                    retry
                  </Button>
                </span>
              }
            />
          </div>
        ) : null}

        <section
          className="mt-5 flex flex-wrap items-center gap-2"
          aria-label="Execution filters"
        >
          <div className="relative min-w-56 flex-1 basis-64">
            <Input
              type="search"
              value={filters.query}
              placeholder="Search label, model, plan, id or date…"
              aria-label="Search executions"
              onChange={(next) => setFilter('query', next)}
            />
            {filters.query ? (
              <Button
                variant="icon"
                size="icon"
                type="button"
                className="absolute top-1/2 right-1 -translate-y-1/2"
                onClick={() => setFilter('query', '')}
                aria-label="Clear search"
              >
                <X aria-hidden="true" />
              </Button>
            ) : null}
          </div>
          {eventCounts.length > 1 ? (
            <Select
              aria-label="Filter by trigger"
              value={filters.event}
              onChange={(next) => setFilter('event', next)}
              options={[
                { value: 'all', label: `all triggers · ${rows.length}` },
                ...eventCounts.map(([value, count]) => ({
                  value,
                  label: `${triggerLabel(value)} · ${count}`,
                })),
              ]}
            />
          ) : null}
          <Select
            aria-label="Sort executions"
            value={filters.sort}
            onChange={(next) => setFilter('sort', next as LedgerSort)}
            options={SORT_OPTIONS}
          />
          <fieldset className="m-0 flex min-w-0 basis-full flex-wrap items-center gap-2 border-0 p-0">
            <legend className="sr-only">Result</legend>
            <ResultFilter
              active={filters.status === 'all'}
              count={rows.length}
              onClick={() => setFilter('status', 'all')}
            >
              all
            </ResultFilter>
            {statusCounts.map(([status, entry]) => (
              <ResultFilter
                key={status}
                active={filters.status === status}
                count={entry.count}
                onClick={() => setFilter('status', status)}
              >
                {entry.label}
              </ResultFilter>
            ))}
            {filtered ? (
              <output
                className="ms-auto font-mono text-xs text-ink-faint"
                aria-live="polite"
              >
                showing {visible.length} of {rows.length} loaded
              </output>
            ) : null}
          </fieldset>
        </section>

        {loading && rows.length === 0 ? (
          <LedgerSkeleton />
        ) : visible.length === 0 ? (
          <div className="mt-6">
            <EmptyState
              title={
                rows.length === 0
                  ? 'No executions retained yet'
                  : 'No executions match these filters'
              }
              description={
                rows.length === 0
                  ? 'Run tests or create a plan to start retaining execution evidence.'
                  : 'Widen the result or trigger filter, or clear the search.'
              }
              action={
                rows.length === 0
                  ? { label: 'run tests', onClick: openRunner }
                  : {
                      label: 'clear filters',
                      onClick: () => setFilters(LEDGER_DEFAULT_FILTERS),
                    }
              }
            />
          </div>
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
                <Button
                  variant="pill"
                  size="sm"
                  type="button"
                  onClick={() => void loadMore()}
                  disabled={loadingMore}
                  aria-busy={loadingMore}
                >
                  {loadingMore ? 'loading…' : `load ${PAGE_SIZE} more`}
                  <ArrowRight aria-hidden="true" />
                </Button>
                <span className="font-mono text-xs text-ink-faint">
                  {rows.length} of {total} loaded
                </span>
              </div>
            ) : null}
          </div>
        )}
      </div>
      <LocalRunnerDialog
        bridge={bridge}
        open={runnerOpen}
        initialScenarios={runnerScope}
        onClose={() => setRunnerOpen(false)}
        onCompleted={() => void load()}
      />
    </div>
  )
}

/** A result filter is the Console's pill button with a pressed state. */
function ResultFilter({
  active,
  count,
  onClick,
  children,
}: {
  active: boolean
  count: number
  onClick: () => void
  children: string
}) {
  return (
    <Button
      variant="pill"
      size="sm"
      type="button"
      aria-pressed={active}
      className={active ? 'bg-surface-selected text-ink' : undefined}
      onClick={onClick}
    >
      {children}
      <span className={active ? 'text-ink' : 'text-ink-faint'}>{count}</span>
    </Button>
  )
}
