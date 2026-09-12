import { describe, expect, it } from 'vitest'
import type {
  DashboardExecutionDetail,
  DashboardRetryAttemptProjection,
  DashboardRunProjection,
} from '@/lib/dashboard-data-source'
import {
  buildPrimaryMetrics,
  comparePrimaryMetrics,
} from '@/lib/primary-metrics'

type RunOptions = {
  score?: number | null
  input?: number | null
  output?: number | null
  cacheRead?: number | null
  cacheWrite?: number | null
  turns?: number | null
  calls?: number | null
  errors?: number | null
  duration?: number | null
  cost?: number | null
  retries?: DashboardRetryAttemptProjection[]
}

function run(id: string, options: RunOptions = {}): DashboardRunProjection {
  const {
    score = 100,
    input = 10,
    output = 2,
    cacheRead = 20,
    cacheWrite = 3,
    turns = 4,
    calls = 5,
    errors = 0,
    duration = 1_000,
    cost = 0.1,
    retries = [],
  } = options
  return {
    run_id: id,
    attempt_id: `${id}-attempt`,
    status: 'passed',
    completion: 'completed',
    technical: 'valid',
    evaluators: { completion: 'available', quality: 'available' },
    assessment: {} as DashboardRunProjection['assessment'],
    objective_score: score,
    quality_score_completed: null,
    metrics: {
      totals: {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cacheRead,
        cache_write_tokens: cacheWrite,
        turns,
        function_calls: calls,
        function_call_errors: errors,
      },
    },
    wall_time_ms: duration,
    cost: cost === null ? null : { total_usd: cost },
    retry_attempts: retries,
  }
}

function detail(
  tests: Array<{
    id: string
    version?: number
    runs?: DashboardRunProjection[]
    planned?: number
    caseId?: string
    available?: boolean
    nativeId?: string
  }>,
): DashboardExecutionDetail {
  return {
    id: 'execution',
    status: 'completed',
    subjects: [],
    reports: tests.map((test) => ({
      subject_id: 'subject',
      scenario_id: test.id,
      scenario_version: test.version ?? 1,
      native_execution_id: test.nativeId ?? `native-${test.id}`,
      available: test.available ?? true,
      report:
        test.available === false
          ? undefined
          : ({
              result_contract_sha256: 'result-contract',
              scoring_profile_sha256: 'scoring-profile',
              scenarios: [
                {
                  scenario_id: test.id,
                  scenario_version: test.version ?? 1,
                  case_id: test.caseId ?? `${test.id}-case`,
                  case: { inputs_sha256: `inputs-${test.id}` },
                  execution_policy: { max_turns: 24 },
                  aggregate: {
                    planned_runs: test.planned ?? test.runs?.length ?? 0,
                    observed_runs: test.runs?.length ?? 0,
                    deferred_runs:
                      (test.planned ?? test.runs?.length ?? 0) -
                      (test.runs?.length ?? 0),
                  },
                  runs: test.runs ?? [],
                },
              ],
            } as unknown as DashboardExecutionDetail['reports'][number]['report']),
    })),
  } as DashboardExecutionDetail
}

