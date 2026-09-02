import type { ReactNode } from 'react'
import { ScenarioChatAction } from '@/components/ScenarioChatAction'
import { type OperationalStatus, StatusBadge } from '@/design-system/primitives'
import type {
  DashboardExecutionDetail,
  ScenarioFlowEvidence,
  SemanticTestReport,
} from '@/lib/dashboard-data-source'
import {
  aggregateWorkflowMetrics,
  type WorkflowMetricsSummary,
  workflowMetricEntries,
  workflowStepUsage,
} from '@/lib/workflow-metrics'

type ObservedFlow = {
  key: string
  subjectId: string
  scenarioId: string
  runId: string
  tests: SemanticTestReport[]
  flow: ScenarioFlowEvidence | null
}

export function SemanticTestFlow({
  detail,
}: {
  detail: DashboardExecutionDetail
}) {
  const flows = observedFlows(detail)
  if (flows.length === 0) return null

  return (
    <section
      className="mt-6 grid gap-5"
      aria-labelledby="semantic-tests-heading"
    >
      <div>
        <h3
          id="semantic-tests-heading"
          className="m-0 text-xl font-semibold tracking-[-0.025em]"
        >
          Execution flow
        </h3>
        <p className="mt-2 max-w-3xl text-sm leading-6 text-ink-muted">
          Read the workflow in execution order. Outcome and hard-gate evidence
          stay visible; raw counters, assets and immutable identifiers are
          available on demand.
        </p>
      </div>
      {flows.map((flow) => (
        <article
          key={flow.key}
          className="overflow-hidden rounded-[var(--ds-radius-md)] border border-line-strong bg-panel-faint"
        >
          <header className="grid gap-4 border-b border-line bg-panel px-4 py-4 md:grid-cols-[minmax(0,1fr)_auto] md:items-center md:px-5">
            <div className="min-w-0">
              <strong className="block break-words text-base font-semibold text-ink">
                {humanize(flow.scenarioId)}
              </strong>
              <p className="m-0 mt-1 text-xs leading-5 text-ink-muted">
                {flow.tests.length} steps executed in dependency order
              </p>
            </div>
            <div className="flex flex-wrap items-center gap-x-4 gap-y-2 md:justify-end">
              <StatusBadge
                status={cleanupStatus(flow.flow?.cleanup.status).status}
                label={cleanupStatus(flow.flow?.cleanup.status).label}
              />
              <code
                className="break-all text-[0.6875rem] text-ink-muted"
                title={flow.runId}
              >
                run {shortHash(flow.runId)}
              </code>
              <ScenarioChatAction
                compact
                detail={detail}
                scenarioId={flow.scenarioId}
                subjectId={flow.subjectId}
                runId={flow.runId}
              />
            </div>
          </header>

          <WorkflowMetricsOverview
            metrics={aggregateWorkflowMetrics(flow.tests)}
          />

          <div className="border-b border-line px-4 py-3 md:px-5">
            <h4 className="m-0 text-sm font-semibold text-ink">
              Steps and outcomes
            </h4>
            <p className="m-0 mt-1 text-xs leading-5 text-ink-muted">
              Required work, dependencies and decision evidence are separated
              from diagnostic artifacts.
            </p>
          </div>

          <ol className="m-0 grid list-none gap-3 p-3 sm:p-4 md:p-5">
            {flow.tests.map((test, index) => (
              <SemanticTestCard
                key={test.node_id}
                test={test}
                number={index + 1}
              />
            ))}
          </ol>
        </article>
      ))}
    </section>
  )
}

