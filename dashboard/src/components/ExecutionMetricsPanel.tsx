import { useMemo } from 'react'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import { buildExecutionMetrics } from '@/lib/execution-metrics'

function number(value: number | null) {
  return value === null
    ? '—'
    : value.toLocaleString('en-US', { maximumFractionDigits: 1 })
}

export function ExecutionMetricsPanel({
  detail,
}: {
  detail: DashboardExecutionDetail
}) {
  const metrics = useMemo(() => buildExecutionMetrics(detail), [detail])
  const failed = metrics.failedAttemptTokens
  const entries = [
    {
      label: 'Failed attempt tokens',
      value: failed.total ?? failed.observed,
      note: 'Retries plus terminal attempts of non-completed tasks.',
      partial: failed.total === null && failed.observed !== null,
    },
    {
      label: 'Tokens per completion',
      value: metrics.tokensPerCompletion,
      note: 'Execution tokens divided by completed runs.',
    },
    {
      label: 'Completed p50 tokens',
      value: metrics.tokensCompletedP50,
      note: 'Median tokens of completed runs, including retries.',
    },
  ]

  return (
    <section
      className="primary-metrics execution-efficiency"
      aria-label="Execution efficiency"
      data-execution-metrics
    >
      <div className="pm-band">
        <div className="pm-band-heading">
          <h3>Efficiency</h3>
          <span className="pm-muted">Across all test runs</span>
        </div>
        {metrics.includedScenarios === 0 ? (
          <p className="m-0 text-sm text-ink-muted">
            No compatible run evidence is available to consolidate. Missing
            metrics are not zero.
          </p>
        ) : (
          <>
            {!metrics.scopeComplete ? (
              <p className="mt-0 mb-4 text-sm text-warning" role="status">
                Efficiency metrics cover only verified scenarios; execution-wide
                consumption is unknown.
              </p>
            ) : null}
            <dl className="execution-efficiency-values">
              {entries.map(({ label, value, note, partial }) => (
                <div className="min-w-0" key={label}>
                  <dt>{label}</dt>
                  <dd className="pm-number m-0 mt-2">
                    {number(value)}
                    {partial ? (
                      <small className="pm-partial">
                        Observed subtotal · {failed.samples}/{failed.expected}{' '}
                        runs reported
                      </small>
                    ) : null}
                  </dd>
                  <dd className="pm-muted m-0 mt-2">{note}</dd>
                </div>
              ))}
            </dl>
          </>
        )}
      </div>
    </section>
  )
}
