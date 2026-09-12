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
      hard_gate_failed: 0,
      subject_error: 0,
      judge_error: 0,
      resource_limit: 0,
      infrastructure_error: 0,
    },
    assessment_outcomes: {} as never,
    asset_validation_outcomes: {} as never,
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
  it('reports signed consumption changes without declaring a winner', () => {
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

    expect(comparison).not.toHaveProperty('verdict')
    expect(metricById(comparison, 'tokens')).toMatchObject({
      delta: 1000,
      delta_percent: 10,
      tone: 'neutral',
    })
    expect(metricById(comparison, 'duration')).toMatchObject({
      delta: -2,
      delta_percent: -10,
      tone: 'neutral',
    })
  })

  it('retains evidence without turning a hard-gate failure into a global verdict', () => {
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
        headline: 'Retained observations',
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
    expect(
      buildPlanComparison(execution('baseline'), candidate),
    ).not.toHaveProperty('verdict')
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

  it('does not assume stability when objective evidence is missing', () => {
    const candidate = execution('candidate', {
      totals: {
        ...execution('candidate').totals,
        scenario_pass_rate: null,
      },
    })

    expect(
      buildPlanComparison(execution('baseline'), candidate),
    ).not.toHaveProperty('verdict')
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
      tone: 'neutral',
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
      tone: 'neutral',
    })
    expect(comparison.scenarios[0]?.metrics).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          id: 'turns',
          baseline: 2,
          candidate: 1,
          tone: 'neutral',
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

  it('does not invent security-review consumption from step diagnostics', () => {
    const detail = (id: string) =>
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
      detail('baseline'),
      detail('candidate'),
    )

    for (const metric of row.execution_metrics) {
      expect(metric.baseline).toBeNull()
      expect(metric.candidate).toBeNull()
      expect(metric.delta).toBeNull()
    }
  })

  // The two token metrics #88 added to the execution page reach the plan
  // through the summary totals (pooled) and the per-scenario averages.
  it('compares failed-attempt tokens and tokens per completion when reported', () => {
    const withTokens = (id: string, perCompletion: number, failed: number) =>
      execution(id, {
        totals: {
          ...execution(id).totals,
          tokens_per_completion: perCompletion,
          failed_attempt_tokens: failed,
        },
        scenario_metrics: [
          {
            scenario_id: 'minimal_path',
            scenario_version: 2,
            contract_fingerprint: 'minimal-v2',
            run_count: 1,
            averages: {
              tokens: perCompletion,
              tokens_per_completion: perCompletion,
              failed_attempt_tokens: failed,
            },
          },
        ],
        subjects: [
          {
            id: 'subject',
            scenarios: [{ id: 'minimal_path', scenario_version: 2 }],
          },
        ] as never,
      })
    const comparison = buildPlanComparison(
      withTokens('baseline-1', 5_000, 1_500),
      withTokens('candidate-1', 4_000, 0),
    )

    expect(metricById(comparison, 'tokens_per_completion')).toMatchObject({
      label: 'Tokens per completion',
      baseline: 5_000,
      candidate: 4_000,
      delta: -1_000,

      format: 'tokens',
      tone: 'neutral',
    })
    expect(metricById(comparison, 'failed_attempt_tokens')).toMatchObject({
      label: 'Failed attempt tokens',
      baseline: 1_500,
      candidate: 0,
      tone: 'neutral',
    })
    const scenario = comparison.scenarios.find(
      (entry) => entry.id === 'minimal_path',
    )
    expect(
      scenario?.metrics.find((metric) => metric.id === 'failed_attempt_tokens'),
    ).toMatchObject({ baseline: 1_500, candidate: 0, tone: 'neutral' })
    expect(
      scenario?.metrics.find((metric) => metric.id === 'tokens_per_completion'),
    ).toMatchObject({ baseline: 5_000, candidate: 4_000 })

    // Absent totals stay unavailable, never zero.
    const bare = buildPlanComparison(
      execution('baseline-1'),
      execution('candidate-1'),
    )
    expect(metricById(bare, 'failed_attempt_tokens')).toMatchObject({
      baseline: null,
      candidate: null,
      tone: 'unavailable',
    })
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

describe('retained criterion points', () => {
  function scored(awards: Array<number | null>, possible = 40) {
    return {
      ...execution('scored'),
      subjects: [
        {
          id: 'model',
          scenarios: [
            {
              id: 'test',
              scenario_version: 1,
              passed: false,
              runs: awards.length,
            },
          ],
        },
      ],
      reports: [
        {
          subject_id: 'model',
          scenario_id: 'test',
          available: true,
          report: {
            result_contract_sha256: 'results-contract',
            scoring_profile_sha256: 'scoring-contract',
            subject: { model: 'subject', provider: 'provider' },
            judge: { model: 'judge', provider: 'provider' },
            scenarios: [
              {
                scenario_id: 'test',
                case_id: 'test-case',
                case: { inputs_sha256: 'inputs' },
                execution_policy: { max_turns: 24 },
                aggregate: { planned_runs: awards.length },
                runs: awards.map((awarded, index) => ({
                  run_id: `run-${index}`,
                  technical: 'valid',
                  status: 'passed',
                  objective_score: awarded,
                  criteria: [{ id: 'delivery', possible, awarded }],
                })),
              },
            ],
          },
        },
      ],
    } as unknown as DashboardExecutionDetail
  }

  it('shows partial criterion points and preserves them in each run score', () => {
    const left = scored([10, 20])
    const right = scored([20, 40])
    const comparison = buildPlanComparison(left, right)
    expect(
      comparison.scenarios[0].metrics.find(
        (metric) => metric.id === 'criterion:delivery:40',
      ),
    ).toMatchObject({ baseline: 15, candidate: 30, delta: 15 })
    expect(comparison.scenarios[0]).not.toHaveProperty('candidate_status')
    expect(comparison.metrics.map((metric) => metric.id)).not.toEqual(
      expect.arrayContaining(['pass_rate', 'hard_gates']),
    )
    expect(
      comparison.metrics.some((metric) => metric.id.startsWith('criterion:')),
    ).toBe(false)
    expect(right.reports[0].report?.scenarios[0].runs[0].objective_score).toBe(
      20,
    )
  })

  it('uses only matched repetitions for deltas while displaying every measured point', () => {
    const result = buildScenarioComparisons(
      scored([0, 30]),
      scored([20, null]),
    )[0].metrics.find((metric) => metric.id === 'criterion:delivery:40')
    expect(result).toMatchObject({
      baseline: 15,
      candidate: 20,
      delta: 20,
      tone: 'neutral',
      evidence: {
        baseline_observed: 2,
        candidate_observed: 1,
        baseline_planned: 2,
        candidate_planned: 2,
        paired: 1,
        paired_baseline: 0,
        paired_candidate: 20,
      },
    })
  })

  it.each([
    [null, null, 1],
    [null, { model: 'judge', provider: 'provider' }, 0],
    [null, undefined, 0],
    [undefined, undefined, 0],
    [null, {}, 0],
  ])(
    'pairs explicit judge identity %j / %j with %i repetitions',
    (leftJudge, rightJudge, paired) => {
      const left = scored([25])
      const right = scored([0])
      Object.assign(left.reports[0].report!, { judge: leftJudge })
      Object.assign(right.reports[0].report!, { judge: rightJudge })

      expect(
        buildScenarioComparisons(left, right)[0].metrics.find(
          (metric) => metric.id === 'criterion:delivery:40',
        ),
      ).toMatchObject({
        baseline: 25,
        candidate: 0,
        delta: paired ? -25 : null,
        evidence: { paired },
      })
    },
  )

  it('accepts compact plan execution summaries without slots', () => {
    const baseline = scored([10, 20])
    const candidate = scored([20, 40])
    Object.assign(baseline, { plan_execution: { planned: 2 } })
    Object.assign(candidate, { plan_execution: { planned: 2 } })

    const result = buildScenarioComparisons(
      baseline,
      candidate,
    )[0].metrics.find((metric) => metric.id === 'criterion:delivery:40')

    expect(result).toMatchObject({
      baseline: 15,
      candidate: 30,
      evidence: { baseline_planned: 2, candidate_planned: 2 },
    })
  })

  it('pairs explicit rounds when an earlier child report is unavailable', () => {
    const left = scored([0, 30])
    const right = scored([20, 10])
    for (const detail of [left, right]) {
      const record = detail.reports[0]
      const report = record.report
      if (!report) throw new Error('Missing fixture')
      const scenario = report.scenarios[0]
      detail.reports = scenario.runs.map((run, index) => ({
        ...record,
        round: index + 1,
        report: {
          ...report,
          scenarios: [
            {
              ...scenario,
              aggregate: { ...scenario.aggregate, planned_runs: 1 },
              runs: [run],
            },
          ],
        },
      }))
    }
    right.reports[0].available = false
    right.reports.reverse()
    const result = buildScenarioComparisons(left, right)[0].metrics.find(
      (metric) => metric.id === 'criterion:delivery:40',
    )
    expect(result).toMatchObject({
      baseline: 15,
      candidate: 10,
      delta: -20,
      evidence: { paired: 1, paired_baseline: 30, paired_candidate: 10 },
    })
  })

  it('preserves measured points from incomplete tasks but never pairs different cases or budgets', () => {
    const right = scored([20, 30])
    const report = right.reports[0].report
    if (!report) throw new Error('Missing fixture')
    const scenario = report.scenarios[0]
    scenario.runs[0].completion = 'task_incomplete'
    scenario.runs[0].status = 'resource_limit'
    expect(
      buildScenarioComparisons(scored([10, 20]), right)[0].metrics.find(
        (metric) => metric.id === 'criterion:delivery:40',
      ),
    ).toMatchObject({ candidate: 25, delta: 10 })
    scenario.execution_policy = { max_turns: 48 }
    expect(
      buildScenarioComparisons(scored([10, 20]), right)[0].metrics.find(
        (metric) => metric.id === 'criterion:delivery:40',
      ),
    ).toMatchObject({ candidate: 25, delta: null })
    scenario.execution_policy = { max_turns: 24 }
    scenario.case = { inputs_sha256: 'different-inputs' }
    expect(
      buildScenarioComparisons(scored([10, 20]), right)[0].metrics.find(
        (metric) => metric.id === 'criterion:delivery:40',
      ),
    ).toMatchObject({ candidate: 25, delta: null })
  })

  it('keeps points visible without pairing changed weights or unknown identity', () => {
    const right = scored([20, 30], 50)
    expect(
      buildScenarioComparisons(scored([10, 20]), right)[0].metrics.find(
        (metric) => metric.id === 'criterion:delivery:40',
      )?.delta,
    ).toBeNull()
    const report = right.reports[0].report
    if (!report) throw new Error('Missing fixture')
    delete report.subject
    const result = buildScenarioComparisons(
      scored([10, 20], 50),
      right,
    )[0].metrics.find((metric) => metric.id === 'criterion:delivery:50')
    expect(result).toMatchObject({
      candidate: 25,
      delta: null,
      evidence: { paired: 0 },
    })
  })

  it('excludes invalid and duplicate evidence without discarding independent valid observations', () => {
    const right = scored([20, 30])
    const report = right.reports[0].report
    if (!report) throw new Error('Missing fixture')
    report.scenarios[0].runs[0].technical = 'technical_invalid'
    let result = buildScenarioComparisons(
      scored([10, 20]),
      right,
    )[0].metrics.find((metric) => metric.id === 'criterion:delivery:40')
    expect(result).toMatchObject({
      candidate: 30,
      delta: 10,
      evidence: { candidate_observed: 1, paired: 1 },
    })
    right.reports.push(right.reports[0])
    result = buildScenarioComparisons(scored([10, 20]), right)[0].metrics.find(
      (metric) => metric.id === 'criterion:delivery:40',
    )
    expect(result).toMatchObject({
      candidate: null,
      delta: null,
      evidence: { candidate_observed: 0 },
    })
  })
})
