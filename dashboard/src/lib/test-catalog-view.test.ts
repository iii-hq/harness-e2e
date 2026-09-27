import { describe, expect, it } from 'vitest'
import type { TestCatalogRow, TestSideSummary } from '@/lib/test-catalog'
import {
  CATALOG_DEFAULT_FILTERS,
  catalogFiltersFromParams,
  catalogFiltersToParams,
  catalogRowView,
  catalogSegments,
  catalogSummary,
  comparisonUtility,
  comparisonWarnings,
  filterCatalog,
  groupCatalog,
  hasRetainedEvidence,
  isMoreUsefulComparison,
  matchesResultFilter,
  sortCatalogRows,
  suitesByTest,
} from '@/lib/test-catalog-view'

function side(issue = false): TestSideSummary {
  return {
    evaluated_version_id: 'version',
    execution_count: 1,
    total_runs: 1,
    scored_runs: 1,
    case_count: 1,
    mean_score: issue ? 0 : 100,
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
        hard_gate_failed: 0,
        subject_error: 0,
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
    current_version:
      'sha256:a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1',
    available_versions: [],
    selected_version:
      'sha256:a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1',
    result: {
      test_id: testId,
      test_version:
        'sha256:a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1',
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
    last_run: null,
    recent_scores: [],
    runs_current: 0,
    runs_total: 0,
  }
}

describe('definition-scoped test catalog view', () => {
  const comparable = row('direct_answer', 'compatible', side(), side())
  const changed = row('persistent_state', 'contract_changed', side(), side())
  const oneSided = row('reactive_automation', 'missing_side', null, side(true))
  const neverRun = row('todo_worker_simple', 'missing_side', null, null)

  it('keeps changed contracts separate from missing evidence', () => {
    expect(matchesResultFilter(changed, 'changed')).toBe(true)
    expect(matchesResultFilter(oneSided, 'changed')).toBe(false)
    expect(matchesResultFilter(oneSided, 'missing')).toBe(true)
    expect(matchesResultFilter(oneSided, 'issues')).toBe(false)
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
      'todo_worker_simple',
    ])
    expect(hasRetainedEvidence(neverRun)).toBe(false)
    expect(hasRetainedEvidence(oneSided)).toBe(true)
  })

  it('prefers the system pair with shared canonical evidence', () => {
    const weak = comparisonUtility([changed, neverRun])
    const useful = comparisonUtility([comparable, oneSided])
    expect(isMoreUsefulComparison(useful, weak)).toBe(true)
    expect(isMoreUsefulComparison(weak, useful)).toBe(false)
  })

  it('explains contract and assessment incompatibilities without a score delta', () => {
    if (!comparable.result) throw new Error('missing fixture result')
    const result = {
      ...comparable.result,
      compatibility: 'assessment_changed' as const,
      compatibility_reasons: [
        'assessment_profile_changed',
        'scenario_contract_changed',
      ],
    }
    expect(comparisonWarnings(result)).toEqual([
      expect.objectContaining({
        title: 'Assessment profile changed',
        detail: expect.stringContaining('prompt and rubric'),
      }),
      expect.objectContaining({
        title: 'Scenario contract changed',
        detail: expect.stringContaining('canonical cases'),
      }),
    ])
  })
})

