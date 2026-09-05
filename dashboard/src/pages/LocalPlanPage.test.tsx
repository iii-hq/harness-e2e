import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type {
  DashboardExecutionSummary,
  LocalPlan,
} from '@/lib/dashboard-data-source'
import { formatDate } from '@/lib/execution-view'
import { buildPlanComparison } from '@/lib/plan-comparison'
import {
  executionHistoryRows,
  executionsScent,
  PLAN_FORM_DEFAULTS,
  PlanComparisonLayers,
  PlanExecutionHistory,
  PlanLifecycle,
  PlanNonComparableAttempts,
  PlanScope,
  planFormDirty,
  planMetricWinnerIds,
  planMovementGroups,
  planProvenanceEntries,
  planProvenanceScent,
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

// Fixture timestamps render in the reader's locale, like the page does.
const captured = formatDate('2026-08-17T12:00:00Z')

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
    expect(html).toContain('view active execution')
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
    expect(html).toContain('>run baseline<')
    expect(html).not.toContain('>run candidate<')
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
    expect(html).toContain('Candidate results are ready')
    expect(html).toContain('view latest candidate')
    expect(html).toContain('run another candidate')
    expect(html).toContain('view baseline execution')
    expect(html).not.toContain('Execution controls')
    expect(html).not.toContain('Plan actions')
    expect(html).not.toContain('Next action')
  })
})

