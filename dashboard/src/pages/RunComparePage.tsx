import { ArrowLeftRight, ArrowRight, Link2 } from 'lucide-react'
import { type ReactNode, useEffect, useMemo, useState } from 'react'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { executionText, runLinks } from '@/components/history/RunsTable'
import {
  BarsLegend,
  PairedBars,
  SessionTree,
} from '@/components/history/SessionTree'
import { InvestigationAction } from '@/components/InvestigationAction'
import {
  buttonClassName,
  EmptyState,
  FactChip,
  FactList,
  PageHeader,
  StatusLabel,
} from '@/design-system'
import {
  hashForRunComparison,
  hashForTestHistory,
  routeParams,
} from '@/hooks/use-hash-route'
import { useLatestRequest } from '@/hooks/use-latest-request'
import { getDashboardDataBridge } from '@/lib/dashboard-data-source'
import { shortDefinition } from '@/lib/definition-digest'
import {
  formatDateTime,
  formatDuration,
  formatTokens,
  NOT_REPORTED,
  plural,
} from '@/lib/format'
import type { Investigation } from '@/lib/investigation'
import type { TestSpec } from '@/lib/test-catalog'
import {
  ALL_DEFINITIONS,
  copiedText,
  copyText,
  differenceText,
  findRun,
  type HistoryObservation,
  type HistoryResponse,
  type HistoryRun,
  keyExecution,
  listedExecution,
  modelText,
  observationState,
  runState,
} from '@/lib/test-history'
import '@/design-system/styles.css'
import '@/components/compare/compare.css'
import './test-history.css'

type Which = 'a' | 'b'
const ROLE: Record<Which, string> = { a: 'Reference', b: 'Compared' }

export type Side = { observation: HistoryObservation; run: HistoryRun | null }

/** A run as the A × B reads it: the observation and its last attempt. */
export function side(observation: HistoryObservation): Side {
  return { observation, run: observation.runs?.at(-1) ?? null }
}

/** The chat reads the two runs as a comparison of their executions,
 *  focused on this test and these runs, with what the page says differs. */
export function runsInvestigation(
  testId: string,
  a: Side,
  b: Side,
): Investigation {
  // Two rounds of one plan are its two native executions.
  const same = listedExecution(a.observation) === listedExecution(b.observation)
  const execution = ({ observation }: Side) =>
    same ? observation.execution_id : listedExecution(observation)
  return {
    executionId: execution(a),
    comparisonExecutionId: execution(b),
    focus: {
      scenarioId: testId,
      runId: a.run?.run_id,
      comparedRunId: b.run?.run_id,
    },
    changes: comparability(a.observation, b.observation).changed.map(
      ({ label, value }) => ({ what: label, change: value }),
    ),
  }
}

function listText(items: string[]) {
  if (items.length <= 1) return items.join('')
  return `${items.slice(0, -1).join(', ')} and ${items.at(-1)}`
}

/** What the two runs share and what differs between them. */
export function comparability(a: HistoryObservation, b: HistoryObservation) {
  const fields: Array<[string, string, string]> = [
    [
      'definition',
      shortDefinition(a.behavior_sha256) ?? NOT_REPORTED,
      shortDefinition(b.behavior_sha256) ?? NOT_REPORTED,
    ],
    ['model', modelText(a), modelText(b)],
    ['profile', a.agent_profile ?? 'none', b.agent_profile ?? 'none'],
    ['system', a.system_label ?? NOT_REPORTED, b.system_label ?? NOT_REPORTED],
  ]
  const same = fields.filter(([, left, right]) => left === right)
  const changed = fields.filter(([, left, right]) => left !== right)
  const definition = same.find(([field]) => field === 'definition')
  return {
    same: same.length
      ? `Same ${listText(
          same.map(([field, value]) =>
            field === 'definition' && definition
              ? `definition ${value}`
              : field,
          ),
        )}.`
      : '',
    changed: changed.map(([field, left, right]) => ({
      label: field,
      value: `${left} → ${right}`,
    })),
    note: changed.length
      ? `Differences below come from the ${listText(changed.map(([field]) => field))} and from run-to-run variation.`
      : 'Nothing recorded differs: the differences below are run-to-run variation.',
  }
}

