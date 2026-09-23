import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type {
  DashboardExecutionSummary,
  LocalPlan,
} from '@/lib/dashboard-data-source'
import { buildPlanComparison } from '@/lib/plan-comparison'
import {
  buildPrimaryMetricsFromValues,
  type MetricId,
  type PrimaryTestValues,
} from '@/lib/primary-metrics'
import {
  PLAN_FORM_DEFAULTS,
  PlanComparisonLayers,
  PlanExecutionHistory,
  PlanNonComparableAttempts,
  PlanRunDialog,
  PlanScope,
  planFormDirty,
  planMovementGroups,
  planProvenanceEntries,
  planProvenanceScent,
  planTrendTiles,
  selectedPlanCandidate,
} from '@/pages/LocalPlanPage'
import {
  buildPlanComparisonModel,
  executionHistoryRows,
  executionMetricValue,
} from '@/pages/PlanDetailPage'

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
    } as never,
    ...overrides,
  }
}

const metricIds: MetricId[] = [
  'score',
  'totalTokens',
  'inputTokens',
  'outputTokens',
  'cacheRead',
  'cacheWrite',
  'turns',
  'functionCalls',
  'functionErrors',
  'durationMs',
  'costUsd',
]

function recordedMetrics(
  tests: Array<{
    key: string
    values: Partial<Record<MetricId, number | null>>
  }>,
) {
  return buildPrimaryMetricsFromValues(
    tests.map(({ key, values }) => ({
      key,
      label: key,
      definition: null,
      expected: 1,
      scopeKnown: true,
      values: Object.fromEntries(
        metricIds.map((id) => [id, [values[id] ?? null]]),
      ) as PrimaryTestValues['values'],
    })),
  )
}

const metricsByExecution = {
  'baseline-1': recordedMetrics([
    {
      key: 'alpha',
      values: {
        score: 80,
        totalTokens: 600,
        durationMs: 6_000,
        turns: 4,
        functionCalls: 2,
        costUsd: 0.06,
      },
    },
    {
      key: 'beta',
      values: {
        score: 60,
        totalTokens: 400,
        durationMs: 6_000,
        turns: 2,
        functionCalls: 1,
        costUsd: 0.04,
      },
    },
  ]),
  'candidate-1': recordedMetrics([
    {
      key: 'alpha',
      values: {
        score: 90,
        totalTokens: 700,
        durationMs: 8_000,
        turns: 3,
        functionCalls: 2,
        costUsd: 0.07,
      },
    },
    {
      key: 'beta',
      values: {
        score: 70,
        totalTokens: 400,
        durationMs: 6_000,
        turns: 2,
        functionCalls: 1,
        costUsd: 0.04,
      },
    },
  ]),
  'candidate-2': recordedMetrics([
    {
      key: 'alpha',
      values: {
        score: 90,
        totalTokens: 500,
        durationMs: 5_000,
        turns: 2,
        functionCalls: 2,
        costUsd: 0.05,
      },
    },
    {
      key: 'beta',
      values: {
        score: 70,
        totalTokens: 400,
        durationMs: 6_000,
        turns: 1,
        functionCalls: 1,
        costUsd: 0.04,
      },
    },
  ]),
}