describe('local plan execution comparison', () => {
  const controls = {
    onVisualBaselineChange: () => undefined,
    onToggleCandidate: () => undefined,
    loading: false,
  }

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
        {...controls}
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
        {...controls}
      />,
    )

    expect(noBaselineHtml).toBe('')
  })

  // Audit ED-26: layer 0 is the filter row, the verdict and the trend tiles;
  // the pivoted table opens in the all-metrics layer beneath it.
  it('leads with trend tiles, keeps the pivoted table in a layer and separates incomplete attempts', () => {
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
    const input = {
      plan,
      summaries,
      visualBaselineId: 'baseline-1',
      comparisonCandidateIds: ['candidate-1', 'candidate-2'],
      selectedCandidateId: 'candidate-2',
    }
    const overviewHtml = renderToStaticMarkup(
      <PlanExecutionHistory {...input} {...controls} />,
    )
    const layersHtml = renderToStaticMarkup(<PlanComparisonLayers {...input} />)
    const diagnosticsHtml = renderToStaticMarkup(
      <PlanNonComparableAttempts
        plan={plan}
        summaries={summaries}
        onRenameCandidate={async () => undefined}
      />,
    )
    const html = overviewHtml + layersHtml + diagnosticsHtml
    const tableHtml = layersHtml.slice(
      layersHtml.indexOf('<table'),
      layersHtml.indexOf('</table>') + '</table>'.length,
    )

    // One filter row scopes everything; it never touches the official baseline.
    expect(overviewHtml).toContain('data-plan-filter-row')
    expect(overviewHtml).toContain('>reference<')
    expect(overviewHtml).toContain('>candidates<')
    expect(overviewHtml).toContain('<option value="baseline-1" selected="">')
    expect(
      overviewHtml.match(/data-candidate-option="selected"/g),
    ).toHaveLength(2)
    expect(overviewHtml).toContain('never changes here')
    expect(overviewHtml).toContain('data-plan-verdict')
    expect(overviewHtml).toContain('Objective results are stable')
    // Trend tiles: the selected candidate's value, its delta, and a sparkline
    // with one point per completed execution.
    expect(overviewHtml).toContain('data-trend-metric="pass_rate"')
    expect(overviewHtml).toContain('data-trend-metric="tokens"')
    expect(overviewHtml).toContain('data-trend-metric="duration"')
    expect(overviewHtml).toContain('data-trend-metric="turns"')
    expect(overviewHtml).toContain('-100 · -10.0%')
    expect(overviewHtml.match(/data-point-role="baseline"/g)).toHaveLength(7)
    expect(overviewHtml.match(/data-point-role="selected"/g)).toHaveLength(7)
    expect(overviewHtml.match(/data-point-role="other"/g)).toHaveLength(7)
    expect(overviewHtml).not.toContain('<table')
    // The layers carry the exact numbers.
    expect(layersHtml).toContain('id="plan-metrics"')
    expect(layersHtml).toContain('all metrics · ')
    expect(html).toContain('baseline and candidates')
    expect(tableHtml).toContain('<th scope="col">Metric</th>')
    expect(tableHtml).toContain('>Reference<')
    expect(tableHtml).toContain('Pass rate')
    expect(tableHtml).toContain('Higher is better')
    expect(tableHtml).toContain('Lower is better')
    expect(tableHtml).toContain('role="tooltip"')
    expect(layersHtml).toContain('--ds-table-min-width:48rem')
    expect(tableHtml).not.toMatch(/improved/i)
    expect(tableHtml).toContain('+100 · +10.0%')
    expect(tableHtml).toMatch(
      /data-metric-id="tokens"[\s\S]*?<td class="is-selected is-winner" data-execution-id="candidate-2"/,
    )
    expect(tableHtml).toContain('Best')
    // Executions keep newest candidates first and list incomplete attempts.
    expect(diagnosticsHtml.indexOf('Baseline')).toBeLessThan(
      diagnosticsHtml.indexOf('Candidate #2'),
    )
    expect(diagnosticsHtml.indexOf('Candidate #2')).toBeLessThan(
      diagnosticsHtml.indexOf('Candidate #1'),
    )
    expect(diagnosticsHtml).toContain('Incomplete attempt')
    expect(diagnosticsHtml).toContain('Incomplete attempts remain excluded')
    expect(diagnosticsHtml).toContain('runs · ')
    expect(diagnosticsHtml).toContain('data-label="Tokens"')
    expect(diagnosticsHtml).toContain('data-label="Duration"')
    expect(diagnosticsHtml).toContain(
      'aria-label="Open report for Official baseline"',
    )
    expect(diagnosticsHtml).toContain('title="baseline-1"')
    expect(overviewHtml).not.toContain('runs · ')
    expect(diagnosticsHtml).toMatch(
      /^<section class="ds-panel[^"]*"[^>]*data-plan-run-history/,
    )
  })

  it('draws the two new token metrics as tiles and rows when the totals carry them', () => {
    const plan: LocalPlan = {
      ...candidateRunningPlan,
      state: 'comparison_ready',
      candidate_execution_ids: ['candidate-1'],
      last_attempt_id: 'candidate-1',
    }
    const withTokens = (id: string, perCompletion: number, failed: number) =>
      execution(id, {
        totals: {
          ...execution(id).totals,
          tokens_per_completion: perCompletion,
          failed_attempt_tokens: failed,
        },
      })
    const input = {
      plan,
      summaries: {
        'baseline-1': withTokens('baseline-1', 1_000, 400),
        'candidate-1': withTokens('candidate-1', 800, 0),
      },
      visualBaselineId: 'baseline-1',
      comparisonCandidateIds: ['candidate-1'],
      selectedCandidateId: 'candidate-1',
    }
    const overviewHtml = renderToStaticMarkup(
      <PlanExecutionHistory {...input} {...controls} />,
    )
    const layersHtml = renderToStaticMarkup(<PlanComparisonLayers {...input} />)

    expect(overviewHtml).toContain('data-trend-metric="tokens_per_completion"')
    expect(overviewHtml).toContain('data-trend-metric="failed_attempt_tokens"')
    expect(overviewHtml).toContain('tokens per completion')
    expect(overviewHtml).toContain('failed attempt tokens')
    expect(overviewHtml).toContain('-200 · -20.0%')
    expect(overviewHtml).toContain('-400 · -100.0%')
    expect(layersHtml).toContain('data-metric-id="tokens_per_completion"')
    expect(layersHtml).toContain('data-metric-id="failed_attempt_tokens"')
    // Without the totals the tiles still exist but say so instead of zero.
    const bare = renderToStaticMarkup(
      <PlanExecutionHistory
        {...input}
        summaries={{
          'baseline-1': execution('baseline-1'),
          'candidate-1': execution('candidate-1'),
        }}
        {...controls}
      />,
    )
    expect(bare).toContain('data-trend-metric="failed_attempt_tokens"')
    expect(bare).toMatch(
      /data-trend-metric="failed_attempt_tokens"[\s\S]*?>Not reported<[\s\S]*?Not comparable/,
    )
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
    const summaries = {
      'baseline-1': execution('baseline-1'),
      'candidate-1': execution('candidate-1'),
      'candidate-2': execution('candidate-2'),
    }
    const overviewHtml = renderToStaticMarkup(
      <PlanExecutionHistory
        plan={plan}
        summaries={summaries}
        visualBaselineId="baseline-1"
        comparisonCandidateIds={['candidate-1', 'candidate-2']}
        selectedCandidateId="candidate-2"
        {...controls}
      />,
    )
    const executionsHtml = renderToStaticMarkup(
      <PlanNonComparableAttempts
        plan={plan}
        summaries={summaries}
        onRenameCandidate={async () => undefined}
      />,
    )
    const html = overviewHtml + executionsHtml

    expect(html).toContain('Harness Latest')
    expect(html).toContain('Harness Next')
    expect(html).toContain('runs · ')
    expect(overviewHtml).toContain('>reference<')
    expect(html).toContain('baseline-1')
    expect(html).toContain('candidate-1')
    expect(html).toContain('candidate-2')
    expect(
      html.match(/aria-label="Rename Harness (?:Latest|Next)"/g),
    ).toHaveLength(2)
    expect(html).toContain('data-plan-run-history')
    expect(executionsHtml).toContain('data-label="Turns"')
    expect(overviewHtml).toContain('Compare candidates')
    // The executions layer reads the same rows headless, under the layer row.
    const headless = renderToStaticMarkup(
      <PlanNonComparableAttempts
        plan={plan}
        summaries={summaries}
        onRenameCandidate={async () => undefined}
        headless
      />,
    )
    expect(headless).toContain('data-plan-run-history="headless"')
    expect(headless).not.toContain('runs · ')
    expect(
      executionsScent(plan, executionHistoryRows(plan, summaries)).split(
        ' \u00a0·\u00a0 ',
      ),
    ).toEqual([
      `Official baseline · passed · ${captured} · 1,000 tokens`,
      `Harness Next · passed · ${captured} · 1,000 tokens`,
      `Harness Latest · passed · ${captured} · 1,000 tokens`,
    ])
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
            label: 'Duration',
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
    const input = {
      plan: {
        ...candidateRunningPlan,
        state: 'comparison_ready' as const,
        candidate_execution_ids: ['candidate-1'],
      },
      summaries: { 'baseline-1': baseline, 'candidate-1': candidate },
      visualBaselineId: 'baseline-1',
      comparisonCandidateIds: ['candidate-1'],
      selectedCandidateId: 'candidate-1',
      scenarioComparison: comparison,
    }
    const overviewHtml = renderToStaticMarkup(
      <PlanExecutionHistory {...input} {...controls} />,
    )
    const layersHtml = renderToStaticMarkup(<PlanComparisonLayers {...input} />)

    expect(layersHtml).toContain('by test')
    expect(layersHtml).toContain('Tokens')
    expect(layersHtml).toContain('Cost')
    expect(layersHtml).toContain('Function calls')
    expect(layersHtml).toContain('Function errors')
    expect(layersHtml).toContain('Turns')
    expect(layersHtml).toContain('Duration')
    expect(layersHtml).toContain('data-plan-by-test')
    expect(layersHtml).not.toContain('Findings')
    expect(layersHtml).toMatch(/data-scenario-id="security_review" open=""/)
    // What moved: three bars, oriented by improvement, the unchanged named once.
    expect(overviewHtml).toContain('data-plan-what-moved')
    const groups = planMovementGroups(comparison)
    expect(groups).toHaveLength(1)
    expect(
      groups[0].rows.map((row) => [row.id, row.improvement, row.tone]),
    ).toEqual([
      ['tokens', -3.11, 'negative'],
      ['duration', -33.33, 'negative'],
      ['turns', 50, 'positive'],
    ])
    expect(groups[0].rows.map((row) => row.valueLabel)).toEqual([
      '+142 · +3.1%',
      '+0.1s · +33.3%',
      '-1 · -50.0%',
    ])
    expect(groups[0].subtitle).toBe('Passed → Passed · 3 of 3 metrics moved')
    // Dumbbells draw the magnitude the percentages hide.
    expect(layersHtml).toContain('data-dumbbell-metric="tokens"')
    expect(layersHtml).toContain('data-dumbbell-metric="duration"')
    expect(layersHtml).not.toContain('data-dumbbell-metric="quality"')
  })

  it('shows ten scenario metrics including the new token ones and highlights strict winners', () => {
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
              tokens_per_completion: tokens,
              failed_attempt_tokens: id === 'baseline-1' ? 500 : 0,
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
      <PlanComparisonLayers
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
      />,
    )
    const scenarioHtml = html.slice(html.indexOf('data-plan-by-test'))

    expect(scenarioHtml).toContain('Candidate #1')
    expect(scenarioHtml).toContain('Candidate #2')
    expect(scenarioHtml).toContain('2 candidates')
    expect(scenarioHtml.match(/data-scenario-metric-id=/g)).toHaveLength(10)
    for (const metricId of [
      'pass_rate',
      'quality',
      'cost',
      'turns',
      'duration',
      'tokens',
      'tokens_per_completion',
      'failed_attempt_tokens',
      'function_calls',
      'function_errors',
    ]) {
      expect(scenarioHtml).toContain(`data-scenario-metric-id="${metricId}"`)
    }
    // The expanded table marks the strict winner per metric.
    expect(scenarioHtml).toMatch(
      /data-scenario-metric-id="tokens"[\s\S]*?<td class="is-winner" data-label="Candidate #2"/,
    )
    expect(scenarioHtml).toContain('data-scenario-metrics')
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
        {...controls}
      />,
    )

    expect(html).toContain('<option value="candidate-2" selected="">')
    expect(html).toContain('Candidate #2')
    expect(html).toContain('never changes here')
    expect(html.match(/data-candidate-option="selected"/g)).toHaveLength(2)
    // The reference point in every sparkline is the visual baseline.
    expect(html).toMatch(
      /data-point-role="baseline"[\s\S]*?<title>Candidate #2 · /,
    )
  })

  it('selects the latest candidate automatically and preserves a valid manual selection', () => {
    expect(selectedPlanCandidate(null, false, ['one', 'two'])).toBe('two')
    expect(selectedPlanCandidate('one', false, ['one', 'two'])).toBe('two')
    expect(selectedPlanCandidate('one', true, ['one', 'two'])).toBe('one')
    expect(selectedPlanCandidate('removed', true, ['one', 'two'])).toBe('two')
  })
})

