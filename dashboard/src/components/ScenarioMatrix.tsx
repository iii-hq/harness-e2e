import { ChevronDown, Eye } from 'lucide-react'
import { useId, useMemo, useState } from 'react'
import { ScenarioChatAction } from '@/components/ScenarioChatAction'
import {
  buttonClassName,
  type OperationalStatus,
  Panel,
  StatusBadge,
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
import {
  buildScenarioMatrix,
  detailForScenario,
  formatScenarioDuration,
  type ScenarioMatrixItem,
  stepSignals,
} from '@/lib/scenario-matrix'

export function ScenarioMatrix({
  detail,
  onTranscript,
  showContract = true,
}: {
  detail: DashboardExecutionDetail
  onTranscript: (run: AssessmentRunView, title: string) => void
  /** The results contract is provenance; the layered execution page renders
   *  it in the provenance layer instead of above the table (audit ED-29). */
  showContract?: boolean
}) {
  const model = useMemo(() => buildScenarioMatrix(detail), [detail])
  if (model.items.length === 0) {
    return (
      <div className="rounded-[var(--ds-radius-sm)] border border-dashed border-[var(--color-edge)] bg-panel-raised p-5 text-sm text-ink-muted">
        No scenario reports were retained for this execution.
      </div>
    )
  }

  return (
    <div className="grid gap-4">
      {showContract ? (
        <ResultContractStrip contracts={model.contracts} />
      ) : null}
      <ScenarioSummary summary={model.summary} />
      <table
        className="scenario-results-table block w-full table-fixed border-collapse text-left text-xs @[1000px]/harness:table"
        aria-label="Scenario results"
      >
        <thead className="hidden border-b border-[var(--color-rule)] text-ink-muted @[1000px]/harness:table-header-group">
          <tr>
            {[
              'Scenario',
              'Result',
              'Score',
              'Runtime',
              'Tokens',
              'Cost',
              'Evidence',
            ].map((label) => (
              <th
                key={label}
                scope="col"
                className={`px-3 py-3 font-medium ${label === 'Scenario' ? 'w-[28%]' : label === 'Evidence' ? 'w-[20%]' : ''}`}
              >
                {label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody className="block @[1000px]/harness:table-row-group">
          {model.items.map((item) => (
            <ScenarioResult
              key={item.key}
              detail={detailForScenario(detail, item)}
              item={item}
              executionId={detail.id}
              onTranscript={onTranscript}
            />
          ))}
        </tbody>
      </table>
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

function ScenarioSummary({
  summary,
}: {
  summary: ReturnType<typeof buildScenarioMatrix>['summary']
}) {
  const entries: Array<{
    status: OperationalStatus
    count: number
    label: string
  }> = [
    { status: 'passed', count: summary.passed, label: 'passed' },
    { status: 'failed', count: summary.failed, label: 'failed' },
    {
      status: 'inconclusive',
      count: summary.inconclusive,
      label: 'inconclusive',
    },
    {
      status: 'unavailable',
      count: summary.unavailable,
      label: 'unavailable',
    },
    { status: 'running', count: summary.running, label: 'running' },
    {
      status: 'incomplete',
      count: summary.incomplete,
      label: 'incomplete',
    },
  ]

  return (
    <section
      className="flex flex-wrap items-center gap-x-5 gap-y-2 border-y border-[var(--color-rule)] py-3"
      aria-label="Scenario result summary"
    >
      <strong className="font-mono text-xs font-semibold text-ink">
        {summary.total} {summary.total === 1 ? 'scenario' : 'scenarios'}
      </strong>
      {entries
        .filter((entry) => entry.count > 0)
        .map((entry) => (
          <StatusBadge
            key={entry.status}
            status={entry.status}
            label={`${entry.count} ${entry.label}`}
          />
        ))}
    </section>
  )
}

function ScenarioResult({
  detail,
  item,
  executionId,
  onTranscript,
}: {
  detail: DashboardExecutionDetail
  item: ScenarioMatrixItem
  executionId: string
  onTranscript: (run: AssessmentRunView, title: string) => void
}) {
  const [expanded, setExpanded] = useState(false)
  const panelId = useId()
  const assessmentRuns = useMemo(
    () => buildAssessmentWorkspace(detail).runs,
    [detail],
  )
  const metrics = useMemo(() => buildExecutionMetrics(detail), [detail])
  const scoreMean = metrics.scoreMean
  const scoreSamples = metrics.scoreSamples
  const planned = metrics.planned
  const usage = [
    { label: 'Runtime', id: 'durationMs' as const, metric: metrics.durationMs },
    {
      label: 'Total tokens',
      id: 'totalTokens' as const,
      metric: metrics.subjectTokens,
    },
    { label: 'Reported cost', id: 'costUsd' as const, metric: metrics.cost },
    {
      label: 'Input tokens',
      id: 'inputTokens' as const,
      metric: metrics.inputTokens,
    },
    {
      label: 'Output tokens',
      id: 'outputTokens' as const,
      metric: metrics.outputTokens,
    },
    {
      label: 'Cache read',
      id: 'cacheRead' as const,
      metric: metrics.cacheReadTokens,
    },
    {
      label: 'Cache written',
      id: 'cacheWrite' as const,
      metric: metrics.cacheWriteTokens,
    },
    { label: 'Turns', id: 'turns' as const, metric: metrics.turns },
    {
      label: 'Function calls',
      id: 'functionCalls' as const,
      metric: metrics.functionCalls,
    },
    {
      label: 'Function errors',
      id: 'functionErrors' as const,
      metric: metrics.functionErrors,
    },
  ].map(({ label, metric }) => {
    const value = metric.total ?? metric.observed
    return {
      label,
      value:
        value === null
          ? '—'
          : label === 'Runtime'
            ? formatScenarioDuration(value)
            : label === 'Reported cost'
              ? value > 0 && value < 0.0001
                ? '<$0.0001'
                : `$${value.toFixed(4)}`
              : formatDecimal(value),
      detail:
        metric.total !== null
          ? 'Accumulated across runs, including retries'
          : metric.observed !== null
            ? `Partial · ${metric.samples}/${metric.expected} runs reported`
            : 'Not reported',
    }
  })
  const runId = item.primaryRun?.run_id
  const definition = shortDefinition(item.behaviorSha256)
  const scenarioTitle = `${titleCase(item.scenarioId)}${definition ? ` · definition ${definition}` : ''}`
  return (
    <>
      <tr
        data-scenario-row={item.key}
        className="grid grid-cols-2 gap-y-2 border-b border-[var(--color-rule)] py-3 align-top @[1000px]/harness:table-row"
        aria-label={`${titleCase(item.scenarioId)} scenario result`}
      >
        <th
          scope="row"
          className="col-span-2 min-w-0 px-3 py-2 text-left font-normal"
        >
          <button
            type="button"
            aria-expanded={expanded}
            aria-controls={panelId}
            onClick={() => setExpanded(!expanded)}
            className="flex min-h-8 w-full items-start gap-2 rounded-sm text-left focus-visible:outline-2 focus-visible:outline-offset-2"
          >
            <ChevronDown
              className={`mt-0.5 size-4 shrink-0 text-ink-muted ${expanded ? '' : '-rotate-90'}`}
              aria-hidden="true"
            />
            <span className="min-w-0">
              <strong
                className="block break-words text-sm"
                title={scenarioTitle}
              >
                {titleCase(item.scenarioId)}
              </strong>
              <span className="mt-1 block break-all text-label text-ink-muted">
                {item.subjectId} · {item.runCount}{' '}
                {item.runCount === 1 ? 'run' : 'runs'}
              </span>
              {definition ? (
                <span className="mt-1 block font-mono text-label text-ink-muted">
                  definition {definition}
                </span>
              ) : null}
            </span>
          </button>
        </th>
        <td className="min-w-0 px-3 py-2">
          <span className="mb-1 block text-label text-ink-muted @[1000px]/harness:hidden">
            Result
          </span>
          <StatusBadge
            status={item.objective.status}
            label={item.objective.label}
          />
        </td>
        <td className="min-w-0 px-3 py-2">
          <span className="mb-1 block text-label text-ink-muted @[1000px]/harness:hidden">
            Score
          </span>
          <strong className="font-mono">{scoreLabel(scoreMean)}</strong>
          <span className="mt-1 block text-label text-ink-muted">
            {scoreSamples > 0
              ? `Mean · ${scoreSamples}/${planned} planned runs scored`
              : 'Not reported'}
          </span>
        </td>
        {usage
          .filter((metric) =>
            ['Runtime', 'Total tokens', 'Reported cost'].includes(metric.label),
          )
          .map((metric) => (
            <td
              key={metric.label}
              className="min-w-0 px-3 py-2"
              title={metric.detail}
              data-primary-metric={metric.label}
            >
              <span className="mb-1 block text-label text-ink-muted @[1000px]/harness:hidden">
                {metric.label}
              </span>
              <strong className="break-words font-mono">{metric.value}</strong>
              {item.runCount > 1 ||
              metric.detail !== 'Accumulated across runs, including retries' ? (
                <span className="mt-1 block text-label text-ink-muted">
                  {metric.detail}
                </span>
              ) : null}
            </td>
          ))}
        <td className="col-span-2 min-w-0 px-3 py-2">
          <div className="flex flex-wrap items-center gap-2">
            {runId ? (
              <a
                className={buttonClassName({
                  variant: 'secondary',
                  size: 'compact',
                  className: 'no-underline',
                })}
                href={hashForExecution(executionId, null, runId)}
                aria-label={`Evidence record for ${titleCase(item.scenarioId)}`}
                title="Evidence record"
              >
                <Eye size={15} aria-hidden="true" />
              </a>
            ) : !runId ? (
              <span className="text-ink-muted">No retained run</span>
            ) : null}
            <ScenarioChatAction
              compact
              detail={detail}
              scenarioId={item.scenarioId}
              subjectId={item.subjectId}
            />
          </div>
        </td>
      </tr>
      <tr
        id={panelId}
        hidden={!expanded}
        className={expanded ? 'block @[1000px]/harness:table-row' : 'hidden'}
      >
        <td
          colSpan={7}
          className="block min-w-0 bg-panel-raised p-4 @[1000px]/harness:table-cell"
        >
          {item.reason ? (
            <p className="m-0 mb-3 break-words text-sm text-ink">
              {item.reason}
            </p>
          ) : null}
          <dl className="m-0 grid gap-3 @[640px]/harness:grid-cols-2 @[1000px]/harness:grid-cols-3">
            {usage
              .filter(
                (metric) =>
                  !['Runtime', 'Total tokens', 'Reported cost'].includes(
                    metric.label,
                  ),
              )
              .map((metric) => (
                <ResultFact
                  key={metric.label}
                  label={metric.label}
                  value={metric.value}
                  detail={metric.detail}
                />
              ))}
          </dl>
          {item.runs.length > 1 ? (
            <ul
              className="m-0 mt-4 grid list-none gap-3 p-0"
              aria-label="Retained runs"
            >
              {item.runs.map((run) => {
                const assessment = assessmentRuns.find(
                  (entry) => entry.runId === run.run_id,
                )
                return (
                  <li
                    key={run.attempt_id}
                    className="flex flex-wrap items-center gap-3"
                  >
                    <span className="break-all font-mono text-label">
                      {run.run_id}
                    </span>
                    <a
                      className={buttonClassName({
                        variant: 'secondary',
                        size: 'compact',
                      })}
                      href={hashForExecution(executionId, null, run.run_id)}
                    >
                      evidence record
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
                            `${titleCase(item.scenarioId)} · ${run.run_id}`,
                          )
                        }
                      >
                        transcript
                      </button>
                    ) : null}
                  </li>
                )
              })}
            </ul>
          ) : null}
          {!item.available ? (
            <p className="m-0 mt-3 text-sm text-ink-muted">
              The expected report for this scenario is unavailable. Runtime and
              workflow data are intentionally not inferred.
            </p>
          ) : null}
          {item.workflowSteps.length > 0 ? (
            <WorkflowDurationProfile tests={item.workflowSteps} />
          ) : null}
        </td>
      </tr>
    </>
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

function ResultFact({
  label,
  value,
  detail,
}: {
  label: string
  value: string
  detail: string
}) {
  return (
    // Bands separate by fill and gap now, so the hairline seams and the
    // negative margins that closed them are gone (audit DS-14).
    <div
      className="min-w-0 rounded-[6px] bg-panel p-3 @[768px]/harness:p-4"
      data-primary-metric={label}
    >
      <dt className="font-mono text-label font-semibold uppercase tracking-[0.06em] text-ink-muted">
        {label}
      </dt>
      <dd className="m-0 mt-2 min-w-0">
        <strong className="block truncate font-mono text-sm font-semibold tabular-nums text-ink">
          {value}
        </strong>
        <span className="mt-1 block text-label leading-4 text-ink-muted">
          {detail}
        </span>
      </dd>
    </div>
  )
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
    return { status: 'incomplete', label: titleCase(status) }
  }
  return { status: 'unavailable', label: titleCase(status) }
}
