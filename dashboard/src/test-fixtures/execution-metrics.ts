import type {
  DashboardExecutionDetail,
  DashboardRunProjection,
} from '@/lib/dashboard-data-source'
import {
  RESULT_CONTRACT_SHA256,
  RESULTS_SCHEMA_VERSION,
  SCORING_PROFILE_SHA256,
} from '@/lib/result-contract.generated'

export function metricRun(
  id: string,
  tokens: number | null,
  overrides: Partial<DashboardRunProjection> = {},
): DashboardRunProjection {
  return {
    run_id: id,
    attempt_id: `${id}-attempt`,
    status: 'passed',
    completion: 'completed',
    technical: 'valid',
    evaluators: {
      completion: 'not_required',
      quality: 'available',
      final_advisory: 'not_required',
    },
    objective_score: 100,
    quality_score_completed: 80,
    assessment: {} as DashboardRunProjection['assessment'],
    efficiency: {
      total_tokens: tokens,
      function_calls: 10,
      function_call_errors: 0,
    },
    judge_usage: { input_tokens: 30, output_tokens: 10 },
    cost: { total_usd: 0.1 },
    wall_time_ms: 1_000,
    retry_attempts: [],
    ...overrides,
  }
}

export function executionMetricsFixture(
  groups: Array<{ runs: DashboardRunProjection[]; deferred?: number }>,
): DashboardExecutionDetail {
  return {
    id: 'execution-1',
    status: 'inconclusive',
    availability: 'full',
    subjects: [],
    reports: groups.map(({ runs, deferred = 0 }, index) => {
      const completed = runs.filter((run) => run.completion === 'completed')
      const incomplete = runs.filter(
        (run) => run.completion === 'task_incomplete',
      ).length
      const valid = runs.filter((run) => run.technical === 'valid').length
      const scored = runs.filter((run) => run.objective_score !== null).length
      const quality = completed.filter(
        (run) => run.quality_score_completed !== null,
      ).length
      return {
        subject_id: 'subject',
        scenario_id: `scenario-${index}`,
        available: true,
        report: {
          schema_version: RESULTS_SCHEMA_VERSION,
          result_contract_sha256: RESULT_CONTRACT_SHA256,
          scoring_profile_sha256: SCORING_PROFILE_SHA256,
          report_state: deferred ? 'partial' : 'complete',
          objective_outcome: 'inconclusive',
          assessment_contract: { runs: [] },
          assessment_summary: {},
          scenarios: [
            {
              scenario_id: `scenario-${index}`,
              scenario_version: 1,
              case_id: `case-${index}`,
              runs,
              aggregate: {
                planned_runs: runs.length + deferred,
                observed_runs: runs.length,
                deferred_runs: deferred,
                completed_runs: completed.length,
                task_incomplete_runs: incomplete,
                undetermined_runs: runs.length - completed.length - incomplete,
                technical_valid_runs: valid,
                technical_invalid_runs: runs.length - valid,
                objective_scored_runs: scored,
                quality_scored_completed_runs: quality,
                execution_reliability: null,
                completion_evidence_coverage: null,
                completion_rate: null,
                objective_median_score: null,
                objective_score_coverage: null,
                quality_score_completed: null,
                quality_coverage: null,
                total_tokens_consumed: null,
                judge_tokens_consumed: null,
                tokens_completed_p50: null,
                failed_attempt_tokens: null,
                tokens_per_completion: null,
                hard_gate_failures: 0,
                technical_failures: runs.length - valid,
              },
            },
          ],
        },
      }
    }),
  } as unknown as DashboardExecutionDetail
}
