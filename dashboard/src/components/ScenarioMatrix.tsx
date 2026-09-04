import { ChevronDown } from 'lucide-react'
import { type ReactNode, useEffect, useMemo, useState } from 'react'
import { AssessmentWorkspace } from '@/components/AssessmentWorkspace'
import { ScenarioChatAction } from '@/components/ScenarioChatAction'
import { SemanticTestFlow } from '@/components/SemanticTestFlow'
import {
  buttonClassName,
  type OperationalStatus,
  Panel,
  StatusBadge,
} from '@/design-system'
import { hashForExecution } from '@/hooks/use-hash-route'
import type { AssessmentRunView } from '@/lib/assessment-view'
import type {
  CompletionState,
  DashboardExecutionDetail,
  DashboardRunProjection,
  DashboardScenarioAggregate,
  EvaluatorAvailability,
  SemanticTestReport,
  TechnicalState,
} from '@/lib/dashboard-data-source'
import { formatPercent, titleCase } from '@/lib/execution-view'
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
}: {
  detail: DashboardExecutionDetail
  onTranscript: (run: AssessmentRunView, title: string) => void
}) {
  const model = useMemo(() => buildScenarioMatrix(detail), [detail])
  // Audit SM-07: the Structure column only carries information when at
  // least one scenario persisted a workflow; otherwise it repeats "Standard".
  const showStructure = model.items.some(
    (item) => item.workflowSteps.length > 0,
  )
  const preferredItem =
    model.items.find((item) => item.workflowSteps.length > 0) ??
    model.items.find((item) => item.objective.status !== 'passed') ??
    model.items[0]
  const [expandedKey, setExpandedKey] = useState<string | null>(
    preferredItem?.key ?? null,
  )

  useEffect(() => {
    if (expandedKey && !model.items.some((item) => item.key === expandedKey)) {
      setExpandedKey(preferredItem?.key ?? null)
    }
  }, [expandedKey, model.items, preferredItem?.key])

  if (model.items.length === 0) {
    return (
      <div className="rounded-[var(--ds-radius-sm)] border border-dashed border-[var(--color-edge)] bg-panel-raised p-5 text-sm text-ink-muted">
        No scenario reports were retained for this execution.
      </div>
    )
  }

  return (
    <div className="grid gap-4">
      <ResultContractStrip contracts={model.contracts} />
      <ScenarioSummary summary={model.summary} />
      <div className="overflow-hidden rounded-[var(--ds-radius-md)] border border-[var(--color-edge)] bg-panel">
        <div
          className={`hidden gap-4 border-b border-[var(--color-rule)] bg-panel-raised px-5 py-3 font-mono text-label font-semibold uppercase tracking-[0.06em] text-ink-muted lg:grid ${matrixColumns(showStructure)}`}
          aria-hidden="true"
        >
          <span>Scenario</span>
          <span>Objective result</span>
          <span>Advisory</span>
          <span>Runtime</span>
          {showStructure ? <span>Structure</span> : null}
        </div>
        <ol className="m-0 grid list-none p-0">
          {model.items.map((item) => {
            const expanded = expandedKey === item.key
            return (
              <ScenarioRow
                key={item.key}
                item={item}
                detail={detail}
                executionId={detail.id}
                expanded={expanded}
                showStructure={showStructure}
                onToggle={() => setExpandedKey(expanded ? null : item.key)}
                onTranscript={onTranscript}
              />
            )
          })}
        </ol>
      </div>
    </div>
  )
}

