import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { TestHistoryResponse, TestObservation } from '@/lib/test-catalog'
import {
  historyStateFromParams,
  historyStateToParams,
  ObservationComparisonPanel,
  ScoreTrendChart,
  statusPresentation,
  versionStatement,
} from '@/pages/TestHistoryPage'

function observation(
  overrides: Partial<TestObservation> = {},
): TestObservation {
  return {
    execution_id: 'baseline',
    evaluated_version_id: 'system-a',
    cohort_id: 'same-cohort',
    completed_at: '2026-08-21T16:51:00Z',
    case_id: 'direct_answer:v2:seed-1',
    contract_sha256: 'contract',
    assessment_profile_sha256: 'assessment',
    analyzer_profile_sha256: 'analyzer',
    status: 'passed',
    median_score: 100,
    run_count: 1,
    scored_runs: 1,
    scenario_version: 2,
    seed: 1,
    stack_mode: 'source',
    subject_provider: 'openai',
    subject_model: 'gpt-5',
    judge_provider: 'openai',
    judge_model: 'gpt-5-judge',
    judge_protocol: 'assessment-json',
    median_cost_usd: 0.04,
    median_tokens: 1449,
    median_duration_seconds: 6.4,
    median_function_calls: 4,
    median_function_call_errors: 0,
    median_turns: 1,
    ...overrides,
  }
}

function history(
  overrides: Partial<TestHistoryResponse> = {},
): TestHistoryResponse {
  return {
    test_id: 'chess_play_ladder',
    test_version: 1,
    current_version: 3,
    available_versions: [
      {
        version: 1,
        execution_count: 1,
        run_count: 1,
        last_seen: '2026-08-21T16:51:00Z',
      },
      { version: 3, execution_count: 0, run_count: 0, last_seen: null },
    ],
    cases: [],
    subjects: [],
    subject_models: [],
    judge_models: [],
    systems: [],
    series: [],
    observations: [],
    total: 1,
    next_cursor: null,
    ...overrides,
  }
}

describe('test history comparison', () => {
  // Audit CP-05 / TH-12: signed, directional deltas on cards; no impact table.
  it('shows a verdict and signed deltas for comparable executions', () => {
    const html = renderToStaticMarkup(
      <ObservationComparisonPanel
        baseline={observation()}
        candidate={observation({
          execution_id: 'candidate',
          completed_at: '2026-08-22T16:51:00Z',
          status: 'hard_gate_failed',
          median_score: 60,
          median_tokens: 2900,
          median_duration_seconds: 3.2,
          median_function_call_errors: 1,
        })}
        testId="direct_answer"
        onClear={() => undefined}
        onSwap={() => undefined}
      />,
    )
    expect(html).toContain('data-test-comparison')
    expect(html).toContain('>mixed result<')
    expect(html).toContain('no longer passes · score down · duration down')
    expect(html).toContain('data-comparison-metric="score"')
    expect(html).toContain('ds-delta-negative')
    expect(html).toContain('ds-delta-positive')
    expect(html).toContain('−40 pts')
    expect(html).toContain('−50%')
    expect(html).not.toContain('Impact by scenario')
    expect(html).not.toContain('Delta not interpreted')
    expect(html).not.toContain('tmh-')
  })

  // Audit TH-11: one banner for an incompatible pair; values shown, no deltas.
  it('keeps incompatible pairs to one banner without interpreted deltas', () => {
    const html = renderToStaticMarkup(
      <ObservationComparisonPanel
        baseline={observation()}
        candidate={observation({
          execution_id: 'candidate',
          seed: 2,
          median_cost_usd: null,
          median_score: 95,
        })}
        testId="direct_answer"
        onClear={() => undefined}
        onSwap={() => undefined}
      />,
    )
    expect(html).toContain(
      'not comparable · values shown, deltas not interpreted',
    )
    expect((html.match(/class="ds-callout /g) ?? []).length).toBe(1)
    expect(html).toContain('Seed differs')
    expect(html).not.toContain('ds-delta-negative')
    expect(html).not.toContain('ds-delta-positive')
    // Audit CP-19: a metric with a value on one side only stays; none on both hides.
    expect(html).toContain('data-comparison-metric="cost"')
    expect(html).toContain('a → b')
  })

  it('renders the trend with green passes, red failures and a/b chips', () => {
    const html = renderToStaticMarkup(
      <ScoreTrendChart
        observations={[
          observation({
            execution_id: 'two',
            completed_at: '2026-08-22T00:00:00Z',
            status: 'hard_gate_failed',
            median_score: 40,
          }),
          observation(),
        ]}
        selectedKeys={[]}
        onSelect={() => undefined}
      />,
    )
    expect(html).toContain('data-score-trend')
    expect(html).toContain('fill-success')
    expect(html).toContain('fill-danger')
    expect(html).not.toContain('tmh-')
  })
})

describe('test history page state', () => {
  // Audit TH-07: the shown version and the current contract are both named.
  it('states which version is shown and whether the contract moved on', () => {
    expect(versionStatement(history())).toBe(
      'showing v1 (latest with executions) · current contract v3 has no executions yet',
    )
    expect(versionStatement(history({ current_version: 1 }))).toBe(
      'contract v1 · current',
    )
    expect(versionStatement(history({ current_version: undefined }))).toBe(
      'contract v1',
    )
  })

  // Audit TH-05: passed is the success status, never the accent.
  it('maps results onto the design-system statuses', () => {
    expect(statusPresentation('passed')).toEqual({
      status: 'passed',
      label: 'passed',
    })
    expect(statusPresentation('hard_gate_failed')).toEqual({
      status: 'hard_gate',
      label: 'hard gate failed',
    })
    expect(statusPresentation('technical_failed').status).toBe('failed')
  })

  // Audit TH-19: filters, a/b and the open dialog round-trip through the hash.
  it('round-trips the page state through the hash params', () => {
    const state = historyStateFromParams(
      new URLSearchParams('version=2&result=failed&a=k1&b=k2&open=k1'),
    )
    expect(state.filters).toMatchObject({
      version: 2,
      result: 'failed',
      model: '',
    })
    expect(state.comparisonKeys).toEqual(['k1', 'k2'])
    expect(state.open).toBe('k1')
    expect(
      historyStateToParams(
        state.filters,
        state.comparisonKeys,
        state.open,
      ).toString(),
    ).toBe('version=2&result=failed&a=k1&b=k2&open=k1')
  })
})
