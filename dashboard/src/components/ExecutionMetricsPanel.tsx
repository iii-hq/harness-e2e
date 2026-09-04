import { useMemo } from 'react'
import { DataTable, DataTableRow, MetricCard, Panel } from '@/design-system'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import {
  buildExecutionMetrics,
  type UsageCoverage,
} from '@/lib/execution-metrics'
import { formatDuration } from '@/lib/execution-view'

function number(value: number | null) {
  return value === null
    ? '—'
    : value.toLocaleString('en-US', { maximumFractionDigits: 1 })
}

function percent(value: number | null) {
  return value === null ? '—' : `${number(value * 100)}%`
}

function cost(value: number | null) {
  if (value === null) return '—'
  return value > 0 && value < 0.0001 ? '<$0.0001' : `$${value.toFixed(4)}`
}

export function ExecutionMetricsPanel({
  detail,
}: {
  detail: DashboardExecutionDetail
}) {
  const metrics = useMemo(() => buildExecutionMetrics(detail), [detail])
  const counts = [
    ['planned', metrics.planned],
    ['recorded', metrics.observed],
    ['completed', metrics.completed],
    ['task incomplete', metrics.incomplete],
    ['undetermined', metrics.undetermined],
    ['deferred', metrics.deferred],
    ['technically invalid', metrics.technicalInvalid],
  ] as const
  const usage: Array<{
    label: string
    metric: UsageCoverage
    format: (value: number | null) => string
    unit: string
    note: string
  }> = [
    {
      label: 'Subject tokens',
      metric: metrics.subjectTokens,
      format: number,
      unit: 'runs',
      note: 'Includes retries exactly once.',
    },
    {
      label: 'Judge tokens',
      metric: metrics.judgeTokens,
      format: number,
      unit: 'attempts',
      note: 'Separate from subject consumption.',
    },
    {
      label: 'Failed attempt tokens',
      metric: metrics.failedAttemptTokens,
      format: number,
      unit: 'runs',
      note: 'Retries plus terminal attempts of non-completed tasks.',
    },
    {
      label: 'Execution cost',
      metric: metrics.cost,
      format: cost,
      unit: 'runs',
      note: 'Reported subject and judge cost, including retries.',
    },
    {
      label: 'Accumulated run time',
      metric: metrics.durationMs,
      format: (value) => (value === null ? '—' : formatDuration(value / 1_000)),
      unit: 'runs',
      note: 'Sum of run durations including retries; not elapsed wall-clock time.',
    },
    {
      label: 'Function calls',
      metric: metrics.functionCalls,
      format: number,
      unit: 'runs',
      note: 'Recorded run efficiency, including retries.',
    },
    {
      label: 'Function errors',
      metric: metrics.functionErrors,
      format: number,
      unit: 'runs',
      note: 'Recorded run efficiency, including retries.',
    },
  ]
  return (
    <Panel
      as="section"
      id="metrics"
      className="scroll-mt-24"
      aria-labelledby="execution-metrics-heading"
      data-execution-metrics
    >
      <div className="flex flex-wrap items-baseline justify-between gap-3">
        <h2
          id="execution-metrics-heading"
          className="m-0 text-sm font-semibold text-ink"
        >
          execution summary
        </h2>
        <span className="font-mono text-xs text-ink-muted">
          {metrics.includedScenarios}/{metrics.scenarios} scenarios ·{' '}
          {metrics.observed} runs ·{' '}
          {metrics.partial ? 'partial evidence' : 'retained results'}
        </span>
      </div>
      <p className="mt-2 mb-0 text-xs leading-5 text-ink-soft">
        Whole-execution metrics, pooled across all scenarios and repetitions.
        Report coverage and scenario pass rate are not task completion.
      </p>
      {!metrics.scopeComplete ? (
        <p className="mt-3 mb-0 text-sm text-warning" role="status">
          {metrics.scenarios - metrics.includedScenarios} scenarios have
          unavailable or inconsistent evidence. Counts and rates below cover
          only the verified subset; execution-wide consumption is unknown.
        </p>
      ) : null}
      {metrics.includedScenarios === 0 ? (
        <p className="mt-4 mb-0 text-sm text-ink-muted">
          No compatible run evidence is available to consolidate. Missing
          metrics are not zero.
        </p>
      ) : (
        <>
          <dl className="m-0 mt-4 flex flex-wrap gap-x-6 gap-y-3">
            {counts.map(([label, value]) => (
              <div key={label}>
                <dt className="ds-label">{label}</dt>
                <dd className="m-0 mt-1 font-mono text-sm font-semibold text-ink">
                  {number(value)}
                </dd>
              </div>
            ))}
          </dl>
          <div className="mt-5 grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
            <MetricCard
              label="completion rate"
              value={percent(metrics.completionRate)}
              detail={`${metrics.completed}/${metrics.completed + metrics.incomplete} determined runs`}
            />
            <MetricCard
              label="execution reliability"
              value={percent(metrics.executionReliability)}
              detail={`${metrics.technicalValid}/${metrics.planned} planned runs technically valid`}
            />
            <MetricCard
              label="completion evidence"
              value={percent(metrics.completionCoverage)}
              detail={`${metrics.completed + metrics.incomplete}/${metrics.planned} planned runs determined`}
            />
            <MetricCard
              label="quality on completed tasks"
              value={
                metrics.qualityMedian === null
                  ? '—'
                  : `${number(metrics.qualityMedian)}/100`
              }
              detail={`Median · ${metrics.qualitySamples}/${metrics.completed} completed runs scored`}
            />
          </div>
          <DataTable
            caption="Consolidated execution consumption and efficiency"
            minWidth="620px"
            wrapClassName="mt-5"
          >
            <thead>
              <tr>
                <th scope="col">Metric</th>
                <th scope="col">Value</th>
                <th scope="col">Coverage / denominator</th>
              </tr>
            </thead>
            <tbody>
              {usage.map(({ label, metric, format, unit, note }) => (
                <DataTableRow key={label}>
                  <td>
                    <span className="font-semibold">{label}</span>
                    <span className="mt-1 block text-xs text-ink-muted">
                      {note}
                    </span>
                  </td>
                  <td className="font-mono tabular-nums">
                    {format(metric.total ?? metric.observed)}
                    {metric.total === null && metric.observed !== null ? (
                      <span className="mt-1 block text-xs text-warning">
                        observed subtotal
                      </span>
                    ) : null}
                  </td>
                  <td className="text-xs text-ink-muted">
                    {metric.samples}/{metric.expected} {unit} with telemetry
                  </td>
                </DataTableRow>
              ))}
              <DataTableRow>
                <td className="font-semibold">Tokens per completion</td>
                <td className="font-mono tabular-nums">
                  {number(metrics.tokensPerCompletion)}
                </td>
                <td className="text-xs text-ink-muted">
                  Total subject tokens / {metrics.completed} completed runs;
                  requires complete token coverage.
                </td>
              </DataTableRow>
              <DataTableRow>
                <td className="font-semibold">Completed p50 tokens</td>
                <td className="font-mono tabular-nums">
                  {number(metrics.tokensCompletedP50)}
                </td>
                <td className="text-xs text-ink-muted">
                  {metrics.completedTokenSamples}/{metrics.completed} completed
                  runs with telemetry; pooled median, including retries.
                </td>
              </DataTableRow>
              <DataTableRow>
                <td className="font-semibold">Objective score</td>
                <td className="font-mono tabular-nums">
                  {metrics.objectiveMedian === null
                    ? '—'
                    : `${number(metrics.objectiveMedian)}/100`}
                </td>
                <td className="text-xs text-ink-muted">
                  Median · {metrics.objectiveSamples}/{metrics.planned} planned
                  runs scored.
                </td>
              </DataTableRow>
            </tbody>
          </DataTable>
          <p className="mt-3 mb-0 text-xs leading-5 text-ink-muted">
            Missing telemetry stays unknown. Observed subtotals are not complete
            totals and must not be interpreted as improved efficiency. Quality
            and token medians are pooled from individual runs, not averaged
            across scenarios.
          </p>
        </>
      )}
    </Panel>
  )
}
