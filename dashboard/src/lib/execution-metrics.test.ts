import { describe, expect, it } from 'vitest'
import { buildExecutionMetrics } from '@/lib/execution-metrics'
import {
  executionMetricsFixture,
  metricRun,
} from '@/test-fixtures/execution-metrics'

describe('whole-execution metrics', () => {
  it('pools the approved A/B/C example into one execution summary', () => {
    const metrics = buildExecutionMetrics(
      executionMetricsFixture([
        { runs: [metricRun('a', 100_000, { quality_score_completed: 60 })] },
        {
          runs: [
            metricRun('b', 20_000, {
              completion: 'task_incomplete',
              quality_score_completed: null,
            }),
          ],
        },
        { runs: [metricRun('c', 120_000, { quality_score_completed: 100 })] },
      ]),
    )
    expect(metrics).toMatchObject({
      scenarios: 3,
      includedScenarios: 3,
      scopeComplete: true,
      partial: false,
      planned: 3,
      observed: 3,
      completed: 2,
      incomplete: 1,
      completionRate: 2 / 3,
      completionCoverage: 1,
      executionReliability: 1,
      tokensCompletedP50: 110_000,
      tokensPerCompletion: 120_000,
      qualityMedian: 80,
      qualitySamples: 2,
      subjectTokens: { total: 240_000, samples: 3, expected: 3 },
      failedAttemptTokens: { total: 20_000 },
      judgeTokens: { total: 120, samples: 3, expected: 3 },
      durationMs: { total: 3_000 },
      functionCalls: { total: 30 },
    })
    expect(metrics.cost.total).toBeCloseTo(0.3)
  })

  it('weights by runs and recomputes medians instead of averaging scenario summaries', () => {
    const detail = executionMetricsFixture([
      {
        runs: [
          metricRun('a', 10, {
            quality_score_completed: 10,
            objective_score: 10,
          }),
        ],
      },
      {
        runs: [
          metricRun('b', 30, {
            quality_score_completed: 30,
            objective_score: 30,
          }),
          metricRun('c', 80, {
            quality_score_completed: 80,
            objective_score: 80,
          }),
          metricRun('d', 90, {
            quality_score_completed: 90,
            objective_score: 90,
          }),
          metricRun('e', 900, {
            completion: 'task_incomplete',
            quality_score_completed: null,
            objective_score: null,
          }),
        ],
      },
    ])
    const metrics = buildExecutionMetrics(detail)
    expect(metrics.completionRate).toBe(4 / 5)
    expect(metrics.qualityMedian).toBe(55)
    expect(metrics.objectiveMedian).toBe(55)
    expect(metrics.tokensCompletedP50).toBe(55)
    expect(metrics.tokensPerCompletion).toBe(1_110 / 4)
  })

  it('includes physical retries exactly once for tokens, judge usage and cost', () => {
    const run = metricRun('a', 150, {
      cost: { total_usd: 0.3 },
      wall_time_ms: 3_000,
      metrics: { totals: { input_tokens: 80, output_tokens: 20 } },
      retry_attempts: [
        {
          ...metricRun('a', 50),
          session_id: 'retry-session',
          attempt_number: 1,
          attempt_id: 'retry',
        },
      ],
    })
    const metrics = buildExecutionMetrics(
      executionMetricsFixture([{ runs: [run] }]),
    )
    expect(metrics.subjectTokens.total).toBe(150)
    expect(metrics.failedAttemptTokens.total).toBe(50)
    expect(metrics.cost.total).toBe(0.3)
    expect(metrics.durationMs.total).toBe(3_000)
    expect(metrics.judgeTokens).toMatchObject({
      total: 80,
      samples: 2,
      expected: 2,
    })
    run.completion = 'task_incomplete'
    run.quality_score_completed = null
    const incomplete = buildExecutionMetrics(
      executionMetricsFixture([{ runs: [run] }]),
    )
    expect(incomplete.failedAttemptTokens.total).toBe(150)
    expect(incomplete.tokensPerCompletion).toBeNull()
  })

  it('preserves known usage when another run lacks telemetry without inventing efficiency', () => {
    const unknown = metricRun('unknown', null, {
      cost: null,
      judge_usage: null,
      efficiency: null,
      wall_time_ms: null,
    })
    const metrics = buildExecutionMetrics(
      executionMetricsFixture([
        { runs: [metricRun('known', 100)] },
        { runs: [unknown] },
      ]),
    )
    expect(metrics.subjectTokens).toEqual({
      total: null,
      observed: 100,
      samples: 1,
      expected: 2,
    })
    expect(metrics.cost).toEqual({
      total: null,
      observed: 0.1,
      samples: 1,
      expected: 2,
    })
    expect(metrics.judgeTokens).toMatchObject({
      total: null,
      observed: 40,
      samples: 1,
    })
    expect(metrics.tokensPerCompletion).toBeNull()
    expect(metrics.tokensCompletedP50).toBeNull()
    expect(metrics.completedTokenSamples).toBe(1)
  })

  it('keeps undetermined and deferred slots separate from task completion', () => {
    const metrics = buildExecutionMetrics(
      executionMetricsFixture([
        { runs: [metricRun('a', 100)] },
        {
          runs: [
            metricRun('b', null, {
              completion: 'undetermined',
              technical: 'technical_invalid',
              quality_score_completed: null,
              objective_score: null,
            }),
          ],
          deferred: 1,
        },
      ]),
    )
    expect(metrics).toMatchObject({
      planned: 3,
      observed: 2,
      completed: 1,
      incomplete: 0,
      undetermined: 1,
      deferred: 1,
      technicalInvalid: 1,
      partial: true,
    })
    expect(metrics.completionRate).toBe(1)
    expect(metrics.completionCoverage).toBe(1 / 3)
    expect(metrics.executionReliability).toBe(1 / 3)
    expect(metrics.qualitySamples).toBe(1)
  })

  it('does not trust a cumulative efficiency counter when retry telemetry is missing', () => {
    const run = metricRun('a', 100, {
      retry_attempts: [
        {
          ...metricRun('a', null, { efficiency: null }),
          session_id: 'retry-session',
          attempt_number: 1,
          attempt_id: 'retry',
        },
      ],
    })
    const metrics = buildExecutionMetrics(
      executionMetricsFixture([{ runs: [run] }]),
    )
    expect(metrics.subjectTokens.total).toBeNull()
    expect(metrics.failedAttemptTokens.total).toBeNull()
    expect(metrics.functionCalls.total).toBeNull()
    expect(metrics.tokensPerCompletion).toBeNull()
  })

  it('keeps entirely deferred and zero-completion executions undefined, not perfect or free', () => {
    const metrics = buildExecutionMetrics(
      executionMetricsFixture([{ runs: [], deferred: 3 }]),
    )
    expect(metrics.completionRate).toBeNull()
    expect(metrics.completionCoverage).toBe(0)
    expect(metrics.subjectTokens.total).toBeNull()
    expect(metrics.cost.total).toBeNull()
    expect(metrics.qualityMedian).toBeNull()
    expect(metrics.tokensPerCompletion).toBeNull()
    expect(
      buildExecutionMetrics(executionMetricsFixture([])).scopeComplete,
    ).toBe(false)
  })

  it('isolates unavailable or incompatible scenario evidence and does not claim a complete total', () => {
    const detail = executionMetricsFixture([
      { runs: [metricRun('a', 100)] },
      { runs: [metricRun('b', 200)] },
    ])
    const first = detail.reports[0].report
    const second = detail.reports[1].report
    if (!first || !second) throw new Error('fixture must contain reports')
    second.result_contract_sha256 = 'incompatible'
    const metrics = buildExecutionMetrics(detail)
    expect(metrics).toMatchObject({
      includedScenarios: 1,
      scenarios: 2,
      scopeComplete: false,
      partial: true,
      observed: 1,
    })
    expect(metrics.subjectTokens).toMatchObject({ observed: 100, total: null })
    expect(metrics.tokensPerCompletion).toBeNull()
    first.scenarios[0].aggregate.observed_runs = 99
    expect(buildExecutionMetrics(detail).includedScenarios).toBe(0)
  })

  it('does not count duplicate report projections twice', () => {
    const detail = executionMetricsFixture([{ runs: [metricRun('a', 100)] }])
    detail.reports.push(detail.reports[0])
    const metrics = buildExecutionMetrics(detail)
    expect(metrics.subjectTokens.observed).toBe(100)
    expect(metrics.observed).toBe(1)
    expect(metrics.scopeComplete).toBe(false)
    const deferred = executionMetricsFixture([{ runs: [], deferred: 3 }])
    deferred.reports.push(deferred.reports[0])
    expect(buildExecutionMetrics(deferred).planned).toBe(3)
  })

  it('preserves a measured zero but rejects unsafe or negative token telemetry', () => {
    const zero = buildExecutionMetrics(
      executionMetricsFixture([{ runs: [metricRun('a', 0)] }]),
    )
    expect(zero.subjectTokens.total).toBe(0)
    expect(zero.tokensPerCompletion).toBe(0)
    for (const tokens of [-1, Number.MAX_SAFE_INTEGER + 1]) {
      expect(
        buildExecutionMetrics(
          executionMetricsFixture([{ runs: [metricRun('a', tokens)] }]),
        ).subjectTokens.total,
      ).toBeNull()
    }
  })
})