describe('local plan scope and provenance', () => {
  it('reads the scope as one band of facts and keeps the endpoint for provenance', () => {
    const plan: LocalPlan = {
      ...candidateRunningPlan,
      scenarios: [
        {
          scenario_id: 'minimal_path',
          scenario_version: 2,
          case_id: 'case-a',
          seed: 7,
          inputs_sha256: 'sha256:1111111111111111111111',
          contract_sha256: 'sha256:2222222222222222222222',
          complexity_tier: 'baseline',
        },
      ],
    }
    const html = renderToStaticMarkup(
      <PlanScope plan={plan} baselineSummary={execution('baseline-1')} />,
    )
    expect(html).toContain('data-plan-scope')
    expect(html).toContain('scope · locked')
    expect(html).toContain('minimal_path v2')
    expect(html).toContain('1 per test · 0 retries · canonical seed')
    expect(html).toContain('baseline captured')
    expect(html).toContain(captured)
    expect(html).not.toContain('example.invalid')

    const entries = planProvenanceEntries(plan)
    expect(entries).toContainEqual([
      'endpoint',
      'https://example.invalid/catalog',
    ])
    expect(entries).toContainEqual(['scope hash', 'sha256:scope'])
    expect(entries).toContainEqual([
      'minimal_path v2',
      'case case-a · seed 7 · tier baseline · contract sha256:222222222222… · inputs sha256:111111111111…',
    ])
    expect(planProvenanceScent(plan)).toContain(
      'plan-1 · scope sha256:scope · endpoint https://example.invalid/catalog',
    )
  })
})