function WorkflowMetricsOverview({
  metrics,
}: {
  metrics: WorkflowMetricsSummary
}) {
  const attentionSteps =
    metrics.failedSteps + metrics.hardGateFailedSteps + metrics.cancelledSteps
  const numericMetrics = workflowMetricEntries(metrics)
  return (
    <section
      className="border-b border-line bg-panel-subtle px-4 py-4 md:px-5"
      aria-label="Workflow metrics"
    >
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h4 className="m-0 text-sm font-semibold text-ink">
            Workflow metrics
          </h4>
          <p className="m-0 mt-1 text-xs leading-5 text-ink-muted">
            Consolidated across all persisted semantic steps.
          </p>
        </div>
        <StatusBadge
          status={workflowStatus(metrics)}
          label={
            attentionSteps
              ? `${attentionSteps} steps need attention`
              : `${metrics.succeededSteps}/${metrics.stepCount} steps succeeded`
          }
        />
      </div>
      <div className="mt-4 grid grid-flow-dense gap-px overflow-hidden rounded-[var(--ds-radius-sm)] border border-line bg-line sm:grid-cols-2 lg:grid-cols-5">
        <WorkflowMetric
          label="Steps"
          value={`${metrics.succeededSteps}/${metrics.stepCount}`}
          caption={
            attentionSteps
              ? `${attentionSteps} need attention`
              : 'all succeeded'
          }
        />
        <WorkflowMetric
          label="Runtime"
          value={formatDuration(metrics.durationMs)}
          caption="total step duration"
        />
        <WorkflowMetric
          label="Workflow tokens"
          value={formatReportedCount(
            metrics.totalTokens,
            metrics.tokenMetricSteps,
          )}
          caption={metricCoverage(metrics.tokenMetricSteps, metrics.stepCount)}
        />
        <WorkflowMetric
          label="Function calls"
          value={formatReportedCount(
            metrics.functionCalls,
            metrics.functionCallMetricSteps,
          )}
          caption={metricCoverage(
            metrics.functionCallMetricSteps,
            metrics.stepCount,
          )}
        />
        <WorkflowMetric
          label="Hard gates"
          value={
            metrics.hardGateCount
              ? `${metrics.passedHardGateCount}/${metrics.hardGateCount}`
              : '—'
          }
          caption={
            metrics.hardGateCount === 0
              ? 'no hard gates reported'
              : metrics.passedHardGateCount === metrics.hardGateCount
                ? 'all objective checks passed'
                : `${metrics.hardGateCount - metrics.passedHardGateCount} failed`
          }
          tone={
            metrics.hardGateCount === 0
              ? 'neutral'
              : metrics.passedHardGateCount === metrics.hardGateCount
                ? 'positive'
                : 'negative'
          }
        />
      </div>
      {numericMetrics.length > 0 && (
        <details className="group mt-3 rounded-md border border-line bg-panel/40">
          <summary className="flex min-h-11 cursor-pointer list-none items-center justify-between gap-3 px-3 py-2 text-xs font-semibold text-ink marker:hidden">
            <span>Additional runtime counters</span>
            <span className="font-mono text-[0.6875rem] font-normal text-ink-muted">
              {numericMetrics.length} metrics
            </span>
          </summary>
          <dl className="grid gap-x-5 gap-y-2 border-t border-line px-3 py-3 text-xs sm:grid-cols-2 lg:grid-cols-3">
            {numericMetrics.map(([key, value]) => (
              <div
                key={key}
                className="flex min-w-0 items-baseline justify-between gap-3"
              >
                <dt className="truncate text-ink-muted" title={key}>
                  {humanize(key)}
                </dt>
                <dd className="m-0 shrink-0 font-mono text-ink-soft">
                  {formatNumericMetric(key, value)}
                </dd>
              </div>
            ))}
          </dl>
        </details>
      )}
    </section>
  )
}

function WorkflowMetric({
  label,
  value,
  caption,
  tone = 'neutral',
}: {
  label: string
  value: string
  caption: string
  tone?: 'neutral' | 'positive' | 'negative'
}) {
  const valueTone =
    tone === 'positive'
      ? 'text-success'
      : tone === 'negative'
        ? 'text-danger'
        : 'text-ink'
  return (
    <div className="min-w-0 bg-panel px-3 py-3">
      <div className="text-[0.6875rem] font-semibold uppercase tracking-[0.08em] text-ink-muted">
        {label}
      </div>
      <strong
        className={`mt-2 block font-mono text-xl font-semibold tracking-[-0.04em] ${valueTone}`}
      >
        {value}
      </strong>
      <span className="mt-1 block text-[0.6875rem] leading-4 text-ink-muted">
        {caption}
      </span>
    </div>
  )
}

