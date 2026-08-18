import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { DashboardExecutionSummary } from '@/lib/dashboard-data-source'
import { buildExecutionPresentation } from '@/lib/execution-view'
import { LatestExecution } from '@/pages/OverviewPage'

function workflowExecution(): DashboardExecutionSummary {
  return {
    id: 'security-review-1',
    label: 'Security review',
    status: 'hard_gate_failed',
    availability: 'full',
    completed_at: '2026-08-17T20:00:00Z',
    subjects: [
      {
        id: 'terra',
        provider: 'openai-codex',
        model: 'gpt-5.6-terra',
        judge: { provider: 'openai-codex', model: 'gpt-5.6-sol' },
        scenarios: [],
      },
    ],
    assessment_summary: {
      system_statuses: { hard_gate_failed: 1 },
    } as never,
    totals: {
      expected_reports: 1,
      received_reports: 1,
      scenario_pass_rate: 0,
      report_coverage: 1,
      total_tokens: 12_500,
      wall_time_seconds: 80,
    },
    workflow_metrics: {
      step_count: 5,
      succeeded_steps: 4,
      failed_steps: 0,
      hard_gate_failed_steps: 1,
      skipped_steps: 0,
      cancelled_steps: 0,
      running_steps: 0,
      pending_steps: 0,
      duration_ms: 65_000,
      asset_count: 8,
      hard_gate_count: 4,
      passed_hard_gate_count: 3,
      evaluation_count: 9,
      failure_count: 1,
      total_tokens: 10_250,
      function_calls: 17,
      token_metric_steps: 4,
      function_call_metric_steps: 4,
    },
  }
}

describe('overview workflow evidence', () => {
  it('replaces generic coverage with persisted workflow progress', () => {
    const html = renderToStaticMarkup(
      <LatestExecution
        presentation={buildExecutionPresentation(workflowExecution())}
      />,
    )

    expect(html).toContain('Semantic steps')
    expect(html).toContain('4/5')
    expect(html).toContain('3/4 hard gates passed')
    expect(html).toContain('Needs review')
    expect(html).toContain('Workflow runtime')
    expect(html).toContain('1m 05s')
    expect(html).toContain('8 assets · 9 evaluations')
    expect(html).toContain('Workflow tokens')
    expect(html).toContain('10,250')
    expect(html).toContain('17 function calls · 4/5 steps reported tokens')
    expect(html).toContain('Partial')
    expect(html).not.toContain('>Report coverage<')
  })
})
