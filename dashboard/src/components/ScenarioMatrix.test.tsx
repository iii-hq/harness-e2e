import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { ScenarioMatrix } from '@/components/ScenarioMatrix'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import {
  RESULT_CONTRACT_SHA256,
  RESULTS_SCHEMA_VERSION,
  SCORING_PROFILE_SHA256,
} from '@/lib/result-contract.generated'

const resultContract = {
  schema_version: RESULTS_SCHEMA_VERSION,
  result_contract_sha256: RESULT_CONTRACT_SHA256,
  scoring_profile_sha256: SCORING_PROFILE_SHA256,
  report_state: 'complete' as const,
  objective_outcome: 'passed' as const,
}

function aggregate(overrides: Record<string, unknown> = {}) {
  return {
    planned_runs: 1,
    observed_runs: 1,
    deferred_runs: 0,
    completed_runs: 1,
    task_incomplete_runs: 0,
    undetermined_runs: 0,
    technical_valid_runs: 1,
    technical_invalid_runs: 0,
    execution_reliability: 1,
    completion_evidence_coverage: 1,
    completion_rate: 1,
    objective_scored_runs: 1,
    objective_median_score: 100,
    objective_score_coverage: 1,
    quality_scored_completed_runs: 1,
    quality_score_completed: 88,
    quality_coverage: 1,
    total_tokens_consumed: 1200,
    judge_tokens_consumed: 100,
    tokens_completed_p50: 1200,
    failed_attempt_tokens: 0,
    tokens_per_completion: 1200,
    hard_gate_failures: 0,
    technical_failures: 0,
    ...overrides,
  }
}

const detail = {
  id: 'execution-1',
  status: 'failed',
  subjects: [{ id: 'terra', scenarios: [] }],
  reports: [
    {
      subject_id: 'terra',
      scenario_id: 'security_review',
      available: true,
      report: {
        ...resultContract,
        assessment_contract: { runs: [] },
        assessment_summary: {},
        scenarios: [
          {
            scenario_id: 'security_review',
            scenario_version: 2,
            passed: true,
            aggregate: aggregate(),
            runs: [
              {
                run_id: 'run-security',
                attempt_id: 'attempt-security',
                status: 'passed',
                completion: 'completed',
                technical: 'valid',
                evaluators: {
                  completion: 'available',
                  quality: 'available',
                  final_advisory: 'available',
                },
                objective_score: 100,
                quality_score_completed: 88,
                wall_time_ms: 3_000,
                assessment: {
                  run_id: 'run-security',
                  attempt_id: 'attempt-security',
                  system_status: 'passed',
                  effective_status: 'passed_with_concerns',
                  assessments: [],
                  ai_final_assessment: {
                    availability: 'available',
                    result: { verdict: 'pass_with_concerns' },
                  },
                },
                semantic_tests: [
                  {
                    node_id: 'scan',
                    step_type: 'security.scan',
                    step_version: 1,
                    required: true,
                    dependencies: [],
                    status: 'succeeded',
                    duration_ms: 2_000,
                    cost_usd: 0.0123,
                    metrics: {
                      totals: {
                        input_tokens: 900,
                        output_tokens: 100,
                        function_calls: 3,
                        function_call_errors: 0,
                      },
                      finding_count: 5,
                    },
                    hard_gates: [],
                  },
                  {
                    node_id: 'report',
                    step_type: 'security.report',
                    step_version: 1,
                    required: true,
                    dependencies: ['scan'],
                    status: 'succeeded',
                    duration_ms: 1_000,
                    metrics: null,
                    hard_gates: [],
                  },
                ],
              },
            ],
          },
        ],
      },
    },
    {
      subject_id: 'terra',
      scenario_id: 'persistent_state',
      available: true,
      report: {
        ...resultContract,
        objective_outcome: 'failed',
        assessment_contract: { runs: [] },
        assessment_summary: {},
        scenarios: [
          {
            scenario_id: 'persistent_state',
            scenario_version: 1,
            passed: false,
            aggregate: aggregate({
              completed_runs: 0,
              task_incomplete_runs: 1,
              completion_rate: 0,
              objective_median_score: 0,
              quality_scored_completed_runs: 0,
              quality_score_completed: null,
              quality_coverage: null,
              total_tokens_consumed: null,
              judge_tokens_consumed: null,
              tokens_completed_p50: null,
              failed_attempt_tokens: null,
              tokens_per_completion: null,
              hard_gate_failures: 1,
            }),
            runs: [
              {
                run_id: 'run-state',
                attempt_id: 'attempt-state',
                status: 'hard_gate_failed',
                completion: 'task_incomplete',
                technical: 'valid',
                evaluators: {
                  completion: 'available',
                  quality: 'not_required',
                  final_advisory: 'not_required',
                },
                objective_score: 0,
                quality_score_completed: null,
                assessment: {
                  system_status: 'hard_gate_failed',
                  effective_status: 'hard_gate_failed',
                  assessments: [],
                  ai_final_assessment: { availability: 'not_requested' },
                },
              },
            ],
          },
        ],
      },
    },
    {
      subject_id: 'terra',
      scenario_id: 'research_pipeline',
      available: true,
      report: {
        ...resultContract,
        objective_outcome: 'inconclusive',
        assessment_contract: { runs: [] },
        assessment_summary: {},
        scenarios: [
          {
            scenario_id: 'research_pipeline',
            scenario_version: 1,
            passed: false,
            aggregate: aggregate({
              observed_runs: 0,
              deferred_runs: 1,
              completed_runs: 0,
              technical_valid_runs: 0,
              execution_reliability: 0,
              completion_evidence_coverage: 0,
              completion_rate: null,
              objective_scored_runs: 0,
              objective_median_score: null,
              objective_score_coverage: 0,
              quality_scored_completed_runs: 0,
              quality_score_completed: null,
              quality_coverage: null,
              total_tokens_consumed: null,
              judge_tokens_consumed: null,
              tokens_completed_p50: null,
              failed_attempt_tokens: null,
              tokens_per_completion: null,
            }),
            runs: [],
          },
        ],
      },
    },
    {
      subject_id: 'terra',
      scenario_id: 'missing_report',
      available: false,
    },
  ],
} as unknown as DashboardExecutionDetail