function SemanticTestCard({
  test,
  number,
}: {
  test: SemanticTestReport
  number: number
}) {
  const assets = test.assets ?? []
  const gates = test.hard_gates ?? []
  const evaluations = test.evaluations ?? []
  const failures = test.failures ?? []
  const failedGates = gates.filter((gate) => !gate.passed)
  const outcome = summarizeTestOutcome(test)
  const status = semanticTestStatus(test.status)
  const facts = primaryStepFacts(test)
  const needsAttention =
    failures.length > 0 || failedGates.length > 0 || test.status !== 'succeeded'

  return (
    <li className="min-w-0 overflow-hidden rounded-[var(--ds-radius-sm)] border border-line bg-panel shadow-sm">
      <header className="grid gap-4 px-4 py-4 sm:px-5 lg:grid-cols-[minmax(0,1fr)_auto] lg:items-center">
        <div className="flex min-w-0 items-start gap-3">
          <span
            className="grid size-8 shrink-0 place-items-center rounded-md border border-line bg-panel-subtle font-mono text-[0.6875rem] font-semibold text-ink-muted"
            title={`Step ${number}`}
          >
            {String(number).padStart(2, '0')}
          </span>
          <div className="min-w-0">
            <h5 className="m-0 break-words text-base font-semibold tracking-[-0.02em] text-ink">
              {humanize(test.node_id)}
            </h5>
            <p className="m-0 mt-1 text-xs leading-5 text-ink-muted">
              {test.required ? 'Required step' : 'Optional step'}
              {' · '}
              {test.dependencies.length
                ? `Runs after ${test.dependencies.map(humanize).join(', ')}`
                : 'Starts the workflow'}
            </p>
          </div>
        </div>
        <StatusBadge status={status.status} label={status.label} />
      </header>

      <div className="grid border-t border-line lg:grid-cols-[minmax(0,1.35fr)_minmax(20rem,0.65fr)]">
        <section
          className="min-w-0 px-4 py-4 sm:px-5"
          aria-label="Step outcome"
        >
          <div className="text-[0.6875rem] font-semibold uppercase tracking-[0.08em] text-ink-muted">
            Outcome
          </div>
          <strong className={`mt-2 block text-base ${outcome.tone}`}>
            {outcome.title}
          </strong>
          <p className="m-0 mt-1 max-w-3xl text-xs leading-5 text-ink-muted">
            {outcome.detail}
          </p>
        </section>

        <dl className="grid grid-cols-[repeat(auto-fit,minmax(7.5rem,1fr))] border-t border-line bg-panel-subtle text-xs lg:border-t-0 lg:border-l">
          {facts.map((fact) => (
            <Fact key={fact.label} label={fact.label} value={fact.value} />
          ))}
        </dl>
      </div>

      {test.skip_reason && (
        <p className="m-0 border-t border-warning/30 bg-warning/5 px-4 py-3 text-xs text-ink-soft sm:px-5">
          <strong>Skip reason:</strong> {test.skip_reason}
        </p>
      )}

      <details
        className="group border-t border-line bg-panel-faint"
        open={needsAttention || undefined}
      >
        <summary className="grid min-h-12 cursor-pointer list-none gap-1 px-4 py-3 marker:hidden sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center sm:px-5">
          <span className="text-xs font-semibold text-ink">
            Decision evidence
          </span>
          <span className="font-mono text-[0.6875rem] text-ink-muted">
            {gates.length} gates · {evaluations.length} evaluations ·{' '}
            {failures.length} failures
          </span>
        </summary>
        <div className="grid gap-5 border-t border-line px-4 py-4 sm:px-5 lg:grid-cols-2">
          <EvidenceGroup title="Hard gates" empty="No hard gates reported.">
            {gates.map((gate) => (
              <li
                key={gate.id}
                className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 border-t border-line/70 pt-2 first:border-t-0 first:pt-0"
              >
                <span
                  className={`mt-1 size-2 rounded-full ${gate.passed ? 'bg-success' : 'bg-danger'}`}
                  aria-hidden="true"
                />
                <div className="min-w-0">
                  <strong
                    className={`block break-words text-xs ${gate.passed ? 'text-ink' : 'text-danger'}`}
                  >
                    {humanize(gate.id)}
                  </strong>
                  <span className="mt-0.5 block text-xs leading-5 text-ink-muted">
                    {gate.reason}
                  </span>
                </div>
              </li>
            ))}
          </EvidenceGroup>
          <div className="grid content-start gap-5">
            <EvidenceGroup title="Evaluations" empty="No evaluations reported.">
              {evaluations.map((evaluation) => (
                <li
                  key={evaluation.id}
                  className="border-t border-line/70 pt-2 first:border-t-0 first:pt-0"
                >
                  <strong className="block break-words text-xs text-ink">
                    {humanize(evaluation.id)}
                  </strong>
                  <span className="mt-0.5 block text-xs leading-5 text-ink-muted">
                    {humanize(evaluation.outcome)} · {evaluation.summary}
                  </span>
                </li>
              ))}
            </EvidenceGroup>
            {failures.length > 0 && (
              <EvidenceGroup title="Failures" empty="No failures reported.">
                {failures.map((failure) => (
                  <li
                    key={`${failure.phase}:${failure.message}`}
                    className="border-t border-danger/30 pt-2 first:border-t-0 first:pt-0"
                  >
                    <strong className="block text-xs text-danger">
                      {humanize(failure.phase)}
                    </strong>
                    <span className="mt-0.5 block text-xs leading-5 text-ink-muted">
                      {failure.message}
                    </span>
                  </li>
                ))}
              </EvidenceGroup>
            )}
          </div>
        </div>
      </details>

      <details className="group border-t border-line bg-panel-faint">
        <summary className="grid min-h-12 cursor-pointer list-none gap-1 px-4 py-3 marker:hidden sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center sm:px-5">
          <span className="text-xs font-semibold text-ink">
            Technical evidence
          </span>
          <span className="font-mono text-[0.6875rem] text-ink-muted">
            {assets.length} assets · raw counters and identity
          </span>
        </summary>
        <div className="grid gap-5 border-t border-line px-4 py-4 sm:px-5 lg:grid-cols-[minmax(0,1fr)_minmax(18rem,0.7fr)]">
          <EvidenceGroup title="Assets" empty="No assets persisted.">
            {assets.map((asset) => (
              <li
                key={asset.id}
                className="grid gap-1 border-t border-line/70 pt-2 first:border-t-0 first:pt-0"
              >
                <strong className="break-words text-xs text-ink">
                  {humanize(asset.id)}
                </strong>
                <span className="text-xs text-ink-muted">
                  {asset.media_type ?? asset.kind ?? 'asset'} ·{' '}
                  {formatBytes(asset.size_bytes ?? 0)}
                </span>
                <code className="break-all text-[0.6875rem] leading-5 text-ink-muted">
                  {asset.artifact.path}
                </code>
              </li>
            ))}
          </EvidenceGroup>
          <div className="min-w-0">
            <h6 className="m-0 text-xs font-semibold text-ink">
              Runtime identity
            </h6>
            <dl className="mt-2 grid gap-2 text-xs">
              <div>
                <dt className="text-ink-muted">Step type</dt>
                <dd className="m-0 mt-0.5 break-all font-mono text-[0.6875rem] text-ink-soft">
                  {test.step_type}@{test.step_version}
                </dd>
              </div>
              {test.cost_usd != null && (
                <div>
                  <dt className="text-ink-muted">Reported cost</dt>
                  <dd className="m-0 mt-0.5 font-mono text-[0.6875rem] text-ink-soft">
                    ${test.cost_usd.toFixed(4)}
                  </dd>
                </div>
              )}
            </dl>
            <h6 className="m-0 mt-4 text-xs font-semibold text-ink">
              Raw counters
            </h6>
            <pre className="mt-2 max-h-52 overflow-auto rounded-md border border-line bg-panel p-3 font-mono text-[0.6875rem] leading-5 text-ink-muted whitespace-pre-wrap">
              {formatJson(test.metrics)}
            </pre>
          </div>
        </div>
      </details>
    </li>
  )
}