describe('tests catalog', () => {
  const now = new Date('2026-09-27T12:00:00Z')
  const test = (
    id: string,
    facts: Partial<TestCatalogRow> = {},
  ): TestCatalogRow => ({
    ...row(id, 'missing_side', null, null),
    result: null,
    spec: {
      prompt: '',
      criteria: Array.from({ length: 4 }, (_, index) => ({
        id: `c${index}`,
        weight: 25,
        description: '',
        kind: 'deterministic',
        policy: 'advisory',
        dimension: 'correctness',
      })) as never,
      execution: { max_turns: null, stuck_timeout_seconds: 10 },
      denied_functions: [],
    },
    ...facts,
  })
  const current = test('kanban_c1_foundation', {
    last_run: {
      at: '2026-09-24T10:00:00Z',
      score: 80,
      status: 'passed',
      completion: 'task_incomplete',
      definition: 'current',
    },
    recent_scores: [null, 100, 80],
    runs_current: 3,
    runs_total: 5,
  })
  const changed = test('kanban_c2_persistence', {
    characterization: { human_horizon: { min_minutes: 90, max_minutes: 180 } },
    last_run: {
      at: '2026-09-08T10:00:00Z',
      score: 100,
      status: 'passed',
      completion: 'completed',
      definition: 'previous',
    },
    recent_scores: [100],
    runs_total: 1,
  })
  const never = test('minimal_path')
  const suites = suitesByTest([
    { id: 'pr', label: 'PR', scenarios: ['minimal_path'] },
    {
      id: 'software-engineering',
      label: 'Software engineering',
      scenarios: ['kanban_c1_foundation', 'minimal_path'],
    },
  ])
  const views = [current, changed, never].map((entry) =>
    catalogRowView(entry, suites.get(entry.test_id) ?? [], now),
  )

  it('writes each row: result, score, date, faded older runs, runs and suites', () => {
    const [ran, moved, fresh] = views
    expect(ran).toMatchObject({
      kind: 'current',
      sub: '4 criteria',
      result: 'incomplete',
      score: '80',
      when: 'Sep 24',
      older: false,
      spark: [null, 1, 0.8],
      sparkLabel: 'Recent scores: none, 100, 80',
      runs: 3,
      runsTitle: '3 on the current definition · 5 retained in all',
      suites: [{ id: 'software-engineering', label: 'Software engineering' }],
    })
    expect(moved).toMatchObject({
      kind: 'changed',
      sub: '4 criteria · human 1.5–3 h',
      result: 'passed',
      when: 'Sep 8 · older definition',
      older: true,
      runs: 0,
    })
    expect(fresh).toMatchObject({
      kind: 'never',
      result: 'never_run',
      score: null,
      when: 'No run retained',
      spark: [],
      sparkLabel: 'No scores',
    })
    // The summary, when there is one, instead of the criteria.
    expect(
      catalogRowView(
        test('chess_engine_build', {
          spec: current.spec && {
            ...current.spec,
            summary: ' Build a worker. ',
          },
        }),
        [],
        now,
      ).sub,
    ).toBe('Build a worker.')
  })

  it('counts each lifecycle over the whole catalog and says so', () => {
    expect(
      catalogSegments(views).map((segment) => [segment.label, segment.count]),
    ).toEqual([
      ['All', 3],
      ['Current', 1],
      ['Definition changed', 1],
      ['Never run', 1],
    ])
    expect(catalogSummary(views)).toBe(
      '3 tests · 1 with a current result · 1 changed since they last ran · 1 never run',
    )
  })

  it('filters by lifecycle, suite and a search over id, summary and suites', () => {
    const ids = (filters: Partial<typeof CATALOG_DEFAULT_FILTERS>) =>
      filterCatalog(views, { ...CATALOG_DEFAULT_FILTERS, ...filters }).map(
        (view) => view.id,
      )
    expect(ids({ filter: 'changed' })).toEqual(['kanban_c2_persistence'])
    expect(ids({ suite: 'pr' })).toEqual(['minimal_path'])
    expect(ids({ query: 'software' })).toEqual([
      'kanban_c1_foundation',
      'minimal_path',
    ])
    expect(ids({ sort: 'last_run' })).toEqual([
      'kanban_c1_foundation',
      'kanban_c2_persistence',
      'minimal_path',
    ])
    expect(ids({ sort: 'runs' })[0]).toBe('kanban_c1_foundation')
  })

  it('groups by family, Standalone last, noting how much of it is current', () => {
    const groups = groupCatalog(views, views)
    expect(
      groups.map((group) => [
        group.label,
        group.note,
        group.rows.map((view) => view.id),
      ]),
    ).toEqual([
      [
        'kanban',
        '1 of 2 current',
        ['kanban_c1_foundation', 'kanban_c2_persistence'],
      ],
      ['Standalone', 'none current', ['minimal_path']],
    ])
    // A family keeps its note over the whole catalog; empty groups go.
    expect(
      groupCatalog(views, [views[1]]).map((group) => [group.label, group.note]),
    ).toEqual([['kanban', '1 of 2 current']])
  })

  it('reads the filters from the hash and writes back only what differs', () => {
    const filters = catalogFiltersFromParams(
      new URLSearchParams('q=chess&filter=changed&suite=pr&sort=runs&x=1'),
    )
    expect(filters).toEqual({
      query: 'chess',
      filter: 'changed',
      suite: 'pr',
      sort: 'runs',
    })
    expect(catalogFiltersToParams(filters).toString()).toBe(
      'q=chess&filter=changed&suite=pr&sort=runs',
    )
    expect(
      catalogFiltersFromParams(new URLSearchParams('filter=toString&sort=x')),
    ).toEqual(CATALOG_DEFAULT_FILTERS)
    expect(catalogFiltersToParams(CATALOG_DEFAULT_FILTERS).toString()).toBe('')
  })
})
