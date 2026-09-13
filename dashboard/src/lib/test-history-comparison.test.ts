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
    case_id: 'direct_answer:seed-0000000000000001',
    contract_sha256: 'contract',
    assessment_profile_sha256: 'assessment',
    status: 'passed',
    median_score: 80,
    run_count: 1,
    scored_runs: 1,
    behavior_sha256:
      'sha256:a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1',
    seed: 1,
    stack_mode: 'source',
    subject_provider: 'openai',
    subject_model: 'gpt-5',
    median_cost_usd: 0.5,
    median_tokens: 1000,
    median_duration_seconds: 20,
    median_function_calls: 4,
    median_function_call_errors: 0,
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
      median_function_calls: 6,
      median_function_call_errors: 1,
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
    expect(result.metrics.functionCalls.delta).toBe(2)
    expect(result.metrics.functionErrors.delta).toBe(1)
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

  // Point 7 of the definition-digest contract: a Release Control ledger that
  // carries neither digest is not comparable, not an error.
  it('settles the scenario by contract digest, then by definition digest', () => {
    const withoutContract = (overrides = {}) =>
      observation({ contract_sha256: '', ...overrides })

    expect(
      compareTestObservations(
        withoutContract(),
        withoutContract({ execution_id: 'candidate' }),
      ).compatible,
    ).toBe(true)

    expect(
      compareTestObservations(
        withoutContract(),
        withoutContract({
          execution_id: 'candidate',
          behavior_sha256:
            'sha256:c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3',
        }),
      ).reasons,
    ).toEqual(['Scenario definition differs'])

    const reference = compareTestObservations(
      withoutContract({
        execution_id: 'rc:reference',
        source: 'release-control',
        behavior_sha256: '',
      }),
      withoutContract({ execution_id: 'local-candidate', source: 'local' }),
    )
    expect(reference.compatible).toBe(false)
    expect(reference.reasons).toEqual([
      'Scenario definition is not recorded on both executions',
    ])
    expect(reference.metrics.score.delta).toBe(0)
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
    expect(result.metrics.functionCalls).toEqual({
      baseline: 4,
      candidate: 4,
      delta: 0,
      relativeDelta: 0,
    })
    expect(testObservationKey(baseline)).not.toBe(testObservationKey(candidate))
  })
})