function Fact({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0 border-l border-line px-3 py-4 first:border-l-0">
      <dt className="text-[0.6rem] font-semibold uppercase tracking-[0.08em] text-ink-muted">
        {label}
      </dt>
      <dd
        className="m-0 mt-1 truncate font-mono text-xs font-semibold text-ink-soft"
        title={value}
      >
        {value}
      </dd>
    </div>
  )
}

function EvidenceGroup({
  title,
  empty,
  children,
}: {
  title: string
  empty: string
  children?: ReactNode
}) {
  const present = Array.isArray(children)
    ? children.length > 0
    : Boolean(children)
  return (
    <section className="min-w-0">
      <h6 className="m-0 text-xs font-semibold text-ink">{title}</h6>
      {present ? (
        <ul className="m-0 mt-2 grid list-none gap-2 p-0">{children}</ul>
      ) : (
        <p className="m-0 mt-1 text-xs text-ink-muted">{empty}</p>
      )}
    </section>
  )
}

function workflowStatus(metrics: WorkflowMetricsSummary): OperationalStatus {
  if (metrics.failedSteps > 0 || metrics.failureCount > 0) return 'failed'
  if (metrics.hardGateFailedSteps > 0) return 'hard_gate'
  if (metrics.cancelledSteps > 0) return 'cancelled'
  if (metrics.runningSteps > 0) return 'running'
  if (metrics.pendingSteps > 0 || metrics.skippedSteps > 0) return 'incomplete'
  return metrics.stepCount > 0 && metrics.succeededSteps === metrics.stepCount
    ? 'passed'
    : 'unavailable'
}

