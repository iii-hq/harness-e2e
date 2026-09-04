import { describe, expect, it, vi } from 'vitest'
import type { AssessmentSummary } from '@/lib/assessment-contract'
import type {
  DashboardExecutionDetail,
  DashboardExecutionSummary,
} from '@/lib/dashboard-data-source'
import {
  buildPlanComparison,
  buildScenarioComparisons,
  loadExecutionSummaries,
  metricById,
} from '@/lib/plan-comparison'

function assessment(
  overrides: Partial<AssessmentSummary> = {},
): AssessmentSummary {
  return {
    run_count: 2,
    assessment_count: 4,
    asset_count: 0,
    evidence_reference_count: 2,
    system_statuses: {
      unavailable: 0,
      passed: 2,
      passed_with_concerns: 0,
      hard_gate_failed: 0,
      subject_error: 0,
      judge_error: 0,
      resource_limit: 0,
      infrastructure_error: 0,
    },
    effective_statuses: {} as never,
    assessment_outcomes: {} as never,
    asset_qualitative_outcomes: {} as never,
    asset_validation_outcomes: {} as never,
    ai_availability: {} as never,
    ai_verdicts: {} as never,
    median_quality_score: 98,
    median_confidence: 0.97,
    ...overrides,
  }
}

function execution(
  id: string,
  overrides: Partial<DashboardExecutionSummary> = {},
): DashboardExecutionSummary {
  return {
    id,
    label: id,
    status: 'passed',
    availability: 'full',
    subjects: [],
    totals: {
      scenario_pass_rate: 100,
      report_coverage: 100,
      hard_gate_failures: 0,
      technical_failures: 0,
      total_tokens: 10_000,
      wall_time_seconds: 20,
      total_cost_usd: null,
      function_calls: 4,
      function_call_errors: 0,
    },
    assessment_summary: assessment(),
    ...overrides,
  }
}

