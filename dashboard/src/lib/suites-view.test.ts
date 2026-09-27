import { describe, expect, it } from 'vitest'
import type {
  DashboardExecutionSummary,
  Suite,
} from '@/lib/dashboard-data-source'
import {
  changedTests,
  draftChanges,
  draftDirty,
  draftProblem,
  sequenceSteps,
  suiteDraft,
  suiteHolds,
  suiteListItem,
  suiteRuns,
  suitesSummary,
  testSuggestions,
  testsNote,
  tickDraft,
  upsertSuite,
} from '@/lib/suites-view'
import type { CatalogRowView } from '@/lib/test-catalog-view'

const NOW = new Date('2026-09-27T12:00:00Z')

function suite(id: string, facts: Partial<Suite> = {}): Suite {
  return {
    id,
    label: id,
    source: 'repository',
    purpose: '',
    scenarios: [],
    repetitions: 1,
    technical_retries: 0,
    sha256: null,
    updated_at: null,
    ...facts,
  }
}

function view(id: string, facts: Partial<CatalogRowView> = {}) {
  return {
    id,
    kind: 'current',
    selectable: true,
    search: id,
    ...facts,
  } as CatalogRowView
}

const GROUPS = [['registry_implementation', 'registry_verification']]

const PARAMETERS = {
  suite: { id: 'software-engineering', label: 'Software engineering' },
  scenarios: ['a', 'b'],
  runs: 1,
  technical_retries: 0,
  model: 'deepseek-flash',
  provider: 'deepseek',
  agent: 'ade-worker-builder',
}

function execution(
  id: string,
  facts: Partial<DashboardExecutionSummary> = {},
): DashboardExecutionSummary {
  return {
    id,
    status: 'failed',
    // Local time: the meta line reads the clock where it runs.
    completed_at: new Date(2026, 8, 24, 4, 25).toISOString(),
    subjects: [
      {
        id: 's',
        model: 'deepseek-flash',
        provider: 'deepseek',
        scenarios: [
          { id: 'a', mean_score: 90 },
          { id: 'b', mean_score: 96.4 },
        ],
      },
    ],
    totals: {
      expected_reports: 15,
      received_reports: 15,
      passed_scenarios: 11,
    },
    parameters: PARAMETERS,
    ...facts,
  }
}

