import type { TestCatalogRow } from '@/lib/test-catalog'

export type ResultFilter = 'all' | 'passed' | 'issues' | 'missing' | 'changed'

export type ComparisonUtility = {
  comparable: number
  evidenceOnBothSides: number
  evidenceRows: number
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
