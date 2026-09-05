import { useMemo } from 'react'
import {
  OutcomeDerivation,
  type OutcomeRow,
} from '@/components/OutcomeDerivation'
import { MetricCard, Panel } from '@/design-system'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import {
  buildExecutionMetrics,
  type ExecutionMetrics,
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

/** completionRate, executionReliability and completionCoverage share numerator
 *  and denominator whenever nothing was incomplete, undetermined, deferred or
 *  technically invalid — they are all 100% by construction, and three tiles
 *  saying so is one fact three times (audit ED-27). */
export function completionIsTrivial(metrics: ExecutionMetrics): boolean {
  return (
    metrics.planned > 0 &&
    metrics.incomplete +
      metrics.undetermined +
      metrics.deferred +
      metrics.technicalInvalid ===
      0
  )
}

const TELEMETRY_METRICS: ReadonlyArray<keyof ExecutionMetrics> = [
  'subjectTokens',
  'judgeTokens',
  'failedAttemptTokens',
  'durationMs',
  'functionCalls',
  'functionErrors',
]

function isCoverage(value: unknown): value is UsageCoverage {
  return Boolean(
    value &&
      typeof value === 'object' &&
      'samples' in value &&
      'expected' in value,
  )
}

/** When every telemetry metric was reported by every run, "N/N runs with
 *  telemetry" beside each of them says the same thing six times. Cost is not
 *  in the set: it is the one that is routinely partial and says so itself. */
export function usageCoverageComplete(metrics: ExecutionMetrics): boolean {
  return TELEMETRY_METRICS.every((key) => {
    const value = metrics[key]
    return isCoverage(value) && value.samples === value.expected
  })
}

/** The median of two values is their mean, so with two or fewer completed runs
 *  "completed p50 tokens" and "tokens per completion" are the same number. */
export function p50EqualsPerCompletion(metrics: ExecutionMetrics): boolean {
  return metrics.completed <= 2
}

function coverageNote(metric: UsageCoverage, unit: string) {
  return `${metric.samples}/${metric.expected} ${unit} with telemetry`
}

function usageValue(
  metric: UsageCoverage,
  format: (v: number | null) => string,
) {
  const value = format(metric.total ?? metric.observed)
  return metric.total === null && metric.observed !== null
    ? `${value} (observed subtotal)`
    : value
}

/** Layer 0 of the execution page: the grouped metrics, and nothing else. The
 *  narrative, the scenario table, the full counts and the provenance open on
 *  demand below it (audit ED-26). */
export function ExecutionOverview({
  detail,
  boundaries,
}: {
  detail: DashboardExecutionDetail
  boundaries: OutcomeRow[]
}) {
  const metrics = useMemo(() => buildExecutionMetrics(detail), [detail])
  const trivial = completionIsTrivial(metrics)
  const complete = usageCoverageComplete(metrics)
  const foldP50 = p50EqualsPerCompletion(metrics)

  return (
    <Panel
      as="section"
      id="overview"
      className="scroll-mt-24 grid gap-5"
      aria-label="Execution overview"
      data-execution-overview
    >
      <div className="grid min-w-0 gap-2">
        <span className="ds-label">outcome</span>
        <OutcomeDerivation rows={boundaries} />
      </div>

      {metrics.includedScenarios === 0 ? (
        <p className="m-0 text-sm text-ink-muted">
          No compatible run evidence is available to consolidate. Missing
          metrics are not zero.
        </p>
      ) : (
        <>
          {!metrics.scopeComplete ? (
            <p className="m-0 text-sm text-warning" role="status">
              {metrics.scenarios - metrics.includedScenarios} scenarios have
              unavailable or inconsistent evidence. Counts and rates below cover
              only the verified subset; execution-wide consumption is unknown.
            </p>
          ) : null}

          <div className="grid min-w-0 gap-2">
            <div className="flex flex-wrap items-baseline justify-between gap-3">
              <span className="ds-label">completion and quality</span>
              <span className="font-mono text-label text-ink-muted">
                {metrics.includedScenarios}/{metrics.scenarios} scenarios ·{' '}
                {metrics.observed} runs ·{' '}
                {metrics.partial ? 'partial evidence' : 'retained results'}
              </span>
            </div>
            <div
              className={`grid min-w-0 gap-3 @[560px]:grid-cols-2 ${trivial ? '@[960px]:grid-cols-3' : '@[960px]:grid-cols-5'}`}
              data-completion={trivial ? 'trivial' : 'detailed'}
            >
              {trivial ? (
                <MetricCard
                  label="every planned run completed"
                  value={`${metrics.completed}/${metrics.planned}`}
                  detail="completed, technically valid and determined · the three ratios are 100% by construction"
                  tone="positive"
                />
              ) : (
                <>
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
                </>
              )}
              <MetricCard
                label="quality on completed tasks"
                value={
                  metrics.qualityMedian === null
                    ? '—'
                    : `${number(metrics.qualityMedian)}/100`
                }
                detail={`median · ${metrics.qualitySamples}/${metrics.completed} completed runs scored`}
                tone={
                  metrics.qualityMedian === null ? 'unavailable' : 'neutral'
                }
              />
              <MetricCard
                label="objective score"
                value={
                  metrics.objectiveMedian === null
                    ? '—'
                    : `${number(metrics.objectiveMedian)}/100`
                }
                detail={`median · ${metrics.objectiveSamples}/${metrics.planned} planned runs scored`}
                tone={
                  metrics.objectiveMedian === null ? 'unavailable' : 'neutral'
                }
              />
            </div>
          </div>

          <div className="grid min-w-0 gap-2">
            <div className="flex flex-wrap items-baseline justify-between gap-3">
              <span className="ds-label">tokens</span>
              <span className="font-mono text-label text-ink-muted">
                {complete
                  ? `subject telemetry from ${metrics.subjectTokens.samples} of ${metrics.subjectTokens.expected} runs · judge from ${metrics.judgeTokens.samples} of ${metrics.judgeTokens.expected} attempts`
                  : 'coverage shown per metric'}
              </span>
            </div>
            <div
              className={`grid min-w-0 gap-3 @[560px]:grid-cols-2 ${foldP50 ? '@[960px]:grid-cols-4' : '@[960px]:grid-cols-5'}`}
              data-usage-coverage={complete ? 'complete' : 'partial'}
            >
              <MetricCard
                label="subject tokens"
                value={usageValue(metrics.subjectTokens, number)}
                detail={
                  complete
                    ? 'includes retries exactly once'
                    : `includes retries exactly once · ${coverageNote(metrics.subjectTokens, 'runs')}`
                }
              />
              <MetricCard
                label="judge tokens"
                value={usageValue(metrics.judgeTokens, number)}
                detail={
                  complete
                    ? 'separate from subject consumption'
                    : `separate from subject consumption · ${coverageNote(metrics.judgeTokens, 'attempts')}`
                }
              />
              <MetricCard
                label="failed attempt tokens"
                value={usageValue(metrics.failedAttemptTokens, number)}
                detail={
                  complete
                    ? 'retries plus terminal attempts of non-completed tasks'
                    : `retries plus terminal attempts of non-completed tasks · ${coverageNote(metrics.failedAttemptTokens, 'runs')}`
                }
              />
              <MetricCard
                label="tokens per completion"
                value={number(metrics.tokensPerCompletion)}
                detail={
                  foldP50
                    ? `total subject tokens / ${metrics.completed} completed runs · equals the completed p50 at two or fewer runs`
                    : `total subject tokens / ${metrics.completed} completed runs · requires complete token coverage`
                }
              />
              {foldP50 ? null : (
                <MetricCard
                  label="completed p50 tokens"
                  value={number(metrics.tokensCompletedP50)}
                  detail={`${metrics.completedTokenSamples}/${metrics.completed} completed runs with telemetry · pooled median, including retries`}
                />
              )}
            </div>
          </div>

          <div className="grid min-w-0 gap-2">
            <div className="flex flex-wrap items-baseline justify-between gap-3">
              <span className="ds-label">runtime and calls</span>
              <span className="font-mono text-label text-ink-muted">
                {complete
                  ? `recorded run efficiency, including retries · ${metrics.functionCalls.samples} of ${metrics.functionCalls.expected} runs`
                  : 'recorded run efficiency, including retries'}
              </span>
            </div>
            <div className="grid min-w-0 gap-3 @[560px]:grid-cols-2 @[960px]:grid-cols-4">
              <MetricCard
                label="accumulated run time"
                value={usageValue(metrics.durationMs, (v) =>
                  v === null ? '—' : formatDuration(v / 1_000),
                )}
                detail={
                  complete
                    ? 'sum of run durations; not elapsed wall-clock time'
                    : `sum of run durations; not elapsed wall-clock time · ${coverageNote(metrics.durationMs, 'runs')}`
                }
              />
              <MetricCard
                label="function calls"
                value={usageValue(metrics.functionCalls, number)}
                detail={
                  complete
                    ? `across ${metrics.observed} runs`
                    : coverageNote(metrics.functionCalls, 'runs')
                }
              />
              <MetricCard
                label="function errors"
                value={usageValue(metrics.functionErrors, number)}
                detail={
                  complete
                    ? `across ${metrics.observed} runs`
                    : coverageNote(metrics.functionErrors, 'runs')
                }
              />
              <MetricCard
                label="execution cost"
                value={usageValue(metrics.cost, cost)}
                detail={
                  metrics.cost.samples === metrics.cost.expected
                    ? 'subject and judge, including retries'
                    : `${metrics.cost.samples} of ${metrics.cost.expected} runs reported cost · subject and judge, including retries`
                }
                tone={
                  metrics.cost.total === null && metrics.cost.observed === null
                    ? 'unavailable'
                    : metrics.cost.samples === metrics.cost.expected
                      ? 'neutral'
                      : 'warning'
                }
              />
            </div>
          </div>
        </>
      )}
    </Panel>
  )
}
