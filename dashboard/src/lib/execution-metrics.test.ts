import { describe, expect, it } from 'vitest'
import { buildExecutionMetrics } from '@/lib/execution-metrics'
import {
  executionMetricsFixture,
  metricRun,
} from '@/test-fixtures/execution-metrics'

describe('whole-execution metrics', () => {
  it('sums input, output, cache and turns across runs and retries while preserving missing cache writes', () => {
    const first = metricRun('first', 13, {
      metrics: {
        totals: {
          input_tokens: 10,
          output_tokens: 2,
          cache_read_tokens: 20,
          cache_write_tokens: 0,
          turns: 2,
        },
      },
      retry_attempts: [
        {
          ...metricRun('retry', 3),
          session_id: 'retry-session',
          attempt_number: 1,
          metrics: {
            totals: {
              input_tokens: 3,
              output_tokens: 1,
              cache_read_tokens: 4,
              cache_write_tokens: 0,
              turns: 1,
            },
          },
        },
      ],
    })
    const second = metricRun('second', 5, {
      metrics: {
        totals: {
          input_tokens: 5,
          output_tokens: 0,
          cache_read_tokens: 8,
          turns: 3,
        },
      },
    })
    const metrics = buildExecutionMetrics(
      executionMetricsFixture([{ runs: [first, second] }]),
    )
    expect(metrics.inputTokens).toMatchObject({
      total: 18,
      samples: 2,
      expected: 2,
    })
    expect(metrics.cacheReadTokens.total).toBe(32)
    expect(metrics.outputTokens).toMatchObject({
      total: 3,
      samples: 2,
      expected: 2,
    })
    expect(metrics.turns.total).toBe(6)
    expect(metrics.cacheWriteTokens).toEqual({
      total: null,
      observed: 0,
      samples: 1,
      expected: 2,
    })
    expect(
      buildExecutionMetrics(executionMetricsFixture([{ runs: [second] }]))
        .cacheWriteTokens,
    ).toEqual({ total: null, observed: null, samples: 0, expected: 1 })
  })

  it('pools the approved A/B/C example into one execution summary', () => {
    const metrics = buildExecutionMetrics(
      executionMetricsFixture([
        { runs: [metricRun('a', 100_000, { score: 60 })] },
        {
          runs: [
            metricRun('b', 20_000, {
              completion: 'task_incomplete',
              score: null,
            }),
          ],
        },
        { runs: [metricRun('c', 120_000, { score: 100 })] },
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
      scoreMean: 80,
      scoreSamples: 2,
      subjectTokens: { total: 240_000, samples: 3, expected: 3 },
      failedAttemptTokens: { total: 20_000 },
      durationMs: { total: 3_000 },
      functionCalls: { total: 30 },
    })
    expect(metrics.cost.total).toBeCloseTo(0.3)
  })

  it('weights by runs instead of averaging scenario summaries', () => {
    const detail = executionMetricsFixture([
      { runs: [metricRun('a', 10, { score: 10 })] },
      {
        runs: [
          metricRun('b', 30, { score: 30 }),
          metricRun('c', 80, { score: 80 }),
          metricRun('d', 90, { score: 90 }),
          metricRun('e', 900, {
            completion: 'task_incomplete',
            score: null,
          }),
        ],
      },
    ])
    const metrics = buildExecutionMetrics(detail)
    expect(metrics.completionRate).toBe(4 / 5)
    expect(metrics.scoreMean).toBe(52.5)
    expect(metrics.scoreSamples).toBe(4)
    expect(metrics.tokensCompletedP50).toBe(55)
    expect(metrics.tokensPerCompletion).toBe(1_110 / 4)
  })

  it('includes physical retries exactly once for tokens and cost', () => {
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
    run.completion = 'task_incomplete'
    const incomplete = buildExecutionMetrics(
      executionMetricsFixture([{ runs: [run] }]),
    )
    expect(incomplete.failedAttemptTokens.total).toBe(150)
    expect(incomplete.tokensPerCompletion).toBeNull()
  })

  it('preserves known usage when another run lacks telemetry without inventing efficiency', () => {
    const unknown = metricRun('unknown', null, {
      cost: null,
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
              score: null,
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
    expect(metrics.scoreSamples).toBe(1)
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
    expect(metrics.scoreMean).toBeNull()
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
    // The results contract is not a gate: only a report that states no
    // completeness is unavailable.
    delete (second as unknown as Record<string, unknown>).report_state
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

  it('keeps evidence written under another results contract in the totals', () => {
    const detail = executionMetricsFixture([
      { runs: [metricRun('a', 100)] },
      { runs: [metricRun('b', 200)] },
    ])
    const second = detail.reports[1].report
    if (!second) throw new Error('fixture must contain reports')
    second.result_contract_sha256 = `sha256:${'1'.repeat(64)}`
    const metrics = buildExecutionMetrics(detail)
    expect(metrics).toMatchObject({
      includedScenarios: 2,
      scenarios: 2,
      observed: 2,
    })
    expect(metrics.subjectTokens).toMatchObject({ observed: 300, total: 300 })
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

  it('pools independent plan repetitions without counting repeated native evidence twice', () => {
    const detail = executionMetricsFixture([
      { runs: [metricRun('round-1', 100)] },
      { runs: [metricRun('round-2', 200)] },
    ])
    for (const [index, record] of detail.reports.entries()) {
      record.native_execution_id = `native-${index}`
      const scenario = record.report?.scenarios[0]
      if (!scenario) throw new Error('missing scenario fixture')
      scenario.scenario_id = 'repeated-case'
      scenario.case_id = 'same-case'
    }
    expect(buildExecutionMetrics(detail)).toMatchObject({
      scopeComplete: true,
      planned: 2,
      completed: 2,
      subjectTokens: { total: 300 },
    })
    detail.reports.push(structuredClone(detail.reports[0]))
    expect(buildExecutionMetrics(detail)).toMatchObject({
      scopeComplete: false,
      planned: 2,
      completed: 2,
      subjectTokens: { total: null, observed: 300 },
    })
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
