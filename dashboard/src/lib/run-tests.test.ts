import { describe, expect, it } from 'vitest'
import type { DashboardExecutionSummary } from '@/lib/dashboard-data-source'
import {
  harnessBusy,
  pendingText,
  recordedStackDeclares,
  runLabel,
  runSummary,
  sequenceStep,
  stackDeclares,
  suiteHint,
  testFamilies,
  tickState,
  visibleTests,
} from '@/lib/run-tests'

// The canvas fixtures (Main.dc.html): a slice of its TESTS, SUITES and STACKS.
const TESTS = [
  'registry_planning',
  'registry_implementation',
  'registry_verification',
  'kanban_c1_foundation',
  'kanban_c2_persistence',
  'minimal_path',
  'persistent_state',
  'context_pressure',
  'timer_wake',
]
const SEQUENCES = [['registry_implementation', 'registry_verification']]

describe('the test list', () => {
  it('groups tests by family and gathers the rest as Standalone', () => {
    expect(testFamilies(TESTS)).toEqual([
      {
        key: 'kanban',
        label: 'kanban',
        mono: true,
        items: ['kanban_c1_foundation', 'kanban_c2_persistence'],
      },
      {
        key: 'registry',
        label: 'registry',
        mono: true,
        items: [
          'registry_planning',
          'registry_implementation',
          'registry_verification',
        ],
      },
      {
        key: 'standalone',
        label: 'Standalone',
        mono: false,
        items: [
          'context_pressure',
          'minimal_path',
          'persistent_state',
          'timer_wake',
        ],
      },
    ])
    expect(testFamilies([])).toEqual([])
  })

  it('reads a box as off, some or on from what is ticked', () => {
    const family = ['kanban_c1_foundation', 'kanban_c2_persistence']
    expect(tickState(family, [])).toBe('off')
    expect(tickState(family, ['kanban_c2_persistence'])).toBe('some')
    expect(tickState(family, [...family, 'timer_wake'])).toBe('on')
    // Nothing shown ticks nothing.
    expect(tickState([], ['timer_wake'])).toBe('off')
  })

  it('filters by id, words of the id, and what is ticked', () => {
    expect(visibleTests(TESTS, ' Kanban C2 ', 'all', [])).toEqual([
      'kanban_c2_persistence',
    ])
    expect(visibleTests(TESTS, 'wake', 'all', [])).toEqual(['timer_wake'])
    expect(
      visibleTests(TESTS, '', 'selected', ['timer_wake', 'minimal_path']),
    ).toEqual(['minimal_path', 'timer_wake'])
    expect(visibleTests(TESTS, 'kanban', 'selected', ['timer_wake'])).toEqual(
      [],
    )
  })

  it('says where a test sits in a sequence', () => {
    expect(sequenceStep('registry_implementation', SEQUENCES)).toBe(
      '1 of 2 · in order',
    )
    expect(sequenceStep('registry_verification', SEQUENCES)).toBe(
      '2 of 2 · in order',
    )
    expect(sequenceStep('registry_planning', SEQUENCES)).toBeNull()
  })
})

describe('the stack field', () => {
  it('says what a stack declares in one line', () => {
    const containers = (commit: string | null) =>
      ['harness', 'harness-e2e', 'shell', 'storage', 'llm-router'].map(
        (name) => ({
          name,
          version: null,
          commit: name === 'harness' ? commit : null,
        }),
      )
    expect(
      stackDeclares({
        iii: 'latest',
        template: null,
        containers: containers(null),
      }),
    ).toBe('iii latest · 5 workers')
    expect(
      stackDeclares({
        iii: 'latest',
        template: 'harness',
        containers: containers(null),
      }),
    ).toBe('iii latest · template harness · 5 workers')
    expect(
      stackDeclares({
        iii: 'latest',
        template: null,
        containers: containers('3f9a1c2e7b40aa'),
      }),
    ).toBe('iii latest · 5 workers · harness at a commit')
    expect(stackDeclares({ iii: null, template: null, containers: [] })).toBe(
      'iii — · 0 workers',
    )
  })

  it('reads the iii of a stack as recorded from its YAML', () => {
    expect(
      recordedStackDeclares(
        '# pinned\niii: "0.24.2" # release\ncontainers: {}\n',
      ),
    ).toBe('iii 0.24.2')
    expect(recordedStackDeclares('containers: {}\n')).toBe('')
  })
})

