import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { ScenarioMatrix } from '@/components/ScenarioMatrix'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'

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
        assessment_contract: { runs: [] },
        assessment_summary: {},
        scenarios: [
          {
            scenario_id: 'security_review',
            scenario_version: 2,
            passed: true,
            runs: [
              {
                run_id: 'run-security',
                attempt_id: 'attempt-security',
                status: 'passed',
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
        assessment_contract: { runs: [] },
        assessment_summary: {},
        scenarios: [
          {
            scenario_id: 'persistent_state',
            scenario_version: 1,
            passed: false,
            runs: [
              {
                run_id: 'run-state',
                attempt_id: 'attempt-state',
                status: 'hard_gate_failed',
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
        assessment_contract: { runs: [] },
        assessment_summary: {},
        scenarios: [
          {
            scenario_id: 'research_pipeline',
            scenario_version: 1,
            status: 'inconclusive',
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
    expect(html).not.toContain('Recorded runs')
    expect(html).not.toContain('This scenario has no persisted workflow')
    expect(html).toContain('aria-expanded="true"')
  })
})
