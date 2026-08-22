import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { TestObservation } from '@/lib/test-catalog'
import { ObservationComparisonPanel } from '@/pages/TestHistoryPage'

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

describe('test history comparison workspace', () => {
  it('renders all seven metric cards and the scenario impact table', () => {
    const html = renderToStaticMarkup(
      <ObservationComparisonPanel
        baseline={observation()}
        candidate={observation({
          execution_id: 'candidate',
          completed_at: '2026-08-21T16:36:00Z',
          median_duration_seconds: 23.9,
          median_tokens: 1401,
          median_function_calls: 6,
          median_function_call_errors: 1,
        })}
        testId="direct_answer"
        onClear={() => undefined}
        onSwap={() => undefined}
      />,
    )

    for (const label of [
      'Score',
      'Duration',
      'Tokens',
      'Cost',
      'Functions',
      'Errors',
      'Turns',
    ]) {
      expect(html).toContain(`>${label}<`)
    }
    expect(html).toContain('Comparable executions')
    expect(html).toContain('Mixed result')
    expect(html).toContain('Impact by scenario')
    expect(html).toContain('direct_answer')
    expect(html).toContain(
      'tmh-impact-change tmh-impact-change-regressed">23.9s',
    )
    expect(html).toContain('tmh-impact-change tmh-impact-change-improved">1.4k')
    expect(html).toContain('tmh-impact-change tmh-impact-change-caution">6')
    expect(html).toContain('tmh-impact-change tmh-impact-change-regressed">1')
  })

  it('preserves missing metrics and avoids interpreting incompatible deltas', () => {
    const html = renderToStaticMarkup(
      <ObservationComparisonPanel
        baseline={observation({ median_function_calls: null })}
        candidate={observation({
          execution_id: 'candidate',
          seed: 2,
          median_function_calls: 5,
        })}
        testId="direct_answer"
        onClear={() => undefined}
        onSwap={() => undefined}
      />,
    )

    expect(html).toContain('Not comparable')
    expect(html).toContain('Seed differs')
    expect(html).toContain('Not reported')
    expect(html).toContain('Delta not interpreted')
    expect(html).toContain('tmh-impact-change tmh-impact-change-neutral">5')
  })
})
