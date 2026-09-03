import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type {
  DashboardExecutionSummary,
  LocalPlan,
} from '@/lib/dashboard-data-source'
import { buildPlanComparison } from '@/lib/plan-comparison'
import {
  PLAN_FORM_DEFAULTS,
  PlanComparisonPanel,
  PlanExecutionHistory,
  PlanLifecycle,
  PlanNonComparableAttempts,
  planFormDirty,
  planMetricWinnerIds,
  selectedPlanCandidate,
} from '@/pages/LocalPlanPage'

describe('new plan form dirtiness', () => {
  // Audit PN-01: the form used to be born dirty because the dirty baseline
  // carried a different retry default than the state.
  it('treats an untouched form as clean', () => {
    expect(planFormDirty(PLAN_FORM_DEFAULTS, PLAN_FORM_DEFAULTS)).toBe(false)
    expect(PLAN_FORM_DEFAULTS.technicalRetries).toBe('0')
  })

  it('flags any edited field', () => {
    expect(
      planFormDirty({ ...PLAN_FORM_DEFAULTS, label: 'x' }, PLAN_FORM_DEFAULTS),
    ).toBe(true)
    expect(
      planFormDirty(
        { ...PLAN_FORM_DEFAULTS, scenarios: ['a'] },
        PLAN_FORM_DEFAULTS,
      ),
    ).toBe(true)
  })
})

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
      turns: id === 'baseline-1' ? 4 : 3,
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
    expect(html).toContain(
      '<h2 id="plan-lifecycle-title">Candidate results are ready</h2>',
    )
    expect(html).toContain('View latest candidate')
    expect(html).toContain('Run another candidate')
    expect(html).toContain('View baseline execution')
    expect(html).not.toContain('Execution controls')
    expect(html).not.toContain('Plan actions')
    expect(html).not.toContain('Next action')
  })
})