describe('suites view', () => {
  it('sums up the suites and what one holds', () => {
    expect(
      suitesSummary([suite('regression'), suite('pr')]).startsWith(
        '2 in the repository, read-only · none in this Console · ',
      ),
    ).toBe(true)
    expect(
      suitesSummary([suite('pr'), suite('mine', { source: 'local' })]),
    ).toContain('1 in the repository, read-only · 1 in this Console')
    expect(suiteHolds(9, 1, 1)).toBe('9 tests · 1 run each · 1 retry')
    expect(suiteHolds(1, 3, 0)).toBe('1 test · 3 runs each · 0 retries')
  })

  it('counts only the tests whose definition changed since they ran', () => {
    const views = new Map([
      ['a', view('a', { kind: 'changed' })],
      ['b', view('b', { kind: 'never' })],
      ['c', view('c')],
    ])
    expect(changedTests(['a', 'b', 'c', 'unknown'], views)).toEqual(['a'])
    expect(
      suiteListItem(suite('s', { scenarios: ['a', 'b', 'c'] }), [], views),
    ).toEqual({
      id: 's',
      label: 's',
      count: '3 tests',
      last: null,
      changed: '1 of 3 changed since their last run',
    })
  })

  it('lists the executions of a suite, newest first as listed', () => {
    const runs = suiteRuns(
      [
        execution('plan-1', {
          source: { kind: 'github', run_id: 35965100994, url: 'u' },
        }),
        execution('plan-2', {
          parameters: { ...PARAMETERS, suite: null },
        }),
        execution('plan-3', {
          parameters: { ...PARAMETERS, suite: { id: 'pr', label: 'PR' } },
        }),
      ],
      'software-engineering',
      NOW,
    )
    expect(runs).toHaveLength(1)
    expect(runs[0]).toMatchObject({
      id: 'plan-1',
      meta: 'GitHub #35965100994 · Sep 24, 4:25 AM · deepseek/deepseek-flash · profile ade-worker-builder',
      result: { state: 'failed' },
      outcome: '11 of 15 passed · score 93.2',
      day: 'Sep 24',
    })
    expect(
      suiteListItem(
        suite('software-engineering', { scenarios: ['a', 'b'] }),
        runs,
        new Map(),
      ).last,
    ).toEqual({ state: 'failed', label: 'Failed · Sep 24' })
  })

  it('numbers the steps of a sequential group the suite runs whole', () => {
    const ids = [
      'kanban_c1',
      'registry_implementation',
      'registry_verification',
    ]
    expect([...sequenceSteps(ids, GROUPS)]).toEqual([
      ['registry_implementation', '1 of 2 · in order'],
      ['registry_verification', '2 of 2 · in order'],
    ])
    expect(sequenceSteps(['registry_implementation'], GROUPS).size).toBe(0)
    expect(testsNote(ids, GROUPS, false)).toBe(
      'Last result of each test in this Console. registry_implementation and registry_verification run whole, in order.',
    )
    expect(testsNote(['kanban_c1'], GROUPS, false)).toBe(
      'Last result of each test in this Console.',
    )
    expect(testsNote(ids, GROUPS, true)).toMatch(/^Untick to take a test out/)
  })

  it('edits a draft: unticked tests stay listed, groups follow their test', () => {
    const saved = suite('mine', {
      source: 'local',
      scenarios: ['a', 'registry_implementation', 'registry_verification'],
    })
    const draft = suiteDraft(saved)
    expect(draftDirty(draft, saved)).toBe(false)

    const unticked = tickDraft(draft, 'registry_verification', false, GROUPS)
    expect(unticked.tests).toEqual(['a'])
    expect(unticked.shown).toEqual(saved.scenarios)
    expect(draftDirty(unticked, saved)).toBe(true)

    const again = tickDraft(unticked, 'registry_implementation', true, GROUPS)
    expect(again.tests).toEqual([
      'a',
      'registry_implementation',
      'registry_verification',
    ])
    expect(draftDirty(again, saved)).toBe(false)

    const added = tickDraft(draft, 'b', true, GROUPS)
    expect(added.shown).toEqual([...saved.scenarios, 'b'])

    expect(draftProblem({ ...draft, label: ' ' })).toBe('Name the suite.')
    expect(draftProblem({ ...draft, tests: [] })).toBe(
      'Keep at least one test.',
    )
    expect(draftProblem(draft)).toBeNull()
  })

  it('suggests catalog tests to add that the draft does not hold', () => {
    const draft = suiteDraft(suite('mine', { scenarios: ['validation_loop'] }))
    const views = [
      view('validation_loop'),
      view('validation_chain'),
      view('validation_old', { selectable: false }),
      view('minimal_path'),
    ]
    expect(testSuggestions('val', views, draft)).toEqual(['validation_chain'])
    expect(testSuggestions('  ', views, draft)).toEqual([])
  })
})

describe('saving', () => {
  it('sends the ticked tests in the order shown, the unticked left out', () => {
    const saved = suite('mine', {
      label: 'Mine',
      source: 'local',
      scenarios: ['a', 'b', 'c'],
    })
    const draft = tickDraft(
      tickDraft(
        { ...suiteDraft(saved), label: '  Fast  ', runs: 3 },
        'b',
        false,
        [],
      ),
      'd',
      true,
      [],
    )
    expect(draftChanges(draft)).toEqual({
      label: 'Fast',
      scenarios: ['a', 'c', 'd'],
      repetitions: 3,
      technical_retries: 0,
    })
  })

  it('puts the suite the worker answered in its place, or last when new', () => {
    const a = suite('a')
    const b = suite('b')
    const renamed = { ...b, label: 'B' }
    expect(upsertSuite([a, b], renamed)).toEqual([a, renamed])
    expect(upsertSuite([a], b)).toEqual([a, b])
  })
})
