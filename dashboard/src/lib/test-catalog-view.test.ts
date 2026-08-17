import { describe, expect, it } from 'vitest'
import type { TestCatalogRow, TestSideSummary } from '@/lib/test-catalog'
import {
  comparisonUtility,
  comparisonWarnings,
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
    assessment_summary: {
      run_count: 1,
      assessment_count: 0,
      asset_count: 0,
      evidence_reference_count: 0,
      system_statuses: {
        unavailable: 0,
        passed: 1,
        passed_with_concerns: 0,
        hard_gate_failed: 0,
        subject_error: 0,
        judge_error: 0,
        resource_limit: 0,
        infrastructure_error: 0,
      },
      effective_statuses: {
        unavailable: 0,
        passed: 1,
        passed_with_concerns: 0,
        hard_gate_failed: 0,
        subject_error: 0,
        judge_error: 0,
        resource_limit: 0,
        infrastructure_error: 0,
      },
      assessment_outcomes: {
        passed: 0,
        failed: 0,
        partial: 0,
        not_evaluated: 0,
        unavailable: 0,
        error: 0,
      },
      asset_qualitative_outcomes: {
        passed: 0,
        failed: 0,
        partial: 0,
        not_evaluated: 0,
        unavailable: 0,
        error: 0,
      },
      asset_validation_outcomes: {
        valid: 0,
        invalid: 0,
        malformed: 0,
        oversized: 0,
        not_produced: 0,
        unreadable: 0,
        unsafe_path: 0,
        removed_during_cleanup: 0,
        unexpected: 0,
        not_evaluated: 0,
      },
      ai_availability: {
        not_requested: 0,
        not_evaluated: 1,
        available: 0,
        unavailable: 0,
        malformed: 0,
        failed: 0,
      },
      ai_verdicts: {
        pass: 0,
        pass_with_concerns: 0,
        fail: 0,
        inconclusive: 0,
      },
      median_quality_score: null,
      median_confidence: null,
    },
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
      compatibility_reasons: [],
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

  it('explains assessment and analyzer incompatibilities without a score delta', () => {
    if (!comparable.result) throw new Error('missing fixture result')
    const result = {
      ...comparable.result,
      compatibility: 'assessment_changed' as const,
      compatibility_reasons: [
        'assessment_profile_changed',
        'analyzer_profile_changed',
      ],
    }
    expect(comparisonWarnings(result)).toEqual([
      expect.objectContaining({
        title: 'Assessment profile changed',
        detail: expect.stringContaining('prompt and rubric'),
      }),
      expect.objectContaining({
        title: 'Analyzer profile changed',
        detail: expect.stringContaining('provider, or model'),
      }),
    ])
  })
})