describe('local plan execution comparison', () => {
  it('hides comparison controls until a baseline and another execution exist', () => {
    const html = renderToStaticMarkup(
      <PlanExecutionHistory
        plan={{
          ...candidateRunningPlan,
          state: 'baseline_ready',
          candidate_execution_ids: [],
          last_attempt_id: 'baseline-1',
        }}
        summaries={{ 'baseline-1': execution('baseline-1') }}
        visualBaselineId="baseline-1"
        comparisonCandidateIds={[]}
        selectedCandidateId={null}
        onVisualBaselineChange={() => undefined}
        onToggleCandidate={() => undefined}
        loading={false}
      />,
    )

    expect(html).toBe('')

    const noBaselineHtml = renderToStaticMarkup(
      <PlanExecutionHistory
        plan={{
          ...candidateRunningPlan,
          state: 'comparison_ready',
          baseline_execution_id: null,
          candidate_execution_ids: ['candidate-1'],
          last_attempt_id: 'candidate-1',
        }}
        summaries={{ 'candidate-1': execution('candidate-1') }}
        visualBaselineId={null}
        comparisonCandidateIds={['candidate-1']}
        selectedCandidateId="candidate-1"
        onVisualBaselineChange={() => undefined}
        onToggleCandidate={() => undefined}
        loading={false}
      />,
    )

    expect(noBaselineHtml).toBe('')
  })

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
    const historyHtml = renderToStaticMarkup(
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
    const diagnosticsHtml = renderToStaticMarkup(
      <PlanNonComparableAttempts
        plan={plan}
        summaries={summaries}
        onRenameCandidate={async () => undefined}
      />,
    )
    const html = historyHtml + diagnosticsHtml
    const tableHtml = historyHtml.slice(
      historyHtml.indexOf('<table'),
      historyHtml.indexOf('</table>') + '</table>'.length,
    )

    expect(html.indexOf('Baseline')).toBeLessThan(html.indexOf('Candidate #2'))
    expect(html.indexOf('Candidate #2')).toBeLessThan(
      html.indexOf('Candidate #1'),
    )
    expect(html).toContain('Incomplete attempt')
    expect(html).toContain('Incomplete attempts remain excluded')
    expect(html).toContain('Baseline and candidates')
    expect(html).toContain('1 visual baseline · 2 selected')
    expect(html).toContain('Visual baseline')
    expect(html).toContain('Official baseline')
    expect(html).toContain('Compare candidates')
    expect(html).toContain(
      'Select one or more executions to show in the table.',
    )
    expect(html).not.toContain('Choose what appears below.')
    expect(html).toContain('Reference')
    expect(html).toContain('Candidate')
    expect(html).toContain('never changes the official baseline')
    expect(html).toContain('<th scope="col">Metric</th>')
    expect(html).toContain('Pass rate')
    expect(html).toContain('Higher is better')
    expect(html).toContain('Lower is better')
    expect(html).toContain('role="tooltip"')
    expect(html).toContain('--plan-execution-column-count:3')
    expect(tableHtml).toContain('>Reference<')
    expect(tableHtml).not.toMatch(/improved/i)
    expect(tableHtml).not.toContain('View details')
    expect(tableHtml).not.toContain('Viewing details')
    expect(tableHtml).not.toContain('View report')
    expect(html).toContain('-100 · -10.0%')
    expect(html).toContain('+100 · +10.0%')
    expect(html).toMatch(
      /data-metric-id="tokens"[\s\S]*?<td class="is-selected is-winner" data-execution-id="candidate-2"/,
    )
    expect(html).toContain('Best')
    expect(html).toContain('Execution history')
    expect(html).toContain('<span>Calls</span>')
    expect(html).toContain('<span>Errors</span>')
    expect(html).toContain('aria-label="Open report for Official baseline"')
    expect(html).toContain('title="baseline-1"')
    expect(historyHtml).not.toContain('Execution history')
    expect(diagnosticsHtml).toMatch(/^<section class="panel plan-run-history"/)
  })

  it('selects only strict metric winners and leaves ties blank', () => {
    const values = [
      { id: 'baseline', value: 90 },
      { id: 'candidate-1', value: 95 },
      { id: 'candidate-2', value: 95 },
      { id: 'missing', value: null },
    ]

    expect(planMetricWinnerIds(values, 'higher')).toEqual([])
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
    const historyHtml = renderToStaticMarkup(
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
        loading={false}
      />,
    )
    const executionsHtml = renderToStaticMarkup(
      <PlanNonComparableAttempts
        plan={plan}
        summaries={{
          'baseline-1': execution('baseline-1'),
          'candidate-1': execution('candidate-1'),
          'candidate-2': execution('candidate-2'),
        }}
        onRenameCandidate={async () => undefined}
      />,
    )
    const html = historyHtml + executionsHtml

    expect(html).toContain('Harness Latest')
    expect(html).toContain('Harness Next')
    expect(html).toContain('Execution history')
    expect(html).toContain('Visual baseline')
    expect(html).toContain('baseline-1')
    expect(html).toContain('candidate-1')
    expect(html).toContain('candidate-2')
    expect(
      html.match(/aria-label="Rename Harness (?:Latest|Next)"/g),
    ).toHaveLength(2)
    expect(html).toContain('plan-run-history-columns')
    expect(executionsHtml).toContain('data-label="Turns"')
    expect(html).toContain('Compare candidates')
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

  it('renders general security metrics as baseline to candidate evidence', () => {
    const baseline = execution('baseline-1')
    const candidate = execution('candidate-1')
    const comparison = buildPlanComparison(baseline, candidate)
    comparison.scenarios = [
      {
        id: 'security_review',
        compatible: true,
        reason: null,
        baseline_status: 'passed',
        candidate_status: 'passed',
        metrics: [],
        execution_metrics: [
          {
            id: 'cost',
            label: 'Cost',
            baseline: 0,
            candidate: 0,
            delta: 0,
            delta_percent: null,
            direction: 'lower',
            format: 'usd',
            tone: 'neutral',
          },
          {
            id: 'tokens',
            label: 'Tokens',
            baseline: 4561,
            candidate: 4703,
            delta: 142,
            delta_percent: 3.11,
            direction: 'lower',
            format: 'tokens',
            tone: 'negative',
          },
          {
            id: 'function_calls',
            label: 'Function calls',
            baseline: 13,
            candidate: 13,
            delta: 0,
            delta_percent: 0,
            direction: 'context',
            format: 'count',
            tone: 'neutral',
          },
          {
            id: 'duration',
            label: 'Time',
            baseline: 0.3,
            candidate: 0.4,
            delta: 0.1,
            delta_percent: 33.33,
            direction: 'lower',
            format: 'seconds',
            tone: 'negative',
          },
          {
            id: 'function_errors',
            label: 'Function errors',
            baseline: 0,
            candidate: 0,
            delta: 0,
            delta_percent: null,
            direction: 'lower',
            format: 'count',
            tone: 'neutral',
          },
          {
            id: 'turns',
            label: 'Turns',
            baseline: 2,
            candidate: 1,
            delta: -1,
            delta_percent: -50,
            direction: 'lower',
            format: 'count',
            tone: 'positive',
          },
        ],
        workflow_metrics: [
          {
            id: 'workflow:finding_count',
            label: 'Findings',
            baseline: 4,
            candidate: 5,
            delta: 1,
            delta_percent: 25,
            direction: 'context',
            format: 'count',
            tone: 'neutral',
          },
        ],
      },
    ]

    const html = renderToStaticMarkup(
      <PlanExecutionHistory
        plan={{
          ...candidateRunningPlan,
          state: 'comparison_ready',
          candidate_execution_ids: ['candidate-1'],
        }}
        summaries={{ 'baseline-1': baseline, 'candidate-1': candidate }}
        visualBaselineId="baseline-1"
        comparisonCandidateIds={['candidate-1']}
        selectedCandidateId="candidate-1"
        scenarioComparison={comparison}
        onVisualBaselineChange={() => undefined}
        onToggleCandidate={() => undefined}
        loading={false}
      />,
    )

    expect(html).toContain('Metrics by test')
    expect(html).toContain('Tokens')
    expect(html).toContain('Cost')
    expect(html).toContain('Function calls')
    expect(html).toContain('Function errors')
    expect(html).toContain('Turns')
    expect(html).toContain('Time')
    expect(html).toContain('plan-scenario-signal-values')
    expect(html).not.toContain('Findings')
    expect(html).toContain('<details class="plan-scenario-disclosure" open="">')
  })

  it('shows eight scenario metrics including turns and highlights strict winners', () => {
    const scenarioExecution = (
      id: string,
      tokens: number,
    ): DashboardExecutionSummary =>
      execution(id, {
        subjects: [
          {
            id: 'subject',
            scenarios: [
              {
                id: 'security_review',
                scenario_version: 3,
                pass_rate: 100,
                assessment_summary: {
                  median_quality_score: 92,
                },
              },
            ],
          },
        ] as never,
        scenario_metrics: [
          {
            scenario_id: 'security_review',
            scenario_version: 3,
            contract_fingerprint: 'security-v3',
            run_count: 1,
            averages: {
              cost_usd: 0.1,
              tokens,
              function_calls: 13,
              function_call_errors: 0,
              turns: id === 'baseline-1' ? 2 : 1,
              duration_seconds: 0.3,
            },
          },
        ],
      })
    const baseline = scenarioExecution('baseline-1', 4_500)
    const candidateOne = scenarioExecution('candidate-1', 4_000)
    const candidateTwo = scenarioExecution('candidate-2', 3_800)
    const html = renderToStaticMarkup(
      <PlanExecutionHistory
        plan={{
          ...candidateRunningPlan,
          state: 'comparison_ready',
          candidate_execution_ids: ['candidate-1', 'candidate-2'],
          last_attempt_id: 'candidate-2',
        }}
        summaries={{
          'baseline-1': baseline,
          'candidate-1': candidateOne,
          'candidate-2': candidateTwo,
        }}
        visualBaselineId="baseline-1"
        comparisonCandidateIds={['candidate-1', 'candidate-2']}
        selectedCandidateId="candidate-2"
        scenarioComparison={buildPlanComparison(baseline, candidateTwo)}
        onVisualBaselineChange={() => undefined}
        onToggleCandidate={() => undefined}
        loading={false}
      />,
    )
    const scenarioHtml = html.slice(html.indexOf('Metrics by test'))

    expect(scenarioHtml).toContain('Candidate #1')
    expect(scenarioHtml).toContain('Candidate #2')
    expect(scenarioHtml).toContain('2 candidates')
    expect(scenarioHtml.match(/data-scenario-metric-id=/g)).toHaveLength(8)
    for (const metricId of [
      'pass_rate',
      'quality',
      'cost',
      'turns',
      'duration',
      'tokens',
      'function_calls',
      'function_errors',
    ]) {
      expect(scenarioHtml).toContain(`data-scenario-metric-id="${metricId}"`)
    }
    expect(scenarioHtml).toMatch(
      /plan-scenario-signal-value is-winner" title="Candidate #2"/,
    )
    expect(scenarioHtml).toContain('plan-scenario-metric-grid')
    expect(scenarioHtml).toContain('class="is-winner"')
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

  it('does not render an empty comparison panel without both executions', () => {
    const html = renderToStaticMarkup(
      <PlanComparisonPanel
        comparison={null}
        baselineExecutionId="baseline-1"
        candidateExecutionId={null}
        candidateNumber={null}
        loading={false}
        error={null}
      />,
    )

    expect(html).toBe('')

    const noBaselineHtml = renderToStaticMarkup(
      <PlanComparisonPanel
        comparison={null}
        baselineExecutionId={null}
        candidateExecutionId="candidate-1"
        candidateNumber={1}
        loading={false}
        error={null}
      />,
    )

    expect(noBaselineHtml).toBe('')
  })

  it('selects the latest candidate automatically and preserves a valid manual selection', () => {
    expect(selectedPlanCandidate(null, false, ['one', 'two'])).toBe('two')
    expect(selectedPlanCandidate('one', false, ['one', 'two'])).toBe('two')
    expect(selectedPlanCandidate('one', true, ['one', 'two'])).toBe('one')
    expect(selectedPlanCandidate('removed', true, ['one', 'two'])).toBe('two')
  })
})
