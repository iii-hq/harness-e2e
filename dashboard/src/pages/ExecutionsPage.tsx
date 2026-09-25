import {
  Button,
  Checkbox,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
  SegmentedControl,
  Table,
  TableBody,
  TableCell,
  TableFrame,
  TableHead,
  TableHeader,
  TableRow,
  TableViewport,
} from '@iii-dev/console-ui'
import {
  AlertTriangle,
  ArrowRight,
  Check,
  Copy,
  Download,
  ExternalLink,
  GitCompare,
  Minus,
  Pencil,
  RotateCcw,
  Search,
  Square,
  Trash2,
  X,
} from 'lucide-react'
import {
  type FormEvent,
  type MouseEvent,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from 'react'
import {
  DashboardPageActions,
  dashboardHeaderActionClassName,
} from '@/components/DashboardPageActions'
import { useDashboardChrome } from '@/components/DashboardShell'
import { consumeQuickExecutionRequest } from '@/components/ExecutionSetup'
import { GithubImportDialog } from '@/components/GithubImportDialog'
import { LocalRunnerDialog } from '@/components/LocalRunnerDialog'
import {
  buttonClassName,
  Callout,
  EmptyState,
  Input,
  isInteractiveTarget,
  RowMenu,
  type RowMenuItem,
  Select,
  StatusLabel,
} from '@/design-system'
import {
  hashForComparison,
  hashForExecution,
  replaceRouteParams,
  routeParams,
} from '@/hooks/use-hash-route'
import { useLatestRequest } from '@/hooks/use-latest-request'
import {
  type DashboardDataBridge,
  type DashboardExecutionSummary,
  type ExecutionParameters,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import {
  buildExecutionPresentation,
  categoryMessage,
  executionOrigin,
  executionProgress,
  executionResult,
  executionScore,
  executionTitle,
  modelNames,
  percentPoints,
  providerModel,
} from '@/lib/execution-view'
import {
  formatCount,
  formatDateTime,
  formatDay,
  formatDayLabel,
  formatDuration,
  formatTime,
  formatTokens,
  NOT_REPORTED,
  plural,
} from '@/lib/format'
import { RESULT_STATES, type ResultState } from '@/lib/result-status'
import { rerunParameters } from '@/pages/ExecutionPage'
import '@/design-system/styles.css'
import './executions-page.css'

const PAGE_SIZE = 50

/* ------------------------------------------------------------ filters */

export type LedgerSort = 'newest' | 'oldest' | 'runtime' | 'tokens' | 'result'

export type LedgerFilters = {
  query: string
  /** `all` or a result state. */
  status: 'all' | ResultState
  sort: LedgerSort
}

export const LEDGER_DEFAULT_FILTERS: LedgerFilters = {
  query: '',
  status: 'all',
  sort: 'newest',
}

const SORTS: Array<{ value: LedgerSort; label: string }> = [
  { value: 'newest', label: 'Newest first' },
  { value: 'oldest', label: 'Oldest first' },
  { value: 'result', label: 'Result' },
  { value: 'runtime', label: 'Longest runtime' },
  { value: 'tokens', label: 'Most tokens' },
]

/** Audit E-04: the list's filters live in the hash, not only in state. */
export function ledgerFiltersFromParams(
  params: URLSearchParams,
): LedgerFilters {
  const sort = params.get('sort')
  const status = params.get('status')
  return {
    query: params.get('q') ?? '',
    status: status && status in RESULT_STATES ? (status as ResultState) : 'all',
    sort: SORTS.find((entry) => entry.value === sort)?.value ?? 'newest',
  }
}

export function ledgerFiltersToParams(filters: LedgerFilters): URLSearchParams {
  const params = new URLSearchParams()
  if (filters.query.trim()) params.set('q', filters.query.trim())
  if (filters.status !== 'all') params.set('status', filters.status)
  if (filters.sort !== 'newest') params.set('sort', filters.sort)
  return params
}

/* --------------------------------------------------------------- rows */

/** One execution as the list shows it: every cell already written. */
export type LedgerRow = {
  execution: DashboardExecutionSummary
  id: string
  title: string
  origin: string
  /** When it ended, or when it started while it runs. */
  date: string
  /** `This harness · 10:10 AM · plan-cf6ab5f9`. */
  meta: string
  result: { state: ResultState; label?: string }
  /** Running, importing or cancelling: it cannot be deleted yet. */
  live: boolean
  /** The line under the result: progress, the first problem, or evidence. */
  issue: string | null
  model: string
  models: string
  profile: string
  tests: string
  /** Test runs recorded, which deleting it removes. */
  runs: number
  score: string
  passRate: string
  runtime: string
  runtimeSeconds: number | null
  tokens: string
  tokenCount: number | null
  github: { runId: number; url: string } | null
  searchText: string
}

function numeric(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function evidenceNote(execution: DashboardExecutionSummary) {
  if (execution.availability === 'aggregate') return 'Aggregate report'
  if (execution.availability === 'unavailable') return 'No report retained'
  return null
}

export function buildLedgerRows(
  executions: DashboardExecutionSummary[],
  now = new Date(),
): LedgerRow[] {
  return executions.map((execution) => {
    const presentation = buildExecutionPresentation(execution)
    const { title } = executionTitle(presentation)
    const result = executionResult(presentation)
    const live = result.state === 'running'
    const origin = executionOrigin(execution)
    const date = live
      ? presentation.startedAt || presentation.completedAt
      : presentation.completedAt || presentation.startedAt
    const subject = presentation.subjects[0]
    const model = subject ? providerModel(subject) : NOT_REPORTED
    const profile = execution.parameters?.agent
      ? `profile ${execution.parameters.agent}`
      : 'no profile'
    const { receivedReports: received, expectedReports: expected } =
      presentation
    const score = executionScore(execution)
    const passRate = percentPoints(presentation.passRate)
    const tokenCount = numeric(execution.totals?.total_tokens)
    const source = execution.source ?? {}
    const github =
      source.kind === 'github' && typeof source.run_id === 'number'
        ? { runId: source.run_id, url: origin.href ?? '' }
        : null
    return {
      execution,
      id: execution.id,
      title,
      origin: origin.label,
      date,
      meta: [
        origin.label,
        date ? formatTime(date) : null,
        execution.id.slice(0, 13),
      ]
        .filter(Boolean)
        .join(' · '),
      result,
      live,
      // While it runs, what has not reported is still to come.
      issue:
        executionProgress(execution) ??
        (live
          ? null
          : presentation.primaryIssue
            ? categoryMessage(
                presentation.primaryIssue.category,
                presentation.primaryIssue.count,
              )
            : evidenceNote(execution)),
      model,
      models: modelNames(presentation.subjects),
      profile,
      tests:
        received === null && expected === null
          ? NOT_REPORTED
          : `${received ?? NOT_REPORTED}/${expected ?? NOT_REPORTED}`,
      runs: (received ?? expected ?? 0) * (execution.parameters?.runs ?? 1),
      score:
        score === null
          ? NOT_REPORTED
          : score.toLocaleString('en-US', { maximumFractionDigits: 1 }),
      passRate:
        passRate === null
          ? NOT_REPORTED
          : `${passRate.toLocaleString('en-US', { maximumFractionDigits: 1 })}%`,
      runtime: formatDuration(
        presentation.modelRuntimeSeconds === null
          ? null
          : presentation.modelRuntimeSeconds * 1000,
      ),
      runtimeSeconds: presentation.modelRuntimeSeconds,
      tokens: formatTokens(tokenCount),
      tokenCount,
      github,
      searchText: [
        title,
        execution.id,
        execution.run_id,
        execution.workflow_name,
        typeof source.sha === 'string' ? source.sha : null,
        ...presentation.subjects.flatMap((subject) => [
          subject.model,
          providerModel(subject),
        ]),
        execution.parameters?.agent,
        ...(execution.parameters?.scenarios ?? []),
        origin.label,
        date ? formatDateTime(date, now) : null,
      ]
        .filter(Boolean)
        .join(' ')
        .toLowerCase(),
    }
  })
}

// Worst first, for the Result sort.
const RESULT_ORDER: ResultState[] = [
  'failed',
  'inconclusive',
  'incomplete',
  'running',
  'cancelled',
  'passed',
]

function resultRank(row: LedgerRow) {
  const rank = RESULT_ORDER.indexOf(row.result.state)
  return rank === -1 ? RESULT_ORDER.length : rank
}

export function filterLedgerRows(rows: LedgerRow[], filters: LedgerFilters) {
  const query = filters.query.trim().toLowerCase()
  const matched = rows.filter(
    (row) =>
      (filters.status === 'all' || row.result.state === filters.status) &&
      (!query || row.searchText.includes(query)),
  )
  const time = (row: LedgerRow) => Date.parse(row.date) || 0
  const newest = (a: LedgerRow, b: LedgerRow) => time(b) - time(a)
  const by: Record<LedgerSort, (a: LedgerRow, b: LedgerRow) => number> = {
    newest,
    oldest: (a, b) => time(a) - time(b),
    runtime: (a, b) =>
      (b.runtimeSeconds ?? -1) - (a.runtimeSeconds ?? -1) || newest(a, b),
    tokens: (a, b) =>
      (b.tokenCount ?? -1) - (a.tokenCount ?? -1) || newest(a, b),
    result: (a, b) => resultRank(a) - resultRank(b) || newest(a, b),
  }
  return [...matched].sort(by[filters.sort])
}

export type LedgerGroup = { key: string; label: string; rows: LedgerRow[] }

function dayKey(value: string) {
  const date = new Date(value)
  return Number.isNaN(date.getTime())
    ? 'undated'
    : `${date.getFullYear()}-${date.getMonth() + 1}-${date.getDate()}`
}

function dayHeading(value: string, now: Date) {
  if (dayKey(value) === 'undated') return 'Date not reported'
  const label = formatDayLabel(value, now)
  const day = formatDay(value, now)
  return label === day ? day : `${label} · ${day}`
}

/** Audit E-12: what runs comes first, then one group per day. */
export function groupLedgerRows(rows: LedgerRow[], now = new Date()) {
  const groups: LedgerGroup[] = []
  const byKey = new Map<string, LedgerGroup>()
  const running = rows.filter((row) => row.live)
  if (running.length > 0)
    groups.push({ key: 'running', label: 'Running', rows: running })
  for (const row of rows) {
    if (row.live) continue
    const key = dayKey(row.date)
    let group = byKey.get(key)
    if (!group) {
      group = { key, label: dayHeading(row.date, now), rows: [] }
      byKey.set(key, group)
      groups.push(group)
    }
    group.rows.push(row)
  }
  return groups
}

// The segments the canvas names, then any other result that is present.
const SEGMENT_ORDER: ResultState[] = [
  'passed',
  'failed',
  'incomplete',
  'running',
  'inconclusive',
  'cancelled',
]

function resultCounts(rows: LedgerRow[]) {
  const counts = new Map<ResultState, number>()
  for (const row of rows)
    counts.set(row.result.state, (counts.get(row.result.state) ?? 0) + 1)
  return [...counts.entries()].sort(
    ([a], [b]) =>
      (SEGMENT_ORDER.indexOf(a) + 1 || 99) -
      (SEGMENT_ORDER.indexOf(b) + 1 || 99),
  )
}

/** The result filter: All, then each result present, with its count. */
export function resultSegments(rows: LedgerRow[]) {
  return [
    { value: 'all' as const, label: 'All', count: rows.length },
    ...resultCounts(rows).map(([state, count]) => ({
      value: state,
      label: RESULT_STATES[state].label,
      count,
    })),
  ]
}

/** `58 retained · 16 loaded · 8 passed · 5 failed · 3 running`. */
export function ledgerSummary(rows: LedgerRow[], total: number) {
  return [
    `${total} retained`,
    `${rows.length} loaded`,
    ...resultCounts(rows).map(
      ([state, count]) =>
        `${count} ${RESULT_STATES[state].label.toLowerCase()}`,
    ),
  ].join(' · ')
}

/* ---------------------------------------------------------- selection */

export function toggleSelection(ids: string[], id: string) {
  return ids.includes(id) ? ids.filter((entry) => entry !== id) : [...ids, id]
}

/** How many of the shown rows are ticked: none, some or all. */
export function shownSelection(ids: string[], shown: string[]) {
  const ticked = shown.filter((id) => ids.includes(id)).length
  return ticked === 0 ? 'none' : ticked === shown.length ? 'all' : 'some'
}

/** The box over the list: clears the shown rows when all are ticked, else
 *  ticks every one of them. Rows filtered out keep their tick. */
export function toggleShown(ids: string[], shown: string[]) {
  return shownSelection(ids, shown) === 'all'
    ? ids.filter((id) => !shown.includes(id))
    : [...ids, ...shown.filter((id) => !ids.includes(id))]
}

/** What the selection bar says and allows. The first ticked is A. */
export function selectionSummary(selected: LedgerRow[]) {
  const deletable = selected.filter((row) => !row.live)
  const kept = selected.length - deletable.length
  const keptNote = kept ? `${kept} running will be kept.` : ''
  const hint =
    selected.length === 1
      ? 'Tick one more to compare.'
      : selected.length === 2
        ? 'A is the first you ticked.'
        : kept
          ? ''
          : 'Compare takes exactly two.'
  return {
    text: `${selected.length} selected`,
    hint: [hint, keptNote].filter(Boolean).join(' '),
    compare: selected.length === 2 ? [selected[0].id, selected[1].id] : null,
    deletable: deletable.map((row) => row.id),
    deleteLabel: deletable.length > 1 ? `Delete ${deletable.length}` : 'Delete',
  }
}

/* --------------------------------------------------------------- menu */

export type LedgerActions = {
  open: (row: LedgerRow) => void
  rename: (row: LedgerRow) => void
  openOnGithub: (row: LedgerRow) => void
  importAgain: (row: LedgerRow) => void
  runAgain: (row: LedgerRow) => void
  copyId: (row: LedgerRow) => void
  cancel: (row: LedgerRow) => void
  delete: (row: LedgerRow) => void
}

/** Only executions started or imported here have a name of their own. */
function renamable(row: LedgerRow) {
  return row.id.startsWith('plan-')
}

/** The row's ⋯ menu. An import in progress cannot be stopped from here. */
export function rowMenuItems(
  row: LedgerRow,
  actions: LedgerActions,
): RowMenuItem[] {
  const icon = (Glyph: typeof ArrowRight) => (
    <Glyph size={16} aria-hidden="true" />
  )
  const importing = row.execution.status === 'importing'
  const cancellable =
    row.live && !importing && row.result.label !== 'Cancelling'
  const items: RowMenuItem[] = [
    {
      label: 'Open',
      icon: icon(ArrowRight),
      onSelect: () => actions.open(row),
    },
  ]
  if (renamable(row))
    items.push({
      label: 'Rename',
      icon: icon(Pencil),
      onSelect: () => actions.rename(row),
    })
  if (row.github) {
    items.push({
      label: 'Open on GitHub',
      icon: icon(ExternalLink),
      onSelect: () => actions.openOnGithub(row),
    })
    if (!row.live)
      items.push({
        label: 'Import again',
        hint: 'Replaces its evidence with the run’s',
        icon: icon(RotateCcw),
        onSelect: () => actions.importAgain(row),
      })
  } else if (!row.live)
    items.push({
      label: 'Run again',
      icon: icon(RotateCcw),
      onSelect: () => actions.runAgain(row),
    })
  items.push({
    label: 'Copy execution id',
    icon: icon(Copy),
    onSelect: () => actions.copyId(row),
  })
  if (cancellable)
    items.push({
      label: 'Cancel execution',
      icon: icon(Square),
      separator: true,
      onSelect: () => actions.cancel(row),
    })
  items.push({
    label: 'Delete…',
    icon: icon(Trash2),
    danger: true,
    separator: !cancellable,
    disabledReason: row.live
      ? importing
        ? 'Wait for the import to finish'
        : 'Finish or cancel it first'
      : undefined,
    onSelect: () => actions.delete(row),
  })
  return items
}

/* ------------------------------------------------------------- delete */

export type DeleteFact = { tone: 'gone' | 'kept' | 'warn'; text: string }

/** What the delete confirmation says: what leaves, what stays. `kept` are
 *  the running executions of the selection, which are not deleted. */
export function deleteConfirmation(
  targets: LedgerRow[],
  kept: LedgerRow[] = [],
  now = new Date(),
) {
  const one = targets.length === 1 ? targets[0] : null
  const runs = targets.reduce((total, row) => total + row.runs, 0)
  const imported = targets.filter((row) => row.github)
  const facts: DeleteFact[] = [
    {
      tone: 'gone',
      // Without a count reported, the runs are named, not numbered.
      text: `${runs ? plural(runs, 'test run') : one ? 'Its test runs' : 'Their test runs'} with their transcripts, reports and screenshots leave this Console.`,
    },
    {
      tone: 'gone',
      text: `Links to ${one ? 'it' : 'them'}, comparisons included, stop working.`,
    },
  ]
  if (imported.length === 1)
    facts.push({
      tone: 'kept',
      text: `The run on GitHub is not touched. You can import #${imported[0].github?.runId} again.`,
    })
  else if (imported.length > 1)
    facts.push({
      tone: 'kept',
      text: 'The runs on GitHub are not touched. You can import them again.',
    })
  if (kept.length === 1)
    facts.push({
      tone: 'warn',
      text: `“${kept[0].title}” is still running and stays. Cancel it first to delete it.`,
    })
  else if (kept.length > 1)
    facts.push({
      tone: 'warn',
      text: `${kept.length} executions are still running and stay. Cancel them first to delete them.`,
    })
  return {
    title: one
      ? `Delete “${one.title}”?`
      : `Delete ${targets.length} executions?`,
    body: 'This can’t be undone.',
    items: targets.slice(0, 5).map((row) => ({
      id: row.id,
      title: row.title,
      meta: [
        row.origin,
        row.date ? formatDateTime(row.date, now) : null,
        row.tests === NOT_REPORTED ? null : `${row.tests} tests`,
        row.tokenCount === null ? null : `${row.tokens} tokens`,
      ]
        .filter(Boolean)
        .join(' · '),
    })),
    more: targets.length > 5 ? `and ${targets.length - 5} more` : null,
    facts,
    action: one ? 'Delete execution' : `Delete ${targets.length} executions`,
  }
}

export function deletedMessage(titles: string[]) {
  return titles.length === 1
    ? `Deleted “${titles[0]}” with its runs and evidence.`
    : `Deleted ${titles.length} executions with their runs and evidence.`
}

/* -------------------------------------------------------------- table */

export type LedgerTableProps = {
  groups: LedgerGroup[]
  selected: string[]
  onSelect: (ids: string[]) => void
  actions: LedgerActions
  /** A narrow pane keeps execution, result, tests and the menu. */
  narrow?: boolean
}

/** One table: the header is read once, each group is a body of its own
 *  with its heading row (audit E-07 / E-12). */
export function LedgerTable({
  groups,
  selected,
  onSelect,
  actions,
  narrow = false,
}: LedgerTableProps) {
  const shown = groups.flatMap((group) => group.rows.map((row) => row.id))
  const all = shownSelection(selected, shown)
  const side = (id: string) =>
    selected.length === 2 && selected.includes(id)
      ? selected[0] === id
        ? 'A'
        : 'B'
      : null
  const open = (row: LedgerRow) => (event: MouseEvent<HTMLTableRowElement>) => {
    if (!isInteractiveTarget(event.target)) actions.open(row)
  }
  return (
    <TableViewport className="ex-table-viewport">
      <TableFrame>
        <Table
          density="compact"
          inset
          className="ex-table"
          data-narrow={narrow || undefined}
          data-ledger-table
        >
          <TableHeader>
            <TableRow>
              <TableHead className="ex-col-select" scope="col">
                <Checkbox
                  aria-label="Select every execution shown"
                  checked={all === 'all'}
                  indeterminate={all === 'some'}
                  onChange={() => onSelect(toggleShown(selected, shown))}
                />
              </TableHead>
              <TableHead scope="col">Execution</TableHead>
              <TableHead className="ex-col-result" scope="col">
                Result
              </TableHead>
              {narrow ? null : (
                <TableHead className="ex-col-model" scope="col">
                  Model
                </TableHead>
              )}
              <TableHead className="ex-col-tests ex-num" scope="col">
                Tests
              </TableHead>
              {narrow ? null : (
                <>
                  <TableHead className="ex-col-score ex-num" scope="col">
                    Score
                  </TableHead>
                  <TableHead className="ex-col-pass ex-num" scope="col">
                    Pass rate
                  </TableHead>
                  <TableHead className="ex-col-runtime ex-num" scope="col">
                    Runtime
                  </TableHead>
                  <TableHead className="ex-col-tokens ex-num" scope="col">
                    Tokens
                  </TableHead>
                </>
              )}
              <TableHead className="ex-col-menu" scope="col">
                <span className="ds-visually-hidden">Actions</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          {groups.map((group) => (
            <TableBody
              key={group.key}
              data-ledger-group={group.key}
              aria-label={group.label}
            >
              <TableRow className="ex-group">
                <TableHead colSpan={narrow ? 5 : 10} scope="colgroup">
                  <span className="ds-label">{group.label}</span>
                  <span className="ex-group-count">{group.rows.length}</span>
                </TableHead>
              </TableRow>
              {group.rows.map((row) => {
                const ticked = selected.includes(row.id)
                const letter = side(row.id)
                return (
                  <TableRow
                    key={row.id}
                    interactive
                    tabIndex={-1}
                    selected={ticked}
                    className="ex-row"
                    data-execution-id={row.id}
                    data-result={row.result.state}
                    onClick={open(row)}
                  >
                    <TableCell className="ex-col-select">
                      <Checkbox
                        aria-label={`Select ${row.title}`}
                        checked={ticked}
                        onChange={() =>
                          onSelect(toggleSelection(selected, row.id))
                        }
                      />
                    </TableCell>
                    <TableCell className="ex-cell-stack">
                      <span className="ex-title">
                        {letter ? (
                          <span
                            className="ex-side"
                            title={`Compared as ${letter}`}
                          >
                            {letter}
                          </span>
                        ) : null}
                        <a href={hashForExecution(row.id)} title={row.title}>
                          {row.title}
                        </a>
                      </span>
                      <span className="ex-sub ex-mono">{row.meta}</span>
                    </TableCell>
                    <TableCell className="ex-cell-stack">
                      <StatusLabel
                        className="ex-result"
                        state={row.result.state}
                        label={row.result.label}
                      />
                      {row.issue ? (
                        <span className="ex-sub" title={row.issue}>
                          {row.issue}
                        </span>
                      ) : null}
                    </TableCell>
                    {narrow ? null : (
                      <TableCell className="ex-cell-stack" title={row.models}>
                        <span className="ex-mono ex-model">{row.model}</span>
                        <span className="ex-sub ex-mono">{row.profile}</span>
                      </TableCell>
                    )}
                    <TableCell className="ex-num">{row.tests}</TableCell>
                    {narrow ? null : (
                      <>
                        <TableCell className="ex-num">{row.score}</TableCell>
                        <TableCell className="ex-num">{row.passRate}</TableCell>
                        <TableCell className="ex-num">{row.runtime}</TableCell>
                        <TableCell
                          className="ex-num"
                          title={
                            row.tokenCount === null
                              ? undefined
                              : `${formatCount(row.tokenCount)} tokens`
                          }
                        >
                          {row.tokens}
                        </TableCell>
                      </>
                    )}
                    <TableCell className="ex-col-menu">
                      <RowMenu
                        label={`Actions for ${row.title}`}
                        items={rowMenuItems(row, actions)}
                      />
                    </TableCell>
                  </TableRow>
                )
              })}
            </TableBody>
          ))}
        </Table>
      </TableFrame>
    </TableViewport>
  )
}

/* ------------------------------------------------------------ dialogs */

const FACT_ICONS = { gone: Minus, kept: Check, warn: AlertTriangle }

function DeleteDialog({
  targets,
  kept,
  deleting,
  onCancel,
  onConfirm,
}: {
  targets: LedgerRow[]
  kept: LedgerRow[]
  deleting: boolean
  onCancel: () => void
  onConfirm: () => void
}) {
  const confirmation = deleteConfirmation(targets, kept)
  return (
    <Dialog
      open={targets.length > 0}
      onOpenChange={(open) => {
        if (!open && !deleting) onCancel()
      }}
    >
      <DialogContent
        role="alertdialog"
        className="ex-dialog"
        aria-describedby="ex-delete-body"
      >
        <div className="ex-dialog-head">
          <span className="ex-dialog-icon" aria-hidden="true">
            <Trash2 size={16} />
          </span>
          <div>
            <DialogTitle className="ex-dialog-title">
              {confirmation.title}
            </DialogTitle>
            <DialogDescription id="ex-delete-body" className="ex-dialog-body">
              {confirmation.body}
            </DialogDescription>
          </div>
        </div>
        <ul className="ex-dialog-items" aria-label="Executions to delete">
          {confirmation.items.map((item) => (
            <li key={item.id}>
              <span className="ex-dialog-item-title">{item.title}</span>
              <span className="ex-sub ex-mono">{item.meta}</span>
            </li>
          ))}
          {confirmation.more ? (
            <li className="ex-sub">{confirmation.more}</li>
          ) : null}
        </ul>
        <ul className="ex-dialog-facts">
          {confirmation.facts.map((fact) => {
            const Icon = FACT_ICONS[fact.tone]
            return (
              <li key={fact.text} data-tone={fact.tone}>
                <Icon size={16} aria-hidden="true" />
                <span>{fact.text}</span>
              </li>
            )
          })}
        </ul>
        <div className="ex-dialog-actions">
          <Button
            type="button"
            variant="pill"
            size="sm"
            disabled={deleting}
            onClick={onCancel}
          >
            Cancel
          </Button>
          <Button
            type="button"
            variant="pill"
            size="sm"
            className="ex-danger"
            disabled={deleting}
            aria-busy={deleting}
            onClick={onConfirm}
          >
            {deleting ? 'Deleting…' : confirmation.action}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  )
}

function RenameDialog({
  row,
  onClose,
  onRename,
}: {
  row: LedgerRow | null
  onClose: () => void
  onRename: (row: LedgerRow, label: string) => Promise<void>
}) {
  const [draft, setDraft] = useState('')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    setDraft(
      typeof row?.execution.label === 'string' ? row.execution.label : '',
    )
    setError(null)
  }, [row])
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!row) return
    setSaving(true)
    setError(null)
    try {
      await onRename(row, draft)
      onClose()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setSaving(false)
    }
  }
  return (
    <Dialog
      open={row !== null}
      onOpenChange={(open) => {
        if (!open && !saving) onClose()
      }}
    >
      <DialogContent className="ex-dialog">
        <DialogTitle className="ex-dialog-title">Rename execution</DialogTitle>
        <DialogDescription className="ex-dialog-body">
          An empty name gives it back its default one.
        </DialogDescription>
        <form className="ex-rename" onSubmit={(event) => void submit(event)}>
          <Input
            aria-label="Execution name"
            maxLength={80}
            placeholder={row?.title}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
          />
          {error ? (
            <p className="ex-error" role="alert">
              {error}
            </p>
          ) : null}
          <div className="ex-dialog-actions">
            <Button
              type="button"
              variant="pill"
              size="sm"
              disabled={saving}
              onClick={onClose}
            >
              Cancel
            </Button>
            <Button type="submit" variant="primary" size="sm" disabled={saving}>
              {saving ? 'Saving…' : 'Save'}
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  )
}

