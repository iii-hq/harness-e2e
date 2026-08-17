import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type {
  DashboardExecutionSummary,
  LocalPlan,
} from '@/lib/dashboard-data-source'
import { PlanComparisonSummary } from '@/pages/PlansPage'

const plan: LocalPlan = {
  schema_version: 1,
  id: 'plan-1',
  label: 'Focused regression check',
  purpose: 'Confirm the affected local flow.',
  created_at: '2026-08-17T00:00:00Z',
  updated_at: '2026-08-17T00:01:00Z',
  state: 'comparison_ready',
  locked: true,
  scope_hash: 'sha256:scope',
  policy_hash: 'sha256:policy',
  url: 'https://example.invalid/catalog',
  model: 'codex/gpt-5.6-terra',
  provider: 'openai-codex',
  judge_model: 'codex/gpt-5.6-sol',
  judge_provider: 'openai-codex',
  scenarios: [],
  scenario_ids: ['direct_answer'],
  runs: 1,
  technical_retries: 0,
  seed: null,
  baseline_execution_id: 'baseline-1',
  candidate_execution_ids: ['candidate-1', 'candidate-2'],
  incomplete_execution_ids: [],
  last_attempt_id: 'candidate-2',
}

function execution(id: string, passRate: number): DashboardExecutionSummary {
  return {
    id,
    status: 'passed',
    availability: 'full',
    completed_at: '2026-08-17T12:00:00Z',
    subjects: [],
    totals: {
      scenario_pass_rate: passRate,
      report_coverage: 100,
      passed_scenarios: passRate === 100 ? 2 : 1,
      hard_gate_failures: passRate === 100 ? 0 : 1,
      technical_failures: 0,
      total_tokens: id === 'baseline-1' ? 1_000 : 900,
      wall_time_seconds: id === 'baseline-1' ? 12 : 10,
      total_cost_usd: id === 'baseline-1' ? 0.1 : 0.09,
    },
    assessment_summary: {
      system_statuses:
        passRate === 100 ? { passed: 2 } : { passed: 1, hard_gate_failed: 1 },
      median_quality_score: 90,
      median_confidence: 0.9,
    } as never,
  }
}

describe('plan list comparison summary', () => {
  it('shows the latest candidate verdict and core baseline deltas', () => {
    const html = renderToStaticMarkup(
      <PlanComparisonSummary
        plan={plan}
        baseline={execution('baseline-1', 100)}
        candidate={execution('candidate-2', 50)}
      />,
    )

    expect(html).toContain('Latest candidate vs baseline')
    expect(html).toContain('Candidate #2')
    expect(html).toContain('Regressed')
    expect(html).toContain('Pass rate')
    expect(html).toContain('100%')
    expect(html).toContain('50%')
    expect(html).toContain('Hard gates')
    expect(html).toContain('Technical failures')
    expect(html).toContain('Cost')
  })

  it('keeps the no-candidate state explicit while exposing the baseline snapshot', () => {
    const html = renderToStaticMarkup(
      <PlanComparisonSummary
        plan={{ ...plan, candidate_execution_ids: [] }}
        baseline={execution('baseline-1', 100)}
        candidate={null}
      />,
    )

    expect(html).toContain('Baseline snapshot')
    expect(html).toContain('Run a candidate to measure evolution')
    expect(html).not.toContain('Regressed')
  })
})