function cleanupStatus(status: string | undefined): {
  status: OperationalStatus
  label: string
} {
  if (status === 'succeeded')
    return { status: 'passed', label: 'Cleanup passed' }
  if (status === 'failed') return { status: 'failed', label: 'Cleanup failed' }
  return { status: 'unavailable', label: 'Cleanup not reported' }
}

function semanticTestStatus(status: string): {
  status: OperationalStatus
  label: string
} {
  switch (status.toLowerCase()) {
    case 'succeeded':
      return { status: 'passed', label: 'Succeeded' }
    case 'failed':
      return { status: 'failed', label: 'Failed' }
    case 'hard_gate_failed':
      return { status: 'hard_gate', label: 'Hard gate failed' }
    case 'running':
      return { status: 'running', label: 'Running' }
    case 'cancelled':
      return { status: 'cancelled', label: 'Cancelled' }
    case 'skipped':
      return { status: 'incomplete', label: 'Skipped' }
    case 'pending':
      return { status: 'incomplete', label: 'Pending' }
    default:
      return { status: 'unavailable', label: humanize(status) }
  }
}

function summarizeTestOutcome(test: SemanticTestReport): {
  title: string
  detail: string
  tone: string
} {
  const gates = test.hard_gates ?? []
  const failedGates = gates.filter((gate) => !gate.passed)
  const failures = test.failures ?? []

  if (failures.length > 0) {
    return {
      title: `${failures.length} technical ${pluralize(failures.length, 'failure')} reported`,
      detail: failures[0]?.message ?? 'The step did not complete as expected.',
      tone: 'text-danger',
    }
  }
  if (failedGates.length > 0 || test.status === 'hard_gate_failed') {
    return {
      title: `${failedGates.length || 1} hard ${pluralize(failedGates.length || 1, 'gate')} failed`,
      detail:
        failedGates[0]?.reason ??
        'The step completed, but objective acceptance evidence did not pass.',
      tone: 'text-danger',
    }
  }
  if (test.status === 'skipped') {
    return {
      title: 'Step was skipped',
      detail: test.skip_reason ?? 'No skip reason was persisted.',
      tone: 'text-warning',
    }
  }
  if (test.status === 'cancelled') {
    return {
      title: 'Step was cancelled',
      detail: 'The workflow stopped before this step produced a final result.',
      tone: 'text-warning',
    }
  }
  if (test.status === 'running' || test.status === 'pending') {
    return {
      title: `Step is ${test.status}`,
      detail: 'A terminal outcome has not been persisted yet.',
      tone: 'text-info',
    }
  }
  if (gates.length > 0) {
    return {
      title: `All ${gates.length} hard ${pluralize(gates.length, 'gate')} passed`,
      detail: `${test.evaluations?.length ?? 0} advisory ${pluralize(test.evaluations?.length ?? 0, 'evaluation')} recorded without overriding the objective result.`,
      tone: 'text-success',
    }
  }
  return {
    title: 'Step completed successfully',
    detail: 'No hard gates or technical failures were reported for this step.',
    tone: 'text-success',
  }
}

