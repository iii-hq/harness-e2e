import type { TestCatalogRow, TestVersionResult } from '@/lib/test-catalog'

export type ResultFilter = 'all' | 'passed' | 'issues' | 'missing' | 'changed'

export type ComparisonUtility = {
  comparable: number
  evidenceOnBothSides: number
  evidenceRows: number
}

export type ComparisonWarning = {
  title: string
  detail: string
}

const REASON_WARNINGS: Record<string, ComparisonWarning> = {
  comparison_side_missing: {
    title: 'Evidence is missing on one side',
    detail:
      'A delta would compare unlike samples, so only the retained side is shown.',
  },
  scenario_contract_changed: {
    title: 'Scenario contract changed',
    detail:
      'The canonical cases, scenario schema/version, or execution policy differ between versions. Scores and deltas are not comparable.',
  },
  scenario_contract_conflict: {
    title: 'Scenario contract conflict',
    detail:
      'At least one side contains conflicting case or scenario identities. Resolve the retained evidence before comparing.',
  },
  assessment_profile_changed: {
    title: 'Assessment profile changed',
    detail:
      'The scenario version or assessment definition differs. Scenario version is the compatibility boundary for prompt and rubric changes.',
  },
  assessment_profile_conflict: {
    title: 'Assessment profile conflict',
    detail:
      'At least one side contains multiple assessment definitions for the same scenario version.',
  },
  analyzer_profile_changed: {
    title: 'Analyzer profile changed',
    detail:
      'The analyzer, provider, or model differs between versions. AI conclusions are shown but their score delta is disabled.',
  },
  analyzer_profile_conflict: {
    title: 'Analyzer profile conflict',
    detail:
      'At least one side contains multiple analyzer, provider, or model identities for the same test version.',
  },
  cohort_changed: {
    title: 'Evaluation cohort changed',
    detail:
      'Subject, judge, model, protocol, or lane identity differs. Cross-cohort deltas are not valid.',
  },
  missing_side: {
    title: 'Evidence is missing on one side',
    detail:
      'A delta would compare unlike samples, so only the retained side is shown.',
  },
  contract_changed: {
    title: 'Scenario contract changed',
    detail:
      'The canonical cases, scenario schema/version, or execution policy differ between versions. Scores and deltas are not comparable.',
  },
  contract_conflict: {
    title: 'Scenario contract conflict',
    detail:
      'At least one side contains conflicting case or scenario identities. Resolve the retained evidence before comparing.',
  },
  assessment_changed: {
    title: 'Assessment profile changed',
    detail:
      'The scenario version or assessment definition differs. Scenario version is the compatibility boundary for prompt and rubric changes.',
  },
  assessment_conflict: {
    title: 'Assessment profile conflict',
    detail:
      'At least one side contains multiple assessment definitions for the same scenario version.',
  },
  analyzer_changed: {
    title: 'Analyzer profile changed',
    detail:
      'The analyzer, provider, or model differs between versions. AI conclusions are shown but their score delta is disabled.',
  },
  analyzer_conflict: {
    title: 'Analyzer profile conflict',
    detail:
      'At least one side contains multiple analyzer, provider, or model identities for the same test version.',
  },
}

export function comparisonWarnings(
  result: TestVersionResult | null,
): ComparisonWarning[] {
  if (!result || result.compatibility === 'compatible') return []
  const reasons =
    result.compatibility_reasons.length > 0
      ? result.compatibility_reasons
      : [result.compatibility]
  return reasons.map(
    (reason) =>
      REASON_WARNINGS[reason] ?? {
        title: 'Comparison is incompatible',
        detail: `The retained data reported ${reason.replaceAll('_', ' ')}. No delta is calculated.`,
      },
  )
}

function hasIssuesInB(row: TestCatalogRow) {
  const to = row.result?.to
  if (!to) return false
  return (
    to.outcomes.hard_gate_failed +
      to.outcomes.technical_failed +
      to.outcomes.infra_failed >
    0
  )
}

export function hasRetainedEvidence(row: TestCatalogRow) {
  return Boolean(row.result?.from || row.result?.to)
}

export function matchesResultFilter(row: TestCatalogRow, filter: ResultFilter) {
  if (filter === 'all') return true
  const result = row.result
  if (filter === 'missing') return result?.compatibility === 'missing_side'
  if (filter === 'changed') {
    return (
      result?.compatibility === 'contract_changed' ||
      result?.compatibility === 'contract_conflict' ||
      result?.compatibility === 'assessment_changed' ||
      result?.compatibility === 'assessment_conflict' ||
      result?.compatibility === 'analyzer_changed' ||
      result?.compatibility === 'analyzer_conflict'
    )
  }
  if (!result?.to) return false
  return filter === 'issues' ? hasIssuesInB(row) : !hasIssuesInB(row)
}

function usefulness(row: TestCatalogRow) {
  const result = row.result
  const hasFrom = Boolean(result?.from)
  const hasTo = Boolean(result?.to)
  const hasBoth = hasFrom && hasTo
  if (hasBoth && result?.compatibility === 'compatible') {
    return hasIssuesInB(row) ? 0 : 1
  }
  if (hasBoth) return 2
  if (hasFrom || hasTo) return hasIssuesInB(row) ? 3 : 4
  return row.lifecycle === 'never_run' ? 6 : 5
}

export function sortCatalogRows(rows: TestCatalogRow[]) {
  return [...rows].sort(
    (left, right) =>
      usefulness(left) - usefulness(right) ||
      left.test_id.localeCompare(right.test_id),
  )
}

export function comparisonUtility(rows: TestCatalogRow[]): ComparisonUtility {
  return rows.reduce<ComparisonUtility>(
    (summary, row) => {
      const result = row.result
      if (result?.compatibility === 'compatible') summary.comparable += 1
      if (result?.from && result.to) summary.evidenceOnBothSides += 1
      if (result?.from || result?.to) summary.evidenceRows += 1
      return summary
    },
    { comparable: 0, evidenceOnBothSides: 0, evidenceRows: 0 },
  )
}

export function isMoreUsefulComparison(
  candidate: ComparisonUtility,
  current: ComparisonUtility,
) {
  return (
    candidate.comparable > current.comparable ||
    (candidate.comparable === current.comparable &&
      candidate.evidenceOnBothSides > current.evidenceOnBothSides) ||
    (candidate.comparable === current.comparable &&
      candidate.evidenceOnBothSides === current.evidenceOnBothSides &&
      candidate.evidenceRows > current.evidenceRows)
  )
}
