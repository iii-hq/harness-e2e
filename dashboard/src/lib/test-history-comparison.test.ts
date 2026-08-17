import { describe, expect, it } from 'vitest'
import type { TestObservation } from '@/lib/test-catalog'
import {
  compareTestObservations,
  testObservationKey,
} from '@/lib/test-history-comparison'

function observation(
  overrides: Partial<TestObservation> = {},
): TestObservation {
  return {
    execution_id: 'baseline',
    evaluated_version_id: 'system-a',
    cohort_id: 'same-cohort',
    completed_at: '2026-08-17T10:00:00Z',
    case_id: 'direct_answer:v2:seed-1',
    contract_sha256: 'contract',
    assessment_profile_sha256: 'assessment',
    analyzer_profile_sha256: 'analyzer',
    status: 'passed',
    median_score: 80,
    run_count: 1,
    scored_runs: 1,
    scenario_version: 2,
    seed: 1,
    stack_mode: 'source',
    subject_provider: 'openai',
    subject_model: 'gpt-5',
    judge_provider: 'openai',
    judge_model: 'gpt-5-judge',
    judge_protocol: 'assessment-json',
    median_cost_usd: 0.5,
    median_tokens: 1000,
    median_duration_seconds: 20,
    median_turns: 2,
    ...overrides,
  }
}

describe('test history execution comparison', () => {
  it('keeps a same-scope candidate comparable while the system changes', () => {
    const baseline = observation()
    const candidate = observation({
      execution_id: 'candidate',
      evaluated_version_id: 'system-b',
      median_cost_usd: 0.6,
      median_duration_seconds: 15,
    })

    const result = compareTestObservations(baseline, candidate)

    expect(result.compatible).toBe(true)
    expect(result.reasons).toEqual([])
    expect(result.metrics.cost.baseline).toBe(0.5)
    expect(result.metrics.cost.candidate).toBe(0.6)
    expect(result.metrics.cost.delta).toBeCloseTo(0.1)
    expect(result.metrics.cost.relativeDelta).toBeCloseTo(0.2)
    expect(result.metrics.duration.delta).toBe(-5)
    expect(result.metrics.duration.relativeDelta).toBe(-0.25)
  })

  it('does not compute a compatible result when the evidence boundary differs', () => {
    const result = compareTestObservations(
      observation(),
      observation({
        execution_id: 'candidate',
        seed: 2,
        contract_sha256: 'changed-contract',
        cohort_id: 'different-cohort',
      }),
    )

    expect(result.compatible).toBe(false)
    expect(result.reasons).toEqual(
      expect.arrayContaining([
        'Seed differs',
        'Scenario contract differs',
        'Cohort differs',
      ]),
    )
  })

  it('keeps missing metrics unknown and makes the selected observation key stable', () => {
    const baseline = observation({ median_tokens: null })
    const candidate = observation({
      execution_id: 'candidate',
      median_tokens: 5,
    })
    const result = compareTestObservations(baseline, candidate)

    expect(result.metrics.tokens).toEqual({
      baseline: null,
      candidate: 5,
      delta: null,
      relativeDelta: null,
    })
    expect(testObservationKey(baseline)).not.toBe(testObservationKey(candidate))
  })
})
