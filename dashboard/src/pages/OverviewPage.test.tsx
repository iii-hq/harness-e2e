import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { DashboardExecutionSummary } from '@/lib/dashboard-data-source'
import { buildExecutionPresentation } from '@/lib/execution-view'
import { ExecutionHistory, LatestExecution } from '@/pages/OverviewPage'

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
      turns: 18,
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

function summary(
  overrides: Partial<DashboardExecutionSummary> & { id: string },
): DashboardExecutionSummary {
  return {
    label: 'e2e::* control-plane run',
    status: 'passed',
    availability: 'full',
    event: 'local',
    completed_at: '2026-08-26T20:11:31Z',
    subjects: [
      {
        id: 'terra',
        provider: 'openai-codex',
        model: 'gpt-5.6-terra',
        judge: { provider: 'openai-codex', model: 'gpt-5.6-sol' },
        scenarios: [],
      },
    ],
    assessment_summary: { system_statuses: { passed: 2 } } as never,
    totals: {
      expected_reports: 2,
      received_reports: 2,
      scenario_pass_rate: 1,
      report_coverage: 1,
      total_tokens: 7_918,
      wall_time_seconds: 241,
    },
    ...overrides,
  }
}

describe('execution history table', () => {
  const executions = [
    summary({ id: 'passed-1' }),
    summary({
      id: 'gated-1',
      status: 'hard_gate_failed',
      assessment_summary: {
        system_statuses: { hard_gate_failed: 1, passed: 1 },
      } as never,
      totals: {
        expected_reports: 2,
        received_reports: 2,
        scenario_pass_rate: 0.5,
        report_coverage: 1,
      },
    }),
    summary({
      id: 'cancelled-1',
      label: 'context impact · baseline',
      status: 'cancelled',
      availability: 'unavailable',
      event: 'workflow_dispatch',
      subjects: [],
      assessment_summary: undefined,
      totals: undefined,
    }),
  ]
  const html = renderToStaticMarkup(
    <ExecutionHistory executions={executions} />,
  )

  it('filters by the same result vocabulary the column shows, with counts', () => {
    expect(html).toContain('>passed · 1<')
    expect(html).toContain('>hard gate failed · 1<')
    expect(html).toContain('>cancelled · 1<')
    expect(html).toContain('>all results · 3<')
    expect(html).not.toContain('Technical failure')
    expect(html).not.toContain('Infrastructure failure')
    expect(html).toContain('>local · 2<')
    expect(html).toContain('>manual · 1<')
  })

  it('drops the evidence pill and the placeholder chains on cancelled rows', () => {
    expect(html).not.toContain('Diagnostic detail')
    expect(html).not.toContain('>Evidence<')
    expect(html).toContain('no report retained')
    expect(html).not.toContain('Not reported coverage')
    expect(html).not.toContain('No blocking events')
    expect(html).not.toContain('Tokens not reported')
  })

  it('shows the model first and the provider as a detail', () => {
    expect(html).toContain('>gpt-5.6-terra<small')
    expect(html).toContain('>openai-codex<')
    expect(html).toContain('title="openai-codex/gpt-5.6-terra"')
  })

  it('links every row to its execution and only notes partial coverage', () => {
    expect(html).toContain('href="#/execution/gated-1"')
    expect(html).toContain('1 hard gate event')
    expect(html).not.toContain('100% coverage')
    expect(html).toContain('7,918 tokens')
  })
})

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
    expect(html).toContain('18 turns')
    expect(html).toContain('17 function calls · 4/5 steps reported tokens')
    expect(html).toContain('Partial')
    expect(html).not.toContain('>Report coverage<')
  })
})
