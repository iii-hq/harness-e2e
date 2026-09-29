import {
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
import { ClipboardCopy, RotateCcw } from 'lucide-react'
import {
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from 'react'
import { ComparisonView, type Sides } from '@/components/compare/ComparisonView'
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
import {
  type DashboardDataBridge,
  type DashboardExecutionDetail,
  type ExecutionParameters,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import {
  compareExecutions,
  comparisonMarkdown,
  type ExecutionComparison,
  exclusionPhrase,
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

/** The choice's params, keeping the Trends view to go back to. */
export function viewParams(choice: Choice, from = origin()): URLSearchParams {
  const params = choiceToParams(choice)
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

/** Both executions, or an error that names the side that failed. */
export async function loadExecutionPair(
  getExecution: (id: string) => Promise<DashboardExecutionDetail>,
  left: string,
  right: string,
): Promise<Sides> {
  const [a, b] = await Promise.allSettled([
    getExecution(left),
    getExecution(right),
  ])
  if (a.status === 'fulfilled' && b.status === 'fulfilled')
    return { a: a.value, b: b.value }
  const reason = (result: PromiseSettledResult<unknown>) =>
    result.status === 'rejected'
      ? result.reason instanceof Error
        ? result.reason.message
        : String(result.reason)
      : null
  throw new Error(
    (
      [
        ['A', left, reason(a)],
        ['B', right, reason(b)],
      ] as const
    )
      .flatMap(([side, id, message]) =>
        message === null
          ? []
          : [`${side} (${id}) could not be loaded: ${message}`],
      )
      .join(' · '),
  )
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

type Candidates = { a: LedgerRow | null; rows: LedgerRow[] }

/** A, named, and the latest executions but A, to pick B from. */
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
  }
}

/** Compare with…: A is set, B is picked from the latest executions. */
function ChooseB({ left }: { left: string }) {
  const [candidates, setCandidates] = useState<Candidates | null>(null)
  const [error, setError] = useState<string | null>(null)
  const beginRequest = useLatestRequest()
  const load = useCallback(async () => {
    const request = beginRequest()
    try {
      const next = await loadCandidates(await getDashboardDataBridge(), left)
      if (!request.isCurrent()) return
      setCandidates(next)
      setError(null)
    } catch (cause) {
      if (request.isCurrent())
        setError(cause instanceof Error ? cause.message : String(cause))
    }
  }, [beginRequest, left])
  useEffect(() => {
    void load()
  }, [load])

  if (error)
    return (
      <ComparisonPlaceholder
        missing={false}
        error={error}
        onRetry={() => void load()}
      />
    )
  if (!candidates) return <ComparisonPlaceholder missing={false} error={null} />
  return <CandidateList left={left} {...candidates} />
}

export function CandidateList({
  left,
  a,
  rows,
}: Candidates & { left: string }) {
  const pick = (row: LedgerRow) => hashForComparison(left, row.id)
  return (
    <>
      <Header
        title="Compare with…"
        summary={
          <>
            A is <strong>{a?.title ?? left}</strong>, the reference. Pick B,
            compared with it.
          </>
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
                    <TableHead colSpan={4} scope="colgroup">
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
          summary="Two executions, side by side: A the reference, B compared with it."
        />
        <EmptyState
          className="cmp-empty"
          title="Choose two executions"
          description="Tick two executions in the list, then Compare A and B. The first one ticked is A, the reference; you can swap them here."
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
      <span className="ds-visually-hidden">Loading both executions</span>
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

export function ExecutionComparePage({
  left,
  right,
}: {
  left: string | null
  right: string | null
}) {
  const [sides, setSides] = useState<Sides | null>(null)
  const [error, setError] = useState<string | null>(null)
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
  const beginRequest = useLatestRequest()

  const load = useCallback(async () => {
    if (!left || !right) return
    const request = beginRequest()
    try {
      const next = await getDashboardDataBridge()
      if (!request.isCurrent()) return
      setBridge(next)
      const loaded = await loadExecutionPair(
        (id) => next.getExecution(id),
        left,
        right,
      )
      if (!request.isCurrent()) return
      setSides(loaded)
      setError(null)
    } catch (cause) {
      if (request.isCurrent())
        setError(cause instanceof Error ? cause.message : String(cause))
    }
  }, [beginRequest, left, right])

  useEffect(() => {
    setSides(null)
    setError(null)
    void load()
  }, [load])

  // A side still running is followed until it finishes.
  const live = (['a', 'b'] as const).filter((which) =>
    LIVE.includes(String(sides?.[which].status ?? '')),
  )
  const liveIds = live.map((which) => sides?.[which].id ?? '').join(',')
  useEffect(() => {
    if (!bridge || !liveIds) return
    const stops = liveIds
      .split(',')
      .map((id) => watchExecution(bridge, id, load))
    return () => {
      for (const stop of stops) stop()
    }
  }, [bridge, liveIds, load])

  useEffect(() => {
    replaceRouteParams(viewParams(choice))
  }, [choice])

  const comparison = useMemo(
    () => (sides ? compareExecutions(sides.a, sides.b, choice) : null),
    [sides, choice],
  )

  const shell = (children: ReactNode) => (
    <div className="ds-root cmp-page text-ink">
      <DashboardPageActions active="executions" context="Compare" />
      <div className="page-shell">{children}</div>
    </div>
  )

  if (left && !right) return shell(<ChooseB left={left} />)
  // Only a first load that failed replaces the page: a refresh that fails
  // keeps what was loaded (open rows, dialogs, the viewer) and says so.
  if (!left || !right || !sides || !comparison)
    return shell(
      <ComparisonPlaceholder
        missing={!left || !right}
        error={error}
        onRetry={() => {
          setError(null)
          void load()
        }}
      />,
    )

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
        title={`${comparison.a.title} × ${comparison.b.title}`}
        summary={summaryLine(comparison)}
        actions={
          <>
            <button
              className={buttonClassName({ variant: 'secondary' })}
              type="button"
              onClick={() => copy('summary', comparisonMarkdown(comparison))}
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
        swap={hashWithParams(
          hashForComparison(right, left),
          viewParams(choice),
        )}
        here={hashWithParams(
          hashForComparison(left, right),
          viewParams(choice),
        )}
        refreshError={error}
        onCount={(ids) =>
          setChoice(
            ids === null
              ? { include: [], exclude: [] }
              : choiceCounting(comparison.scenarios, ids),
          )
        }
        onRunTest={bridge ? (id) => runAgain([id]) : undefined}
      />
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