describe('local plan lifecycle', () => {
  it('averages test scores without treating zero or missing scores as a pass rate', () => {
    const scored = (values: Array<number | null>) =>
      execution('scored', {
        subjects: [
          {
            id: 'subject',
            scenarios: values.map((mean_score, index) => ({
              id: `test-${index}`,
              mean_score,
            })),
          },
        ],
      })
    expect(executionMetricValue(scored([82, 40, 90, 100, 100]), 'score')).toBe(
      '82.4',
    )
    expect(executionMetricValue(scored([0]), 'score')).toBe('0')
    expect(executionMetricValue(scored([0, 100]), 'score')).toBe('50')
    expect(executionMetricValue(scored([100, null]), 'score')).toBe('—')
    expect(executionMetricValue(scored([]), 'score')).toBe('—')
  })

  it('keeps a started candidate visible with one clear active-execution action', () => {
    const html = renderToStaticMarkup(
      <PlanRunDialog
        open
        onClose={() => undefined}
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
    expect(html).toContain('ds-dialog')
    expect(html).not.toContain('Frozen test scope')
  })

  it('makes the first incomplete lifecycle action a baseline, not a candidate', () => {
    const html = renderToStaticMarkup(
      <PlanRunDialog
        open
        onClose={() => undefined}
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

    expect(html).toContain('Run baseline?')
    expect(html).toContain('>Run baseline<')
    expect(html).not.toContain('>run candidate<')
  })

  it('confirms another candidate while keeping its previous report available', () => {
    const html = renderToStaticMarkup(
      <PlanRunDialog
        open
        onClose={() => undefined}
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

    expect(html).toContain('Run candidate #2?')
    expect(html).toContain('Review latest candidate')
    expect(html).toContain('>Run candidate #2<')
    expect(html).not.toContain('Execution controls')
    expect(html).not.toContain('Plan actions')
    expect(html).not.toContain('Next action')
  })
})

describe('local plan execution comparison', () => {
  const controls = {
    metricsByExecution,
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

  // Audit ED-26: layer 0 is the filter row, the observations and the trend tiles;
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
        onRenameExecution={async () => undefined}
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
    expect(overviewHtml).toContain('data-plan-observations')
    expect(overviewHtml).toContain('Retained observations')
    // Tiles use the same retained group values as the summary above, with
    // one point per recorded execution, including an incomplete attempt.
    for (const id of [
      'score',
      'costUsd',
      'durationMs',
      'totalTokens',
      'turns',
      'functionCalls',
    ]) {
      expect(overviewHtml).toContain(`data-trend-metric="${id}"`)
    }
    expect(overviewHtml).toMatch(
      /data-trend-metric="score"[\s\S]*?>80<[\s\S]*?\+14\.29%/,
    )
    expect(overviewHtml).toMatch(
      /data-trend-metric="totalTokens"[\s\S]*?>900<[\s\S]*?−10%/,
    )
    expect(overviewHtml).toMatch(
      /data-trend-metric="durationMs"[\s\S]*?>11s<[\s\S]*?−8\.33%/,
    )
    expect(overviewHtml).toMatch(
      /data-trend-metric="turns"[\s\S]*?>3<[\s\S]*?−50%/,
    )
    expect(overviewHtml.match(/data-point-role="baseline"/g)).toHaveLength(6)
    expect(overviewHtml.match(/data-point-role="selected"/g)).toHaveLength(6)
    expect(overviewHtml.match(/data-point-role="other"/g)).toHaveLength(12)
    expect(overviewHtml).toContain('id="plan-movement-metric"')
    expect(overviewHtml).toContain(
      '<option value="score" selected="">Score</option>',
    )
    expect(overviewHtml).toContain(
      '<option value="totalTokens">Total tokens</option>',
    )
    expect(overviewHtml).toContain('<option value="turns">Turns</option>')
    expect(overviewHtml).not.toContain('<table')
    // The layers carry the exact numbers.
    expect(layersHtml).toContain('id="plan-diagnostic-metrics"')
    expect(layersHtml).toContain('Run statistics · ')
    expect(html).toContain('baseline and candidates')
    expect(tableHtml).toContain('<th scope="col">Metric</th>')
    expect(tableHtml).toContain('>Reference<')
    expect(tableHtml).toContain('Coverage')
    expect(tableHtml).not.toContain('Pass rate')
    expect(tableHtml).not.toContain('Higher is better')
    expect(tableHtml).not.toContain('Lower is better')
    expect(tableHtml).not.toContain('role="tooltip"')
    expect(layersHtml).toContain('--ds-table-min-width:48rem')
    expect(tableHtml).not.toMatch(/improved/i)
    expect(tableHtml).toContain('+100 · +10.0%')
    expect(tableHtml).toMatch(
      /data-metric-id="tokens"[\s\S]*?<td class="is-selected" data-execution-id="candidate-2"/,
    )
    expect(tableHtml).not.toContain('Best')
    expect(diagnosticsHtml).toContain('Incomplete results')
    expect(diagnosticsHtml).toContain('Executions')
    expect(diagnosticsHtml).toContain('data-label="Tokens"')
    expect(diagnosticsHtml).toContain('data-label="Duration"')
    expect(diagnosticsHtml).toContain('data-label="Score / 100"')
    expect(diagnosticsHtml).not.toContain('<code')
    expect(diagnosticsHtml).not.toContain('Official baseline')
    expect(diagnosticsHtml).not.toContain(' · candidate')
    expect(overviewHtml).not.toContain('Executions')
    expect(diagnosticsHtml).toMatch(
      /^<section id="plan-executions"[^>]*data-plan-run-history/,
    )
  })

  it('keeps legacy token diagnostics in the detail table without adding different trend tiles', () => {
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

    expect(overviewHtml).toContain('data-trend-metric="totalTokens"')
    expect(overviewHtml).not.toContain(
      'data-trend-metric="tokens_per_completion"',
    )
    expect(overviewHtml).not.toContain(
      'data-trend-metric="failed_attempt_tokens"',
    )
    expect(layersHtml).toContain('data-metric-id="tokens_per_completion"')
    expect(layersHtml).toContain('data-metric-id="failed_attempt_tokens"')
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
        onRenameExecution={async () => undefined}
      />,
    )
    const html = overviewHtml + executionsHtml

    expect(html).toContain('Harness Latest')
    expect(html).toContain('Harness Next')
    expect(html).toContain('Executions')
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
    expect(html).not.toContain('<details id="plan-executions"')
  })

  it('orders executions by execution time, keeps missing dates last, and renders compact names', () => {
    const plan: LocalPlan = {
      ...candidateRunningPlan,
      label: 'Smoke',
      candidate_execution_ids: ['candidate-1'],
      candidate_labels: { 'candidate-1': 'Reviewed release' },
    }
    const summaries = {
      'baseline-1': execution('baseline-1', {
        started_at: '2026-09-08T12:00:00Z',
        label: 'Smoke',
      }),
      'candidate-1': execution('candidate-1', {
        started_at: '2026-09-08T15:00:00+02:00',
        label: 'candidate-1',
      }),
      'extra-old': execution('extra-old', {
        started_at: '2026-09-07T12:00:00Z',
        generated_at: '2026-09-14T12:00:00Z',
        label: 'Smoke',
        subjects: [{ id: 'test', model: 'deepseek-v4-flash', scenarios: [] }],
      }),
      'unknown-date': execution('unknown-date', {
        started_at: 'invalid',
        generated_at: '2026-09-01T00:00:00Z',
      }),
    }
    const ids = ['unknown-date', 'extra-old']
    expect(
      executionHistoryRows(plan, summaries, ids).map((row) => row.id),
    ).toEqual(['extra-old', 'baseline-1', 'candidate-1', 'unknown-date'])
    const html = renderToStaticMarkup(
      <PlanNonComparableAttempts
        plan={plan}
        summaries={summaries}
        executionIds={ids}
        onRenameExecution={async () => undefined}
      />,
    )
    expect(html).toContain('deepseek-v4-flash')
    expect(html).toContain('Reviewed release')
    expect(html).toContain('dateTime="2026-09-07T12:00:00Z"')
    expect(html).toContain('Execution date unavailable')
    expect(html).not.toContain('release-control')
    expect(html).not.toContain('<code')
    expect(html).not.toMatch(/>(?:extra-old|candidate-1|rename|report)</)
    expect(html.match(/data-execution-id="candidate-1"/g)).toHaveLength(1)
    expect(html).toContain('aria-label="Rename Reviewed release"')
    const comparisonHtml = renderToStaticMarkup(
      <PlanExecutionHistory
        plan={plan}
        summaries={summaries}
        visualBaselineId="baseline-1"
        comparisonCandidateIds={['candidate-1']}
        selectedCandidateId="candidate-1"
        {...controls}
      />,
    )
    expect(comparisonHtml).toContain('Reviewed release')
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
        metrics: [],
        execution_metrics: [
          {
            id: 'cost',
            label: 'Cost',
            baseline: 0,
            candidate: 0,
            delta: 0,
            delta_percent: null,

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

            format: 'tokens',
            tone: 'neutral',
          },
          {
            id: 'function_calls',
            label: 'Function calls',
            baseline: 13,
            candidate: 13,
            delta: 0,
            delta_percent: 0,

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

            format: 'seconds',
            tone: 'neutral',
          },
          {
            id: 'function_errors',
            label: 'Function errors',
            baseline: 0,
            candidate: 0,
            delta: 0,
            delta_percent: null,

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

            format: 'count',
            tone: 'neutral',
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
    expect(layersHtml).toContain('Time')
    expect(layersHtml).toContain('data-plan-by-test')
    expect(layersHtml).not.toContain('Findings')
    expect(layersHtml).toMatch(/data-scenario-id="security_review" open=""/)
    // The chart remains above the diagnostic table.
    expect(overviewHtml).toContain('data-plan-what-moved')
    expect(layersHtml).not.toContain('data-dumbbell-metric="tokens"')
    expect(layersHtml).not.toContain('data-dumbbell-metric="duration"')
    expect(layersHtml).not.toContain('data-dumbbell-metric="quality"')
  })

  it('uses the same per-test values and aggregate mean as the group summary for each metric', () => {
    const a = metricsByExecution['baseline-1']
    const b = metricsByExecution['candidate-2']
    expect(a.metrics.score.value).toBe(70)
    expect(b.metrics.score.value).toBe(80)
    expect(a.metrics.totalTokens.value).toBe(1_000)
    expect(b.metrics.totalTokens.value).toBe(900)

    expect(
      planMovementGroups(a, b, 'score').map((group) => group.rows[0]),
    ).toEqual([
      { id: 'score', label: '', change: 12.5, valueLabel: '+12.5%' },
      {
        id: 'score',
        label: '',
        change: 16.666666666666664,
        valueLabel: '+16.67%',
      },
    ])
    expect(
      planMovementGroups(a, b, 'totalTokens').map(
        (group) => group.rows[0].change,
      ),
    ).toEqual([-16.666666666666664, 0])
    expect(
      planMovementGroups(a, b, 'turns').map((group) => group.rows[0].change),
    ).toEqual([-50, -50])
    expect(
      planMovementGroups(a, b, 'durationMs').map(
        (group) => group.rows[0].valueLabel,
      ),
    ).toEqual(['−16.67%', '0%'])
  })

  it('keeps zero, missing and the shared score filter distinct in the selected chart', () => {
    const a = recordedMetrics([
      { key: 'zero', values: { score: 0, totalTokens: 0 } },
      { key: 'missing', values: { score: 40, totalTokens: null } },
      { key: 'unscored', values: { score: 60, totalTokens: 20 } },
    ])
    const b = recordedMetrics([
      { key: 'zero', values: { score: 10, totalTokens: 10 } },
      { key: 'missing', values: { score: 40, totalTokens: 1 } },
      { key: 'unscored', values: { score: null, totalTokens: 30 } },
    ])
    expect(
      planMovementGroups(a, b, 'score').map(
        (group) => group.rows[0].valueLabel,
      ),
    ).toEqual(['0%', 'Not comparable', '+10 pts · A is zero'])
    expect(
      planMovementGroups(a, b, 'totalTokens').map(
        (group) => group.rows[0].valueLabel,
      ),
    ).toEqual(['Not comparable', '+50%', '+10 · A is zero'])
    expect(
      planMovementGroups(a, b, 'totalTokens', true).map((group) => group.title),
    ).toEqual(['Missing'])
  })

  it('plots every execution over the selected pair’s filtered test scope', () => {
    const plan: LocalPlan = {
      ...candidateRunningPlan,
      state: 'comparison_ready',
      candidate_execution_ids: ['candidate-1', 'candidate-2'],
    }
    const metrics = {
      'baseline-1': recordedMetrics([
        { key: 'x', values: { score: 100, totalTokens: 100 } },
        { key: 'y', values: { score: 50, totalTokens: 50 } },
      ]),
      'candidate-1': recordedMetrics([
        { key: 'x', values: { score: 90, totalTokens: 90 } },
        { key: 'y', values: { score: 0, totalTokens: 0 } },
      ]),
      'candidate-2': recordedMetrics([
        { key: 'x', values: { score: 0, totalTokens: 0 } },
        { key: 'y', values: { score: 80, totalTokens: 80 } },
      ]),
    }
    const model = buildPlanComparisonModel({
      plan,
      summaries: {
        'baseline-1': execution('baseline-1'),
        'candidate-1': execution('candidate-1'),
        'candidate-2': execution('candidate-2'),
      },
      visualBaselineId: 'baseline-1',
      comparisonCandidateIds: ['candidate-1'],
      selectedCandidateId: 'candidate-1',
    })
    if (!model) throw new Error('comparison model unavailable')
    const tokens = planTrendTiles(
      plan,
      model,
      'baseline-1',
      metrics,
      true,
    ).find((tile) => tile.id === 'totalTokens')
    expect(tokens?.points.map(({ id, value }) => [id, value])).toEqual([
      ['baseline-1', 100],
      ['candidate-1', 90],
      ['candidate-2', 0],
    ])
  })

  it('shows criterion evidence and consumption without highlighting winners', () => {
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
                behavior_sha256:
                  'sha256:c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3',
                pass_rate: 100,
              },
            ],
          },
        ] as never,
        scenario_metrics: [
          {
            scenario_id: 'security_review',
            behavior_sha256:
              'sha256:c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3',
            contract_fingerprint: 'security-contract',
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
    const comparison = buildPlanComparison(baseline, candidateTwo)
    comparison.scenarios[0].metrics.push({
      id: 'criterion:delivery:40',
      label: 'Criterion delivery · mean points / 40',
      baseline: 15,
      candidate: 30,
      delta: 15,
      delta_percent: 100,

      format: 'score',
      tone: 'neutral',
      evidence: {
        baseline_observed: 2,
        candidate_observed: 1,
        baseline_planned: 2,
        candidate_planned: 2,
        paired: 1,
        paired_baseline: 15,
        paired_candidate: 30,
      },
    })
    const html = renderToStaticMarkup(
      <PlanComparisonLayers
        plan={{
          ...candidateRunningPlan,
          state: 'comparison_ready',
          candidate_execution_ids: ['candidate-1', 'candidate-2'],
          candidate_labels: {
            'candidate-1': 'Candidate #1',
            'candidate-2': 'Candidate #2',
          },
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
        scenarioComparison={comparison}
      />,
    )
    const scenarioHtml = html.slice(html.indexOf('data-plan-by-test'))

    expect(scenarioHtml).toContain('data-execution-id="candidate-1"')
    expect(scenarioHtml).toContain('data-execution-id="candidate-2"')
    expect(scenarioHtml).toContain('2 candidates')
    expect(scenarioHtml.match(/data-scenario-metric-id=/g)).toHaveLength(9)
    expect(scenarioHtml).toContain(
      'data-scenario-metric-id="criterion:delivery:40"',
    )
    expect(scenarioHtml).toContain('Criterion delivery · mean points / 40')
    expect(scenarioHtml).toContain('1 matched repetitions')
    expect(scenarioHtml).toContain('paired means 15 → 30')
    for (const metricId of [
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
    // Values and paired evidence remain visible without ranking candidates.
    expect(scenarioHtml).toMatch(
      /data-scenario-metric-id="tokens"[\s\S]*?<td data-label="Candidate #2"/,
    )
    expect(scenarioHtml).toContain('data-scenario-metrics')
    expect(scenarioHtml).not.toContain('is-winner')
    expect(scenarioHtml).not.toContain('Best')
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
        label: 'Candidate two',
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
    expect(html).toContain('Candidate two')
    expect(html).toContain('never changes here')
    expect(html.match(/data-candidate-option="selected"/g)).toHaveLength(2)
    // The reference point in every sparkline is the visual baseline.
    expect(html).toMatch(
      /data-point-role="baseline"[\s\S]*?<title>Candidate two · /,
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
          behavior_sha256:
            'sha256:a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1',
          case_id: 'case-a',
          seed: 7,
          inputs_sha256: 'sha256:1111111111111111111111',
          contract_sha256: 'sha256:2222222222222222222222',
        },
      ],
    }
    const html = renderToStaticMarkup(<PlanScope plan={plan} />)
    expect(html).toContain('data-plan-scope')
    expect(html).toContain('Test scope')
    expect(html).toContain('Minimal Path')
    expect(html).toContain('a1a1a1a1')
    expect(html).toContain('per test')
    expect(html).toContain('Technical retries')
    expect(html).toContain('Canonical')
    expect(html).not.toContain('baseline captured')
    expect(html).not.toContain('example.invalid')

    const entries = planProvenanceEntries(plan)
    expect(entries).toContainEqual([
      'endpoint',
      'https://example.invalid/catalog',
    ])
    expect(entries).toContainEqual(['scope hash', 'sha256:scope'])
    expect(entries).toContainEqual([
      'minimal_path · a1a1a1a1',
      'case case-a · seed 7 · contract sha256:222222222222… · inputs sha256:111111111111…',
    ])
    expect(planProvenanceScent(plan)).toContain(
      'plan-1 · scope sha256:scope · endpoint https://example.invalid/catalog',
    )
  })
})
