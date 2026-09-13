import type {
  DashboardExecutionDetail,
  DashboardRunProjection,
} from '@/lib/dashboard-data-source'
import { RESULT_CONTRACT_SHA256 } from '@/lib/result-contract.generated'

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
    },
    score: 100,
    assessment: {} as DashboardRunProjection['assessment'],
    efficiency: {
      total_tokens: tokens,
      function_calls: 10,
      function_call_errors: 0,
    },
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
      const scores = runs
        .filter((run) => run.technical === 'valid')
        .map((run) => run.score)
        .filter((score): score is number => score !== null)
      return {
        subject_id: 'subject',
        scenario_id: `scenario-${index}`,
        available: true,
        report: {
          result_contract_sha256: RESULT_CONTRACT_SHA256,
          report_state: deferred ? 'partial' : 'complete',
          objective_outcome: 'inconclusive',
          assessment_contract: { runs: [] },
          assessment_summary: {},
          scenarios: [
            {
              scenario_id: `scenario-${index}`,
              behavior_sha256:
                'sha256:a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1',
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
                scored_runs: scores.length,
                execution_reliability: null,
                completion_evidence_coverage: null,
                completion_rate: null,
                mean_score:
                  scores.length === 0
                    ? null
                    : scores.reduce((total, score) => total + score, 0) /
                      scores.length,
                total_tokens_consumed: null,
                tokens_completed_p50: null,
                failed_attempt_tokens: null,
                tokens_per_completion: null,
                technical_failures: runs.length - valid,
              },
            },
          ],
        },
      }
    }),
  } as unknown as DashboardExecutionDetail
}
