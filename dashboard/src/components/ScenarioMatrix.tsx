import {
  Check,
  ChevronRight,
  FileCheck,
  RotateCcw,
  ScrollText,
} from 'lucide-react'
import { useEffect, useId, useMemo, useRef, useState } from 'react'
import {
  formatFull,
  formatSpan,
  formatTokens,
  formatUsd,
  type Kpi,
  KpiTile,
  plural,
} from '@/components/execution/ExecutionTotals'
import { ScreenshotGallery } from '@/components/execution/screenshots'
import { ScenarioChatAction } from '@/components/ScenarioChatAction'
import {
  buttonClassName,
  type OperationalStatus,
  Panel,
  StatusBadge,
  StatusLabel,
} from '@/design-system'
import { hashForExecution } from '@/hooks/use-hash-route'
import {
  type AssessmentRunView,
  buildAssessmentWorkspace,
} from '@/lib/assessment-view'
import type {
  DashboardExecutionDetail,
  SemanticTestReport,
} from '@/lib/dashboard-data-source'
import { shortDefinition } from '@/lib/definition-digest'
import { buildExecutionMetrics } from '@/lib/execution-metrics'
import { titleCase } from '@/lib/execution-view'
import { sentenceCase } from '@/lib/format'
import type { ResultState } from '@/lib/result-status'
import {
  buildScenarioMatrix,
  detailForScenario,
  formatScenarioDuration,
  ownReason,
  type PreviousAttempt,
  previousAttempts,
  roundKey,
  type ScenarioMatrixItem,
  stepSignals,
  unreported,
} from '@/lib/scenario-matrix'
import { screenshotsOf } from '@/lib/screenshots'

export type ResultFilter = 'all' | 'lost' | 'notrun' | 'passed'

/** No run retained and not waiting to report: what Not run counts. */
function leftNoRun(item: Pick<ScenarioMatrixItem, 'objective' | 'runCount'>) {
  return item.runCount === 0 && !unreported(item)
}

export function matchesFilter(item: ScenarioMatrixItem, filter: ResultFilter) {
  const score = itemScore(item)
  if (filter === 'lost') return score !== null && score < 100
  if (filter === 'notrun') return leftNoRun(item)
  if (filter === 'passed')
    return item.objective.status === 'passed' && score === 100
  return true
}