describe('local plan comparison view model', () => {
  it('does not calculate deltas from unsupported historical contracts', () => {
    const comparison = buildPlanComparison(
      execution('legacy', {
        status: 'unsupported',
        availability: 'unsupported',
      }),
      execution('current'),
    )
    expect(comparison.verdict).toBe('inconclusive')
    expect(comparison.headline).toBe('Result contracts are incompatible')
    expect(comparison.metrics).toEqual([])
    expect(comparison.scenarios).toEqual([])
  })

  it('keeps objective stability separate from directional efficiency', () => {
    const comparison = buildPlanComparison(
      execution('baseline'),
      execution('candidate', {
        totals: {
          ...execution('candidate').totals,
          total_tokens: 11_000,
          wall_time_seconds: 18,
        },
      }),
    )

    expect(comparison.verdict).toBe('stable')
    expect(metricById(comparison, 'tokens')).toMatchObject({
      delta: 1000,
      delta_percent: 10,
      tone: 'negative',
    })
    expect(metricById(comparison, 'duration')).toMatchObject({
      delta: -2,
      delta_percent: -10,
      tone: 'positive',
    })
  })

  it('makes an added hard-gate failure an objective regression', () => {
    const candidate = execution('candidate', {
      totals: {
        ...execution('candidate').totals,
        hard_gate_failures: 1,
        scenario_pass_rate: 50,
      },
      assessment_summary: assessment({
        system_statuses: {
          ...assessment().system_statuses,
          passed: 1,
          hard_gate_failed: 1,
        },
      }),
    })

    expect(buildPlanComparison(execution('baseline'), candidate)).toMatchObject(
      {
        verdict: 'regressed',
        headline: 'Objective regression detected',
      },
    )
  })

  it('does not mislabel technical failure as subject regression', () => {
    const candidate = execution('candidate', {
      totals: {
        ...execution('candidate').totals,
        technical_failures: 1,
      },
    })
    expect(buildPlanComparison(execution('baseline'), candidate).verdict).toBe(
      'inconclusive',
    )
  })

  it('preserves missing cost as unavailable instead of zero', () => {
    const comparison = buildPlanComparison(
      execution('baseline'),
      execution('candidate'),
    )
    expect(metricById(comparison, 'cost')).toMatchObject({
      baseline: null,
      candidate: null,
      delta: null,
      tone: 'unavailable',
    })
  })

  it('makes missing objective evidence inconclusive instead of assuming stability', () => {
    const candidate = execution('candidate', {
      totals: {
        ...execution('candidate').totals,
        scenario_pass_rate: null,
      },
    })

    expect(buildPlanComparison(execution('baseline'), candidate).verdict).toBe(
      'inconclusive',
    )
  })

  it('derives fully reported function errors from retained scenario metrics', () => {
    const comparison = buildPlanComparison(
      execution('baseline', {
        totals: {
          ...execution('baseline').totals,
          function_call_errors: null,
        },
        scenario_metrics: [
          {
            scenario_id: 'direct_answer',
            run_count: 2,
            averages: { function_call_errors: 0.5 },
            samples: { function_call_errors: 2 },
          },
        ],
      }),
      execution('candidate', {
        totals: {
          ...execution('candidate').totals,
          function_call_errors: null,
        },
        scenario_metrics: [
          {
            scenario_id: 'direct_answer',
            run_count: 2,
            averages: { function_call_errors: 0 },
            samples: { function_call_errors: 2 },
          },
        ],
      }),
    )

    expect(metricById(comparison, 'function_errors')).toMatchObject({
      baseline: 1,
      candidate: 0,
      delta: -1,
      tone: 'positive',
    })
  })

  it('derives execution and per-test turns from retained scenario metrics', () => {
    const retained = (id: string, turns: number): DashboardExecutionSummary =>
      execution(id, {
        subjects: [
          {
            id: 'subject',
            scenarios: [
              {
                id: 'direct_answer',
                scenario_version: 1,
                pass_rate: 100,
                assessment_summary: assessment(),
              },
            ],
          },
        ],
        scenario_metrics: [
          {
            scenario_id: 'direct_answer',
            scenario_version: 1,
            run_count: 2,
            averages: { turns },
            samples: { turns: 2 },
          },
        ],
      })
    const baseline = retained('baseline', 2)
    const candidate = retained('candidate', 1)
    const comparison = buildPlanComparison(baseline, candidate)

    expect(metricById(comparison, 'turns')).toMatchObject({
      baseline: 4,
      candidate: 2,
      delta: -2,
      tone: 'positive',
    })
    expect(comparison.scenarios[0]?.metrics).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          id: 'turns',
          baseline: 2,
          candidate: 1,
          tone: 'positive',
        }),
      ]),
    )
  })

  it('derives security calls and errors from retained workflow evidence', () => {
    const retained = (id: string): DashboardExecutionSummary =>
      execution(id, {
        totals: {
          ...execution(id).totals,
          function_calls: null,
          function_call_errors: null,
        },
        scenario_metrics: [
          {
            scenario_id: 'security_review',
            run_count: 1,
            averages: {
              function_calls: null,
              function_call_errors: null,
            },
            samples: {
              function_calls: 0,
              function_call_errors: 0,
            },
            workflow: {
              failure_count: 0,
              numeric_metrics: {
                request_count: 4,
                'poll.poll_count': 3,
                reconciliation_operations: 4,
              },
            },
          },
        ],
      })

    const comparison = buildPlanComparison(
      retained('baseline'),
      retained('candidate'),
    )

    expect(metricById(comparison, 'function_calls')).toMatchObject({
      baseline: 13,
      candidate: 13,
      delta: 0,
    })
    expect(metricById(comparison, 'function_errors')).toMatchObject({
      baseline: 0,
      candidate: 0,
      delta: 0,
    })
  })

  it('adds derived security calls to natively reported execution calls', () => {
    const retained = execution('baseline', {
      totals: {
        ...execution('baseline').totals,
        function_calls: 3,
        function_call_errors: 0,
      },
      scenario_metrics: [
        {
          scenario_id: 'security_review',
          run_count: 1,
          averages: {
            function_calls: null,
            function_call_errors: null,
          },
          workflow: {
            failure_count: 0,
            numeric_metrics: {
              request_count: 4,
              'poll.poll_count': 3,
              reconciliation_operations: 4,
            },
          },
        },
      ],
    })

    expect(buildPlanComparison(retained, retained).metrics).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ id: 'function_calls', baseline: 16 }),
        expect.objectContaining({ id: 'function_errors', baseline: 0 }),
      ]),
    )
  })

  it('disables per-test deltas for different retained contracts', () => {
    const detail = (
      id: string,
      contract: string,
    ): DashboardExecutionDetail => ({
      ...execution(id),
      reports: [],
      subjects: [
        {
          id: 'subject',
          scenarios: [
            {
              id: 'direct_answer',
              scenario_version: 2,
              pass_rate: 100,
              assessment_summary: assessment(),
            },
          ],
        },
      ],
      scenario_metrics: [
        {
          scenario_id: 'direct_answer',
          scenario_version: 2,
          contract_fingerprint: contract,
          averages: { tokens: 1000, duration_seconds: 4 },
        },
      ],
    })
    const [row] = buildScenarioComparisons(
      detail('baseline', 'contract-a'),
      detail('candidate', 'contract-b'),
    )

    expect(row.compatible).toBe(false)
    expect(row.reason).toMatch(/contract differs/i)
    expect(row.metrics.every((metric) => metric.delta === null)).toBe(true)
  })

  it('compares backfilled general metrics for security review', () => {
    const detail = (id: string, outputTokens: number) =>
      ({
        ...execution(id),
        reports: [
          {
            subject_id: 'subject',
            scenario_id: 'security_review',
            available: true,
            report: {
              scenarios: [
                {
                  scenario_id: 'security_review',
                  scenario_version: 3,
                  runs: [
                    {
                      run_id: `${id}-run`,
                      attempt_id: `${id}-attempt`,
                      judge_usage: {
                        input_tokens: 3_000,
                        output_tokens: outputTokens,
                      },
                      semantic_tests: [
                        {
                          node_id: 'scan_commit_a',
                          status: 'succeeded',
                          duration_ms: 10,
                          metrics: {
                            request_count: 4,
                            poll: { poll_count: 3 },
                            reconciliation_operations: 4,
                          },
                          failures: [],
                        },
                        {
                          node_id: 'list_run_history',
                          status: 'succeeded',
                          duration_ms: 1,
                          failures: [],
                        },
                      ],
                    },
                  ],
                },
              ],
            },
          },
        ],
        subjects: [
          {
            id: 'subject',
            scenarios: [
              {
                id: 'security_review',
                scenario_version: 3,
                pass_rate: 100,
                assessment_summary: assessment(),
              },
            ],
          },
        ],
        scenario_metrics: [
          {
            scenario_id: 'security_review',
            scenario_version: 3,
            contract_fingerprint: 'security-v3',
          },
        ],
      }) as unknown as DashboardExecutionDetail
    const [row] = buildScenarioComparisons(
      detail('baseline', 500),
      detail('candidate', 400),
    )

    expect(row.execution_metrics).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          id: 'tokens',
          baseline: 3500,
          candidate: 3400,
          delta: -100,
          format: 'tokens',
        }),
        expect.objectContaining({
          id: 'cost',
          baseline: 0,
          candidate: 0,
          format: 'usd',
        }),
        expect.objectContaining({
          id: 'function_calls',
          baseline: 13,
          candidate: 13,
        }),
        expect.objectContaining({
          id: 'function_errors',
          baseline: 0,
          candidate: 0,
        }),
      ]),
    )
  })

  it('loads referenced summaries in bounded batches', async () => {
    const ids = Array.from({ length: 205 }, (_, index) => `execution-${index}`)
    const list = vi.fn(async ({ ids: batch }: { ids: string[] }) => ({
      executions: batch.map((id) => execution(id)),
    }))

    const result = await loadExecutionSummaries(list, ids)

    expect(list).toHaveBeenCalledTimes(3)
    expect(Object.keys(result)).toHaveLength(205)
  })
})
