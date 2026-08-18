import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type {
  DashboardExecutionSummary,
  LocalPlan,
} from '@/lib/dashboard-data-source'
import { buildPlanComparison } from '@/lib/plan-comparison'
import {
  PlanComparisonPanel,
  PlanExecutionHistory,
  PlanLifecycle,
  planMetricWinnerIds,
  selectedPlanCandidate,
} from '@/pages/LocalPlanPage'

const candidateRunningPlan: LocalPlan = {
  schema_version: 1,
  id: 'plan-1',
  label: 'Focused regression check',
  purpose: 'Confirm the affected local flow.',
  created_at: '2026-08-17T00:00:00Z',
  updated_at: '2026-08-17T00:01:00Z',
  state: 'candidate_running',
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
  candidate_execution_ids: [],
  incomplete_execution_ids: [],
  last_attempt_id: 'candidate-1',
}

function execution(
  id: string,
  overrides: Partial<DashboardExecutionSummary> = {},
): DashboardExecutionSummary {
  return {
    id,
    status: 'passed',
    availability: 'full',
    completed_at: '2026-08-17T12:00:00Z',
    subjects: [],
    totals: {
      scenario_pass_rate: 100,
      report_coverage: 100,
      hard_gate_failures: 0,
      technical_failures: 0,
      total_tokens: 1_000,
      wall_time_seconds: 12,
      total_cost_usd: 0.1,
      function_calls: 3,
      function_call_errors: 0,
    },
    assessment_summary: {
      system_statuses: { passed: 1 },
      median_quality_score: 90,
      median_confidence: 0.9,
    } as never,
    ...overrides,
  }
}

describe('local plan lifecycle', () => {
  it('keeps a started candidate visible with one clear active-execution action', () => {
    const html = renderToStaticMarkup(
      <PlanLifecycle
        plan={candidateRunningPlan}
        starting={null}
        feedback={{
          role: 'candidate',
          phase: 'running',
          message:
            'Candidate is running. This page refreshes automatically while the report is collected.',
          executionId: 'candidate-1',
        }}
        onStart={() => undefined}
      />,
    )

    expect(html).toContain('Candidate is running')
    expect(html).toContain('View active execution')
    expect(html).not.toContain('Open active execution')
    expect(html).not.toContain('disabled=""')
    expect(html).toContain('aria-live="polite"')
    expect(html).not.toContain('Frozen test scope')
  })

  it('makes the first incomplete lifecycle action a baseline, not a candidate', () => {
    const html = renderToStaticMarkup(
      <PlanLifecycle
        plan={{
          ...candidateRunningPlan,
          state: 'draft',
          locked: false,
          baseline_execution_id: null,
          candidate_execution_ids: [],
          last_attempt_id: null,
        }}
        starting={null}
        feedback={null}
        onStart={() => undefined}
      />,
    )

    expect(html).toContain('Capture the baseline')
    expect(html).toContain('Run baseline')
    expect(html).not.toContain('>Run candidate<')
  })

  it('prioritizes reviewing a completed candidate before offering another run', () => {
    const html = renderToStaticMarkup(
      <PlanLifecycle
        plan={{
          ...candidateRunningPlan,
          state: 'comparison_ready',
          candidate_execution_ids: ['candidate-1'],
          last_attempt_id: 'candidate-1',
        }}
        starting={null}
        feedback={null}
        onStart={() => undefined}
      />,
    )

    expect(html).toContain('Candidate results are ready')
    expect(html).toContain('View latest candidate')
    expect(html).toContain('Run another candidate')
    expect(html).toContain('View baseline execution')
  })
})

