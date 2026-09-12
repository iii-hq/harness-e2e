import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type {
  DashboardExecutionSummary,
  ImportedPlan,
  LocalPlan,
} from '@/lib/dashboard-data-source'
import {
  matchesFilter,
  PlanBaselineCell,
  PlanComparisonSummary,
  planStatePresentation,
} from '@/pages/PlansPage'

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
      technical_failures: 0,
      total_tokens: id === 'baseline-1' ? 1_000 : 900,
      wall_time_seconds: id === 'baseline-1' ? 12 : 10,
      total_cost_usd: id === 'baseline-1' ? 0.1 : 0.09,
      turns: id === 'baseline-1' ? 4 : 3,
    },
    assessment_summary: {
      system_statuses: { passed: 2 },
    } as never,
  }
}

describe('plan list comparison summary', () => {
  it('shows coverage and neutral signed deltas despite objective failures', () => {
    const html = renderToStaticMarkup(
      <PlanComparisonSummary
        plan={plan}
        baseline={execution('baseline-1', 100)}
        candidate={execution('candidate-2', 50)}
      />,
    )
    expect(html).not.toContain('>regressed<')
    expect(html).toContain('Retained observations')
    expect(html).toContain('candidate #2')
    expect(html).toContain('>coverage<')
    expect(html).not.toContain('−50pp')
    expect(html).not.toContain('ds-delta-negative')
    expect(html).toContain('>tokens<')
    expect(html).toContain('−10%')
    expect(html).not.toContain('ds-delta-positive')
    expect(html).not.toContain('Not reported')
    expect(html).not.toContain('Not comparable')
  })

  it('keeps the no-candidate state explicit and shows the baseline figures in their own cell', () => {
    const ready = {
      ...plan,
      state: 'baseline_ready' as const,
      candidate_execution_ids: [],
    }
    const html = renderToStaticMarkup(
      <PlanComparisonSummary
        plan={ready}
        baseline={execution('baseline-1', 100)}
        candidate={null}
      />,
    )
    expect(html).toContain('No candidate yet')
    expect(html).not.toContain('regressed')
    const cell = renderToStaticMarkup(
      <PlanBaselineCell plan={ready} baseline={execution('baseline-1', 100)} />,
    )
    expect(cell).toContain('100%')
    expect(cell).toContain('1K tokens')
    expect(cell).toContain('4 turns')
    expect(
      renderToStaticMarkup(
        <PlanBaselineCell
          plan={{ ...ready, state: 'draft' }}
          baseline={null}
        />,
      ),
    ).toContain('not captured')
  })

  it('gives every state one status line and one action', () => {
    expect(
      planStatePresentation({ ...plan, state: 'draft', locked: false }),
    ).toMatchObject({
      label: 'draft',
      action: 'run baseline',
    })
    expect(
      planStatePresentation({
        ...plan,
        state: 'draft',
        locked: true,
        incomplete_execution_ids: ['x'],
      }),
    ).toMatchObject({ label: 'retry available', action: 'retry baseline' })
    expect(
      planStatePresentation({ ...plan, state: 'candidate_running' }),
    ).toMatchObject({
      status: 'running',
      label: 'candidate running',
    })
    expect(
      planStatePresentation({ ...plan, state: 'baseline_ready' }),
    ).toMatchObject({
      action: 'run candidate',
    })
    expect(planStatePresentation(plan)).toMatchObject({ action: 'compare' })
  })
})

it('keeps imported history out of local operational filters and their counts', () => {
  const imported: ImportedPlan = {
    origin: 'remote',
    id: 'imported-plan',
    label: 'Retained RC history',
    purpose: '',
    created_at: null,
    updated_at: '2026-09-12T00:00:00Z',
    template_id: null,
    source: {
      instance_id: 'rc-production',
      plan_key: 'regression',
      captured_at: '2026-09-12T00:00:00Z',
      active: true,
      limitation: null,
    },
    configuration: null,
    execution_ids: [],
  }
  const plans = [plan, imported]
  expect(plans.filter((entry) => matchesFilter(entry, 'all'))).toEqual(plans)
  expect(plans.filter((entry) => matchesFilter(entry, 'compared'))).toEqual([
    plan,
  ])
  expect(plans.filter((entry) => matchesFilter(entry, 'running'))).toEqual([])
  expect(plans.filter((entry) => matchesFilter(entry, 'needs_action'))).toEqual(
    [],
  )
  expect(matchesFilter({ ...plan, state: 'baseline_running' }, 'running')).toBe(
    true,
  )
  expect(matchesFilter({ ...plan, state: 'draft' }, 'needs_action')).toBe(true)
})