/* --------------------------------------------------------------- page */

const NO_CONFIRM = { ids: [], kept: [] }

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

export function ExecutionsPage() {
  const narrow = useDashboardChrome()?.narrow ?? false
  const [runnerOpen, setRunnerOpen] = useState(false)
  const [importOpen, setImportOpen] = useState(false)
  const [runnerScope, setRunnerScope] = useState<string[]>([])
  const [rerun, setRerun] = useState<{
    parameters: ExecutionParameters
    label: string
  } | null>(null)
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
  // Ticked executions, in the order they were ticked: the first is A.
  const [selected, setSelected] = useState<string[]>([])
  // What the delete confirmation deletes, and the running ones it keeps.
  const [confirm, setConfirm] = useState<{ ids: string[]; kept: string[] }>(
    NO_CONFIRM,
  )
  const [deleting, setDeleting] = useState(false)
  const [renaming, setRenaming] = useState<LedgerRow | null>(null)
  const [flash, setFlash] = useState<string | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)
  const beginRequest = useLatestRequest()

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
    } catch (cause) {
      setError(errorText(cause))
    } finally {
      setLoading(false)
    }
  }, [beginRequest, bridge])

  useEffect(() => {
    void load()
  }, [load])

  // Audit E-12: the list follows run changes instead of waiting for F5.
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
      setError(errorText(cause))
    } finally {
      setLoadingMore(false)
    }
  }

  const rows = useMemo(() => buildLedgerRows(executions), [executions])
  const byId = useMemo(() => new Map(rows.map((row) => [row.id, row])), [rows])
  // A tick outlives a reload only while its execution is still listed.
  const ticked = useMemo(
    () => selected.filter((id) => byId.has(id)),
    [selected, byId],
  )
  const visible = useMemo(
    () => filterLedgerRows(rows, filters),
    [rows, filters],
  )
  const groups = useMemo(() => groupLedgerRows(visible), [visible])
  const setFilter = <K extends keyof LedgerFilters>(
    key: K,
    value: LedgerFilters[K],
  ) => setFilters((current) => ({ ...current, [key]: value }))
  const filtered = ledgerFiltersToParams(filters).toString() !== ''
  const selectedRows = ticked.flatMap((id) => byId.get(id) ?? [])
  const bar = selectionSummary(selectedRows)
  const rowsOf = (ids: string[]) => ids.flatMap((id) => byId.get(id) ?? [])
  const targets = rowsOf(confirm.ids)

  const act = async (work: () => Promise<unknown>) => {
    setActionError(null)
    try {
      await work()
      await load()
    } catch (cause) {
      setActionError(errorText(cause))
    }
  }

  const actions: LedgerActions = {
    open: (row) => {
      window.location.hash = hashForExecution(row.id)
    },
    rename: (row) => setRenaming(row),
    openOnGithub: (row) => {
      if (row.github?.url) window.open(row.github.url, '_blank', 'noopener')
    },
    importAgain: (row) =>
      void act(async () => {
        if (bridge && row.github) await bridge.importGithubRun(row.github.runId)
      }),
    runAgain: (row) =>
      setRerun({
        parameters: rerunParameters(
          { parameters: row.execution.parameters },
          row.execution.subjects.flatMap((subject) =>
            subject.scenarios.map((scenario) => scenario.id),
          ),
          buildExecutionPresentation(row.execution).subjects[0],
        ),
        label:
          typeof row.execution.label === 'string' ? row.execution.label : '',
      }),
    copyId: (row) => {
      void navigator.clipboard
        ?.writeText(row.id)
        .then(() => setFlash(`Copied ${row.id}.`))
        .catch((cause) => setActionError(errorText(cause)))
    },
    // A composed execution stops by id; a native run is the runner's one.
    cancel: (row) =>
      void act(async () => {
        if (!bridge) return
        if (row.id.startsWith('plan-')) await bridge.cancelExecution(row.id)
        else await bridge.cancelRun()
      }),
    delete: (row) => setConfirm({ ids: [row.id], kept: [] }),
  }

  // Deletes one after another; each refusal is said with its execution.
  const deleteConfirmed = async () => {
    if (!bridge) return
    setDeleting(true)
    setActionError(null)
    const deleted: LedgerRow[] = []
    const refused: string[] = []
    for (const row of targets) {
      try {
        await bridge.deleteExecution(row.id)
        deleted.push(row)
      } catch (cause) {
        refused.push(`“${row.title}”: ${errorText(cause)}`)
      }
    }
    const gone = new Set(deleted.map((row) => row.id))
    setExecutions((current) => current.filter((entry) => !gone.has(entry.id)))
    setTotal((current) => Math.max(0, current - gone.size))
    setSelected((current) => current.filter((id) => !gone.has(id)))
    setConfirm(NO_CONFIRM)
    setDeleting(false)
    setFlash(
      deleted.length ? deletedMessage(deleted.map((row) => row.title)) : null,
    )
    if (refused.length)
      setActionError(
        `${refused.length === 1 ? 'One execution was' : `${refused.length} executions were`} not deleted. ${refused.join(' ')}`,
      )
    void load()
  }

  const importLabel = narrow ? 'Import' : 'Import from GitHub'
  const headerActions = useMemo(
    () =>
      bridge ? (
        <>
          <button
            className={dashboardHeaderActionClassName()}
            type="button"
            onClick={() => setImportOpen(true)}
          >
            <Download size={16} aria-hidden="true" />
            {importLabel}
          </button>
          <button
            className={dashboardHeaderActionClassName({ primary: true })}
            type="button"
            onClick={() => {
              setRunnerScope([])
              setRunnerOpen(true)
            }}
          >
            Run tests
          </button>
        </>
      ) : null,
    [bridge, importLabel],
  )

  return (
    <div className="ds-root ex-page">
      <DashboardPageActions
        active="executions"
        actionsLabel="Execution actions"
        actions={headerActions}
      />
      <header className="ex-header">
        <h1 id="executions-title">Executions</h1>
        <p>
          {loading && rows.length === 0
            ? 'Loading the executions…'
            : ledgerSummary(rows, total)}
        </p>
      </header>

      {error ? (
        <Callout tone="danger" title="Executions could not be loaded">
          <span className="ex-callout-line">
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

      <section className="ex-toolbar" aria-label="Execution filters">
        <div className="ex-search">
          <Search size={16} aria-hidden="true" />
          <Input
            type="text"
            value={filters.query}
            placeholder="Search label, model, id or date"
            aria-label="Search executions"
            onChange={(event) => setFilter('query', event.target.value)}
          />
          {filters.query ? (
            <button
              className="ex-icon-button"
              type="button"
              onClick={() => setFilter('query', '')}
              aria-label="Clear search"
            >
              <X size={14} aria-hidden="true" />
            </button>
          ) : null}
        </div>
        <SegmentedControl
          variant="radio"
          aria-label="Result"
          className="ex-segments"
          value={filters.status}
          onChange={(value) => setFilter('status', value)}
          options={resultSegments(rows).map((segment) => ({
            value: segment.value,
            label: (
              <>
                {segment.label}
                <span className="ex-count">{segment.count}</span>
              </>
            ),
          }))}
        />
        <Select
          aria-label="Sort executions"
          className="ex-sort"
          value={filters.sort}
          onChange={(event) =>
            setFilter('sort', event.target.value as LedgerSort)
          }
        >
          {SORTS.map((sort) => (
            <option key={sort.value} value={sort.value}>
              {sort.label}
            </option>
          ))}
        </Select>
      </section>

      {flash ? (
        <div className="ex-flash" role="status">
          <Check size={16} aria-hidden="true" />
          <span>{flash}</span>
          <button
            className="ex-icon-button"
            type="button"
            aria-label="Dismiss"
            onClick={() => setFlash(null)}
          >
            <X size={16} aria-hidden="true" />
          </button>
        </div>
      ) : null}

      {actionError ? (
        <Callout tone="danger" title="That did not go through">
          <span className="ex-callout-line">
            {actionError}
            <button
              className={buttonClassName({ variant: 'quiet', size: 'compact' })}
              type="button"
              onClick={() => setActionError(null)}
            >
              dismiss
            </button>
          </span>
        </Callout>
      ) : null}

      {loading && rows.length === 0 ? (
        <div className="ex-loading" aria-busy="true" role="status">
          <span className="ds-visually-hidden">Loading executions</span>
          {Array.from({ length: 6 }, (_, index) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: static placeholders
            <div key={index} />
          ))}
        </div>
      ) : visible.length === 0 ? (
        <EmptyState
          title={
            rows.length === 0
              ? 'No executions retained yet'
              : 'No executions match these filters'
          }
          description={
            rows.length === 0
              ? 'Run tests here or import a run from GitHub to start retaining execution evidence.'
              : 'Widen the result filter or clear the search.'
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
            ) : rows.length === 0 && bridge ? (
              <>
                <button
                  className={buttonClassName({ variant: 'primary' })}
                  type="button"
                  onClick={() => {
                    setRunnerScope([])
                    setRunnerOpen(true)
                  }}
                >
                  run tests
                </button>
                <button
                  className={buttonClassName({ variant: 'secondary' })}
                  type="button"
                  onClick={() => setImportOpen(true)}
                >
                  import from GitHub
                </button>
              </>
            ) : null
          }
        />
      ) : (
        <div className="ex-ledger" data-ledger>
          <LedgerTable
            narrow={narrow}
            groups={groups}
            selected={ticked}
            onSelect={setSelected}
            actions={actions}
          />
          {cursor ? (
            <button
              className="ex-more"
              type="button"
              onClick={() => void loadMore()}
              disabled={loadingMore}
              aria-busy={loadingMore}
            >
              {loadingMore
                ? 'Loading…'
                : `Load older executions · ${rows.length} of ${total} loaded`}
            </button>
          ) : null}
        </div>
      )}

      {selectedRows.length > 0 ? (
        <div
          className="ex-selection shadow-floating"
          role="toolbar"
          aria-label="Selected executions"
        >
          <span className="ex-selection-count">{bar.text}</span>
          <span className="ex-selection-hint">{bar.hint}</span>
          <Button
            type="button"
            variant="pill"
            size="sm"
            disabled={!bar.compare}
            onClick={() => {
              if (bar.compare)
                window.location.hash = hashForComparison(...bar.compare)
            }}
          >
            <GitCompare aria-hidden="true" />
            Compare A and B
          </Button>
          <Button
            type="button"
            variant="pill"
            size="sm"
            className="ex-danger"
            disabled={bar.deletable.length === 0}
            onClick={() =>
              setConfirm({
                ids: bar.deletable,
                kept: selectedRows
                  .filter((row) => row.live)
                  .map((row) => row.id),
              })
            }
          >
            <Trash2 aria-hidden="true" />
            {bar.deleteLabel}
          </Button>
          <Button
            type="button"
            variant="icon"
            size="icon"
            aria-label="Clear selection"
            onClick={() => setSelected([])}
          >
            <X aria-hidden="true" />
          </Button>
        </div>
      ) : null}

      <DeleteDialog
        targets={targets}
        kept={rowsOf(confirm.kept)}
        deleting={deleting}
        onCancel={() => setConfirm(NO_CONFIRM)}
        onConfirm={() => void deleteConfirmed()}
      />
      <RenameDialog
        row={renaming}
        onClose={() => setRenaming(null)}
        onRename={async (row, label) => {
          if (!bridge) return
          await bridge.renameExecution(row.id, label)
          await load()
        }}
      />
      <LocalRunnerDialog
        bridge={bridge}
        open={runnerOpen || rerun !== null}
        initialScenarios={rerun ? undefined : runnerScope}
        parameters={rerun?.parameters ?? null}
        label={rerun?.label ?? ''}
        onClose={() => {
          setRunnerOpen(false)
          setRerun(null)
        }}
      />
      <GithubImportDialog
        bridge={bridge}
        open={importOpen}
        onClose={() => setImportOpen(false)}
        onImported={() => void load()}
      />
    </div>
  )
}
