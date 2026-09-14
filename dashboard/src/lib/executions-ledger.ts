import type {
  DashboardExecutionSummary,
  ReleaseControlIdentity,
} from '@/lib/dashboard-data-source'
import {
  buildExecutionPresentation,
  type ExecutionPresentation,
  executionTitle,
  formatDate,
  formatDuration,
  formatPercent,
  percentPoints,
  statusCopy,
} from '@/lib/execution-view'

/** The executions ledger: filters, sort, day and plan grouping. Pure functions; the page only renders. */

export const PAGE_SIZE = 50

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
      status: execution.id.startsWith('rc:')
        ? {
            label: execution.status.replaceAll('_', ' '),
            status:
              execution.status === 'running'
                ? ('running' as const)
                : execution.status === 'cancelled'
                  ? ('cancelled' as const)
                  : ('inconclusive' as const),
          }
        : statusCopy(presentation),
      searchText: [
        title,
        detail,
        execution.label,
        execution.workflow_name,
        execution.id,
        execution.run_id,
        formatDate(presentation.completedAt),
        execution.source?.sha,
        execution.release_control?.execution_id,
        execution.release_control?.profile,
        execution.release_control?.campaign_id,
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

export function tokensOf(row: LedgerRow) {
  const value = row.execution.totals?.total_tokens
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

export const RESULT_ORDER = [
  'failed',
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

export type LedgerGroup = {
  key: string
  label: string
  rows: LedgerRow[]
  /** Present when the group is one Release Control execution (its plan). */
  plan?: ReleaseControlIdentity
}

/** One Release Control execution reads as its plan: profile · campaign · id. */
export function planGroupLabel(plan: ReleaseControlIdentity): string {
  const head = [plan.profile, plan.campaign_id].filter(Boolean).join(' · ')
  return `${head || 'release control'} · release control ${plan.execution_id.slice(0, 8)}`
}

/** Additive figures over a group's rows; absence stays absent, never zero. */
export function groupStats(rows: LedgerRow[]) {
  const passed = rows.filter((row) => row.status.status === 'passed').length
  const tokens = rows.map(tokensOf).filter((value) => value !== null)
  const seconds = rows
    .map((row) => row.presentation.modelRuntimeSeconds)
    .filter((value): value is number => value !== null)
  return {
    runs: rows.length,
    passed,
    passRate: rows.length > 0 ? passed / rows.length : null,
    tokens: tokens.length > 0 ? tokens.reduce((sum, v) => sum + v, 0) : null,
    seconds: seconds.length > 0 ? seconds.reduce((sum, v) => sum + v, 0) : null,
  }
}

export function groupHeading(group: LedgerGroup): string {
  if (
    !group.plan ||
    group.rows.some((row) => row.execution.id.startsWith('rc:'))
  )
    return `${group.label} · ${group.rows.length}`
  const stats = groupStats(group.rows)
  const parts = [
    group.label,
    `${stats.runs} run${stats.runs === 1 ? '' : 's'}`,
    stats.passRate === null
      ? null
      : `${formatPercent(percentPoints(stats.passRate), false)} pass`,
    stats.tokens === null ? null : `${stats.tokens.toLocaleString()} tokens`,
    stats.seconds === null ? null : formatDuration(stats.seconds),
  ]
  return parts.filter(Boolean).join(' · ')
}

/**
 * Audit E-12: a running execution is pinned above the groups. Runs that
 * Release Control dispatched are grouped by their execution (the plan they
 * belong to); everything else keeps its day group.
 */
export function groupLedgerRows(rows: LedgerRow[], now = Date.now()) {
  const running = rows.filter(
    (row) =>
      row.status.status === 'running' || row.status.status === 'cancelling',
  )
  const settled = rows.filter((row) => !running.includes(row))
  const groups: LedgerGroup[] = []
  const byKey = new Map<string, LedgerGroup>()
  const push = (group: LedgerGroup, row: LedgerRow) => {
    const existing = byKey.get(group.key)
    if (existing) existing.rows.push(row)
    else {
      group.rows.push(row)
      byKey.set(group.key, group)
      groups.push(group)
    }
  }
  for (const row of settled) {
    const plan = row.execution.release_control
    if (plan?.execution_id) {
      push(
        {
          key: `plan:${plan.execution_id}`,
          label: planGroupLabel(plan),
          rows: [],
          plan,
        },
        row,
      )
      continue
    }
    push(
      {
        key: dayKey(row.presentation.completedAt),
        label: dayLabel(row.presentation.completedAt, now),
        rows: [],
      },
      row,
    )
  }
  return { running, groups }
}
