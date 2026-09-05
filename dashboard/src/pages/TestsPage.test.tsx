import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { TestCatalogRow, TestSideSummary } from '@/lib/test-catalog'
import {
  comparisonHasNoOverlap,
  comparisonVerdict,
  matchesCompareFilter,
  oneSidedTests,
  RowDetails,
  rowState,
  SideResult,
  sortCompareRows,
} from '@/pages/TestsPage'

function side(overrides: Partial<TestSideSummary> = {}): TestSideSummary {
  return {
    evaluated_version_id: 'legacy-version',
    execution_count: 1,
    total_runs: 1,
    scored_runs: 1,
    case_count: 1,
    median_score: 100,
    pass_rate: 1,
    median_cost_usd: null,
    median_tokens: null,
    median_duration_seconds: null,
    outcomes: {
      passed: 1,
      hard_gate_failed: 0,
      technical_failed: 0,
      infra_failed: 0,
    },
    samples: { score: 1, cost_usd: 0, tokens: 0, duration_seconds: 0 },
    ...overrides,
  }
}

function row(
  from: TestSideSummary | null,
  to: TestSideSummary | null,
  overrides: Partial<TestCatalogRow> = {},
  compatibility: TestCatalogRow['result'] extends infer R
    ? R extends { compatibility: infer C }
      ? C
      : never
    : never = 'compatible',
): TestCatalogRow {
  return {
    test_id: 'direct_answer',
    lifecycle: 'active',
    current_version: 2,
    available_versions: [],
    selected_version: 2,
    result:
      from || to
        ? {
            test_id: 'direct_answer',
            test_version: 2,
            compatibility,
            compatibility_reasons: [],
            from,
            to,
            delta: {
              score:
                from &&
                to &&
                from.median_score !== null &&
                to.median_score !== null
                  ? to.median_score - from.median_score
                  : null,
              cost_usd: null,
              tokens: null,
              duration_seconds: null,
            },
            from_observations: [],
            to_observations: [],
          }
        : null,
    ...overrides,
  }
}

describe('versioned test side presentation', () => {
  // Audit CP-04 / CP-16: one line per side; nothing is invented for legacy
  // summaries without assessments.
  it('renders retained legacy summaries as one status line', () => {
    const html = renderToStaticMarkup(<SideResult summary={side()} />)
    expect(html).toContain('ds-status-passed')
    expect(html).toContain('>passed<')
    expect(html).toContain('100')
    expect(html).toContain('n=1')
    expect(html).not.toContain('ai:')
    expect(html).not.toContain('Effective')
    expect(html).not.toContain('evidence references')
  })

  it('names a missing side plainly', () => {
    expect(renderToStaticMarkup(<SideResult summary={null} />)).toContain(
      'no evidence',
    )
  })
})

describe('comparison row states', () => {
  // Audit CP-01: one state per row decides the group and the default filter.
  it('classifies rows and sorts comparable first', () => {
    const regressed = row(side(), side({ median_score: 60 }))
    const improved = row(side({ median_score: 60 }), side())
    const oneSide = row(null, side(), { test_id: 'b_only' })
    const none = row(null, null, { test_id: 'nothing' })
    const changed = row(
      side(),
      side(),
      { test_id: 'changed' },
      'contract_changed',
    )
    expect(rowState(regressed)).toBe('regressed')
    expect(rowState(improved)).toBe('improved')
    expect(rowState(row(side(), side()))).toBe('unchanged')
    expect(rowState(oneSide)).toBe('one_side')
    expect(rowState(none)).toBe('none')
    expect(rowState(changed)).toBe('changed')
    expect(
      sortCompareRows([none, oneSide, changed, improved, regressed]).map(
        rowState,
      ),
    ).toEqual(['regressed', 'improved', 'changed', 'one_side', 'none'])
    expect(matchesCompareFilter('none', 'evidence')).toBe(false)
    expect(matchesCompareFilter('one_side', 'evidence')).toBe(true)
    expect(matchesCompareFilter('unchanged', 'comparable')).toBe(true)
    expect(matchesCompareFilter('changed', 'comparable')).toBe(false)
  })

  // Audit CP-19: tiles only with data; the evidence list names the side.
  it('hides tiles that are dashes on both sides', () => {
    const html = renderToStaticMarkup(
      <RowDetails
        result={row(side({ median_cost_usd: 0.02 }), side()).result}
        aLabel="source a37f82be"
        bLabel="source bc26991d"
      />,
    )
    expect(html).toContain('pass rate')
    expect(html).toContain('median cost')
    expect(html).not.toContain('median duration')
    expect(html).not.toContain('median tokens')
    expect(html).toContain('No retained observations.')
    expect(html).toContain('open history')
  })

  // Audit CP-20: two sides that share nothing produce a table of empty delta
  // columns. That is a state to name, not a comparison to render.
  it('separates a comparison with no overlap from an empty cohort', () => {
    expect(comparisonHasNoOverlap(8, 0)).toBe(true)
    expect(comparisonHasNoOverlap(12, 5)).toBe(false)
    // Nothing ran at all: the empty state already covers it.
    expect(comparisonHasNoOverlap(0, 0)).toBe(false)
  })

  // Audit CP-23: the page exists to answer one question, so it answers it
  // rather than leaving three chip counts to be read in the right direction.
  it('states which side is behind, and never calls a tie a regression', () => {
    expect(
      comparisonVerdict({ regressed: 3, improved: 2, unchanged: 7 }),
    ).toEqual({
      tone: 'negative',
      headline: 'b is behind a on 3 of 12 comparable tests',
    })
    // A regression outranks an improvement: it is the one worth acting on.
    expect(
      comparisonVerdict({ regressed: 1, improved: 5, unchanged: 0 })?.tone,
    ).toBe('negative')
    expect(
      comparisonVerdict({ regressed: 0, improved: 2, unchanged: 1 }),
    ).toEqual({
      tone: 'positive',
      headline: 'b is ahead of a on 2 of 3 comparable tests',
    })
    expect(
      comparisonVerdict({ regressed: 0, improved: 0, unchanged: 1 }),
    ).toEqual({
      tone: 'neutral',
      headline: 'b matches a on all 1 comparable test',
    })
    // Nothing comparable is not a verdict; the no-overlap callout covers it.
    expect(
      comparisonVerdict({ regressed: 0, improved: 0, unchanged: 0 }),
    ).toBeNull()
  })

  // Audit CP-24: naming which side holds what is what decides which side is
  // worth running, and it is the input to the two recovery actions.
  it('names which side ran what, and counts a shared test as neither', () => {
    const rows = [
      { ...row(side(), null), test_id: 'only_a' },
      { ...row(null, side()), test_id: 'only_b' },
      { ...row(side(), side()), test_id: 'both' },
      { ...row(null, null), test_id: 'neither' },
    ]
    expect(oneSidedTests(rows)).toEqual({
      onlyOnA: ['only_a'],
      onlyOnB: ['only_b'],
    })
  })
})