describe('ScenarioMatrix', () => {
  it('renders a comparable matrix with an inline workflow expansion', () => {
    const html = renderToStaticMarkup(
      <ScenarioMatrix detail={detail} onTranscript={() => {}} />,
    )

    expect(html).toContain('4 scenarios')
    expect(html).toContain('report state')
    expect(html).toContain('objective outcome')
    expect(html).toContain(`Results V${RESULTS_SCHEMA_VERSION}`)
    expect(html).toContain('Completion and evidence yield')
    expect(html).toContain('execution reliability')
    expect(html).toContain('quality score completed')
    expect(html).toContain('Physical attempt outcomes')
    expect(html).not.toContain('Technical Invalid')
    expect(html).toContain('1 passed')
    expect(html).toContain('1 hard gate')
    expect(html).toContain('1 inconclusive')
    expect(html).toContain('1 unavailable')
    expect(html).toContain('Security Review v2')
    expect(html).toContain('Objective result')
    expect(html).toContain('AI concerns')
    expect(html).toContain('Workflow · 2 steps')
    expect(html).toContain('Workflow duration profile')
    expect(html).toContain('Tokens')
    expect(html).toContain('1,000')
    expect(html).toContain('Function calls')
    expect(html).toContain('Function errors')
    expect(html).toContain('Cost')
    expect(html).toContain('$0.0123')
    expect(html).toContain('data-primary-metric="Runtime"')
    expect(html).toContain('data-primary-metric="Total tokens"')
    expect(html).toContain('data-primary-metric="Function calls"')
    expect(html).toContain('data-primary-metric="Reported cost"')
    expect(html).not.toContain('data-primary-metric="Hard gates"')
    expect(html).not.toContain('data-step-metric="Findings"')
    expect(html).not.toContain('data-step-metric="Requests"')
    expect(html).not.toContain('data-step-metric="Polls"')
    expect(html).toContain('Not captured')
    expect(html).toContain('Inspect scenario evidence')
    expect(html).toContain('>Structure<')
    expect(html).not.toContain('logical run')
    expect(html).not.toContain('>avg<')
    expect(html).not.toContain('Recorded runs')
    expect(html).not.toContain('This scenario has no persisted workflow')
    expect(html).toContain('aria-expanded="true"')
  })

  // Audit SM-07 / SM-12: without any persisted workflow the Structure column
  // only repeats "Standard", and a failed scenario opens its evidence at once.
  it('hides the structure column and opens evidence for a failed standard scenario', () => {
    const failedOnly = {
      ...detail,
      reports: detail.reports.filter(
        (report) => report.scenario_id === 'persistent_state',
      ),
    } as DashboardExecutionDetail
    const html = renderToStaticMarkup(
      <ScenarioMatrix detail={failedOnly} onTranscript={() => {}} />,
    )

    expect(html).not.toContain('>Structure<')
    expect(html).not.toContain('>Standard<')
    expect(html).toContain('Inspect scenario evidence')
    expect(html).toMatch(/<details[^>]*open/)
    expect(html).toContain('title="Persistent State v1"')
    expect(html).toContain('Task Incomplete')
    expect(html).toContain('Not Required')
  })
})
