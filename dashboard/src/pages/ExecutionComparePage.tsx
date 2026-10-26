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
  TableCaption,
  TableCell,
  TableFrame,
  TableHead,
  TableHeader,
  TableRow,
  TableViewport,
} from '@iii-dev/console-ui'
import { AlertTriangle, ClipboardCopy, RotateCcw } from 'lucide-react'
import {
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from 'react'
import {
  ComparisonView,
  type GroupView,
  type Sides,
} from '@/components/compare/ComparisonView'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { ExecutionMoreMenu } from '@/components/execution/NeedsAttention'
import { InvestigationAction } from '@/components/InvestigationAction'
import { LocalRunnerDialog } from '@/components/LocalRunnerDialog'
import {
  buttonClassName,
  EmptyState,
  isInteractiveTarget,
  PageHeader,
  StatusLabel,
} from '@/design-system'
import {
  hashForComparison,
  hashForWorkspace,
  hashWithParams,
  replaceRouteParams,
  routeParams,
  trendsOrigin,
} from '@/hooks/use-hash-route'
import { useLatestRequest } from '@/hooks/use-latest-request'
import { groupLetter, scoringGroups } from '@/lib/comparison-group'
import {
  type DashboardDataBridge,
  type DashboardExecutionDetail,
  type ExecutionParameters,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import {
  compareExecutions,
  comparisonGroup,
  type ExecutionComparison,
  exclusionPhrase,
  groupMarkdown,
  type ScenarioComparison,
  stackChanges,
} from '@/lib/execution-comparison'
import { buildExecutionPresentation } from '@/lib/execution-view'
import { plural } from '@/lib/format'
import type { Investigation } from '@/lib/investigation'
import { watchExecution } from '@/lib/watch-execution'
import { rerunParameters } from '@/pages/ExecutionPage'
import {
  buildLedgerRows,
  groupLedgerRows,
  type LedgerRow,
} from '@/pages/ExecutionsPage'
import '@/design-system/styles.css'
import { copyText } from '@/lib/clipboard'

type Choice = { include: string[]; exclude: string[] }

/** How many of the latest executions Compare with… offers as B. */
const CANDIDATES = 50

function listParam(params: URLSearchParams, key: string): string[] {
  return (params.get(key) ?? '').split(',').filter(Boolean)
}

/** The reader's choices live in the hash, so a shared link shows the same
 *  totals. */
export function choiceFromParams(params: URLSearchParams): Choice {
  return {
    include: listParam(params, 'include'),
    exclude: listParam(params, 'exclude'),
  }
}

export function choiceToParams(choice: Choice): URLSearchParams {
  const params = new URLSearchParams()
  if (choice.include.length > 0) params.set('include', choice.include.join(','))
  if (choice.exclude.length > 0) params.set('exclude', choice.exclude.join(','))
  return params
}

/** The Trends view this comparison was opened from, if any. */
function origin() {
  return typeof window === 'undefined'
    ? null
    : trendsOrigin(window.location.hash)
}

/** Which execution is the reference and which is read against it in
 *  detail, as the hash keeps them; null for the default. */
export type Selection = { reference?: string | null; compared?: string | null }

/** The choice's params and the executions picked, keeping the Trends view
 *  to go back to. */
export function viewParams(
  choice: Choice,
  from = origin(),
  selection: Selection = {},
): URLSearchParams {
  const params = choiceToParams(choice)
  if (selection.reference) params.set('reference', selection.reference)
  if (selection.compared) params.set('compared', selection.compared)
  if (from) params.set('from', from)
  return params
}

/** The choice that counts exactly these tests: the rule's exclusions they
 *  name come back (`include`), any other test they leave out goes
 *  (`exclude`). */
export function choiceCounting(
  scenarios: Pick<ScenarioComparison, 'id' | 'exclusion'>[],
  ids: Iterable<string>,
): Choice {
  const keep = new Set(ids)
  return {
    include: scenarios
      .filter((scenario) => scenario.exclusion && keep.has(scenario.id))
      .map((scenario) => scenario.id)
      .sort(),
    exclude: scenarios
      .filter((scenario) => !scenario.exclusion && !keep.has(scenario.id))
      .map((scenario) => scenario.id)
      .sort(),
  }
}

function errorText(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause)
}

/** Every execution, in the order given, or an error that names each one
 *  that could not be loaded. */
export async function loadExecutions(
  getExecution: (id: string) => Promise<DashboardExecutionDetail>,
  ids: string[],
): Promise<DashboardExecutionDetail[]> {
  const results = await Promise.allSettled(ids.map((id) => getExecution(id)))
  const failed = results.flatMap((result, index) =>
    result.status === 'rejected'
      ? [`${ids[index]} could not be loaded: ${errorText(result.reason)}`]
      : [],
  )
  if (failed.length > 0) throw new Error(failed.join(' · '))
  return results.flatMap((result) =>
    result.status === 'fulfilled' ? [result.value] : [],
  )
}

/** The reference and the one read against it in detail: the hash's picks,
 *  else the first execution and the next one; and what is wrong with the
 *  picks, if anything. */
export function pickSides(
  ids: string[],
  reference: string | null,
  compared: string | null,
): { a: string; b: string; error: string | null } {
  const a = reference ?? ids[0] ?? ''
  const b = compared ?? ids.find((id) => id !== a) ?? ''
  return {
    a,
    b,
    error: !ids.includes(a)
      ? `The reference ${a} is not one of the executions compared.`
      : !ids.includes(b)
        ? `${b} is not one of the executions compared.`
        : a === b
          ? 'The execution read in detail cannot be the reference.'
          : null,
  }
}

/** The picks as the hash keeps them: only those that are not the defaults. */
export function selectionOf(
  ids: string[],
  reference: string,
  compared: string,
): Selection {
  return {
    reference: reference === ids[0] ? null : reference,
    compared: compared === ids.find((id) => id !== reference) ? null : compared,
  }
}

const LIVE = ['running', 'importing', 'cancelling']

function pair(one: string, two: string) {
  return one === two ? one : `${one} → ${two}`
}

/** The status line: suite, model and how many tests, each once when both
 *  sides agree. */
function summaryLine(comparison: ExecutionComparison) {
  const { a, b, scenarios } = comparison
  const counted = scenarios.filter((scenario) => scenario.counted).length
  return [
    a.suite !== null && b.suite !== null ? pair(a.suite, b.suite) : null,
    pair(a.subject, b.subject),
    plural(scenarios.length, 'test'),
    counted === scenarios.length ? null : `${counted} counted`,
  ]
    .filter(Boolean)
    .join(' · ')
}

/** What the chat gets from this page: the tests the totals count, the ones
 *  left out and why, the deltas that cannot be read, and what changed. */
export function comparisonInvestigation({
  a,
  b,
  scenarios,
  totals,
  parameters,
  stack,
  runner,
}: ExecutionComparison): Investigation {
  return {
    executionId: a.id,
    comparisonExecutionId: b.id,
    visibleScenarioIds: scenarios
      .filter((scenario) => scenario.counted)
      .map((scenario) => scenario.id),
    excludedScenarios: scenarios
      .filter((scenario) => !scenario.counted)
      .map((scenario) => ({
        scenario_id: scenario.id,
        reason: exclusionPhrase(scenario) ?? 'not counted',
      })),
    unavailableDeltas: totals
      .filter((metric) => metric.delta === null)
      .map((metric) => metric.id),
    changes: [
      ...parameters.map(({ field, a, b }) => ({
        what: field,
        change: `${a} → ${b}`,
      })),
      // Without a recorded stack on a side nothing below is listed.
      ...(!stack.recorded.a || !stack.recorded.b
        ? [{ what: 'stack', change: stackChanges(stack) ?? 'not recorded' }]
        : []),
      ...stack.changed.map(({ field, a, b }) => ({
        what: field,
        change: `${a} → ${b}`,
      })),
      ...stack.notComparable.map(({ field, a, b, reason }) => ({
        what: field,
        change: `${a} → ${b} (not comparable: ${reason})`,
      })),
      ...(['a', 'b'] as const).flatMap((side) => {
        const only = side === 'a' ? stack.onlyA : stack.onlyB
        return only.length > 0
          ? [{ what: `only in ${side.toUpperCase()}`, change: only.join(', ') }]
          : []
      }),
      // The runner is a worker of the stack; listed once.
      ...(runner.differs &&
      !stack.changed.some((change) => change.field === 'harness-e2e')
        ? [
            {
              what: 'runner',
              change: `${runner.a ?? '—'} → ${runner.b ?? '—'}`,
            },
          ]
        : []),
    ],
  }
}

/** The header every state of the page shares: back to Executions, the
 *  title, the line under it and the actions. */
function Header({
  title,
  summary,
  actions,
}: {
  title: string
  summary: ReactNode
  actions?: ReactNode
}) {
  const from = origin()
  return (
    <PageHeader
      variant="detail"
      className="cmp-header"
      back={
        from
          ? { label: 'Back to Trends', href: from }
          : {
              label: 'Back to Executions',
              href: hashForWorkspace('executions'),
            }
      }
      title={title}
      summary={summary}
      headingId="comparison-title"
      actions={actions}
    />
  )
}

type Candidates = {
  a: LedgerRow | null
  rows: LedgerRow[]
  /** Where older executions go on; null once every one is listed. */
  cursor: string | null
}

type More = { loading: boolean; error: string | null }

/** A, named, and the latest executions but A, to pick the others from. */
export async function loadCandidates(
  bridge: Pick<DashboardDataBridge, 'listExecutions'>,
  left: string,
): Promise<Candidates> {
  const [latest, own] = await Promise.all([
    bridge.listExecutions({ limit: CANDIDATES }),
    bridge.listExecutions({ ids: [left], limit: 1 }),
  ])
  return {
    a: buildLedgerRows(own.executions)[0] ?? null,
    rows: buildLedgerRows(latest.executions).filter((row) => row.id !== left),
    cursor: latest.next_cursor ?? null,
  }
}

/** The next page of candidates, without A or a row already listed. */
export async function loadOlderCandidates(
  bridge: Pick<DashboardDataBridge, 'listExecutions'>,
  left: string,
  current: Candidates,
): Promise<Candidates> {
  if (!current.cursor) return current
  const page = await bridge.listExecutions({
    limit: CANDIDATES,
    cursor: current.cursor,
  })
  const listed = new Set([left, ...current.rows.map((row) => row.id)])
  return {
    ...current,
    rows: [
      ...current.rows,
      ...buildLedgerRows(page.executions).filter((row) => !listed.has(row.id)),
    ],
    cursor: page.next_cursor ?? null,
  }
}

/** Compare with…: A is set, the others are picked from the latest executions. */
function ChooseB({ left }: { left: string }) {
  const [candidates, setCandidates] = useState<Candidates | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [more, setMore] = useState<More>({ loading: false, error: null })
  const beginRequest = useLatestRequest()
  const load = useCallback(async () => {
    const request = beginRequest()
    try {
      const next = await loadCandidates(await getDashboardDataBridge(), left)
      if (!request.isCurrent()) return
      setCandidates(next)
      setError(null)
    } catch (cause) {
      if (request.isCurrent()) setError(errorText(cause))
    }
  }, [beginRequest, left])
  useEffect(() => {
    void load()
  }, [load])
  const loadMore = async () => {
    if (!candidates) return
    setMore({ loading: true, error: null })
    try {
      setCandidates(
        await loadOlderCandidates(
          await getDashboardDataBridge(),
          left,
          candidates,
        ),
      )
      setMore({ loading: false, error: null })
    } catch (cause) {
      setMore({ loading: false, error: errorText(cause) })
    }
  }

  if (error)
    return (
      <ComparisonPlaceholder
        missing={false}
        error={error}
        onRetry={() => void load()}
      />
    )
  if (!candidates) return <ComparisonPlaceholder missing={false} error={null} />
  return (
    <CandidateList
      left={left}
      {...candidates}
      more={more}
      onMore={() => void loadMore()}
    />
  )
}

export function CandidateList({
  left,
  a,
  rows,
  cursor = null,
  more = { loading: false, error: null },
  onMore,
}: Omit<Candidates, 'cursor'> & {
  left: string
  cursor?: string | null
  more?: More
  onMore?: () => void
}) {
  // Ticked in order: the order of the columns after A. Older pages keep it.
  const [picked, setPicked] = useState<string[]>([])
  const pick = (row: LedgerRow) => hashForComparison(left, row.id)
  const toggle = (id: string) =>
    setPicked((current) =>
      current.includes(id)
        ? current.filter((entry) => entry !== id)
        : [...current, id],
    )
  return (
    <>
      <Header
        title="Compare with…"
        summary={
          <>
            A is <strong>{a?.title ?? left}</strong>, the reference. Pick B,
            compared with it, or tick several to compare them all with A.
          </>
        }
        actions={
          rows.length > 0 ? (
            <button
              type="button"
              className={buttonClassName({ variant: 'primary' })}
              disabled={picked.length === 0}
              data-compare-picked={picked.join(',')}
              onClick={() => {
                window.location.hash = hashForComparison(left, ...picked)
              }}
            >
              {picked.length > 1
                ? `Compare ${picked.length + 1} executions`
                : 'Compare A and B'}
            </button>
          ) : null
        }
      />
      {rows.length === 0 ? (
        <EmptyState
          className="cmp-empty"
          title="No other execution to compare with"
          description="Run tests or import a GitHub run, then compare it with this one."
        />
      ) : (
        <TableViewport className="ex-table-viewport cmp-choose">
          <TableFrame>
            <Table density="compact" inset className="ex-table" data-narrow>
              <TableCaption className="ds-visually-hidden">
                Executions to compare with A
              </TableCaption>
              <TableHeader>
                <TableRow>
                  <TableHead className="ex-col-select" scope="col">
                    <span className="ds-visually-hidden">Select</span>
                  </TableHead>
                  <TableHead scope="col">Execution</TableHead>
                  <TableHead className="ex-col-result" scope="col">
                    Result
                  </TableHead>
                  <TableHead className="ex-col-model" scope="col">
                    Model
                  </TableHead>
                  <TableHead className="ex-col-score ex-num" scope="col">
                    Score
                  </TableHead>
                </TableRow>
              </TableHeader>
              {groupLedgerRows(rows).map((group) => (
                <TableBody key={group.key} aria-label={group.label}>
                  <TableRow className="ex-group">
                    <TableHead colSpan={5} scope="colgroup">
                      <span className="ds-label">{group.label}</span>
                    </TableHead>
                  </TableRow>
                  {group.rows.map((row) => (
                    <TableRow
                      key={row.id}
                      interactive
                      tabIndex={-1}
                      className="ex-row"
                      data-candidate={row.id}
                      onClick={(event) => {
                        if (!isInteractiveTarget(event.target))
                          window.location.hash = pick(row)
                      }}
                    >
                      <TableCell className="ex-col-select">
                        <Checkbox
                          aria-label={`Select ${row.title}`}
                          checked={picked.includes(row.id)}
                          onChange={() => toggle(row.id)}
                        />
                      </TableCell>
                      <TableCell className="ex-cell-stack">
                        <span className="ex-title">
                          <a href={pick(row)} title={row.title}>
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
                      </TableCell>
                      <TableCell className="ex-cell-stack" title={row.models}>
                        <span className="ex-mono ex-model">{row.model}</span>
                        <span className="ex-sub ex-mono">{row.profile}</span>
                      </TableCell>
                      <TableCell className="ex-num">{row.score}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              ))}
            </Table>
          </TableFrame>
        </TableViewport>
      )}
      {cursor && onMore ? (
        <button
          type="button"
          className={buttonClassName({ variant: 'secondary' })}
          onClick={onMore}
          disabled={more.loading}
          aria-busy={more.loading}
          data-candidates-more
        >
          {more.loading
            ? 'Loading…'
            : `Load older executions · ${rows.length} loaded`}
        </button>
      ) : null}
      {more.error ? (
        <p className="cmp-warning" role="status">
          Older executions could not be loaded: {more.error}
        </p>
      ) : null}
    </>
  )
}

/** What the page shows before a comparison: a choice to make, an error or
 *  the loading skeleton. */
export function ComparisonPlaceholder({
  missing,
  error,
  onRetry,
}: {
  missing: boolean
  error: string | null
  onRetry?: () => void
}) {
  const back = (
    <a
      className={buttonClassName({
        variant: 'secondary',
        className: 'no-underline',
      })}
      href={hashForWorkspace('executions')}
    >
      Go to Executions
    </a>
  )
  if (missing)
    return (
      <>
        <Header
          title="Compare executions"
          summary="Executions side by side, each against the reference."
        />
        <EmptyState
          className="cmp-empty"
          title="Choose executions to compare"
          description="Tick two or more executions in the list, then Compare. The first one ticked is the reference; you can pick another here."
          actions={back}
        />
      </>
    )
  if (error)
    return (
      <>
        <Header title="Compare executions" summary="Not loaded" />
        <EmptyState
          className="cmp-empty"
          tone="error"
          title="The comparison could not be loaded"
          description={error}
          actions={
            <>
              {onRetry ? (
                <button
                  type="button"
                  className={buttonClassName({ variant: 'secondary' })}
                  onClick={onRetry}
                >
                  Retry
                </button>
              ) : null}
              {back}
            </>
          }
        />
      </>
    )
  return (
    <div className="cmp-skeleton" aria-busy="true" role="status">
      <span className="ds-visually-hidden">Loading the executions</span>
      <div className="cmp-skeleton-title" />
      <div className="cmp-skeleton-sides">
        <div />
        <div />
      </div>
      {['picker', 'highlights', 'results'].map((placeholder) => (
        <div key={placeholder} className="cmp-skeleton-block" />
      ))}
    </div>
  )
}

/** The status line of a group: suite and model when every execution
 *  shares them, how many executions and tests, and how many of them every
 *  total counts. */
function groupLine(pairs: ExecutionComparison[]) {
  const [first] = pairs
  const { scenarios } = first
  const counted = scenarios.filter((scenario) => scenario.counted).length
  const shared = (pick: (pair: ExecutionComparison) => string | null) =>
    pairs.every((pair) => pick(pair) === pick(first)) ? pick(first) : null
  return [
    shared((pair) => pair.b.suite) === first.a.suite ? first.a.suite : null,
    shared((pair) => pair.b.subject) === first.a.subject
      ? first.a.subject
      : null,
    plural(pairs.length + 1, 'execution'),
    plural(scenarios.length, 'test'),
    counted === scenarios.length ? null : `${counted} counted in every one`,
  ]
    .filter(Boolean)
    .join(' · ')
}

/** Whether two list rows are the same series: suite and model. */
function sameSeries(one: LedgerRow, two: LedgerRow) {
  return (
    one.model === two.model &&
    (one.execution.parameters?.suite?.id ?? null) ===
      (two.execution.parameters?.suite?.id ?? null)
  )
}

/** How a candidate's test list differs from the reference's, when both are
 *  recorded and they differ. */
export function testListNote(
  candidate: LedgerRow,
  reference: LedgerRow | null,
): string | null {
  const own = candidate.execution.parameters?.scenarios
  const theirs = reference?.execution.parameters?.scenarios
  if (!own || !theirs) return null
  const only = own.filter((id) => !theirs.includes(id)).length
  const missing = theirs.filter((id) => !own.includes(id)).length
  if (only === 0 && missing === 0) return null
  return `different test list: ${[
    only ? `${plural(only, 'test')} only here` : null,
    missing ? `${missing} missing` : null,
  ]
    .filter(Boolean)
    .join(', ')}`
}

/** Add executions to a group (canvas: Add executions · they become G and
 *  H): the latest executions, the reference's series first, each ticked one
 *  taking the next letter. */
function AddExecutionsDialog({
  open,
  onClose,
  ids,
  reference,
  onAdd,
}: {
  open: boolean
  onClose: () => void
  ids: string[]
  reference: string
  onAdd: (picked: string[]) => void
}) {
  const [candidates, setCandidates] = useState<Candidates | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [more, setMore] = useState<More>({ loading: false, error: null })
  const [picked, setPicked] = useState<string[]>([])
  const [scope, setScope] = useState<'series' | 'all'>('series')
  const beginRequest = useLatestRequest()
  useEffect(() => {
    if (!open) return
    setPicked([])
    const request = beginRequest()
    void (async () => {
      try {
        const next = await loadCandidates(
          await getDashboardDataBridge(),
          reference,
        )
        if (!request.isCurrent()) return
        setCandidates(next)
        setError(null)
      } catch (cause) {
        if (request.isCurrent()) setError(errorText(cause))
      }
    })()
  }, [open, reference, beginRequest])
  const loadMore = async () => {
    if (!candidates) return
    setMore({ loading: true, error: null })
    try {
      setCandidates(
        await loadOlderCandidates(
          await getDashboardDataBridge(),
          reference,
          candidates,
        ),
      )
      setMore({ loading: false, error: null })
    } catch (cause) {
      setMore({ loading: false, error: errorText(cause) })
    }
  }
  const rows = (candidates?.rows ?? []).filter((row) => !ids.includes(row.id))
  const base = candidates?.a ?? null
  const shown =
    scope === 'series' && base
      ? rows.filter((row) => sameSeries(row, base))
      : rows
  const letter = (id: string) => groupLetter(ids.length + picked.indexOf(id))
  const odd = rows.filter(
    (row) => picked.includes(row.id) && testListNote(row, base),
  )
  const toggle = (id: string) =>
    setPicked((current) =>
      current.includes(id)
        ? current.filter((entry) => entry !== id)
        : [...current, id],
    )
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) onClose()
      }}
    >
      <DialogContent
        className="ex-dialog ep-confirm-wide cmp-add"
        aria-describedby="cmp-add-body"
        data-add-dialog
      >
        <div>
          <DialogTitle className="ex-dialog-title">Add executions</DialogTitle>
          <DialogDescription id="cmp-add-body" className="ex-dialog-body">
            Each one takes the next letter and is read against{' '}
            {groupLetter(ids.indexOf(reference))}, like the others.
          </DialogDescription>
        </div>
        {base ? (
          <SegmentedControl
            variant="radio"
            aria-label="Show"
            value={scope}
            onChange={setScope}
            options={[
              {
                value: 'series',
                label: [
                  base.execution.parameters?.suite?.label ??
                    base.execution.parameters?.suite?.id ??
                    'This suite',
                  base.model,
                ].join(' · '),
              },
              { value: 'all', label: 'All executions' },
            ]}
          />
        ) : null}
        {error ? (
          <p className="ex-error" role="alert">
            The executions could not be listed: {error}
          </p>
        ) : !candidates ? (
          <p className="cmp-faint" role="status">
            Loading the latest executions…
          </p>
        ) : shown.length === 0 ? (
          <p className="cmp-faint">
            {scope === 'series'
              ? 'No other execution of this suite and model. Show all executions to pick from every one.'
              : 'No other execution to add.'}
          </p>
        ) : (
          <ul className="cmp-add-list" aria-label="Executions to add">
            {shown.map((row) => {
              const note = testListNote(row, base)
              return (
                <li key={row.id} data-candidate={row.id}>
                  <Checkbox
                    className="cmp-add-row"
                    checked={picked.includes(row.id)}
                    onChange={() => toggle(row.id)}
                    aria-label={`Add ${row.title}, ${row.meta}`}
                    label={
                      <>
                        <span className="cmp-add-copy">
                          <span className="cmp-strong">{row.title}</span>
                          <span
                            className="cmp-faint"
                            data-tone={note ? 'warn' : undefined}
                          >
                            {[row.meta, note].filter(Boolean).join(' · ')}
                          </span>
                        </span>
                        <span className="cmp-mono">{row.score}</span>
                        <span className="cmp-letter" aria-hidden="true">
                          {picked.includes(row.id) ? letter(row.id) : ''}
                        </span>
                      </>
                    }
                  />
                </li>
              )
            })}
          </ul>
        )}
        {candidates?.cursor ? (
          <button
            type="button"
            className="cmp-act"
            onClick={() => void loadMore()}
            disabled={more.loading}
            aria-busy={more.loading}
          >
            {more.loading ? 'Loading…' : 'Load older executions'}
          </button>
        ) : null}
        {more.error ? (
          <p className="ex-error" role="alert">
            Older executions could not be loaded: {more.error}
          </p>
        ) : null}
        {odd.length > 0 ? (
          <ul className="ex-dialog-facts">
            <li data-tone="warn">
              <AlertTriangle size={16} aria-hidden="true" />
              <span>
                {odd.map((row) => row.title).join(', ')} ran a different test
                list. Tests that one execution did not run leave every total;
                you can remove it again from its column.
              </span>
            </li>
          </ul>
        ) : null}
        <div className="ex-dialog-actions">
          <span className="cmp-faint cmp-add-summary" id="cmp-add-why">
            {picked.length > 0
              ? `${plural(picked.length, 'execution')} ticked · ${picked.length === 1 ? 'it becomes' : 'they become'} ${picked.map(letter).join(', ')}`
              : 'Tick at least one execution to add.'}
          </span>
          <Button type="button" variant="pill" size="sm" onClick={onClose}>
            Cancel
          </Button>
          <Button
            type="button"
            variant="primary"
            size="sm"
            aria-disabled={picked.length === 0}
            aria-describedby="cmp-add-why"
            onClick={() => {
              if (picked.length > 0) onAdd(picked)
            }}
            data-add-confirm
          >
            {picked.length > 0
              ? `Add ${plural(picked.length, 'execution')}`
              : 'Add executions'}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  )
}

