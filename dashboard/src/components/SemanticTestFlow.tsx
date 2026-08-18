import type { ReactNode } from 'react'
import type {
  DashboardExecutionDetail,
  ScenarioFlowEvidence,
  SemanticTestReport,
} from '@/lib/dashboard-data-source'
import {
  aggregateWorkflowMetrics,
  type WorkflowMetricsSummary,
  workflowMetricEntries,
} from '@/lib/workflow-metrics'

type ObservedFlow = {
  key: string
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
        <div className="section-kicker mb-2">Rust-defined flow</div>
        <h3
          id="semantic-tests-heading"
          className="m-0 text-xl font-semibold tracking-[-0.025em]"
        >
          Semantic test execution
        </h3>
        <p className="mt-2 max-w-3xl text-sm leading-6 text-ink-muted">
          Each card is a meaningful test. Requests, polling and capture remain
          internal operations; dependencies, branching and cleanup are decided
          by Rust and this view only projects persisted evidence.
        </p>
      </div>
      {flows.map((flow) => (
        <article
          key={flow.key}
          className="overflow-hidden rounded-[10px] border border-line-strong bg-panel-faint"
        >
          <header className="flex flex-wrap items-center justify-between gap-3 border-b border-line px-4 py-3">
            <div className="min-w-0">
              <strong className="block break-words text-sm text-ink">
                {humanize(flow.scenarioId)}
              </strong>
              <code className="mt-1 block break-all text-[0.62rem] text-ink-muted">
                run {flow.runId}
              </code>
            </div>
            <div className="flex flex-wrap items-center gap-2 text-[0.65rem] text-ink-muted">
              <span className="data-badge">
                {flow.tests.length} semantic tests
              </span>
              <span className="data-badge">
                cleanup {flow.flow?.cleanup.status ?? 'not reported'}
              </span>
              {flow.flow?.definition_sha256 && (
                <code title={flow.flow.definition_sha256}>
                  {shortHash(flow.flow.definition_sha256)}
                </code>
              )}
            </div>
          </header>

          <WorkflowMetricsOverview
            metrics={aggregateWorkflowMetrics(flow.tests)}
          />

          <ol className="m-0 grid list-none gap-3 p-4 lg:grid-cols-2">
            {flow.tests.map((test, index) => (
              <SemanticTestCard
                key={test.node_id}
                test={test}
                number={index + 1}
              />
            ))}
          </ol>

          <div className="overflow-x-auto border-t border-line">
            <table className="w-full min-w-[680px] border-collapse text-left text-xs">
              <caption className="sr-only">
                Accessible semantic test execution summary
              </caption>
              <thead className="bg-panel-subtle text-ink-muted">
                <tr>
                  <th className="px-4 py-2.5">Test</th>
                  <th className="px-4 py-2.5">Status</th>
                  <th className="px-4 py-2.5">Duration</th>
                  <th className="px-4 py-2.5">Dependencies</th>
                  <th className="px-4 py-2.5">Evidence</th>
                </tr>
              </thead>
              <tbody>
                {flow.tests.map((test) => (
                  <tr key={test.node_id} className="border-t border-line">
                    <th className="px-4 py-3 font-medium text-ink">
                      {humanize(test.node_id)}
                    </th>
                    <td className="px-4 py-3">{humanize(test.status)}</td>
                    <td className="px-4 py-3">
                      {formatDuration(test.duration_ms)}
                    </td>
                    <td className="px-4 py-3">
                      {test.dependencies.length
                        ? test.dependencies.join(', ')
                        : 'None'}
                    </td>
                    <td className="px-4 py-3">
                      {test.assets?.length ?? 0} assets ·{' '}
                      {test.hard_gates?.length ?? 0} gates ·{' '}
                      {test.evaluations?.length ?? 0} evaluations
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
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
      className="border-b border-line bg-panel-subtle px-4 py-3"
      aria-label="Workflow metrics"
    >
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <div>
          <div className="section-kicker">Persisted workflow metrics</div>
          <p className="mt-1 text-xs text-ink-muted">
            Operational counters collected by the Rust semantic tests. Harness
            token and session metrics remain separate when they apply.
          </p>
        </div>
        <span className="text-[0.65rem] text-ink-muted">
          {metrics.stepCount} tests observed
        </span>
      </div>
      <div className="mt-3 grid gap-2 sm:grid-cols-2 lg:grid-cols-5">
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
          label="Step duration"
          value={formatDuration(metrics.durationMs)}
          caption="sum of semantic tests"
        />
        <WorkflowMetric
          label="Assets"
          value={String(metrics.assetCount)}
          caption="persisted before cleanup"
        />
        <WorkflowMetric
          label="Hard gates"
          value={`${metrics.passedHardGateCount}/${metrics.hardGateCount}`}
          caption={`${metrics.evaluationCount} evaluations`}
        />
        <WorkflowMetric
          label="Failures"
          value={String(metrics.failureCount)}
          caption={`${metrics.skippedSteps} skipped`}
        />
      </div>
      {numericMetrics.length > 0 && (
        <dl className="mt-3 grid gap-x-4 gap-y-1 text-xs sm:grid-cols-2 lg:grid-cols-3">
          {numericMetrics.map(([key, value]) => (
            <div
              key={key}
              className="flex min-w-0 items-baseline justify-between gap-3 border-t border-line/70 pt-1.5"
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
      )}
    </section>
  )
}

function WorkflowMetric({
  label,
  value,
  caption,
}: {
  label: string
  value: string
  caption: string
}) {
  return (
    <div className="rounded-md border border-line bg-panel px-3 py-2">
      <div className="section-kicker">{label}</div>
      <strong className="mt-1 block text-sm text-ink">{value}</strong>
      <span className="mt-0.5 block text-[0.65rem] text-ink-muted">
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
  return (
    <li className="min-w-0 rounded-lg border border-line bg-panel p-4 shadow-sm">
      <header className="flex items-start gap-3">
        <span className="font-mono text-[0.65rem] text-ink-muted">
          {String(number).padStart(2, '0')}
        </span>
        <div className="min-w-0 flex-1">
          <h4 className="m-0 break-words text-sm font-semibold text-ink">
            {humanize(test.node_id)}
          </h4>
          <code className="mt-1 block break-all text-[0.6rem] text-ink-muted">
            {test.step_type}@{test.step_version}
          </code>
        </div>
        <span className={`status-badge status-${test.status}`}>
          {humanize(test.status)}
        </span>
      </header>

      <dl className="mt-4 grid gap-2 text-xs sm:grid-cols-2">
        <Fact label="Duration" value={formatDuration(test.duration_ms)} />
        <Fact
          label="Role"
          value={test.required ? 'Required test' : 'Optional test'}
        />
        <Fact
          label="Dependencies"
          value={
            test.dependencies.length ? test.dependencies.join(', ') : 'None'
          }
        />
        <Fact
          label="Cost"
          value={
            test.cost_usd == null
              ? 'Not reported'
              : `$${test.cost_usd.toFixed(4)}`
          }
        />
      </dl>

      {test.skip_reason && (
        <p className="mt-3 border-l-2 border-warning bg-warning/5 px-3 py-2 text-xs text-ink-soft">
          <strong>Skip reason:</strong> {test.skip_reason}
        </p>
      )}

      <details className="mt-3 rounded-md border border-line bg-panel-subtle">
        <summary className="cursor-pointer px-3 py-2 text-xs font-semibold">
          Metrics
        </summary>
        <pre className="max-h-44 overflow-auto border-t border-line p-3 text-[0.65rem] text-ink-muted whitespace-pre-wrap">
          {formatJson(test.metrics)}
        </pre>
      </details>

      <EvidenceGroup title="Assets" empty="No assets persisted.">
        {test.assets?.map((asset) => (
          <li key={asset.id} className="grid gap-1">
            <strong className="break-words text-xs">{asset.id}</strong>
            <span className="text-xs text-ink-muted">
              {asset.media_type ?? asset.kind ?? 'asset'} ·{' '}
              {asset.size_bytes ?? 0} bytes
            </span>
            <code className="break-all text-[0.6rem] text-ink-muted">
              {asset.artifact.path}
            </code>
          </li>
        ))}
      </EvidenceGroup>
      <EvidenceGroup title="Hard gates" empty="No hard gates reported.">
        {test.hard_gates?.map((gate) => (
          <li key={gate.id} className="grid gap-1">
            <strong className={gate.passed ? 'text-success' : 'text-danger'}>
              {gate.passed ? 'PASS' : 'FAIL'} · {gate.id}
            </strong>
            <span className="text-xs text-ink-muted">{gate.reason}</span>
          </li>
        ))}
      </EvidenceGroup>
      <EvidenceGroup title="Evaluations" empty="No evaluations reported.">
        {test.evaluations?.map((evaluation) => (
          <li key={evaluation.id} className="grid gap-1">
            <strong className="text-xs">
              {humanize(evaluation.outcome)} · {evaluation.id}
            </strong>
            <span className="text-xs text-ink-muted">{evaluation.summary}</span>
          </li>
        ))}
      </EvidenceGroup>
      <EvidenceGroup title="Failures" empty="No failures reported.">
        {test.failures?.map((failure) => (
          <li
            key={`${failure.phase}:${failure.message}`}
            className="grid gap-1"
          >
            <strong className="text-xs text-danger">
              {humanize(failure.phase)}
            </strong>
            <span className="text-xs text-ink-muted">{failure.message}</span>
          </li>
        ))}
      </EvidenceGroup>
    </li>
  )
}

function Fact({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-md bg-panel-subtle px-3 py-2">
      <dt className="section-kicker">{label}</dt>
      <dd className="m-0 mt-1 break-words text-ink-soft">{value}</dd>
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
    <section className="mt-4">
      <h5 className="m-0 text-xs font-semibold text-ink">{title}</h5>
      {present ? (
        <ul className="m-0 mt-2 grid list-none gap-2 p-0">{children}</ul>
      ) : (
        <p className="m-0 mt-1 text-xs text-ink-muted">{empty}</p>
      )}
    </section>
  )
}

function observedFlows(detail: DashboardExecutionDetail): ObservedFlow[] {
  return (detail.reports ?? []).flatMap((record) =>
    (record.report?.scenarios ?? []).flatMap((scenario) =>
      (scenario.runs ?? [])
        .filter((run) => (run.semantic_tests?.length ?? 0) > 0)
        .map((run) => ({
          key: `${record.subject_id}:${scenario.scenario_id}:${run.run_id}:${run.attempt_id}`,
          scenarioId: scenario.scenario_id,
          runId: run.run_id,
          tests: run.semantic_tests ?? [],
          flow: run.scenario_flow ?? null,
        })),
    ),
  )
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

function formatJson(value: unknown) {
  return value == null ? 'Not reported' : JSON.stringify(value, null, 2)
}