function pluralize(count: number, singular: string) {
  return count === 1 ? singular : `${singular}s`
}

function observedFlows(detail: DashboardExecutionDetail): ObservedFlow[] {
  return (detail.reports ?? []).flatMap((record) =>
    (record.report?.scenarios ?? []).flatMap((scenario) =>
      (scenario.runs ?? [])
        .filter((run) => (run.semantic_tests?.length ?? 0) > 0)
        .map((run) => ({
          key: `${record.subject_id}:${scenario.scenario_id}:${run.run_id}:${run.attempt_id}`,
          subjectId: record.subject_id,
          scenarioId: scenario.scenario_id,
          runId: run.run_id,
          tests: run.semantic_tests ?? [],
          flow: run.scenario_flow ?? null,
        })),
    ),
  )
}

type StepFact = { label: string; value: string }

function primaryStepFacts(test: SemanticTestReport): StepFact[] {
  const usage = workflowStepUsage(test.metrics)
  return [
    { label: 'Duration', value: formatDuration(test.duration_ms) },
    {
      label: 'Tokens',
      value: formatAvailableCount(usage.totalTokens),
    },
    {
      label: 'Function calls',
      value: formatAvailableCount(usage.functionCalls),
    },
  ]
}

function humanize(value: string) {
  return value
    .replaceAll('.', ' / ')
    .replaceAll('_', ' ')
    .replace(/\b\w/g, (letter) => letter.toUpperCase())
}

function shortHash(value: string) {
  return value.length > 20 ? `${value.slice(0, 20)}…` : value
}

function formatDuration(milliseconds: number) {
  if (milliseconds < 1_000) return `${milliseconds} ms`
  if (milliseconds < 60_000) return `${(milliseconds / 1_000).toFixed(1)} s`
  return `${(milliseconds / 60_000).toFixed(1)} min`
}

function formatNumericMetric(key: string, value: number) {
  const formatted = Number.isInteger(value)
    ? value.toLocaleString('en-US')
    : value.toLocaleString('en-US', { maximumFractionDigits: 2 })
  return key.endsWith('_ms') ? `${formatted} ms` : formatted
}

function formatCount(value: number | null) {
  return value == null ? '—' : value.toLocaleString('en-US')
}

function formatAvailableCount(value: number | null) {
  return value == null ? 'Not captured' : formatCount(value)
}

function formatReportedCount(value: number, reportingSteps: number) {
  return reportingSteps === 0 ? 'Not captured' : formatCount(value)
}

function metricCoverage(reportingSteps: number, stepCount: number) {
  if (reportingSteps === 0) return 'not reported by workflow steps'
  if (reportingSteps === stepCount) return `all ${stepCount} steps reported`
  return `${reportingSteps}/${stepCount} steps reported · partial total`
}

function formatBytes(bytes: number) {
  if (bytes < 1_000) return `${bytes} B`
  if (bytes < 1_000_000) return `${(bytes / 1_000).toFixed(1)} KB`
  return `${(bytes / 1_000_000).toFixed(1)} MB`
}

function formatJson(value: unknown) {
  return value == null ? 'Not reported' : JSON.stringify(value, null, 2)
}