export function ScenarioMatrix({
  detail,
  onTranscript,
  showContract = true,
  onRerun,
  openKey = null,
  running = false,
  liveNote = 'Rows fill in as tests report.',
  notes,
  heading,
}: {
  detail: DashboardExecutionDetail
  /** The section title, on the filter row as the canvas draws it. */
  heading?: string
  /** Open this row from outside (Needs attention · Show test). */
  openKey?: string | null
  onTranscript: (run: AssessmentRunView, title: string) => void
  /** The results contract is provenance; the layered execution page renders
   *  it in the provenance layer instead of above the table (audit ED-29). */
  showContract?: boolean
  /** Run one scenario of the execution again. */
  onRerun?: (scenarioId: string) => void
  running?: boolean
  /** What the bar says while it runs. */
  liveNote?: string
  /** The line under a test that has not reported yet, by `roundKey`. */
  notes?: Record<string, string>
}) {
  const model = useMemo(() => buildScenarioMatrix(detail), [detail])
  const [filter, setFilter] = useState<ResultFilter>('all')
  useEffect(() => {
    if (openKey) setFilter('all')
  }, [openKey])
  if (model.items.length === 0) {
    return (
      <div className="ep-empty">
        No scenario reports were retained for this execution.
      </div>
    )
  }
  const segments: Array<[ResultFilter, string, number]> = (
    [
      ['all', 'All', model.items.length],
      [
        'lost',
        'Lost points',
        model.items.filter((i) => matchesFilter(i, 'lost')).length,
      ],
      [
        'notrun',
        'Not run',
        model.items.filter((i) => matchesFilter(i, 'notrun')).length,
      ],
      [
        'passed',
        'Full marks',
        model.items.filter((i) => matchesFilter(i, 'passed')).length,
      ],
    ] as Array<[ResultFilter, string, number]>
  ).filter(([key, , count]) => key === 'all' || count > 0)
  const shown = model.items.filter((item) => matchesFilter(item, filter))
  return (
    <div className="ep-results">
      {showContract ? (
        <ResultContractStrip contracts={model.contracts} />
      ) : null}
      <div className="ep-results-bar">
        {heading ? (
          <h2 id="execution-results-heading" className="ep-results-title">
            {heading}
          </h2>
        ) : null}
        <fieldset className="ep-segments">
          <legend className="ep-sr">Show</legend>
          {segments.map(([key, label, count]) => (
            <button
              key={key}
              type="button"
              aria-pressed={filter === key}
              data-selected={filter === key}
              data-result-filter={key}
              onClick={() => setFilter(key)}
            >
              {label} <span className="ep-faint">{count}</span>
            </button>
          ))}
        </fieldset>
        <span className="ep-faint">
          {running
            ? liveNote
            : 'Suite order. Open a test for its criteria, run and evidence.'}
        </span>
      </div>
      <table className="ep-results-table" aria-label="Scenario results">
        <thead>
          <tr>
            {[
              'Test',
              'Result',
              'Score',
              'Duration',
              'Tokens',
              'Cost',
              'Turns',
            ].map((label) => (
              <th
                key={label}
                scope="col"
                className={
                  ['Duration', 'Tokens', 'Cost', 'Turns'].includes(label)
                    ? 'ep-cell-wide'
                    : undefined
                }
              >
                {label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {shown.map((item) => (
            <ScenarioResult
              key={item.key}
              detail={detailForScenario(detail, item)}
              item={item}
              previous={previousAttempts(detail, item)}
              executionId={detail.id}
              onTranscript={onTranscript}
              onRerun={onRerun}
              open={openKey === item.key}
              waitingNote={
                notes?.[
                  roundKey(
                    detail.reports[item.reportIndex]?.round,
                    item.scenarioId,
                  )
                ]
              }
            />
          ))}
        </tbody>
      </table>
      {shown.length === 0 ? (
        <p className="ep-faint">No test matches this filter.</p>
      ) : null}
    </div>
  )
}

/** Closed-row scent for the provenance layer of the execution page: the
 *  contract in one line, hashes kept as hashes (audit ED-26 / ED-30). */
export function contractScent(
  contracts: ReturnType<typeof buildScenarioMatrix>['contracts'],
): string {
  if (contracts.length === 0) return 'results contract unavailable'
  const distinct = (values: string[]) => [...new Set(values)].join(' / ')
  return [
    `results contract ${distinct(contracts.map((c) => c.reportState ?? 'unavailable'))}`,
    distinct(contracts.map((c) => c.objectiveOutcome ?? 'unavailable')),
    `contract ${distinct(contracts.map((c) => shortDigest(c.resultContractSha256)))}`,
    ...(contracts.some((c) => !c.resultContractCurrent)
      ? ['differs from this console']
      : []),
  ].join(' · ')
}

/** What differs from the contract this Console was built with, if anything. */
export function contractDrift(contract: {
  resultContractCurrent: boolean
}): string | null {
  if (contract.resultContractCurrent) return null
  return 'Written under another results contract than this Console; figures are shown as reported.'
}

export function ResultContractStrip({
  contracts,
}: {
  contracts: ReturnType<typeof buildScenarioMatrix>['contracts']
}) {
  if (contracts.length === 0) {
    return (
      <Panel
        tone="raised"
        padding="compact"
        className="text-sm text-ink-muted"
        data-results-contract="unavailable"
      >
        Results contract unavailable. Objective outcome and report completeness
        are not inferred from execution status.
      </Panel>
    )
  }
  return (
    <Panel
      as="section"
      tone="raised"
      padding="compact"
      className="grid min-w-0 gap-3"
      aria-label="Results contract"
    >
      {contracts.map((contract) => (
        <div
          key={contract.key}
          className="grid gap-3 @[640px]/harness:grid-cols-2 @[1000px]/harness:grid-cols-3"
          data-results-contract={contract.valid ? 'valid' : 'invalid'}
        >
          {/* Audit ED-30: title case belongs to words. Applied to every value it
              turned a hash into "Sha256:A7eb…". */}
          <ContractFact
            label="report state"
            value={titleCase(contract.reportState ?? 'unavailable')}
          />
          <ContractFact
            label="objective outcome"
            value={titleCase(contract.objectiveOutcome ?? 'unavailable')}
          />
          {/* The report is identified by the contract it was written against,
              the digest the runner also refuses to read across. */}
          <ContractFact
            label="results contract"
            value={shortDigest(contract.resultContractSha256)}
          />
          {contractDrift(contract) ? (
            <p
              className="m-0 text-xs text-warning @[640px]/harness:col-span-2 @[1000px]/harness:col-span-3"
              data-results-contract-drift="true"
            >
              {contractDrift(contract)}
            </p>
          ) : null}
        </div>
      ))}
    </Panel>
  )
}

function ContractFact({ label, value }: { label: string; value: string }) {
  return (
    <span className="min-w-0">
      <span className="ds-label block">{label}</span>
      <strong className="mt-1 block truncate font-mono text-xs text-ink">
        {value}
      </strong>
    </span>
  )
}

/** Digests are shown as the first hex chars of the SHA-256, as elsewhere in
 *  the Console; the `sha256:` prefix carries no identity. */
function shortDigest(value: string | null) {
  if (!value) return 'unavailable'
  return value.replace(/^sha256:/, '').slice(0, 12)
}

export type RunCriterion = {
  id: string
  description: string
  awarded: number
  possible: number
  gate: boolean
  reason: string
}

/** The criteria a run projection carries, read defensively. */
export function runCriteria(run: unknown): RunCriterion[] {
  const list = (run as { criteria?: unknown } | null)?.criteria
  if (!Array.isArray(list)) return []
  return list.flatMap((entry) => {
    if (!entry || typeof entry !== 'object') return []
    const c = entry as Record<string, unknown>
    if (typeof c.id !== 'string') return []
    return [
      {
        id: c.id,
        description: typeof c.description === 'string' ? c.description : '',
        awarded: typeof c.awarded === 'number' ? c.awarded : 0,
        possible: typeof c.possible === 'number' ? c.possible : 0,
        gate: c.gate === true,
        reason: typeof c.reason === 'string' ? c.reason : '',
      },
    ]
  })
}

/** A test's figures, summed over its attempts. */
export type RunFigures = {
  durationMs: number | null
  costUsd: number | null
  inputTokens: number | null
  outputTokens: number | null
  tokens: number | null
  cacheRead: number | null
  cacheWrite: number | null
  turns: number | null
  functionCalls: number | null
  functionErrors: number | null
}

/** The open row's six run cards, as the canvas captions them. */
export function runMetricTiles(f: RunFigures): Kpi[] {
  return [
    {
      label: 'Duration',
      value: formatSpan(f.durationMs),
      sub: 'sum of attempts',
    },
    {
      label: 'Cost',
      value: formatUsd(f.costUsd),
      sub: f.costUsd === null ? 'not reported' : 'recorded spend',
    },
    {
      label: 'Tokens',
      value: formatTokens(f.tokens),
      sub: `in ${formatTokens(f.inputTokens)} · out ${formatTokens(f.outputTokens)}`,
      full:
        f.tokens === null
          ? undefined
          : `${formatFull(f.tokens)} input + output`,
    },
    {
      label: 'Cache',
      value: formatTokens(f.cacheRead),
      sub: `read · written ${formatTokens(f.cacheWrite)}`,
      full:
        f.cacheRead === null ? undefined : `${formatFull(f.cacheRead)} read`,
    },
    { label: 'Turns', value: formatFull(f.turns), sub: '' },
    {
      label: 'Function calls',
      value: formatFull(f.functionCalls),
      sub:
        f.functionErrors === null
          ? ''
          : plural(f.functionErrors, 'error', 'errors'),
    },
  ]
}

/** Mean of the retained runs' scores, null when none was scored. */
export function itemScore(item: ScenarioMatrixItem): number | null {
  const scores = item.runs
    .map((run) => run.score)
    .filter((score): score is number => typeof score === 'number')
  if (scores.length === 0) return null
  return (
    Math.round((scores.reduce((a, b) => a + b, 0) / scores.length) * 10) / 10
  )
}

/** The reason's first clause, as the data wrote it: its case is left alone
 *  (it may start with an id or an acronym). */
function firstClause(text: string) {
  const clause = text.split(/\.\s|\n|:\s/)[0].trim()
  return clause.length > 64 ? `${clause.slice(0, 63)}…` : clause
}

/** One line under the test id, in the result's terms: why it lost points,
 *  did not finish the task or did not run. */
export function rowNote(item: ScenarioMatrixItem): string {
  const reason = ownReason(item)
  if (item.runCount === 0) {
    if (!reason) return 'No run retained'
    const clause = firstClause(reason)
    return item.objective.status === 'not-run' && !/^Didn’t start/.test(clause)
      ? `Didn’t start · ${clause}`
      : clause
  }
  const lost = runCriteria(item.primaryRun).filter(
    (c) => c.awarded < c.possible,
  )
  const prefix =
    item.objective.status === 'incomplete'
      ? 'Task incomplete'
      : item.objective.status === 'failed' && lost.some((c) => c.gate)
        ? 'Failed a hard gate'
        : item.objective.status !== 'passed' && reason
          ? firstClause(reason)
          : null
  const suffix = lost.length
    ? `${lost.length} ${lost.length === 1 ? 'criterion' : 'criteria'} lost`
    : null
  return [prefix, suffix].filter(Boolean).join(' · ')
}

/** A row's result in the canvas's vocabulary (RESULT): the tone paints the
 *  dot and, tinted, the word. Cancelling is still live; an unavailable
 *  report of a test that ran is only undetermined. */
const ROW_RESULT: Record<OperationalStatus, ResultState> = {
  passed: 'passed',
  failed: 'failed',
  inconclusive: 'inconclusive',
  unavailable: 'inconclusive',
  'not-run': 'not_run',
  recommendation: 'inconclusive',
  running: 'running',
  cancelling: 'running',
  cancelled: 'cancelled',
  incomplete: 'incomplete',
  queued: 'queued',
}

/** Not run exactly where the Not run filter counts it; otherwise the
 *  result's own tone. */
export function rowResultState(
  item: Pick<ScenarioMatrixItem, 'objective' | 'runCount'>,
): ResultState {
  return leftNoRun(item) ? 'not_run' : ROW_RESULT[item.objective.status]
}

function ScenarioResult({
  detail,
  item,
  previous,
  executionId,
  onTranscript,
  onRerun,
  open = false,
  waitingNote,
}: {
  detail: DashboardExecutionDetail
  item: ScenarioMatrixItem
  open?: boolean
  /** Its line while it has not reported yet. */
  waitingNote?: string
  /** Attempts the last one replaced: listed, counted nowhere. */
  previous: PreviousAttempt[]
  executionId: string
  onTranscript: (run: AssessmentRunView, title: string) => void
  onRerun?: (scenarioId: string) => void
}) {
  const [expanded, setExpanded] = useState(false)
  const rowRef = useRef<HTMLTableRowElement>(null)
  useEffect(() => {
    if (!open) return
    setExpanded(true)
    // Opened from outside (Needs attention): bring it into view once shown.
    window.requestAnimationFrame(() =>
      rowRef.current?.scrollIntoView({ block: 'center' }),
    )
  }, [open])
  const panelId = useId()
  const assessmentRuns = useMemo(
    () => buildAssessmentWorkspace(detail).runs,
    [detail],
  )
  const turns = assessmentRuns.reduce<number | null>(
    (sum, run) =>
      typeof run.metrics.turns === 'number'
        ? (sum ?? 0) + run.metrics.turns
        : sum,
    null,
  )
  const metrics = useMemo(() => buildExecutionMetrics(detail), [detail])
  const pick = (metric: { total: number | null; observed: number | null }) =>
    metric.total ?? metric.observed
  // A figure only part of the runs reported says so on hover.
  const partialNote = (metric: {
    total: number | null
    observed: number | null
    samples: number
    expected: number
  }) =>
    metric.total === null && metric.observed !== null
      ? `Partial · ${metric.samples}/${metric.expected} runs reported`
      : undefined
  const duration = pick(metrics.durationMs)
  const tokens = pick(metrics.subjectTokens)
  const cost = pick(metrics.cost)
  const score =
    metrics.scoreMean === null
      ? itemScore(item)
      : Math.round(metrics.scoreMean * 10) / 10
  const scoreNote =
    metrics.scoreSamples > 0
      ? `Mean · ${metrics.scoreSamples}/${metrics.planned} planned runs scored`
      : undefined
  const runId = item.primaryRun?.run_id
  const primaryAssessment = assessmentRuns.find((run) => run.runId === runId)
  const criteria = runCriteria(item.primaryRun)
  const lost = criteria.filter((c) => c.awarded < c.possible)
  // Running or queued: nothing to open until it reports.
  const waiting = unreported(item)
  const note = waiting ? (waitingNote ?? '') : rowNote(item)
  const definition = shortDefinition(item.behaviorSha256)
  const count = (value: number | null) =>
    value === null
      ? '—'
      : new Intl.NumberFormat('en-US').format(Math.round(value))
  const runTiles = runMetricTiles({
    durationMs: duration,
    costUsd: cost,
    inputTokens: pick(metrics.inputTokens),
    outputTokens: pick(metrics.outputTokens),
    tokens,
    cacheRead: pick(metrics.cacheReadTokens),
    cacheWrite: pick(metrics.cacheWriteTokens),
    turns,
    functionCalls: pick(metrics.functionCalls),
    functionErrors: pick(metrics.functionErrors),
  })
  const attempt = Number(item.primaryRun?.attempt_number ?? 1)
  // The last retained run is the one shown: its number among them.
  const runMeta = runId ? `run ${item.runs.length} · attempt ${attempt}` : ''
  const screenshots = expanded ? screenshotsOf(detail, item.scenarioId) : []
  const title = `${item.scenarioId}${definition ? ` · definition ${definition}` : ''}`
  return (
    <>
      <tr
        ref={rowRef}
        data-scenario-row={item.key}
        className="ep-result-row"
        data-row-state={waiting ? item.objective.status : undefined}
        aria-label={`${titleCase(item.scenarioId)} scenario result`}
      >
        <th scope="row" className="ep-cell ep-cell-test">
          <button
            type="button"
            aria-expanded={expanded}
            aria-controls={panelId}
            disabled={waiting}
            onClick={() => setExpanded(!expanded)}
            className="ep-row-toggle"
          >
            <ChevronRight
              size={16}
              className={`ep-chevron ${expanded ? 'ep-rot' : ''}`}
              aria-hidden="true"
            />
            <span className="ep-row-id">
              <span className="ep-mono ep-strong" title={title}>
                {item.scenarioId}
              </span>
              {note ? <span className="ep-row-note">{note}</span> : null}
              {previous.length > 0 ? (
                <span
                  className="ep-row-note ep-warn-text"
                  data-reruns={previous.length}
                  title="Ran again; only the last attempt counts"
                >
                  rerun ×{previous.length}
                </span>
              ) : null}
            </span>
          </button>
        </th>
        <td className="ep-cell" data-label="Result">
          <StatusLabel
            tinted
            state={rowResultState(item)}
            label={item.objective.label}
          />
        </td>
        <td className="ep-cell" data-label="Score" title={scoreNote}>
          <span className="ep-score">
            <span className="ep-score-bar" aria-hidden="true">
              <span
                style={{ width: `${Math.max(0, Math.min(100, score ?? 0))}%` }}
              />
            </span>
            <span className="ep-mono">{score ?? '—'}</span>
          </span>
        </td>
        <td
          className="ep-cell ep-cell-wide ep-mono"
          data-label="Duration"
          data-primary-metric="Runtime"
          title={partialNote(metrics.durationMs)}
        >
          {formatSpan(duration)}
        </td>
        <td
          className="ep-cell ep-cell-wide ep-mono"
          data-label="Tokens"
          data-primary-metric="Total tokens"
          title={
            partialNote(metrics.subjectTokens) ??
            (tokens === null ? undefined : `${count(tokens)} input + output`)
          }
        >
          {formatTokens(tokens)}
        </td>
        <td
          className="ep-cell ep-cell-wide ep-mono"
          data-label="Cost"
          data-primary-metric="Reported cost"
          title={partialNote(metrics.cost)}
        >
          {formatUsd(cost)}
        </td>
        <td
          className="ep-cell ep-cell-wide ep-mono"
          data-label="Turns"
          data-primary-metric="Turns"
        >
          {turns ?? '—'}
        </td>
      </tr>
      <tr id={panelId} hidden={!expanded} className="ep-detail-row">
        <td colSpan={7} className="ep-detail-cell">
          <div className="ep-detail">
            {item.runCount === 0 ? (
              <div className="ep-notrun">
                <p>
                  {item.reason
                    ? 'The test didn’t start, so it has no score or evidence. Needs attention above has the error.'
                    : 'No run was retained for this test, so it has no score or evidence.'}
                </p>
                {onRerun ? (
                  <button
                    type="button"
                    className="ep-act ep-act-ctl"
                    data-rerun-scenario={item.scenarioId}
                    aria-label={`Run ${titleCase(item.scenarioId)} again`}
                    onClick={() => onRerun(item.scenarioId)}
                  >
                    Run this test again
                  </button>
                ) : null}
              </div>
            ) : (
              <div className="ep-row-grid">
                <div className="ep-row-criteria">
                  <h3 className="ep-h3">
                    {lost.length ? 'Criteria that lost points' : 'Criteria'}
                  </h3>
                  {criteria.length > 0 && lost.length === 0 ? (
                    <p className="ep-met">
                      <Check size={16} aria-hidden="true" />
                      Every criterion met.
                    </p>
                  ) : null}
                  {lost.map((c) => (
                    <div
                      className="ep-lost"
                      key={c.id}
                      data-lost-criterion={c.id}
                    >
                      <div className="ep-lost-head">
                        <span className="ep-lost-id">{c.id}</span>
                        {c.gate ? (
                          <span className="ep-gate">hard gate</span>
                        ) : null}
                        <span className="ep-lost-points">
                          −{Math.round((c.possible - c.awarded) * 10) / 10}
                        </span>
                      </div>
                      <code className="ep-lost-reason">
                        {c.reason || c.description}
                      </code>
                    </div>
                  ))}
                  {item.reason && item.objective.status !== 'passed' ? (
                    <p className="ep-row-reason">{item.reason}</p>
                  ) : null}
                  {criteria.length === 0 && !item.reason ? (
                    <p className="ep-met">This run reported no criteria.</p>
                  ) : null}
                </div>
                <div className="ep-row-run">
                  <div className="ep-run-head">
                    <h3 className="ep-h3">Run</h3>
                    <span className="ep-run-meta" title={runId}>
                      {runMeta}
                    </span>
                  </div>
                  <ul
                    // biome-ignore lint/a11y/noRedundantRoles: Safari drops the list role under list-style none
                    role="list"
                    className="ep-kpis ep-run-kpis"
                    aria-label="Run metrics"
                  >
                    {runTiles.map((kpi) => (
                      <KpiTile key={kpi.label} kpi={kpi} as="li" />
                    ))}
                  </ul>
                  {/* biome-ignore lint/a11y/useSemanticElements: a labelled group of commands, not a form fieldset */}
                  <div
                    role="group"
                    className="ep-run-actions"
                    aria-label="Run actions"
                  >
                    {primaryAssessment?.transcript ? (
                      <button
                        type="button"
                        className="ep-act ep-row-act"
                        aria-label={`View transcript for ${titleCase(item.scenarioId)}`}
                        onClick={() =>
                          onTranscript(
                            primaryAssessment,
                            `${item.scenarioId} · ${runId}`,
                          )
                        }
                      >
                        <ScrollText aria-hidden="true" />
                        Transcript
                      </button>
                    ) : null}
                    {runId ? (
                      <a
                        className="ep-act ep-row-act"
                        href={hashForExecution(executionId, null, runId)}
                        aria-label={`Evidence record for ${titleCase(item.scenarioId)}`}
                      >
                        <FileCheck aria-hidden="true" />
                        Evidence record
                      </a>
                    ) : null}
                    <ScenarioChatAction
                      label="Ask in chat"
                      buttonClass="ep-act ep-row-act"
                      detail={detail}
                      scenarioId={item.scenarioId}
                      subjectId={item.subjectId}
                    />
                    {onRerun ? (
                      <button
                        type="button"
                        className="ep-act ep-row-act"
                        aria-label={`Run ${titleCase(item.scenarioId)} again`}
                        data-rerun-scenario={item.scenarioId}
                        onClick={() => onRerun(item.scenarioId)}
                      >
                        <RotateCcw aria-hidden="true" />
                        Run again
                      </button>
                    ) : null}
                  </div>
                </div>
              </div>
            )}
            {screenshots.length > 0 ? (
              <div className="ep-row-shots">
                <h3 className="ep-h3">
                  Screenshots{' '}
                  <span className="ep-h3-note">
                    · select one to open it full size
                  </span>
                </h3>
                <ScreenshotGallery
                  bridge={null}
                  heading={false}
                  screenshots={screenshots}
                />
              </div>
            ) : null}
            {item.runs.length > 1 ? (
              <ul className="ep-runs" aria-label="Retained runs">
                {item.runs.map((run) => {
                  const assessment = assessmentRuns.find(
                    (entry) => entry.runId === run.run_id,
                  )
                  return (
                    <li key={run.attempt_id}>
                      <span className="ep-mono">{run.run_id}</span>
                      <a
                        className={buttonClassName({
                          variant: 'secondary',
                          size: 'compact',
                        })}
                        href={hashForExecution(executionId, null, run.run_id)}
                      >
                        Evidence record
                      </a>
                      {assessment?.transcript ? (
                        <button
                          type="button"
                          className={buttonClassName({
                            variant: 'quiet',
                            size: 'compact',
                          })}
                          onClick={() =>
                            onTranscript(
                              assessment,
                              `${item.scenarioId} · ${run.run_id}`,
                            )
                          }
                        >
                          Transcript
                        </button>
                      ) : null}
                    </li>
                  )
                })}
              </ul>
            ) : null}
            {previous.length > 0 ? (
              <PreviousAttempts previous={previous} />
            ) : null}
            {!item.available && !waiting ? (
              <p className="ep-faint">
                The expected report for this scenario is unavailable. Runtime
                and workflow data are intentionally not inferred.
              </p>
            ) : null}
            {item.workflowSteps.length > 0 ? (
              <WorkflowDurationProfile tests={item.workflowSteps} />
            ) : null}
          </div>
        </td>
      </tr>
    </>
  )
}

/** The attempts the last one replaced, oldest first, each with its result,
 *  score, reason and evidence; none of them counts anywhere. */
function PreviousAttempts({ previous }: { previous: PreviousAttempt[] }) {
  return (
    <section
      className="mt-4 grid gap-2"
      aria-label="Previous attempts"
      data-previous-attempts
    >
      <h4 className="m-0 ds-label">previous attempts · not counted</h4>
      <p className="m-0 text-xs text-ink-muted">
        Only the last attempt counts; these stay out of the score, the totals
        and the comparison.
      </p>
      <ol className="m-0 grid list-none gap-2 p-0">
        {previous.map(({ executionId, item }, index) => (
          <li
            key={executionId}
            className="flex flex-wrap items-center gap-3"
            data-previous-attempt={executionId}
          >
            <span className="font-mono text-label text-ink-muted">
              attempt {index + 1}
            </span>
            <StatusBadge
              status={item.objective.status}
              label={item.objective.label}
            />
            <strong className="font-mono text-xs">
              {scoreLabel(item.primaryRun?.score)}
            </strong>
            {item.reason ? (
              <span className="min-w-0 break-words text-xs text-ink-muted">
                {item.reason}
              </span>
            ) : null}
            {item.primaryRun ? (
              <a
                className={buttonClassName({
                  variant: 'quiet',
                  size: 'compact',
                })}
                href={hashForExecution(
                  executionId,
                  null,
                  item.primaryRun.run_id,
                )}
              >
                Evidence record
              </a>
            ) : null}
          </li>
        ))}
      </ol>
    </section>
  )
}

function scoreLabel(value: number | null | undefined) {
  return typeof value === 'number' && Number.isFinite(value)
    ? `${formatDecimal(value)}/100`
    : '—'
}

function formatDecimal(value: number) {
  return value.toLocaleString('en-US', { maximumFractionDigits: 1 })
}

function WorkflowDurationProfile({ tests }: { tests: SemanticTestReport[] }) {
  const maxDuration = Math.max(...tests.map((test) => test.duration_ms), 1)
  const totalDuration = tests.reduce(
    (total, test) => total + Math.max(test.duration_ms, 0),
    0,
  )

  return (
    <section
      className="border-t border-[var(--color-rule)]"
      aria-label="Workflow duration profile"
    >
      <header className="grid gap-2 border-b border-[var(--color-rule)] px-4 py-4 @[768px]/harness:grid-cols-[minmax(0,1fr)_auto] @[768px]/harness:items-end @[768px]/harness:px-5">
        <div>
          <h3 className="m-0 text-sm font-semibold text-ink">
            Workflow duration profile
          </h3>
          <p className="m-0 mt-1 text-xs leading-5 text-ink-muted">
            Bars compare recorded step duration. They do not infer parallel
            timing that is absent from the report.
          </p>
        </div>
        <span className="font-mono text-label text-ink-muted">
          {formatScenarioDuration(totalDuration)} recorded step time
        </span>
      </header>
      <div className="hidden grid-cols-[minmax(12rem,0.9fr)_7rem_minmax(14rem,1.4fr)_minmax(12rem,1fr)] gap-4 border-b border-[var(--color-rule)] bg-panel-raised px-5 py-2.5 font-mono text-label font-semibold uppercase tracking-[0.06em] text-ink-muted @[1000px]/harness:grid">
        <span>Step</span>
        <span>Duration</span>
        <span>Duration profile</span>
        <span>Step metrics</span>
      </div>
      <ol className="m-0 grid list-none p-0">
        {tests.map((test, index) => (
          <WorkflowStepRow
            key={test.node_id}
            test={test}
            index={index}
            maxDuration={maxDuration}
            totalDuration={totalDuration}
          />
        ))}
      </ol>
    </section>
  )
}

function WorkflowStepRow({
  test,
  index,
  maxDuration,
  totalDuration,
}: {
  test: SemanticTestReport
  index: number
  maxDuration: number
  totalDuration: number
}) {
  const status = workflowStepStatus(test.status)
  const metrics = stepSignals(test)
  const width = Math.max(
    1.5,
    (Math.max(test.duration_ms, 0) / maxDuration) * 100,
  )
  const share =
    totalDuration > 0
      ? (Math.max(test.duration_ms, 0) / totalDuration) * 100
      : 0
  const dependencies = test.dependencies.length
    ? `After ${test.dependencies.map(titleCase).join(', ')}`
    : 'Starts workflow'
  const barTone =
    status.status === 'passed'
      ? 'bg-success'
      : status.status === 'failed'
        ? 'bg-danger'
        : 'bg-warning'

  return (
    <li
      className="grid gap-3 border-t border-[var(--color-rule)] px-4 py-4 first:border-t-0 @[1000px]/harness:grid-cols-[minmax(12rem,0.9fr)_7rem_minmax(14rem,1.4fr)_minmax(12rem,1fr)] @[1000px]/harness:items-center @[1000px]/harness:gap-4 @[1000px]/harness:px-5"
      data-workflow-step={test.node_id}
    >
      <div className="flex min-w-0 items-start gap-3">
        <span className="grid size-7 shrink-0 place-items-center rounded-[6px] bg-panel-raised font-mono text-label font-semibold text-ink-muted">
          {String(index + 1).padStart(2, '0')}
        </span>
        <div className="min-w-0">
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <strong className="truncate text-sm text-ink">
              {titleCase(test.node_id)}
            </strong>
            <StatusBadge
              className="[&_.ds-status-dot]:size-1.5"
              status={status.status}
              label={status.label}
            />
          </div>
          <span className="mt-1 block truncate text-label text-ink-muted">
            {dependencies}
          </span>
        </div>
      </div>
      <div className="grid grid-cols-[7rem_minmax(0,1fr)] items-center gap-3 @[1000px]/harness:block">
        <span className="font-mono text-label font-semibold uppercase tracking-[0.06em] text-ink-muted @[1000px]/harness:hidden">
          Duration
        </span>
        <strong className="font-mono text-xs font-semibold tabular-nums text-ink">
          {formatScenarioDuration(test.duration_ms)}
        </strong>
      </div>
      <div className="grid grid-cols-[7rem_minmax(0,1fr)] items-center gap-3 @[1000px]/harness:block">
        <span className="font-mono text-label font-semibold uppercase tracking-[0.06em] text-ink-muted @[1000px]/harness:hidden">
          Profile
        </span>
        <div>
          <div className="h-2 overflow-hidden rounded-full bg-[var(--color-surface-hover)]">
            <div
              className={`h-full rounded-full ${barTone}`}
              style={{ width: `${width}%` }}
            />
          </div>
          <span className="mt-1 block font-mono text-label tabular-nums text-ink-muted">
            {share.toFixed(share >= 10 ? 0 : 1)}% of recorded step time
          </span>
        </div>
      </div>
      <div className="grid grid-cols-[7rem_minmax(0,1fr)] items-start gap-3 @[1000px]/harness:block">
        <span className="font-mono text-label font-semibold uppercase tracking-[0.06em] text-ink-muted @[1000px]/harness:hidden">
          Metrics
        </span>
        <div
          className="flex min-w-0 flex-wrap gap-1.5"
          data-step-metrics={test.node_id}
        >
          {metrics.map((metric) => (
            <span
              key={metric.label}
              className="inline-flex min-w-0 items-baseline gap-1 rounded-[6px] bg-panel-raised px-2 py-1 font-mono text-label text-ink-muted"
              data-step-metric={metric.label}
            >
              <span className="truncate">{metric.label}</span>
              <strong className="shrink-0 text-[var(--color-ink-faint)]">
                {metric.value}
              </strong>
            </span>
          ))}
        </div>
      </div>
    </li>
  )
}

function workflowStepStatus(statusValue: string): {
  status: OperationalStatus
  label: string
} {
  const status = statusValue.toLowerCase()
  if (status === 'succeeded') return { status: 'passed', label: 'Succeeded' }
  if (status === 'hard_gate_failed') {
    return { status: 'failed', label: 'Runtime check failed' }
  }
  if (status === 'failed') return { status: 'failed', label: 'Failed' }
  if (status === 'running') return { status: 'running', label: 'Running' }
  if (status === 'cancelled') {
    return { status: 'cancelled', label: 'Cancelled' }
  }
  if (status === 'pending' || status === 'skipped') {
    return { status: 'incomplete', label: sentenceCase(status) }
  }
  return { status: 'unavailable', label: sentenceCase(status) }
}