describe('primary execution metrics', () => {
  it('gives each test equal score weight after averaging its repetitions', () => {
    const result = buildPrimaryMetrics(
      detail([
        {
          id: 'two-runs',
          runs: [run('a', { score: 100 }), run('b', { score: 0 })],
        },
        { id: 'one-run', runs: [run('c', { score: 100 })] },
      ]),
    )

    expect(result.tests.map((test) => test.metrics.score.value)).toEqual([
      100, 50,
    ])
    expect(result.metrics.score).toEqual({
      value: 75,
      observed: 75,
      samples: 2,
      expected: 2,
    })
  })

  it('sums raw retry attempts once while trusting cumulative terminal time and cost', () => {
    const retry = run('logical', {
      input: 4,
      output: 1,
      cacheRead: 6,
      cacheWrite: 2,
      turns: 2,
      calls: 3,
      errors: 1,
      duration: 400,
      cost: 0.04,
    }) as unknown as DashboardRetryAttemptProjection
    const terminal = run('logical', {
      input: 10,
      output: 2,
      cacheRead: 20,
      cacheWrite: 3,
      turns: 4,
      calls: 5,
      errors: 0,
      duration: 1_400,
      cost: 0.14,
      retries: [retry],
    })
    const metrics = buildPrimaryMetrics(
      detail([{ id: 'retry', runs: [terminal] }]),
    ).metrics

    expect(metrics).toMatchObject({
      inputNormal: { value: 14 },
      cacheRead: { value: 26 },
      cacheWrite: { value: 5 },
      inputTokens: { value: 45 },
      outputTokens: { value: 3 },
      totalTokens: { value: 48 },
      turns: { value: 6 },
      functionCalls: { value: 8 },
      functionErrors: { value: 1 },
      durationMs: { value: 1_400 },
      costUsd: { value: 0.14 },
    })
  })

  it('keeps missing repetitions and telemetry in coverage without fabricating zero', () => {
    const result = buildPrimaryMetrics(
      detail([
        {
          id: 'partial',
          planned: 2,
          runs: [run('known', { cacheWrite: null, cost: null })],
        },
        { id: 'unavailable', available: false },
      ]),
    )

    const partial = result.tests.find((test) => test.label === 'partial')
    expect(partial?.metrics.score).toEqual({
      value: null,
      observed: 100,
      samples: 1,
      expected: 2,
    })
    expect(partial?.metrics.inputNormal).toEqual({
      value: null,
      observed: 10,
      samples: 1,
      expected: 2,
    })
    expect(partial?.metrics.inputTokens).toEqual({
      value: null,
      observed: null,
      samples: 0,
      expected: 2,
    })
    expect(result.metrics.score).toEqual({
      value: null,
      observed: null,
      samples: 0,
      expected: 2,
    })
    expect(result.metrics.costUsd.observed).toBeNull()
  })

  it('does not declare complete coverage when the planned scope is unknown', () => {
    const input = detail([{ id: 'unknown-scope', runs: [run('observed')] }])
    const scenario = input.reports[0].report?.scenarios[0]
    if (!scenario) throw new Error('fixture must contain a scenario')
    delete (scenario.aggregate as Partial<typeof scenario.aggregate>)
      .planned_runs

    const result = buildPrimaryMetrics(input)

    expect(result.tests[0].scopeKnown).toBe(false)
    expect(result.tests[0].metrics.score).toMatchObject({
      value: null,
      observed: 100,
      samples: 1,
      expected: 1,
    })
    expect(result.metrics.inputTokens.value).toBeNull()
  })

  it('keeps measurements visible but incomplete when aggregate counts disagree', () => {
    const input = detail([{ id: 'bad-counts', runs: [run('observed')] }])
    const scenario = input.reports[0].report?.scenarios[0]
    if (!scenario) throw new Error('fixture must contain a scenario')
    scenario.aggregate.observed_runs = 0

    const result = buildPrimaryMetrics(input)

    expect(result.tests[0].scopeKnown).toBe(false)
    expect(result.tests[0].metrics.score).toMatchObject({
      value: null,
      observed: 100,
    })
    expect(result.tests[0].metrics.inputNormal.observed).toBe(10)
  })

  it('deduplicates repeated native projections and logical runs', () => {
    const shared = run('same-run')
    const first = detail([
      { id: 'same-test', runs: [shared], nativeId: 'native-1' },
    ])
    first.reports.push(structuredClone(first.reports[0]))

    const result = buildPrimaryMetrics(first)

    expect(result.tests).toHaveLength(1)
    expect(result.tests[0].repetitions).toBe(1)
    expect(result.metrics.inputTokens.value).toBe(33)
  })

  it('rejects an unsafe aggregate instead of rounding an exact counter', () => {
    const result = buildPrimaryMetrics(
      detail([
        {
          id: 'overflow',
          runs: [
            run('large', {
              input: Number.MAX_SAFE_INTEGER,
              cacheRead: 0,
              cacheWrite: 0,
            }),
            run('one-more', { input: 1, cacheRead: 0, cacheWrite: 0 }),
          ],
        },
      ]),
    )

    expect(result.metrics.inputNormal.value).toBeNull()
    expect(result.metrics.inputNormal.observed).toBeNull()
  })
})