describe('local plan execution comparison', () => {
  it('pivots metrics into rows, keeps newest candidates first and separates incomplete attempts', () => {
    const plan: LocalPlan = {
      ...candidateRunningPlan,
      state: 'comparison_ready',
      candidate_execution_ids: ['candidate-1', 'candidate-2'],
      incomplete_execution_ids: ['attempt-1'],
      last_attempt_id: 'candidate-2',
    }
    const summaries = {
      'baseline-1': execution('baseline-1'),
      'candidate-1': execution('candidate-1', {
        totals: {
          ...execution('candidate-1').totals,
          total_tokens: 1_100,
        },
      }),
      'candidate-2': execution('candidate-2', {
        totals: {
          ...execution('candidate-2').totals,
          total_tokens: 900,
        },
      }),
    }
    const html = renderToStaticMarkup(
      <PlanExecutionHistory
        plan={plan}
        summaries={summaries}
        visualBaselineId="baseline-1"
        comparisonCandidateIds={['candidate-1', 'candidate-2']}
        selectedCandidateId="candidate-2"
        onVisualBaselineChange={() => undefined}
        onToggleCandidate={() => undefined}
        loading={false}
      />,
    )
    const tableHtml = html.slice(
      html.indexOf('<table'),
      html.indexOf('</table>') + '</table>'.length,
    )

    expect(html.indexOf('Baseline')).toBeLessThan(html.indexOf('Candidate #2'))
    expect(html.indexOf('Candidate #2')).toBeLessThan(
      html.indexOf('Candidate #1'),
    )
    expect(html).toContain('Incomplete attempt')
    expect(html).toContain('Excluded from comparison')
    expect(html).toContain('Baseline and candidates')
    expect(html).toContain('1 visual baseline · 2 selected')
    expect(html).toContain('Visual baseline')
    expect(html).toContain('Official baseline')
    expect(html).toContain('Plan executions')
    expect(html).toContain('never changes the official baseline')
    expect(html).toContain('<th scope="col">Metric</th>')
    expect(html).toContain('Pass rate')
    expect(html).toContain('Higher is better')
    expect(html).toContain('Lower is better')
    expect(html).toContain('--plan-execution-column-count:3')
    expect(tableHtml).not.toMatch(/reference/i)
    expect(tableHtml).not.toMatch(/improved/i)
    expect(tableHtml).not.toContain('View details')
    expect(tableHtml).not.toContain('Viewing details')
    expect(tableHtml).not.toContain('View report')
    expect(html).toContain('-100 · -10.0%')
    expect(html).toContain('+100 · +10.0%')
    expect(html).toMatch(
      /data-metric-id="tokens"[\s\S]*?<td class="is-selected is-winner" data-execution-id="candidate-2"/,
    )
    expect(html).toContain('Winner')
    expect(html).toContain('Non-comparable attempts')
    expect(html).toContain('View report')
  })

  it('selects metric winners by direction, preserves ties and ignores missing values', () => {
    const values = [
      { id: 'baseline', value: 90 },
      { id: 'candidate-1', value: 95 },
      { id: 'candidate-2', value: 95 },
      { id: 'missing', value: null },
    ]

    expect(planMetricWinnerIds(values, 'higher')).toEqual([
      'candidate-1',
      'candidate-2',
    ])
    expect(planMetricWinnerIds(values, 'lower')).toEqual(['baseline'])
    expect(planMetricWinnerIds(values, 'context')).toEqual([])
    expect(planMetricWinnerIds([{ id: 'only', value: 1 }], 'higher')).toEqual(
      [],
    )
  })

  it('keeps the verdict and test drill-down without duplicate metric cards', () => {
    const baseline = execution('baseline-1')
    const candidate = execution('candidate-2', {
      totals: {
        ...execution('candidate-2').totals,
        total_tokens: 900,
        wall_time_seconds: 10,
      },
    })
    const html = renderToStaticMarkup(
      <PlanComparisonPanel
        comparison={buildPlanComparison(baseline, candidate)}
        baselineExecutionId={baseline.id}
        candidateExecutionId={candidate.id}
        candidateNumber={2}
        loading={false}
        error={null}
      />,
    )

    expect(html).toContain('Baseline vs Candidate #2')
    expect(html).toContain('Objective results are stable')
    expect(html).not.toContain('plan-comparison-metrics')
    expect(html).not.toContain('plan-comparison-metric-card')
    expect(html).toContain('Stable')
  })

  it('lists plan executions with persisted names and contextual rename controls', () => {
    const plan: LocalPlan = {
      ...candidateRunningPlan,
      state: 'comparison_ready',
      candidate_execution_ids: ['candidate-1', 'candidate-2'],
      candidate_labels: {
        'candidate-1': 'Harness Latest',
        'candidate-2': 'Harness Next',
      },
      last_attempt_id: 'candidate-2',
    }
    const html = renderToStaticMarkup(
      <PlanExecutionHistory
        plan={plan}
        summaries={{
          'baseline-1': execution('baseline-1'),
          'candidate-1': execution('candidate-1'),
          'candidate-2': execution('candidate-2'),
        }}
        visualBaselineId="baseline-1"
        comparisonCandidateIds={['candidate-1', 'candidate-2']}
        selectedCandidateId="candidate-2"
        onVisualBaselineChange={() => undefined}
        onToggleCandidate={() => undefined}
        onRenameCandidate={async () => undefined}
        loading={false}
      />,
    )

    expect(html).toContain('Harness Latest')
    expect(html).toContain('Harness Next')
    expect(html).toContain('Plan executions')
    expect(html).toContain('Visual baseline')
    expect(html).toContain('baseline-1')
    expect(html).toContain('candidate-1')
    expect(html).toContain('candidate-2')
    expect(html.match(/>Edit name</g)).toHaveLength(2)
    expect(html.match(/>Compare</g)).toHaveLength(2)
  })

  it('can use a candidate as a visual-only baseline', () => {
    const plan: LocalPlan = {
      ...candidateRunningPlan,
      state: 'comparison_ready',
      candidate_execution_ids: ['candidate-1', 'candidate-2'],
      last_attempt_id: 'candidate-2',
    }
    const summaries = {
      'baseline-1': execution('baseline-1'),
      'candidate-1': execution('candidate-1'),
      'candidate-2': execution('candidate-2', {
        totals: {
          ...execution('candidate-2').totals,
          total_tokens: 900,
        },
      }),
    }
    const html = renderToStaticMarkup(
      <PlanExecutionHistory
        plan={plan}
        summaries={summaries}
        visualBaselineId="candidate-2"
        comparisonCandidateIds={['baseline-1', 'candidate-1']}
        selectedCandidateId="baseline-1"
        onVisualBaselineChange={() => undefined}
        onToggleCandidate={() => undefined}
        loading={false}
      />,
    )

    expect(html).toContain('<option value="candidate-2" selected="">')
    expect(html).toContain('Candidate #2')
    expect(html).toContain('The official plan baseline remains unchanged.')
    expect(html).toContain('1 visual baseline · 2 selected')
  })

  it('keeps the comparison focus target mounted while evidence is loading', () => {
    const html = renderToStaticMarkup(
      <PlanComparisonPanel
        comparison={null}
        baselineExecutionId="baseline-1"
        candidateExecutionId="candidate-1"
        candidateNumber={1}
        loading
        error={null}
      />,
    )

    expect(html).toContain('plan-comparison-panel')
    expect(html).toContain('id="plan-comparison-title"')
    expect(html).toContain('tabindex="-1"')
    expect(html).toContain('aria-busy="true"')
    expect(html).toContain('Baseline vs Candidate #1')
  })

  it('selects the latest candidate automatically and preserves a valid manual selection', () => {
    expect(selectedPlanCandidate(null, false, ['one', 'two'])).toBe('two')
    expect(selectedPlanCandidate('one', false, ['one', 'two'])).toBe('two')
    expect(selectedPlanCandidate('one', true, ['one', 'two'])).toBe('one')
    expect(selectedPlanCandidate('removed', true, ['one', 'two'])).toBe('two')
  })
})
