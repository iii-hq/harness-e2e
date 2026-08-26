import { describe, expect, it } from 'vitest'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import {
  buildScenarioMatrix,
  detailForScenario,
  stepSignals,
} from '@/lib/scenario-matrix'

function executionDetail() {
  return {
    id: 'execution-1',
    status: 'failed',
    subjects: [
      {
        id: 'terra',
        scenarios: [
          { id: 'security_review', scenario_version: 2 },
          { id: 'missing_report', scenario_version: 1 },
        ],
      },
    ],
    scenario_metrics: [
      {
        scenario_id: 'security_review',
        subject_id: 'terra',
        averages: { duration_seconds: 3 },
      },
    ],
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
                  wall_time_ms: 3_200,
                  assessment: {
                    run_id: 'run-security',
                    attempt_id: 'attempt-security',
                    system_status: 'passed',
                    effective_status: 'passed_with_concerns',
                    assessments: [
                      {
                        criterion_id: 'seeded_vulnerability_detection',
                        policy: 'advisory',
                        outcome: 'partial',
                        summary: 'Detected 3 of 4 seeded paths.',
                      },
                    ],
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
                      hard_gates: [
                        { id: 'request_valid', passed: true, reason: 'ok' },
                      ],
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
                      hard_gates: [
                        { id: 'report_valid', passed: true, reason: 'ok' },
                      ],
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
                  wall_time_ms: 1_500,
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
}

describe('scenario matrix presentation model', () => {
  it('keeps Markdown validation, adherence, pipeline, and technical failures separate', () => {
    const detail = executionDetail()
    const run = detail.reports[1]?.report?.scenarios[0]?.runs[0]
    if (!run) throw new Error('expected fixture run')
    run.validation_score = 80
    run.instruction_adherence = {
      availability: 'available',
      score: 92,
      summary: 'Most requirements were followed.',
    }
    run.markdown_execution = {
      pipeline_complete: true,
      source_path: 'insert-record.md',
    }
    run.failures = []

    expect(
      buildScenarioMatrix(detail).items[1]?.primaryMetrics.slice(0, 4),
    ).toEqual([
      {
        label: 'Validation score',
        value: '80/100',
        detail: 'Deterministic sum of isolated validator outcomes',
      },
      {
        label: 'Instruction adherence',
        value: '92/100',
        detail: 'Advisory prompt-following assessment',
      },
      {
        label: 'Pipeline integrity',
        value: 'Complete',
        detail: 'Correct revision, section routing, and phase completion',
      },
      {
        label: 'Technical failures',
        value: '0',
        detail: 'Infrastructure, evaluator, resource, or cleanup failures',
      },
    ])
  })

  it('keeps objective states separate and associates workflows with scenarios', () => {
    const detail = executionDetail()
    const model = buildScenarioMatrix(detail)

    expect(model.summary).toMatchObject({
      total: 4,
      passed: 1,
      hardGate: 1,
      failed: 0,
      inconclusive: 1,
      unavailable: 1,
    })
    const security = model.items[0]
    expect(security.objective).toMatchObject({
      status: 'passed',
      label: 'Passed',
    })
    expect(security.advisory).toMatchObject({
      status: 'recommendation',
      label: 'AI concerns',
    })
    expect(security.durationMs).toBe(3_000)
    expect(security.durationKind).toBe('average')
    expect(security.workflowSteps).toHaveLength(2)
    expect(security.primaryMetrics).toEqual([
      {
        label: 'Runtime',
        value: '3.0 s',
        detail: 'Average scenario duration',
      },
      {
        label: 'Total tokens',
        value: '1,000',
        detail: '1/2 workflow steps reported',
      },
      {
        label: 'Function calls',
        value: '3',
        detail: '0 errors',
      },
      {
        label: 'Function errors',
        value: '0',
        detail: 'Subject execution errors',
      },
      {
        label: 'Reported cost',
        value: '—',
        detail: 'Not captured for this run',
      },
    ])

    const scoped = detailForScenario(detail, security)
    expect(scoped.reports).toHaveLength(1)
    expect(scoped.reports[0]?.report?.scenarios).toHaveLength(1)
    expect(scoped.reports[0]?.report?.scenarios[0]?.scenario_id).toBe(
      'security_review',
    )
  })

  it('prioritizes usage and domain counters for each workflow step', () => {
    const workflow = buildScenarioMatrix(executionDetail()).items[0]

    expect(stepSignals(workflow.workflowSteps[0])).toEqual([
      { label: 'Tokens', value: '1,000' },
      { label: 'Function calls', value: '3' },
      { label: 'Function errors', value: '0' },
      { label: 'Cost', value: '$0.0123' },
    ])
    expect(stepSignals(workflow.workflowSteps[1])).toEqual([
      { label: 'Metrics', value: 'Not captured' },
    ])
  })

  it('backfills general security-review metrics from retained evidence', () => {
    const detail = executionDetail()
    const run = detail.reports[0]?.report?.scenarios[0]?.runs[0]
    const scan = run?.semantic_tests?.[0]
    if (!scan) throw new Error('expected workflow fixture')
    scan.metrics = {
      request_count: 4,
      reconciliation_operations: 4,
    }
    scan.node_id = 'scan_commit_a'
    scan.cost_usd = null
    run.judge_usage = { input_tokens: 3_000, output_tokens: 500 }

    expect(buildScenarioMatrix(detail).items[0]?.primaryMetrics).toEqual([
      {
        label: 'Runtime',
        value: '3.0 s',
        detail: 'Average scenario duration',
      },
      {
        label: 'Total tokens',
        value: '3,500',
        detail: 'Evaluator usage · backfilled',
      },
      {
        label: 'Function calls',
        value: '9',
        detail: 'Workflow operations · backfilled',
      },
      {
        label: 'Function errors',
        value: '0',
        detail: 'Workflow failures · backfilled',
      },
      {
        label: 'Reported cost',
        value: '$0.0000',
        detail: 'Local run · no metered charge',
      },
    ])
  })
})
