import { describe, expect, it } from 'vitest'
import type { TestCatalogRow, TestSideSummary } from '@/lib/test-catalog'
import {
  comparisonUtility,
  hasRetainedEvidence,
  isMoreUsefulComparison,
  matchesResultFilter,
  sortCatalogRows,
} from '@/lib/test-catalog-view'

function side(issue = false): TestSideSummary {
  return {
    evaluated_version_id: 'version',
    execution_count: 1,
    total_runs: 1,
    scored_runs: 1,
    case_count: 1,
    median_score: issue ? 0 : 100,
    pass_rate: issue ? 0 : 1,
    median_cost_usd: null,
    median_tokens: null,
    median_duration_seconds: null,
    outcomes: {
      passed: issue ? 0 : 1,
      hard_gate_failed: issue ? 1 : 0,
      technical_failed: 0,
      infra_failed: 0,
    },
    samples: { score: 1, cost_usd: 0, tokens: 0, duration_seconds: 0 },
  }
}

function row(
  testId: string,
  compatibility: 'compatible' | 'missing_side' | 'contract_changed',
  from: TestSideSummary | null,
  to: TestSideSummary | null,
): TestCatalogRow {
  return {
    test_id: testId,
    lifecycle: from || to ? 'active' : 'never_run',
    current_version: 1,
    available_versions: [],
    selected_version: 1,
    result: {
      test_id: testId,
      test_version: 1,
      compatibility,
      from,
      to,
      delta: {
        score: null,
        cost_usd: null,
        tokens: null,
        duration_seconds: null,
      },
      from_observations: [],
      to_observations: [],
    },
  }
}

describe('versioned test catalog view', () => {
  const comparable = row('direct_answer', 'compatible', side(), side())
  const changed = row('persistent_state', 'contract_changed', side(), side())
  const oneSided = row('reactive_automation', 'missing_side', null, side(true))
  const neverRun = row('coordination.1', 'missing_side', null, null)

  it('keeps changed contracts separate from missing evidence', () => {
    expect(matchesResultFilter(changed, 'changed')).toBe(true)
    expect(matchesResultFilter(oneSided, 'changed')).toBe(false)
    expect(matchesResultFilter(oneSided, 'missing')).toBe(true)
    expect(matchesResultFilter(oneSided, 'issues')).toBe(true)
  })

  it('puts useful comparable evidence before empty catalog entries', () => {
    expect(
      sortCatalogRows([neverRun, oneSided, changed, comparable]).map(
        (item) => item.test_id,
      ),
    ).toEqual([
      'direct_answer',
      'persistent_state',
      'reactive_automation',
      'coordination.1',
    ])
    expect(hasRetainedEvidence(neverRun)).toBe(false)
    expect(hasRetainedEvidence(oneSided)).toBe(true)
  })

  it('prefers the version pair with shared canonical evidence', () => {
    const weak = comparisonUtility([changed, neverRun])
    const useful = comparisonUtility([comparable, oneSided])
    expect(isMoreUsefulComparison(useful, weak)).toBe(true)
    expect(isMoreUsefulComparison(weak, useful)).toBe(false)
  })
})
