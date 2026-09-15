import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type {
  DashboardExecutionSummary,
  ImportedPlan,
  LocalPlan,
} from '@/lib/dashboard-data-source'
import {
  matchesFilter,
  PlanMetricsCell,
  PlanRow,
  planStatePresentation,
  releaseControlPlans,
} from '@/pages/PlansPage'

const plan: LocalPlan = {
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

describe('plan list metrics', () => {
  it('orders Release Control executions by the instant across timezones', () => {
    const dated = (id: string, profile: string, started_at: string) => ({
      ...execution(id, 100),
      release_control: {
        profile,
        execution_id: id,
        attempt: 1,
        campaign_id: null,
        group_id: null,
      },
      started_at,
    })
    const result = releaseControlPlans([
      dated('earlier', 'smoke', '2026-09-14T12:00:00+03:00'),
      dated('later', 'smoke', '2026-09-14T07:00:00-03:00'),
      dated('middle', 'other', '2026-09-14T09:30:00Z'),
    ])
    expect(result.map((plan) => plan.key)).toEqual(['smoke', 'other'])
    expect(result[0].executions.map((execution) => execution.id)).toEqual([
      'later',
      'earlier',
    ])
  })

  it('shows the latest candidate metrics and puts identity before status', () => {
    const html = renderToStaticMarkup(
      <table>
        <tbody>
          <PlanRow
            plan={plan}
            executionSummaries={{
              'baseline-1': execution('baseline-1', 100),
              'candidate-2': execution('candidate-2', 50),
            }}
          />
        </tbody>
      </table>,
    )
    expect(html).toContain('Candidate #2')
    expect(html.replace(/<[^>]*>/g, '')).toContain(
      'Candidate #2 · 900 tokens · $0.0900 · 10s',
    )
    expect(html).not.toContain('Turns')
    expect(html).not.toContain('Local plan')
    expect(html).not.toContain('Latest candidate vs baseline')
    expect(html.indexOf('Focused regression check')).toBeLessThan(
      html.indexOf('comparison available'),
    )
  })

  it('shows baseline metrics before a candidate exists without substituting a missing candidate', () => {
    const summaries = { 'baseline-1': execution('baseline-1', 100) }
    const render = (selected: LocalPlan) =>
      renderToStaticMarkup(
        <table>
          <tbody>
            <PlanRow plan={selected} executionSummaries={summaries} />
          </tbody>
        </table>,
      )
    const baseline = render({
      ...plan,
      state: 'baseline_ready',
      candidate_execution_ids: [],
    })
    expect(baseline).toContain('1K tokens · $0.1000 · 12s')
    expect(baseline).not.toContain('No candidate yet')
    const missingCandidate = render(plan)
    expect(missingCandidate).toContain('Candidate #2 · No execution metrics')
    expect(missingCandidate).not.toContain('$0.1000')
    const running = render({ ...plan, state: 'candidate_running' })
    expect(running).toContain('Current run · No execution metrics')
  })

  it('preserves zero and absent metric values', () => {
    const html = renderToStaticMarkup(
      <PlanMetricsCell
        source="Baseline"
        execution={{
          ...execution('zero', 100),
          totals: {
            total_tokens: 0,
            total_cost_usd: 0,
            wall_time_seconds: null,
            turns: null,
          },
        }}
      />,
    )
    expect(html).toContain('0 tokens · $0.0000 · Not reported')
    expect(html).toContain(
      'Baseline metrics: Tokens 0, spend $0.0000, time Not reported',
    )
    expect(html).not.toContain('Turns')
  })

  it('shows a positive sub-cent spend without rounding it to zero and identifies imported metrics', () => {
    const html = renderToStaticMarkup(
      <PlanMetricsCell
        source="Latest execution"
        execution={{
          ...execution('small-spend', 100),
          totals: {
            total_tokens: 500,
            total_cost_usd: 0.00003,
            wall_time_seconds: 12,
          },
        }}
      />,
    )
    expect(html.replace(/<[^>]*>/g, '')).toContain(
      'Latest execution · 500 tokens · &lt;$0.0001 · 12s',
    )
    expect(html).not.toContain('$0.0000')
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
