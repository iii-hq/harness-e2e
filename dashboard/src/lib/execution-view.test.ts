import { describe, expect, it } from 'vitest'
import type { DashboardExecutionSummary } from '@/lib/dashboard-data-source'
import {
  attentionState,
  buildExecutionPresentation,
  failureBreakdown,
  primaryIssue,
} from '@/lib/execution-view'

function execution(
  overrides: Partial<DashboardExecutionSummary> = {},
): DashboardExecutionSummary {
  return {
    id: 'execution-1',
    label: 'Ten case audit',
    status: 'technical_failed',
    subjects: [
      {
        id: 'terra',
        model: 'gpt-5.6-terra',
        provider: 'openai-codex',
        judge: { model: 'gpt-5.6-sol', provider: 'openai-codex' },
        scenarios: [],
      },
    ],
    assessment_summary: {
      run_count: 10,
      assessment_count: 0,
      asset_count: 0,
      evidence_reference_count: 0,
      system_statuses: {
        passed: 5,
        hard_gate_failed: 3,
        infrastructure_error: 1,
        resource_limit: 1,
        subject_error: 0,
        judge_error: 0,
        unavailable: 0,
        passed_with_concerns: 0,
      },
      effective_statuses: {} as never,
      assessment_outcomes: {} as never,
      asset_qualitative_outcomes: {} as never,
      asset_validation_outcomes: {} as never,
      ai_availability: {} as never,
      ai_verdicts: {} as never,
      median_quality_score: null,
      median_confidence: null,
    },
    totals: {
      expected_reports: 10,
      received_reports: 10,
      passed_scenarios: 5,
      scenario_pass_rate: 0.5,
      report_coverage: 1,
      wall_time_seconds: 1501,
    },
    ...overrides,
  }
}

describe('execution presentation view model', () => {
  it('keeps mixed objective statuses visible instead of collapsing them to technical failure', () => {
    const value = failureBreakdown(execution())
    expect(value).toMatchObject({
      passed: 5,
      hard_gate: 3,
      infrastructure: 1,
      resource_limit: 1,
      total: 10,
      issues: 5,
    })
    expect(primaryIssue(value)).toEqual({
      category: 'infrastructure',
      count: 1,
    })
    expect(attentionState(execution(), value)).toBe('needs_attention')
  })

  it('uses friendly identity and model provenance in the presentation', () => {
    const presentation = buildExecutionPresentation(execution())
    expect(presentation.label).toBe('Ten case audit')
    expect(presentation.subjects[0]).toEqual({
      provider: 'openai-codex',
      model: 'gpt-5.6-terra',
    })
    expect(presentation.judges[0]).toEqual({
      provider: 'openai-codex',
      model: 'gpt-5.6-sol',
    })
  })

  it('treats an execution with only passed scenarios as healthy', () => {
    const passed = execution({
      status: 'passed',
      assessment_summary: {
        ...execution().assessment_summary,
        system_statuses: { passed: 10 } as never,
      } as never,
    })
    expect(buildExecutionPresentation(passed).attention).toBe('passed')
  })
})