function ResultContractStrip({
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
        Results v3 contract unavailable. Objective outcome and report
        completeness are not inferred from execution status.
      </Panel>
    )
  }
  return (
    <Panel
      as="section"
      tone="raised"
      padding="compact"
      className="grid gap-3"
      aria-label="Results contract"
    >
      {contracts.map((contract) => (
        <div
          key={contract.key}
          className="grid gap-3 sm:grid-cols-2 lg:grid-cols-5"
          data-results-contract={contract.valid ? 'valid' : 'invalid'}
        >
          <ContractFact
            label="report state"
            value={contract.reportState ?? 'unavailable'}
          />
          <ContractFact
            label="objective outcome"
            value={contract.objectiveOutcome ?? 'unavailable'}
          />
          <ContractFact
            label="schema"
            value={
              contract.schemaVersion === null
                ? 'unavailable'
                : `Results v${contract.schemaVersion}`
            }
          />
          <ContractFact
            label="result contract"
            value={shortHash(contract.resultContractSha256)}
          />
          <ContractFact
            label="scoring profile"
            value={shortHash(contract.scoringProfileSha256)}
          />
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
        {titleCase(value)}
      </strong>
    </span>
  )
}

function shortHash(value: string | null) {
  if (!value) return 'unavailable'
  return value.length > 19 ? `${value.slice(0, 19)}…` : value
}

const MATRIX_COLUMNS =
  'lg:grid-cols-[minmax(13rem,1.2fr)_minmax(9rem,0.75fr)_minmax(10rem,1fr)_minmax(7rem,0.55fr)]'
const MATRIX_COLUMNS_WITH_STRUCTURE =
  'lg:grid-cols-[minmax(13rem,1.2fr)_minmax(9rem,0.75fr)_minmax(10rem,1fr)_minmax(7rem,0.55fr)_minmax(8rem,0.7fr)]'

function matrixColumns(showStructure: boolean) {
  return showStructure ? MATRIX_COLUMNS_WITH_STRUCTURE : MATRIX_COLUMNS
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
    { status: 'hard_gate', count: summary.hardGate, label: 'hard gate' },
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

function ScenarioRow({
  item,
  detail,
  executionId,
  expanded,
  showStructure,
  onToggle,
  onTranscript,
}: {
  item: ScenarioMatrixItem
  executionId: string
  detail: DashboardExecutionDetail
  expanded: boolean
  showStructure: boolean
  onToggle: () => void
  onTranscript: (run: AssessmentRunView, title: string) => void
}) {
  const panelId = `${safeId(item.key)}-scenario-panel`
  const scenarioTitle = `${titleCase(item.scenarioId)}${
    item.scenarioVersion != null ? ` v${item.scenarioVersion}` : ''
  }`
  const structure = item.workflowSteps.length
    ? `Workflow · ${item.workflowSteps.length} ${item.workflowSteps.length === 1 ? 'step' : 'steps'}`
    : item.available
      ? 'Standard'
      : 'No report'

  return (
    <li className="border-t border-[var(--color-rule)] first:border-t-0">
      <button
        data-scenario-row={item.key}
        className={`group grid min-h-16 w-full min-w-0 gap-x-4 gap-y-3 border-0 px-4 py-4 text-left transition-colors motion-reduce:transition-none lg:items-center lg:px-5 ${matrixColumns(showStructure)} ${
          expanded ? 'bg-panel-raised' : 'bg-panel hover:bg-panel-raised'
        }`}
        type="button"
        aria-expanded={expanded}
        aria-controls={panelId}
        onClick={onToggle}
      >
        <span className="flex min-w-0 items-start gap-3">
          <ChevronDown
            className={`mt-0.5 size-4 shrink-0 text-ink-muted transition-transform duration-[var(--ds-motion-duration-fast)] motion-reduce:transition-none ${expanded ? 'rotate-0' : '-rotate-90'}`}
            aria-hidden="true"
          />
          <span className="min-w-0">
            <h3
              className="m-0 block truncate text-sm font-semibold text-ink"
              title={scenarioTitle}
            >
              {scenarioTitle}
            </h3>
            <span
              className="mt-1 block truncate font-mono text-label text-ink-muted"
              title={item.reason ?? item.subjectId}
            >
              {item.reason ??
                `${item.subjectId} · ${item.runCount} ${item.runCount === 1 ? 'run' : 'runs'}`}
            </span>
          </span>
        </span>
        <MatrixCell label="Objective result">
          <StatusBadge
            status={item.objective.status}
            label={item.objective.label}
          />
        </MatrixCell>
        <MatrixCell label="Advisory">
          <StatusBadge
            status={item.advisory.status}
            label={item.advisory.label}
          />
        </MatrixCell>
        <MatrixCell label="Runtime">
          <strong className="font-mono text-xs font-semibold tabular-nums text-ink">
            {formatScenarioDuration(item.durationMs)}
          </strong>
          {item.durationKind === 'average' && item.runCount > 1 ? (
            <span className="ml-1 font-mono text-label text-ink-muted">
              avg
            </span>
          ) : null}
        </MatrixCell>
        {showStructure ? (
          <MatrixCell label="Structure">
            <span className="text-xs font-medium text-[var(--color-ink-faint)]">
              {structure}
            </span>
          </MatrixCell>
        ) : null}
      </button>
      {expanded ? (
        <ScenarioExpansion
          id={panelId}
          detail={detailForScenario(detail, item)}
          item={item}
          executionId={executionId}
          onTranscript={onTranscript}
        />
      ) : null}
    </li>
  )
}

function MatrixCell({
  label,
  children,
}: {
  label: string
  children: ReactNode
}) {
  return (
    <span className="grid min-w-0 grid-cols-[7rem_minmax(0,1fr)] items-center gap-3 lg:block">
      <span className="font-mono text-label font-semibold uppercase tracking-[0.06em] text-ink-muted lg:hidden">
        {label}
      </span>
      <span className="min-w-0">{children}</span>
    </span>
  )
}

function ScenarioExpansion({
  id,
  detail,
  item,
  executionId,
  onTranscript,
}: {
  id: string
  detail: DashboardExecutionDetail
  item: ScenarioMatrixItem
  executionId: string
  onTranscript: (run: AssessmentRunView, title: string) => void
}) {
  const runId =
    (item.primaryRun as unknown as { run_id?: string } | null)?.run_id ?? null
  const evidenceHref = runId ? hashForExecution(executionId, null, runId) : null
  return (
    <section
      id={id}
      className="border-t border-[var(--color-edge)] bg-panel-raised p-3 sm:p-4 md:p-5"
      aria-label={`${titleCase(item.scenarioId)} scenario result`}
    >
      <div className="overflow-hidden rounded-[var(--ds-radius-sm)] border border-[var(--color-rule)] bg-panel">
        <div className="flex min-h-14 flex-col gap-3 border-b border-[var(--color-rule)] bg-panel px-4 py-3 sm:flex-row sm:items-center sm:justify-between md:px-5">
          <div className="min-w-0">
            <strong className="block truncate text-sm text-ink">
              {titleCase(item.scenarioId)} run evidence
            </strong>
            <span className="mt-1 block font-mono text-label text-ink-muted">
              {item.runCount} {item.runCount === 1 ? 'run' : 'runs'} retained ·{' '}
              {item.subjectId}
            </span>
          </div>
          <div className="flex flex-wrap items-center gap-2 max-[560px]:*:w-full">
            {evidenceHref ? (
              <a
                className={buttonClassName({
                  variant: 'secondary',
                  size: 'compact',
                  className: 'no-underline',
                })}
                href={evidenceHref}
              >
                evidence record
              </a>
            ) : null}
            <ScenarioChatAction
              detail={detail}
              scenarioId={item.scenarioId}
              subjectId={item.subjectId}
            />
          </div>
        </div>
        {item.reason ? (
          <p className="m-0 bg-[var(--surface-fill)] px-4 py-3 text-sm leading-6 text-ink md:px-5">
            <span className="ds-label mr-2">why it failed</span>
            {item.reason}
          </p>
        ) : null}
        <ScenarioResultBand item={item} />
        <ScenarioReliabilityBand aggregate={item.aggregate} />
        <RunOutcomeLedger runs={item.runs} />
        {!item.available ? (
          <div className="border-t border-[var(--color-rule)] px-4 py-6 text-sm leading-6 text-ink-muted md:px-5">
            The expected report for this scenario is unavailable. Runtime,
            workflow, and advisory data are intentionally not inferred.
          </div>
        ) : item.workflowSteps.length > 0 ? (
          <WorkflowDurationProfile tests={item.workflowSteps} />
        ) : null}
        {item.available ? (
          <details
            className="group border-t border-[var(--color-rule)]"
            open={item.objective.status !== 'passed'}
          >
            <summary className="flex min-h-12 cursor-pointer list-none items-center gap-3 px-4 py-3 text-sm font-semibold text-ink marker:hidden md:px-5">
              <ChevronDown
                className="size-4 shrink-0 -rotate-90 text-ink-muted transition-transform duration-[var(--ds-motion-duration-fast)] group-open:rotate-0 motion-reduce:transition-none"
                aria-hidden="true"
              />
              <span>Inspect scenario evidence</span>
              <span className="ml-auto font-mono text-label font-normal text-ink-muted">
                assessments, gates, artifacts, and provenance
              </span>
            </summary>
            <div className="grid gap-6 border-t border-[var(--color-rule)] p-4 md:p-5">
              <AssessmentWorkspace
                detail={detail}
                onTranscript={onTranscript}
              />
              {item.workflowSteps.length > 0 ? (
                <SemanticTestFlow detail={detail} />
              ) : null}
            </div>
          </details>
        ) : null}
      </div>
    </section>
  )
}

function ScenarioReliabilityBand({
  aggregate,
}: {
  aggregate: DashboardScenarioAggregate | null
}) {
  if (!aggregate) {
    return (
      <p
        className="m-0 bg-panel-raised px-4 py-3 text-xs text-ink-muted md:px-5"
        data-scenario-aggregate="unavailable"
      >
        Required Results v3 aggregate is unavailable; no completion or
        reliability metric was inferred.
      </p>
    )
  }
  const counts = [
    ['planned', aggregate.planned_runs],
    ['observed', aggregate.observed_runs],
    ['deferred', aggregate.deferred_runs],
    ['completed', aggregate.completed_runs],
    ['task incomplete', aggregate.task_incomplete_runs],
    ['undetermined', aggregate.undetermined_runs],
    ['technical valid', aggregate.technical_valid_runs],
    ['technical invalid', aggregate.technical_invalid_runs],
  ] as const
  const rates = [
    ['execution reliability', aggregate.execution_reliability],
    ['completion evidence coverage', aggregate.completion_evidence_coverage],
    ['completion rate', aggregate.completion_rate],
    ['objective score coverage', aggregate.objective_score_coverage],
    ['quality coverage', aggregate.quality_coverage],
  ] as const
  const tokenMetrics = [
    ['subject tokens', aggregate.total_tokens_consumed],
    ['judge tokens', aggregate.judge_tokens_consumed],
    ['completed p50 tokens', aggregate.tokens_completed_p50],
    ['failed attempt tokens', aggregate.failed_attempt_tokens],
    ['tokens per completion', aggregate.tokens_per_completion],
  ] as const
  return (
    <section
      className="bg-panel-raised px-4 py-4 md:px-5"
      aria-label="Scenario reliability and completion"
      data-scenario-aggregate="available"
    >
      <h4 className="m-0 text-xs font-semibold text-ink">
        Completion and evidence yield
      </h4>
      <dl className="m-0 mt-3 grid grid-cols-2 gap-x-5 gap-y-3 sm:grid-cols-4 xl:grid-cols-8">
        {counts.map(([label, value]) => (
          <AggregateFact key={label} label={label} value={formatCount(value)} />
        ))}
      </dl>
      <dl className="m-0 mt-4 grid grid-cols-2 gap-x-5 gap-y-3 sm:grid-cols-3 xl:grid-cols-6">
        {rates.map(([label, value]) => (
          <AggregateFact
            key={label}
            label={label}
            value={formatPercentOrDash(value)}
          />
        ))}
        <AggregateFact
          label="quality score completed"
          value={
            aggregate.quality_score_completed == null
              ? '—'
              : `${formatDecimal(aggregate.quality_score_completed)}/100`
          }
          detail={`${aggregate.quality_scored_completed_runs}/${aggregate.completed_runs} completed runs scored`}
        />
      </dl>
      <dl className="m-0 mt-4 grid grid-cols-2 gap-x-5 gap-y-3 sm:grid-cols-3 xl:grid-cols-5">
        {tokenMetrics.map(([label, value]) => (
          <AggregateFact
            key={label}
            label={label}
            value={value == null ? '—' : formatDecimal(value)}
          />
        ))}
      </dl>
    </section>
  )
}

function AggregateFact({
  label,
  value,
  detail,
}: {
  label: string
  value: string
  detail?: string
}) {
  return (
    <div className="min-w-0">
      <dt className="ds-label">{label}</dt>
      <dd className="m-0 mt-1 font-mono text-xs font-semibold tabular-nums text-ink">
        {value}
      </dd>
      {detail ? (
        <dd className="m-0 mt-1 text-label text-ink-muted">{detail}</dd>
      ) : null}
    </div>
  )
}

type PhysicalAttempt = {
  key: string
  runId: string
  attemptNumber: number | null
  completion?: CompletionState
  technical?: TechnicalState
  evaluators?: DashboardRunProjection['evaluators']
  objectiveScore?: number | null
  qualityScoreCompleted?: number | null
}

function physicalAttempts(runs: DashboardRunProjection[]): PhysicalAttempt[] {
  return runs.flatMap((run) => [
    ...(run.retry_attempts ?? []).map((attempt) => ({
      key: `${attempt.attempt_id}:retry`,
      runId: attempt.run_id,
      attemptNumber: attempt.attempt_number,
      completion: attempt.completion,
      technical: attempt.technical,
      evaluators: attempt.evaluators,
      objectiveScore: attempt.objective_score,
      qualityScoreCompleted: attempt.quality_score_completed,
    })),
    {
      key: `${run.attempt_id}:terminal`,
      runId: run.run_id,
      attemptNumber: run.attempt_number ?? null,
      completion: run.completion,
      technical: run.technical,
      evaluators: run.evaluators,
      objectiveScore: run.objective_score,
      qualityScoreCompleted: run.quality_score_completed,
    },
  ])
}

function RunOutcomeLedger({ runs }: { runs: DashboardRunProjection[] }) {
  const attempts = physicalAttempts(runs)
  if (attempts.length === 0) return null
  return (
    <details className="group bg-panel-raised">
      <summary className="flex min-h-11 cursor-pointer list-none items-center gap-3 px-4 py-3 text-xs font-semibold text-ink marker:hidden md:px-5">
        <ChevronDown
          className="size-4 shrink-0 -rotate-90 text-ink-muted transition-transform group-open:rotate-0"
          aria-hidden="true"
        />
        Physical attempt outcomes
        <span className="ml-auto font-mono text-label font-normal text-ink-muted">
          {attempts.length} {attempts.length === 1 ? 'attempt' : 'attempts'}
        </span>
      </summary>
      <div className="overflow-x-auto bg-panel">
        <table className="w-full min-w-[860px] border-collapse text-left text-xs">
          <thead className="bg-panel-raised font-mono text-label uppercase tracking-[0.06em] text-ink-muted">
            <tr>
              <th className="px-4 py-2 font-semibold">run / attempt</th>
              <th className="px-4 py-2 font-semibold">completion</th>
              <th className="px-4 py-2 font-semibold">technical</th>
              <th className="px-4 py-2 font-semibold">completion evaluator</th>
              <th className="px-4 py-2 font-semibold">quality evaluator</th>
              <th className="px-4 py-2 font-semibold">final advisory</th>
              <th className="px-4 py-2 font-semibold">objective</th>
              <th className="px-4 py-2 font-semibold">quality</th>
            </tr>
          </thead>
          <tbody>
            {attempts.map((attempt) => (
              <tr key={attempt.key} className="even:bg-panel-raised">
                <td className="px-4 py-3 font-mono text-ink">
                  {attempt.runId}
                  <span className="ml-2 text-ink-muted">
                    #{attempt.attemptNumber ?? '—'}
                  </span>
                </td>
                <td className="px-4 py-3">{stateLabel(attempt.completion)}</td>
                <td className="px-4 py-3">{stateLabel(attempt.technical)}</td>
                <td className="px-4 py-3">
                  {evaluatorLabel(attempt.evaluators?.completion)}
                </td>
                <td className="px-4 py-3">
                  {evaluatorLabel(attempt.evaluators?.quality)}
                </td>
                <td className="px-4 py-3">
                  {evaluatorLabel(attempt.evaluators?.final_advisory)}
                </td>
                <td className="px-4 py-3 font-mono tabular-nums">
                  {scoreLabel(attempt.objectiveScore)}
                </td>
                <td className="px-4 py-3 font-mono tabular-nums">
                  {scoreLabel(attempt.qualityScoreCompleted)}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </details>
  )
}

function stateLabel(value: CompletionState | TechnicalState | undefined) {
  return value ? titleCase(value) : 'Contract unavailable'
}

function evaluatorLabel(value: EvaluatorAvailability | undefined) {
  return value ? titleCase(value) : 'Contract unavailable'
}

function scoreLabel(value: number | null | undefined) {
  return typeof value === 'number' && Number.isFinite(value)
    ? `${formatDecimal(value)}/100`
    : '—'
}

function formatPercentOrDash(value: number | null) {
  return value == null || !Number.isFinite(value) ? '—' : formatPercent(value)
}

function formatDecimal(value: number) {
  return value.toLocaleString('en-US', { maximumFractionDigits: 1 })
}

function formatCount(value: number) {
  return value.toLocaleString('en-US', { maximumFractionDigits: 0 })
}

/** Audit ED-24: eleven facts in one seven-column grid wrapped into a ragged
 *  second row with a visible empty cell. Three bands, each sized to its own
 *  count, divide evenly and say what kind of number each one is. */
function ScenarioResultBand({ item }: { item: ScenarioMatrixItem }) {
  const scoring = item.primaryMetrics.filter(
    (metric) => metric.band === 'scoring',
  )
  const execution = item.primaryMetrics.filter(
    (metric) => metric.band === 'execution',
  )
  return (
    <div className="grid gap-4" data-scenario-primary-metrics>
      <MetricBandGroup label="outcome" columns="sm:grid-cols-2">
        <ResultFact
          label="Objective"
          value={item.objective.label}
          status={item.objective.status}
          detail="Authoritative system result"
        />
        <ResultFact
          label="Advisory"
          value={item.advisory.label}
          status={item.advisory.status}
          detail="Does not override objective result"
        />
      </MetricBandGroup>
      {scoring.length > 0 ? (
        <MetricBandGroup
          label="scoring"
          columns="sm:grid-cols-2 lg:grid-cols-4"
        >
          {scoring.map((metric) => (
            <ResultFact
              key={metric.label}
              label={metric.label}
              value={metric.value}
              detail={metric.detail}
            />
          ))}
        </MetricBandGroup>
      ) : null}
      {execution.length > 0 ? (
        <MetricBandGroup
          label="execution"
          columns="sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-5"
        >
          {execution.map((metric) => (
            <ResultFact
              key={metric.label}
              label={metric.label}
              value={metric.value}
              detail={metric.detail}
            />
          ))}
        </MetricBandGroup>
      ) : null}
    </div>
  )
}

function MetricBandGroup({
  label,
  columns,
  children,
}: {
  label: string
  columns: string
  children: ReactNode
}) {
  return (
    <div className="grid gap-2">
      <span className="ds-label">{label}</span>
      <dl className={`m-0 grid gap-2 ${columns}`}>{children}</dl>
    </div>
  )
}

function ResultFact({
  label,
  value,
  detail,
  status,
}: {
  label: string
  value: string
  detail: string
  status?: OperationalStatus
}) {
  return (
    // Bands separate by fill and gap now, so the hairline seams and the
    // negative margins that closed them are gone (audit DS-14).
    <div
      className="min-w-0 rounded-[6px] bg-panel p-3 md:p-4"
      data-primary-metric={label}
    >
      <dt className="font-mono text-label font-semibold uppercase tracking-[0.06em] text-ink-muted">
        {label}
      </dt>
      <dd className="m-0 mt-2 min-w-0">
        {status ? (
          <StatusBadge status={status} label={value} />
        ) : (
          <strong className="block truncate font-mono text-sm font-semibold tabular-nums text-ink">
            {value}
          </strong>
        )}
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
      aria-labelledby="workflow-duration-heading"
    >
      <header className="grid gap-2 border-b border-[var(--color-rule)] px-4 py-4 md:grid-cols-[minmax(0,1fr)_auto] md:items-end md:px-5">
        <div>
          <h3
            id="workflow-duration-heading"
            className="m-0 text-sm font-semibold text-ink"
          >
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
      <div className="hidden grid-cols-[minmax(12rem,0.9fr)_7rem_minmax(14rem,1.4fr)_minmax(12rem,1fr)] gap-4 border-b border-[var(--color-rule)] bg-panel-raised px-5 py-2.5 font-mono text-label font-semibold uppercase tracking-[0.06em] text-ink-muted lg:grid">
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
      : status.status === 'hard_gate' || status.status === 'failed'
        ? 'bg-danger'
        : 'bg-warning'

  return (
    <li
      className="grid gap-3 border-t border-[var(--color-rule)] px-4 py-4 first:border-t-0 lg:grid-cols-[minmax(12rem,0.9fr)_7rem_minmax(14rem,1.4fr)_minmax(12rem,1fr)] lg:items-center lg:gap-4 lg:px-5"
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
      <div className="grid grid-cols-[7rem_minmax(0,1fr)] items-center gap-3 lg:block">
        <span className="font-mono text-label font-semibold uppercase tracking-[0.06em] text-ink-muted lg:hidden">
          Duration
        </span>
        <strong className="font-mono text-xs font-semibold tabular-nums text-ink">
          {formatScenarioDuration(test.duration_ms)}
        </strong>
      </div>
      <div className="grid grid-cols-[7rem_minmax(0,1fr)] items-center gap-3 lg:block">
        <span className="font-mono text-label font-semibold uppercase tracking-[0.06em] text-ink-muted lg:hidden">
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
      <div className="grid grid-cols-[7rem_minmax(0,1fr)] items-start gap-3 lg:block">
        <span className="font-mono text-label font-semibold uppercase tracking-[0.06em] text-ink-muted lg:hidden">
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
    return { status: 'hard_gate', label: 'Hard gate failed' }
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

function safeId(value: string) {
  return value.replace(/[^a-zA-Z0-9_-]/g, '-')
}