describe('the suite hint', () => {
  const regression = {
    id: 'regression',
    label: 'Regression',
    source: 'repository' as const,
    scenarios: TESTS,
    repetitions: 1,
    technical_retries: 1,
  }

  it('names where the suite comes from, its runs and retries', () => {
    expect(suiteHint(regression, regression)).toBe(
      'Repository suite · 1 run per test · 1 retry',
    )
    expect(
      suiteHint(
        {
          ...regression,
          source: 'local',
          repetitions: 3,
          technical_retries: 0,
        },
        null,
      ),
    ).toBe('Saved in this Console · 3 runs per test · 0 retries')
    const recorded = { ...regression, source: undefined, recorded: true }
    expect(suiteHint(recorded, recorded)).toBe(
      'As this execution ran · 1 run per test · 1 retry',
    )
  })

  it('says a changed suite runs as a custom selection', () => {
    expect(suiteHint(null, regression)).toBe(
      'Changed from Regression. Runs as a custom selection.',
    )
    expect(suiteHint(null, null)).toBeNull()
  })
})

describe('the footer', () => {
  it('sums up the tests, runs, model and where', () => {
    expect(
      runSummary({
        tests: 9,
        runs: 1,
        retries: 1,
        suite: 'Regression',
        where: 'docker',
        stack: 'default',
      }),
    ).toEqual({
      counts: '9 tests · 9 runs',
      detail: '1 run per test · 1 retry · Regression · in Docker on default',
    })
    expect(
      runSummary({
        tests: 1,
        runs: 3,
        retries: 0,
        suite: null,
        where: 'harness',
        stack: null,
      }),
    ).toEqual({
      counts: '1 test · 3 runs',
      detail:
        '3 runs per test · 0 retries · custom selection · on this harness',
    })
  })

  it('says what is missing before running', () => {
    expect(
      pendingText({ loading: false, noStack: false, noModel: true, tests: 0 }),
    ).toBe('Before running, choose a model and tick at least one test.')
    expect(
      pendingText({ loading: false, noStack: true, noModel: false, tests: 2 }),
    ).toBe('Before running, pick the stack it runs on.')
    expect(
      pendingText({ loading: true, noStack: false, noModel: false, tests: 2 }),
    ).toBe('The catalog has to load before running.')
    expect(
      pendingText({ loading: false, noStack: false, noModel: false, tests: 2 }),
    ).toBeNull()
  })

  it('names the tests and where on the run button', () => {
    expect(runLabel(9, 'docker')).toBe('Run 9 tests in Docker')
    expect(runLabel(1, 'harness')).toBe('Run 1 test')
    expect(runLabel(0, 'docker')).toBe('Run tests')
  })
})

describe('a busy harness', () => {
  const execution = (
    id: string,
    status: string,
    extra: Partial<DashboardExecutionSummary> = {},
  ) =>
    ({
      id,
      status,
      label: id,
      subjects: [],
      ...extra,
    }) as DashboardExecutionSummary

  it('is the execution running on this harness, not in Docker or GitHub', () => {
    expect(
      harnessBusy([
        execution('Docker run', 'running', {
          parameters: {
            scenarios: [],
            runs: 1,
            technical_retries: 0,
            model: 'm',
            provider: 'p',
            agent: null,
            where: 'docker',
          },
        }),
        execution('Import', 'importing', { source: { kind: 'github' } }),
        execution('Done', 'passed'),
        execution('Nightly regression', 'running'),
      ]),
    ).toEqual({ id: 'Nightly regression', title: 'Nightly regression' })
    expect(harnessBusy([execution('Done', 'passed')])).toBeNull()
  })
})