export function ExecutionComparePage({
  executionIds,
  reference = null,
  compared = null,
}: {
  executionIds: string[]
  /** The reference and the one read against it in detail, from the hash;
   *  null for the defaults. */
  reference?: string | null
  compared?: string | null
}) {
  const [details, setDetails] = useState<DashboardExecutionDetail[] | null>(
    null,
  )
  const [error, setError] = useState<string | null>(null)
  // Why the last refresh of an execution failed, by id: what was loaded
  // stays on screen.
  const [stale, setStale] = useState<Record<string, string>>({})
  const [choice, setChoice] = useState<Choice>(() =>
    typeof window === 'undefined'
      ? { include: [], exclude: [] }
      : choiceFromParams(routeParams(window.location.hash)),
  )
  const [copied, setCopied] = useState<'summary' | 'link' | null>(null)
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  // Run again for B: its parameters with the tests the page picked.
  const [rerun, setRerun] = useState<{
    parameters: ExecutionParameters
    scenarios: string[]
  } | null>(null)
  // Open apart from the parameters, so Run again keeps its title while the
  // dialog animates closed.
  const [rerunOpen, setRerunOpen] = useState(false)
  const [addOpen, setAddOpen] = useState(false)
  const beginRequest = useLatestRequest()
  // The executions as one value: a new array of the same ids is no change.
  const key = executionIds.join('\n')

  const load = useCallback(async () => {
    const ids = key.split('\n')
    if (ids.length < 2) return
    const request = beginRequest()
    try {
      const next = await getDashboardDataBridge()
      if (!request.isCurrent()) return
      setBridge(next)
      const loaded = await loadExecutions((id) => next.getExecution(id), ids)
      if (!request.isCurrent()) return
      setDetails(loaded)
      setError(null)
      setStale({})
    } catch (cause) {
      if (request.isCurrent()) setError(errorText(cause))
    }
  }, [beginRequest, key])

  useEffect(() => {
    setDetails(null)
    setError(null)
    void load()
  }, [load])

  // Only the execution that changed is loaded again.
  const refresh = useCallback(
    async (id: string) => {
      if (!bridge) return
      try {
        const detail = await bridge.getExecution(id)
        setDetails(
          (current) =>
            current?.map((entry) => (entry.id === id ? detail : entry)) ?? null,
        )
        setStale((current) =>
          Object.fromEntries(
            Object.entries(current).filter(([entry]) => entry !== id),
          ),
        )
      } catch (cause) {
        setStale((current) => ({ ...current, [id]: errorText(cause) }))
      }
    },
    [bridge],
  )

  // An execution still running is followed until it finishes.
  const liveIds = (details ?? [])
    .filter((detail) => LIVE.includes(String(detail.status ?? '')))
    .map((detail) => detail.id)
    .join('\n')
  useEffect(() => {
    if (!bridge || !liveIds) return
    const stops = liveIds
      .split('\n')
      .map((id) => watchExecution(bridge, id, () => refresh(id)))
    return () => {
      for (const stop of stops) stop()
    }
  }, [bridge, liveIds, refresh])

  const picked = pickSides(executionIds, reference, compared)
  const referenceId = picked.a
  const comparedId = picked.b
  const pickError = executionIds.length < 2 ? null : picked.error

  useEffect(() => {
    if (pickError) return
    replaceRouteParams(
      viewParams(
        choice,
        origin(),
        selectionOf(key.split('\n'), referenceId, comparedId),
      ),
    )
  }, [choice, key, referenceId, comparedId, pickError])

  const group = useMemo(
    () => (details ? comparisonGroup(details) : null),
    [details],
  )
  const scoring = useMemo(
    () => (details ? scoringGroups(details) : new Map<string, number[][]>()),
    [details],
  )
  // The reference against every other execution, in the order chosen.
  const pairs = useMemo(() => {
    const base = details?.find((detail) => detail.id === referenceId)
    if (!details || !group || !base) return null
    return details
      .filter((detail) => detail !== base)
      .map((detail) => compareExecutions(base, detail, { ...choice, group }))
  }, [details, group, referenceId, choice])
  const comparison = pairs?.find((pair) => pair.b.id === comparedId) ?? null

  const shell = (children: ReactNode) => (
    <div className="ds-root cmp-page text-ink">
      <DashboardPageActions active="executions" context="Compare" />
      <div className="page-shell">{children}</div>
    </div>
  )

  if (executionIds.length === 1)
    return shell(<ChooseB left={executionIds[0]} />)
  if (pickError)
    return shell(<ComparisonPlaceholder missing={false} error={pickError} />)
  const a = details?.find((detail) => detail.id === referenceId)
  const b = details?.find((detail) => detail.id === comparedId)
  // Only a first load that failed replaces the page: a refresh that fails
  // keeps what was loaded (open rows, dialogs, the viewer) and says so.
  if (!details || !group || !pairs || !comparison || !a || !b)
    return shell(
      <ComparisonPlaceholder
        missing={executionIds.length < 2}
        error={error}
        onRetry={() => {
          setError(null)
          void load()
        }}
      />,
    )

  const sides: Sides = { a, b }
  const hashFor = (one: string, two: string) =>
    hashWithParams(
      hashForComparison(...executionIds),
      viewParams(choice, origin(), selectionOf(executionIds, one, two)),
    )
  // Without some executions: the picks stay where they still can.
  const withoutHref = (remove: string[]) => {
    const rest = executionIds.filter((id) => !remove.includes(id))
    if (rest.length === 0) return hashForWorkspace('executions')
    const one = rest.includes(referenceId) ? referenceId : rest[0]
    const two =
      rest.includes(comparedId) && comparedId !== one
        ? comparedId
        : (rest.find((id) => id !== one) ?? one)
    return hashWithParams(
      hashForComparison(...rest),
      viewParams(choice, origin(), selectionOf(rest, one, two)),
    )
  }
  const grouped: GroupView = {
    executions: details,
    pairs,
    group,
    scoring,
    compareHref: (id) => hashFor(referenceId, id),
    // The one in detail made the reference gives its place to the old one.
    referenceHref: (id) =>
      hashFor(id, id === comparedId ? referenceId : comparedId),
    withoutHref,
    onAdd: bridge ? () => setAddOpen(true) : undefined,
  }
  const refreshError =
    Object.entries(stale)
      .map(
        ([id, message]) =>
          `${group.names.get(id) ?? id} could not be refreshed: ${message}`,
      )
      .join(' · ') || error

  const copy = (what: 'summary' | 'link', value: string) => {
    void copyText(value).then((ok) => {
      if (!ok) return
      setCopied(what)
      window.setTimeout(() => setCopied(null), 1500)
    })
  }
  const runAgain = (scenarios: string[]) => {
    setRerunOpen(true)
    setRerun({
      parameters: rerunParameters(
        sides.b,
        sides.b.reports.map((record) => record.scenario_id),
        buildExecutionPresentation(sides.b).subjects[0],
      ),
      scenarios,
    })
  }
  // B's counted tests that scored lower than in A; none, and B runs again
  // as it ran.
  const lower = comparison.scenarios
    .filter(
      (scenario) =>
        scenario.counted &&
        (scenario.metrics.find((metric) => metric.id === 'score')?.delta ?? 0) <
          0,
    )
    .map((scenario) => scenario.id)

  return shell(
    <>
      <Header
        title={
          pairs.length > 1
            ? `${comparison.a.title} × ${plural(pairs.length, 'execution')}`
            : `${comparison.a.title} × ${comparison.b.title}`
        }
        summary={pairs.length > 1 ? groupLine(pairs) : summaryLine(comparison)}
        actions={
          <>
            <button
              className={buttonClassName({ variant: 'secondary' })}
              type="button"
              onClick={() => copy('summary', groupMarkdown(pairs))}
            >
              <ClipboardCopy size={16} aria-hidden="true" />
              {copied === 'summary' ? 'Summary copied' : 'Copy summary'}
            </button>
            <InvestigationAction {...comparisonInvestigation(comparison)} />
            {bridge ? (
              <button
                className={buttonClassName({ variant: 'primary' })}
                type="button"
                title={
                  lower.length > 0
                    ? `Run B again with its parameters, on the ${plural(lower.length, 'test')} B scored lower on`
                    : 'Run B again with its parameters'
                }
                onClick={() => runAgain(lower)}
                data-run-again={lower.join(',')}
              >
                <RotateCcw size={16} aria-hidden="true" />
                {lower.length > 0
                  ? `Run again · ${plural(lower.length, 'test')}`
                  : 'Run again'}
              </button>
            ) : null}
            {copied === 'link' ? (
              <span className="cmp-faint" role="status">
                Link copied
              </span>
            ) : null}
            <ExecutionMoreMenu
              onCopyLink={() => copy('link', window.location.href)}
            />
          </>
        }
      />
      <ComparisonView
        comparison={comparison}
        sides={sides}
        bridge={bridge}
        group={grouped}
        swap={hashFor(comparedId, referenceId)}
        here={hashFor(referenceId, comparedId)}
        refreshError={refreshError}
        onCount={(ids) =>
          setChoice(
            ids === null
              ? { include: [], exclude: [] }
              : choiceCounting(comparison.scenarios, ids),
          )
        }
        onRunTest={bridge ? (id) => runAgain([id]) : undefined}
      />
      {pairs.length > 1 ? (
        <AddExecutionsDialog
          open={addOpen}
          onClose={() => setAddOpen(false)}
          ids={executionIds}
          reference={referenceId}
          onAdd={(picked) => {
            setAddOpen(false)
            const all = [...executionIds, ...picked]
            window.location.hash = hashWithParams(
              hashForComparison(...all),
              viewParams(
                choice,
                origin(),
                selectionOf(all, referenceId, comparedId),
              ),
            )
          }}
        />
      ) : null}
      <LocalRunnerDialog
        bridge={bridge}
        open={rerunOpen}
        parameters={rerun?.parameters ?? null}
        initialScenarios={rerun?.scenarios}
        label={sides.b.plan_execution?.label ?? sides.b.label ?? ''}
        onClose={() => setRerunOpen(false)}
      />
    </>,
  )
}
