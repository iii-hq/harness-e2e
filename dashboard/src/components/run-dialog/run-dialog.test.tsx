import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { LocalRunnerDialog } from '../LocalRunnerDialog'
import { GithubCard } from './GithubCard'
import {
  checkState,
  pendingReasons,
  pendingText,
  runLabel,
  selectionText,
  sequenceStep,
  stackDeclares,
  summaryCounts,
  summaryDetail,
  testFamilies,
  toggleAll,
  visibleTests,
  whereHint,
} from './run-dialog-model'
import { TestsColumn } from './TestsColumn'

const TESTS = [
  'registry_planning',
  'registry_implementation',
  'registry_verification',
  'minimal_path',
  'kanban_c1_foundation',
  'kanban_c2_persistence',
  'timer_wake',
]
const SEQUENCES = [['registry_implementation', 'registry_verification']]

describe('run dialog model', () => {
  it('groups tests by a shared prefix, then Standalone', () => {
    expect(
      testFamilies(TESTS).map((family) => [family.label, family.items]),
    ).toEqual([
      ['kanban', ['kanban_c1_foundation', 'kanban_c2_persistence']],
      [
        'registry',
        [
          'registry_planning',
          'registry_implementation',
          'registry_verification',
        ],
      ],
      ['Standalone', ['minimal_path', 'timer_wake']],
    ])
  })

  it('filters by id or words and by what is ticked', () => {
    expect(visibleTests(TESTS, [], 'timer wake', false)).toEqual(['timer_wake'])
    expect(visibleTests(TESTS, ['minimal_path'], '', true)).toEqual([
      'minimal_path',
    ])
    expect(visibleTests(TESTS, [], 'zzz', false)).toEqual([])
  })

  it('reads and toggles a group as off, some or on', () => {
    const ids = ['a', 'b']
    expect(checkState(ids, [])).toBe('off')
    expect(checkState(ids, ['a'])).toBe('some')
    expect(checkState(ids, ['a', 'b'])).toBe('on')
    expect(toggleAll(ids, ['a'])).toEqual(['a', 'b'])
    expect(toggleAll(ids, ['a', 'b', 'c'])).toEqual(['c'])
  })

  it('marks the place of a test in its sequence', () => {
    expect(sequenceStep('registry_verification', SEQUENCES)).toBe(
      '2 of 2 · in order',
    )
    expect(sequenceStep('minimal_path', SEQUENCES)).toBe('')
  })

  it('says what is missing before Run, in order', () => {
    const pending = pendingReasons({
      ready: true,
      where: 'github',
      hasStack: false,
      githubBlocked: true,
      hasModel: false,
      tests: 0,
    })
    expect(pendingText(true, pending)).toBe(
      'Before running, pick the stack it runs on and sign in with gh on the worker’s machine and choose a model and tick at least one test.',
    )
    expect(pendingText(false, [])).toBe(
      'The catalog has to load before running.',
    )
    expect(
      pendingReasons({
        ready: true,
        where: 'harness',
        hasStack: false,
        githubBlocked: false,
        hasModel: true,
        tests: 3,
      }),
    ).toEqual([])
  })

  it('summarizes what runs and names the Run button after it', () => {
    expect(summaryCounts(12, 3)).toBe('12 tests · 36 runs')
    expect(summaryCounts(1, 1)).toBe('1 test · 1 run')
    expect(
      summaryDetail({
        runs: 3,
        retries: 1,
        suite: 'Regression',
        where: 'docker',
        stack: 'default',
      }),
    ).toBe('3 runs per test · 1 retry · Regression · in Docker on default')
    expect(
      summaryDetail({
        runs: 1,
        retries: 0,
        suite: null,
        where: 'harness',
        stack: null,
      }),
    ).toBe('1 run per test · 0 retries · custom selection · on this harness')
    expect(runLabel(12, 'docker')).toBe('Run 12 tests in Docker')
    expect(runLabel(1, 'github')).toBe('Run 1 test on GitHub')
    expect(runLabel(0, 'harness')).toBe('Run tests')
    expect(selectionText(4, 2)).toBe('4 selected · 2 hidden')
    expect(selectionText(0, 0)).toBe('none selected')
  })

  it('describes where it runs and what a stack declares', () => {
    expect(whereHint('docker', 2)).toContain('2 groups at a time')
    expect(whereHint('github', 2)).toContain('exact-stack workflow')
    expect(
      stackDeclares({ iii: null, template: 'harness', containers: [1, 2, 3] }),
    ).toBe('iii latest · template harness · 3 workers')
  })
})