type Row = {
  label: string
  a: number | null
  b: number | null
  /** Each side's figure as written, `8/9` for criteria met. */
  text: Record<Which, string>
  /** How the difference is written. */
  delta: (value: number) => string
  points?: boolean
}

function count(value: number) {
  return String(Math.round(value))
}

function number(value: number | null | undefined) {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function met(run: HistoryRun | null) {
  const criteria = run?.details?.criteria ?? []
  return criteria.length
    ? criteria.filter((item) => item.awarded === item.possible).length
    : null
}

/** The side's score: the run's, where its criteria and tokens come from. */
export function sideScore(value: Side) {
  return number(value.run ? value.run.score : value.observation.mean_score)
}

function sideDuration(value: Side) {
  const seconds = value.run
    ? value.run.duration_seconds
    : value.observation.median_duration_seconds
  return seconds == null ? null : seconds * 1000
}

/** The metrics side by side, B minus A beside them, no verdict. Every
 *  figure is the chosen run's, not the observation's mean or median. */
export function metricRows(a: Side, b: Side): Row[] {
  const both = (read: (value: Side) => number | null | undefined) =>
    [number(read(a)), number(read(b))] as const
  const rows: Array<
    [string, readonly [number | null, number | null], Row['delta'], boolean?]
  > = [
    ['Score', both(sideScore), count, true],
    ['Criteria met', both((s) => met(s.run)), count],
    ['Duration', both(sideDuration), formatDuration],
    [
      'Turns',
      both((s) => (s.run ? s.run.turns : s.observation.median_turns)),
      count,
    ],
    [
      'Sessions',
      both((s) => (s.run?.details ? s.run.details.sessions.length : null)),
      count,
    ],
    [
      'Function calls',
      both((s) =>
        s.run ? s.run.function_calls : s.observation.median_function_calls,
      ),
      count,
    ],
    [
      'Function call errors',
      both((s) =>
        s.run
          ? s.run.function_call_errors
          : s.observation.median_function_call_errors,
      ),
      count,
    ],
    ['Input tokens', both((s) => s.run?.details?.input_tokens), formatTokens],
    ['Output tokens', both((s) => s.run?.details?.output_tokens), formatTokens],
    [
      'Cache read',
      both((s) => s.run?.details?.cache_read_tokens),
      formatTokens,
    ],
    [
      'Cache written',
      both((s) => s.run?.details?.cache_write_tokens),
      formatTokens,
    ],
  ]
  // Criteria met reads `8/9`, each side over its own criteria.
  const total = (value: Side) => value.run?.details?.criteria.length ?? '?'
  const write = (label: string, value: number | null, side: Side) =>
    value === null
      ? NOT_REPORTED
      : label === 'Criteria met'
        ? `${value}/${total(side)}`
        : (rows.find((row) => row[0] === label)?.[2] ?? count)(value)
  return rows
    .filter(([, [left, right]]) => left !== null || right !== null)
    .map(([label, [left, right], delta, points]) => ({
      label,
      a: left,
      b: right,
      text: { a: write(label, left, a), b: write(label, right, b) },
      delta,
      points,
    }))
}

export type CriterionChange = {
  id: string
  possible: number
  a: number
  b: number
  reasons: Record<Which, string | null>
}

/** Criteria whose points differ, the largest change first, with the reason
 *  each run gave; and how many were met on both sides. */
export function criteriaChanges(a: Side, b: Side) {
  const left = new Map(
    (a.run?.details?.criteria ?? []).map((item) => [item.id, item]),
  )
  const right = new Map(
    (b.run?.details?.criteria ?? []).map((item) => [item.id, item]),
  )
  const ids = [...new Set([...left.keys(), ...right.keys()])]
  const changes: CriterionChange[] = []
  let metOnBoth = 0
  let lostOnBoth = 0
  for (const id of ids) {
    const one = left.get(id)
    const two = right.get(id)
    if (!one || !two) continue
    const pointsA = one.awarded ?? 0
    const pointsB = two.awarded ?? 0
    if (pointsA === pointsB) {
      if (pointsA === one.possible) metOnBoth += 1
      else lostOnBoth += 1
      continue
    }
    changes.push({
      id,
      possible: Math.max(one.possible, two.possible),
      a: pointsA,
      b: pointsB,
      reasons: { a: one.reason || null, b: two.reason || null },
    })
  }
  changes.sort(
    (one, two) =>
      Math.abs(two.b - two.a) - Math.abs(one.b - one.a) ||
      one.id.localeCompare(two.id),
  )
  return { changes, metOnBoth, lostOnBoth }
}

/** The root session's calls of each worker, A against B, most called first. */
export function workerCalls(a: Side, b: Side) {
  const calls = (value: Side) =>
    new Map(
      (value.run?.details?.calls_by_worker ?? []).map((item) => [
        item.worker,
        item.calls,
      ]),
    )
  const left = calls(a)
  const right = calls(b)
  const workers = [...new Set([...left.keys(), ...right.keys()])]
    .map((worker) => ({
      worker,
      a: left.get(worker) ?? 0,
      b: right.get(worker) ?? 0,
    }))
    .sort(
      (one, two) =>
        two.a + two.b - (one.a + one.b) || one.worker.localeCompare(two.worker),
    )
  const max = Math.max(0, ...workers.flatMap((item) => [item.a, item.b]))
  return { workers, max }
}

function callsNote(a: Side, b: Side) {
  const children = (['a', 'b'] as const).flatMap((which) => {
    const count = (which === 'a' ? a : b).run?.details?.child_sessions ?? 0
    return count > 0
      ? [`${which.toUpperCase()} also ran ${plural(count, 'child session')}`]
      : []
  })
  return ['root session only', ...children].join('; ')
}

function SideCard({ which, value }: { which: Which; value: Side }) {
  const { observation, run } = value
  const links = runLinks(observation, run ?? undefined)
  const sessions = run?.details?.sessions.length
  return (
    <article
      className="cmp-side"
      aria-label={`${which.toUpperCase()} · ${ROLE[which]}`}
      data-comparison-side={which}
    >
      <div className="cmp-side-top">
        <span className="cmp-letter" aria-hidden="true">
          {which.toUpperCase()}
        </span>
        <span className="cmp-faint">{ROLE[which]}</span>
        <StatusLabel
          className="cmp-side-state"
          state={run ? runState(run) : observationState(observation)}
          label={
            sideScore(value) === null
              ? NOT_REPORTED
              : String(Math.round(sideScore(value) as number))
          }
        />
      </div>
      <p className="cmp-side-title">
        {formatDateTime(observation.completed_at)}
      </p>
      <p className="cmp-side-meta">
        {[
          modelText(observation),
          observation.agent_profile
            ? `profile ${observation.agent_profile}`
            : 'no profile',
          run ? `run ${run.run_id.slice(0, 8)}` : null,
          sessions ? plural(sessions, 'session') : null,
        ]
          .filter(Boolean)
          .join(' · ')}
      </p>
      <div className="cmp-side-foot">
        <a className="cmp-side-origin" href={links.execution}>
          {executionText(observation)}
        </a>
        {links.transcript ? (
          <a className="cmp-open" href={links.transcript}>
            Transcript
          </a>
        ) : null}
        {links.evidence ? (
          <a className="cmp-open" href={links.evidence}>
            Evidence record
            <ArrowRight size={16} aria-hidden="true" />
          </a>
        ) : null}
      </div>
    </article>
  )
}

export function RunComparison({
  a,
  b,
  spec,
  onSwap,
}: {
  a: Side
  b: Side
  spec: TestSpec | null
  onSwap: () => void
}) {
  const shared = comparability(a.observation, b.observation)
  const criteria = criteriaChanges(a, b)
  const calls = workerCalls(a, b)
  const description = (id: string) =>
    spec?.criteria.find((item) => item.id === id)?.description ?? null
  const unread = (['a', 'b'] as const).filter(
    (which) => !(which === 'a' ? a : b).run?.details,
  )
  return (
    <>
      <section className="cmp-sides" aria-label="Runs compared">
        <SideCard which="a" value={a} />
        <button
          type="button"
          className="cmp-swap"
          aria-label="Swap A and B"
          onClick={onSwap}
        >
          <ArrowLeftRight size={16} aria-hidden="true" />
        </button>
        <SideCard which="b" value={b} />
      </section>

      <section
        className="cmp-changes"
        aria-label="What changed"
        data-comparison-changes
      >
        <div className="cmp-changes-row">
          {shared.same ? <span className="rc-same">{shared.same}</span> : null}
          {shared.changed.length > 0 ? (
            <>
              <span className="cmp-eyebrow">What changed</span>
              <FactList>
                {shared.changed.map((change) => (
                  <FactChip key={change.label} {...change} />
                ))}
              </FactList>
            </>
          ) : null}
          <span className="cmp-faint">{shared.note}</span>
        </div>
        {unread.length > 0 ? (
          <p className="cmp-warning">
            The native results of{' '}
            {listText(unread.map((which) => which.toUpperCase()))} could not be
            read: criteria, calls and sessions are missing for{' '}
            {unread.length === 1 ? 'that side' : 'both'}.
          </p>
        ) : null}
      </section>

      <div className="cmp-detail-grid rc-grid">
        <section className="cmp-detail-col" aria-labelledby="rc-metrics">
          <h2 id="rc-metrics" className="cmp-h2">
            Metrics
          </h2>
          <div className="cmp-inset">
            <table className="cmp-metrics" data-comparison-metrics>
              <caption className="ds-visually-hidden">
                Metrics of A and B, with B minus A
              </caption>
              <thead>
                <tr>
                  <th scope="col">
                    <span className="ds-visually-hidden">Metric</span>
                  </th>
                  <th scope="col">A</th>
                  <th scope="col">B</th>
                  <th scope="col">Difference</th>
                </tr>
              </thead>
              <tbody>
                {metricRows(a, b).map((row) => (
                  <tr key={row.label} data-metric={row.label}>
                    <th scope="row">{row.label}</th>
                    <td className="cmp-faint-num">{row.text.a}</td>
                    <td>{row.text.b}</td>
                    <td className="cmp-delta">
                      {differenceText(
                        row.a,
                        row.b,
                        row.delta,
                        row.points ? 'points' : 'value',
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>

        <div className="cmp-detail-col">
          <section className="cmp-detail-col" aria-labelledby="rc-criteria">
            <div className="th-section-head">
              <h2 id="rc-criteria" className="cmp-h2">
                Criteria that changed
              </h2>
              <span className="cmp-faint">
                {[
                  criteria.metOnBoth
                    ? `${criteria.metOnBoth} more met on both sides`
                    : null,
                  criteria.lostOnBoth
                    ? `${criteria.lostOnBoth} lost the same points on both`
                    : null,
                ]
                  .filter(Boolean)
                  .join(' · ')}
              </span>
            </div>
            {criteria.changes.length === 0 ? (
              <p className="cmp-faint cmp-empty-line">
                No criterion scored differently.
              </p>
            ) : null}
            {criteria.changes.map((change) => (
              <div
                key={change.id}
                className="cmp-criterion"
                data-criterion={change.id}
              >
                <div className="cmp-criterion-head">
                  <span className="cmp-criterion-id">{change.id}</span>
                  <span className="cmp-mono">
                    <span className="cmp-faint-num">
                      {change.a}/{change.possible} →{' '}
                    </span>
                    {change.b}/{change.possible}
                  </span>
                  <span className="cmp-tag">
                    {change.b > change.a ? '+' : '−'}
                    {Math.abs(change.b - change.a)}
                  </span>
                </div>
                {description(change.id) ? (
                  <p className="cmp-faint cmp-criterion-label">
                    {description(change.id)}
                  </p>
                ) : null}
                {(['a', 'b'] as const).flatMap((which) =>
                  change.reasons[which]
                    ? [
                        <div className="cmp-reason" key={which}>
                          <span className="cmp-letter-sm">
                            {which.toUpperCase()}
                          </span>
                          <code>{change.reasons[which]}</code>
                        </div>,
                      ]
                    : [],
                )}
              </div>
            ))}
          </section>

          <section className="cmp-detail-col" aria-labelledby="rc-calls">
            <div className="th-section-head">
              <h2 id="rc-calls" className="cmp-h2">
                Calls by worker
              </h2>
              <span className="cmp-faint">{callsNote(a, b)}</span>
            </div>
            <div className="th-inset">
              {calls.workers.length === 0 ? (
                <p className="cmp-faint cmp-empty-line">
                  No function call was recorded on either side.
                </p>
              ) : (
                <ul className="rc-calls">
                  {calls.workers.map((item) => (
                    <li key={item.worker} data-worker={item.worker}>
                      <span className="th-mono th-ellipsis">{item.worker}</span>
                      <PairedBars
                        first={
                          calls.max ? Math.round((item.a / calls.max) * 100) : 0
                        }
                        second={
                          calls.max ? Math.round((item.b / calls.max) * 100) : 0
                        }
                      />
                      <span className="th-mono rc-calls-figures">
                        <span className="th-faint-num">{item.a} → </span>
                        {item.b}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
              <BarsLegend first="A" second="B" />
            </div>
          </section>
        </div>
      </div>

      <section className="rc-subs" aria-label="Sub-agents of A and B">
        {(['a', 'b'] as const).map((which) => {
          const value = which === 'a' ? a : b
          return value.run?.details ? (
            <SessionTree
              key={which}
              details={value.run.details}
              turns={value.run.turns ?? value.observation.median_turns ?? null}
              heading={`${which.toUpperCase()} · Sub-agents`}
              headingId={`rc-subs-${which}`}
            />
          ) : (
            <section key={which} className="th-sessions">
              <h2 className="th-h3">{which.toUpperCase()} · Sub-agents</h2>
              <p className="th-faint th-empty-line">
                Not known: the run’s native results could not be read.
              </p>
            </section>
          )
        })}
      </section>
    </>
  )
}

/** A and B as the hash names them. */
export function pairFromHash(hash: string) {
  const params = routeParams(hash)
  return [params.get('a') ?? '', params.get('b') ?? '']
}

/** Follows the hash: back, forward or a pasted link change the pair. */
function useRoutePair() {
  const [pair, setPair] = useState<string[]>(() =>
    typeof window === 'undefined'
      ? ['', '']
      : pairFromHash(window.location.hash),
  )
  useEffect(() => {
    const follow = () =>
      setPair((current) => {
        const next = pairFromHash(window.location.hash)
        return next.join('|') === current.join('|') ? current : next
      })
    window.addEventListener('hashchange', follow)
    return () => window.removeEventListener('hashchange', follow)
  }, [])
  return pair
}

export function RunComparePage({ testId }: { testId: string }) {
  const pair = useRoutePair()
  // What was loaded, and for which executions, so a new pair never shows
  // the previous one's runs.
  const [loaded, setLoaded] = useState<{
    wanted: string
    history: HistoryResponse
  } | null>(null)
  const [spec, setSpec] = useState<TestSpec | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [copied, setCopied] = useState<string | null>(null)
  const beginRequest = useLatestRequest()
  // Swapping A and B asks for nothing new.
  const wanted = [...new Set(pair.filter(Boolean).map(keyExecution))]
    .sort()
    .join(',')

  useEffect(() => {
    if (!wanted) return
    const request = beginRequest()
    setError(null)
    void getDashboardDataBridge()
      .then(async (bridge) => {
        const [data, tests] = await Promise.all([
          // Exactly the two runs, wherever they sit in the history.
          bridge.getTestHistory({
            test_id: testId,
            test_version: ALL_DEFINITIONS,
            executions: wanted.split(','),
            limit: 100,
          }),
          bridge.listTests({ limit: 100 }).catch(() => null),
        ])
        if (!request.isCurrent()) return
        setLoaded({ wanted, history: data as HistoryResponse })
        setSpec(
          tests?.rows.find((item) => item.test_id === testId)?.spec ?? null,
        )
      })
      .catch((cause) => {
        if (request.isCurrent())
          setError(cause instanceof Error ? cause.message : String(cause))
      })
  }, [beginRequest, testId, wanted])

  const history = loaded?.wanted === wanted ? loaded.history : null
  const sides = useMemo(() => {
    const [a, b] = pair.map((key) => findRun(history?.observations ?? [], key))
    return a && b ? { a: side(a), b: side(b) } : null
  }, [history, pair])

  const back = {
    label: `Back to ${testId}`,
    href: hashForTestHistory(testId),
  }
  const header = (summary: string, actions?: ReactNode) => (
    <PageHeader
      variant="detail"
      className="cmp-header"
      back={back}
      title={`Two runs of ${testId}`}
      headingId="run-comparison-title"
      summary={summary}
      actions={actions}
    />
  )
  const shell = (children: ReactNode) => (
    <div className="ds-root cmp-page rc-page">
      <DashboardPageActions active="tests" context={`${testId} · A × B`} />
      <div className="page-shell">{children}</div>
    </div>
  )

  if (!pair[0] || !pair[1])
    return shell(
      <>
        {header('Two runs of the test, side by side.')}
        <EmptyState
          title="Choose two runs"
          description="Tick two runs in the test's history, then Compare A and B. The first one ticked is A, the reference."
          actions={
            <a
              className={buttonClassName({
                variant: 'secondary',
                className: 'no-underline',
              })}
              href={back.href}
            >
              Go to the history
            </a>
          }
        />
      </>,
    )
  if (error || (history && !sides))
    return shell(
      <>
        {header('Not loaded')}
        <EmptyState
          tone="error"
          title="The runs could not be compared"
          description={
            error ??
            'One of the two runs is not in this test’s history any more.'
          }
        />
      </>,
    )
  if (!sides)
    return shell(
      <div className="cmp-skeleton" aria-busy="true" role="status">
        <span className="ds-visually-hidden">Loading both runs</span>
        <div className="cmp-skeleton-title" />
        <div className="cmp-skeleton-sides">
          <div />
          <div />
        </div>
        <div className="cmp-skeleton-block" />
      </div>,
    )

  return shell(
    <>
      {header(
        `A ${formatDateTime(sides.a.observation.completed_at)} · B ${formatDateTime(sides.b.observation.completed_at)}`,
        <>
          {copied ? (
            <span className="cmp-faint" role="status">
              {copied}
            </span>
          ) : null}
          <button
            type="button"
            className={buttonClassName({ variant: 'secondary' })}
            onClick={() =>
              void copyText(window.location.href).then((ok) => {
                setCopied(copiedText('Link', ok))
                window.setTimeout(() => setCopied(null), 2500)
              })
            }
          >
            <Link2 size={16} aria-hidden="true" />
            Copy link
          </button>
          <InvestigationAction
            label="Investigate A and B"
            {...runsInvestigation(testId, sides.a, sides.b)}
          />
        </>,
      )}
      <RunComparison
        a={sides.a}
        b={sides.b}
        spec={spec}
        onSwap={() => {
          window.location.hash = hashForRunComparison(testId, pair[1], pair[0])
        }}
      />
    </>,
  )
}