describe('primary metrics comparison', () => {
  it('filters zero and missing scores symmetrically and recalculates every metric', () => {
    const baseline = buildPrimaryMetrics(
      detail([
        { id: 'kept', runs: [run('a-kept', { score: 80, input: 10 })] },
        { id: 'zero', runs: [run('a-zero', { score: 70, input: 30 })] },
        { id: 'missing', runs: [run('a-missing', { score: 90, input: 50 })] },
      ]),
    )
    const candidate = buildPrimaryMetrics(
      detail([
        { id: 'kept', runs: [run('b-kept', { score: 90, input: 8 })] },
        { id: 'zero', runs: [run('b-zero', { score: 0, input: 15 })] },
        { id: 'missing', planned: 1, runs: [] },
      ]),
    )

    const unfiltered = comparePrimaryMetrics(baseline, candidate, false)
    expect(unfiltered.baseline.metrics.inputNormal.value).toBe(90)
    expect(unfiltered.candidate.metrics.score.value).toBeNull()
    expect(unfiltered.deltas.score).toBeNull()

    const filtered = comparePrimaryMetrics(baseline, candidate, true)
    expect(filtered).toMatchObject({ excluded: 2, totalTests: 3 })
    expect(filtered.baseline.tests.map((test) => test.label)).toEqual(['kept'])
    expect(filtered.candidate.tests.map((test) => test.label)).toEqual(['kept'])
    expect(filtered.baseline.metrics.inputNormal.value).toBe(10)
    expect(filtered.candidate.metrics.inputNormal.value).toBe(8)
    expect(filtered.deltas.score).toBe(10)
    expect(filtered.deltas.inputNormal).toBe(-2)
  })

  it('suppresses deltas for incompatible cases, repetitions, or partial metrics', () => {
    const baseline = buildPrimaryMetrics(
      detail([
        { id: 'changed-case', caseId: 'case-a', runs: [run('a-case')] },
        { id: 'changed-runs', runs: [run('a-run')] },
        { id: 'partial', runs: [run('a-partial')] },
      ]),
    )
    const candidate = buildPrimaryMetrics(
      detail([
        { id: 'changed-case', caseId: 'case-b', runs: [run('b-case')] },
        { id: 'changed-runs', runs: [run('b-run-1'), run('b-run-2')] },
        { id: 'partial', runs: [run('b-partial', { cacheRead: null })] },
      ]),
    )

    const result = comparePrimaryMetrics(baseline, candidate, false)

    expect(result.totalTests).toBe(3)
    expect(result.baseline.tests.map((test) => test.label)).toEqual([
      'changed-case',
      'changed-runs',
      'partial',
    ])
    expect(result.deltas.score).toBeNull()
    expect(result.deltas.inputTokens).toBeNull()
    expect(result.deltas.cacheRead).toBeNull()
  })

  it('requires explicit report and case identity for a controlled delta', () => {
    const baseline = detail([{ id: 'identity', runs: [run('a')] }])
    const candidate = detail([{ id: 'identity', runs: [run('b')] }])
    const report = candidate.reports[0].report
    if (!report) throw new Error('fixture must contain a report')
    report.scoring_profile_sha256 = ''

    const result = comparePrimaryMetrics(
      buildPrimaryMetrics(baseline),
      buildPrimaryMetrics(candidate),
      false,
    )

    expect(result.baseline.metrics.score.value).toBe(100)
    expect(result.candidate.metrics.score.value).toBe(100)
    expect(result.deltas.score).toBeNull()
  })
})