function column(
  props: Partial<Parameters<typeof TestsColumn>[0]> = {},
): string {
  return renderToStaticMarkup(
    <TestsColumn
      status="ready"
      tests={TESTS}
      sequences={SEQUENCES}
      selected={[]}
      onSelect={() => {}}
      query=""
      onQuery={() => {}}
      onlySelected={false}
      onOnlySelected={() => {}}
      modelCount={54}
      onRefresh={() => {}}
      {...props}
    />,
  )
}

describe('run dialog', () => {
  it('renders Run tests with the setup beside the tests before the catalog loads', () => {
    const html = renderToStaticMarkup(
      <LocalRunnerDialog bridge={null} open onClose={() => {}} />,
    )
    expect(html).toContain('Run tests')
    expect(html).toContain(
      'Starts a new execution on this harness, in Docker or on GitHub.',
    )
    expect(html).toContain('This harness')
    expect(html).toContain('Runs per test')
    expect(html).toContain('The catalog has to load before running.')
  })
})

describe('GitHub card', () => {
  it('names the repository and the signed-in account', () => {
    const html = renderToStaticMarkup(
      <GithubCard
        status={{
          ready: true,
          repository: 'iii-hq/harness-e2e',
          account: 'octo',
          message: null,
        }}
        onCheckAgain={() => {}}
      />,
    )
    expect(html).toContain('iii-hq/harness-e2e · default branch')
    expect(html).toContain('gh on this worker’s machine · octo')
    expect(html).not.toContain('role="alert"')
  })

  it('says how to fix a signed-out gh', () => {
    const html = renderToStaticMarkup(
      <GithubCard
        status={{
          ready: false,
          repository: 'iii-hq/harness-e2e',
          account: null,
          message:
            '`gh` is not signed in on the worker’s machine. Run `gh auth login` there, then reopen this dialog.',
        }}
        onCheckAgain={() => {}}
      />,
    )
    expect(html).toContain('role="alert"')
    expect(html).toContain('gh isn’t ready on this worker’s machine.')
    expect(html).toContain('gh auth login')
    expect(html).toContain('Check again')
  })

  it('says it is checking while gh answers', () => {
    expect(
      renderToStaticMarkup(
        <GithubCard status="loading" onCheckAgain={() => {}} />,
      ),
    ).toContain('Checking gh on this worker’s machine…')
  })
})

describe('tests column', () => {
  it('lists families with counts and marks sequences', () => {
    const html = column({ selected: ['registry_planning'] })
    expect(html).toContain('aria-label="Select every test in registry"')
    expect(html).toContain('1/3')
    expect(html).toContain('1 of 2 · in order')
    expect(html).toContain('1 selected')
    expect(html).toContain('Catalog ready · 7 tests · 54 models')
  })

  it('says how many ticked tests a filter hides', () => {
    const html = column({
      selected: ['registry_planning', 'timer_wake'],
      query: 'timer',
    })
    expect(html).toContain('2 selected · 1 hidden')
    expect(html).toContain('1 of 7')
  })

  it('offers a way out of an empty list', () => {
    expect(column({ query: 'zzz' })).toContain('No tests match “zzz”.')
    expect(column({ query: 'zzz' })).toContain('Clear filter')
    const none = column({ onlySelected: true })
    expect(none).toContain('No tests ticked yet.')
    expect(none).toContain('Show all tests')
  })

  it('shows skeleton rows while loading and a retry when the catalog fails', () => {
    const loading = column({ status: 'loading' })
    expect(loading).toContain('aria-label="Loading tests"')
    expect(loading).toContain('Loading catalog…')
    const failed = column({ status: 'failed' })
    expect(failed).toContain('Couldn’t load the test catalog')
    expect(failed).toContain('Catalog unavailable')
    expect(failed).toContain('Retry')
  })
})
