import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  contractDetail,
  GithubImportDialog,
  GithubRunRow,
  githubRunAction,
  importSummary,
  listStatus,
  runDays,
  runMatches,
  shortSha,
  sortGithubRuns,
  withContracts,
  withExecutionState,
} from '@/components/GithubImportDialog'
import type { GithubRun } from '@/lib/dashboard-data-source'

function run(overrides: Partial<GithubRun> & { run_id: number }): GithubRun {
  return {
    run_attempt: 1,
    title: 'E2E · 366030b3',
    created_at: '2026-09-20T10:00:00Z',
    conclusion: 'success',
    head_branch: 'main',
    head_sha: '7a16130c9f1e2d3a4b5c6d7e8f9012345678abcd',
    url: `https://github.com/iii-hq/harness-e2e/actions/runs/${overrides.run_id}`,
    release_control_execution_id: '366030b3',
    suite: 'software-engineering',
    suite_label: 'Software engineering',
    model: 'deepseek-flash',
    provider: 'deepseek',
    agent: null,
    stack: 'default',
    runner_version: '0.14.0',
    execution_id: null,
    execution_state: null,
    ...overrides,
  }
}

const text = (html: string) => html.replace(/<[^>]*>/g, ' ')

describe('GitHub import model', () => {
  it('orders runs by creation, not by their latest attempt', () => {
    const runs = sortGithubRuns([
      run({ run_id: 1, created_at: '2026-09-18T10:00:00Z' }),
      run({
        run_id: 2,
        created_at: '2026-09-20T10:00:00Z',
        run_attempt: 3,
        attempt_started_at: '2026-09-23T10:00:00Z',
      }),
      run({ run_id: 3, created_at: '2026-09-22T10:00:00Z' }),
    ])
    expect(runs.map((entry) => entry.run_id)).toEqual([3, 2, 1])
  })

  it('merges what the contracts say; a run left out stops waiting', () => {
    const listed = [
      run({ run_id: 1, contract_pending: true, suite_label: null }),
      run({ run_id: 2 }),
    ]
    const read = withContracts(
      listed,
      [{ run_id: 1, suite_label: 'Regression', runner_version: '0.11.28' }],
      [1],
    )
    expect(read[0]).toMatchObject({
      contract_pending: false,
      suite_label: 'Regression',
      runner_version: '0.11.28',
      contract_error: undefined,
    })
    expect(read[1]).toBe(listed[1])
    expect(withContracts(listed, [], [1])[0].contract_error).toBe(
      'The contract could not be read',
    )
  })

  it('names what each row stands at', () => {
    expect(githubRunAction(run({ run_id: 1 }))).toBe('import')
    expect(githubRunAction(run({ run_id: 1 }), true)).toBe('importing')
    expect(
      githubRunAction(
        run({
          run_id: 2,
          execution_id: 'plan-a',
          execution_state: 'completed',
        }),
      ),
    ).toBe('imported')
    expect(
      githubRunAction(
        run({ run_id: 2, execution_id: 'plan-a', execution_state: 'failed' }),
      ),
    ).toBe('failed')
    expect(
      githubRunAction(
        run({ run_id: 3, execution_id: 'p', execution_state: 'importing' }),
      ),
    ).toBe('importing')
  })

  it('shortens the commit and says what the contract ran in one line', () => {
    expect(shortSha('7a16130c9f1e2d3a')).toBe('7a16130')
    expect(shortSha(null)).toBe('')
    expect(contractDetail(run({ run_id: 1 }))).toBe(
      'deepseek/deepseek-flash · profile default · stack default · runner 0.14.0',
    )
    expect(
      contractDetail(
        run({ run_id: 1, agent: 'tech-lead', stack: null, provider: null }),
      ),
    ).toBe('deepseek-flash · profile tech-lead · runner 0.14.0')
  })

  it('filters by text, branch and whether a run was imported', () => {
    const fresh = run({ run_id: 36030672844, head_branch: 'feat/executor' })
    const imported = run({
      run_id: 36027540504,
      execution_id: 'plan-a',
      execution_state: 'completed',
    })
    const all = { query: '', branch: '', show: 'all' as const }
    expect(runMatches(fresh, { ...all, query: '7A16130' })).toBe(true)
    expect(runMatches(fresh, { ...all, query: 'executor' })).toBe(true)
    expect(runMatches(fresh, { ...all, query: '3603067' })).toBe(true)
    expect(runMatches(fresh, { ...all, query: 'software eng' })).toBe(true)
    expect(runMatches(fresh, { ...all, query: 'nightly' })).toBe(false)
    expect(runMatches(fresh, { ...all, branch: 'main' })).toBe(false)
    expect(runMatches(imported, { ...all, branch: 'main' })).toBe(true)
    expect(runMatches(fresh, { ...all, show: 'new' })).toBe(true)
    expect(runMatches(imported, { ...all, show: 'new' })).toBe(false)
    expect(runMatches(imported, { ...all, show: 'imported' })).toBe(true)
    // An import this dialog is starting counts as imported.
    expect(runMatches(fresh, { ...all, show: 'imported' }, true)).toBe(true)
    // One whose import failed does not: it can be imported again.
    const failed = run({
      run_id: 5,
      execution_id: 'plan-f',
      execution_state: 'failed',
    })
    expect(runMatches(failed, { ...all, show: 'imported' })).toBe(false)
    expect(runMatches(failed, { ...all, show: 'new' })).toBe(true)
  })

  it('follows an import the worker ended, to Imported or to Failed', () => {
    const listed = [
      run({ run_id: 1, execution_id: 'plan-a', execution_state: 'importing' }),
      run({ run_id: 2, execution_id: 'plan-b', execution_state: 'importing' }),
    ]
    const done = withExecutionState(listed, 'plan-a', 'completed', null)
    expect(done.map((entry) => githubRunAction(entry))).toEqual([
      'imported',
      'importing',
    ])
    expect(done[1]).toBe(listed[1])
    const failed = withExecutionState(
      done,
      'plan-b',
      'failed',
      'This run keeps no e2e-contract artifact',
    )
    expect(githubRunAction(failed[1])).toBe('failed')
    expect(failed[1].execution_error).toBe(
      'This run keeps no e2e-contract artifact',
    )
  })

  it('groups runs by the local day they were created', () => {
    const now = new Date(2026, 8, 24, 15, 0)
    const days = runDays(
      [
        run({ run_id: 1, created_at: new Date(2026, 8, 24, 13).toISOString() }),
        run({ run_id: 2, created_at: new Date(2026, 8, 24, 9).toISOString() }),
        run({ run_id: 3, created_at: new Date(2026, 8, 23, 18).toISOString() }),
        run({ run_id: 4, created_at: new Date(2026, 8, 20, 18).toISOString() }),
        run({ run_id: 5, created_at: null }),
      ],
      now,
    )
    expect(
      days.map((day) => [
        day.label,
        day.date,
        day.runs.map((entry) => entry.run_id),
      ]),
    ).toEqual([
      ['Today', 'Sep 24', [1, 2]],
      ['Yesterday', 'Sep 23', [3]],
      ['Sep 20', '', [4]],
      ['Date not reported', '', [5]],
    ])
  })

  it('says what the footer and the status line stand at', () => {
    const base = { phase: 'ready' as const, selected: 0, again: 0, started: 0 }
    expect(importSummary(base)).toEqual({
      title: 'No runs selected',
      detail: 'Tick the runs to import.',
      busy: false,
    })
    expect(importSummary({ ...base, phase: 'loading' }).detail).toBe(
      'Runs appear as soon as GitHub answers.',
    )
    expect(importSummary({ ...base, phase: 'failed' }).detail).toBe(
      'Nothing can be imported until GitHub answers.',
    )
    expect(importSummary({ ...base, selected: 2, again: 1 })).toEqual({
      title: '2 runs selected',
      detail: '1 was imported before. Importing again replaces its evidence.',
      busy: false,
    })
    expect(importSummary({ ...base, selected: 1 }).detail).toBe(
      'Evidence downloads in the background. You can keep working.',
    )
    expect(importSummary({ ...base, started: 2 })).toMatchObject({
      title: 'Importing 2 runs in the background',
      busy: true,
    })
    expect(listStatus('ready', 20, 143, 'iii-hq/harness-e2e')).toBe(
      '20 of 143 runs loaded · iii-hq/harness-e2e',
    )
    expect(listStatus('ready', 1, null, null)).toBe('1 run loaded')
    expect(listStatus('loading', 0, null, null)).toBe(
      'Asking GitHub for completed runs…',
    )
    expect(listStatus('failed', 0, null, null)).toBe('GitHub unavailable')
  })
})

describe('GitHub run row', () => {
  const row = (entry: GithubRun, props: { starting?: boolean } = {}) =>
    renderToStaticMarkup(
      <GithubRunRow
        run={entry}
        selected={false}
        starting={props.starting ?? false}
        failure={null}
        onToggle={() => {}}
      />,
    )

  it('reads branch, short commit, conclusion, attempt and Release Control', () => {
    const html = row(
      run({
        run_id: 36010839787,
        run_attempt: 2,
        conclusion: 'failure',
        release_control_execution_id: 'ab2e2d9b-5f55-4c7d',
      }),
    )
    const words = text(html)
    expect(html).toContain('data-github-run="36010839787"')
    expect(words).toContain('Software engineering')
    expect(words).toContain('main')
    expect(words).toContain('7a16130')
    expect(words).not.toContain('7a16130c')
    expect(words).toContain('Failed')
    expect(words).toContain('attempt 2')
    expect(words).toMatch(/Release Control\s+ab2e2d9b/)
    expect(words).toContain(
      'deepseek/deepseek-flash · profile default · stack default · runner 0.14.0',
    )
    expect(html).toContain('aria-label="Import run 36010839787"')
    expect(html).toContain('aria-label="Run 36010839787 on GitHub"')
  })

  it('waits for the contract, or says why it has none', () => {
    const pending = row(
      run({ run_id: 1, contract_pending: true, suite_label: null }),
    )
    expect(pending).toContain('aria-busy="true"')
    expect(pending).toContain('aria-label="Reading the run’s contract"')
    const missing = text(
      row(
        run({
          run_id: 2,
          suite: null,
          suite_label: null,
          title: 'Release Control run',
          contract_error: 'This run keeps no e2e-contract artifact',
        }),
      ),
    )
    expect(missing).toContain('Release Control run')
    expect(missing).toContain('This run keeps no e2e-contract artifact')
  })

  it('links an imported run and locks one being imported', () => {
    const imported = row(
      run({ run_id: 1, execution_id: 'plan-a', execution_state: 'completed' }),
    )
    expect(text(imported)).toContain('Imported')
    expect(imported).toContain('href="#/ext/harness-e2e/execution/plan-a"')
    const importing = row(run({ run_id: 2 }), { starting: true })
    expect(text(importing)).toContain('Importing…')
    expect(importing).toMatch(/<input[^>]*disabled/)
    // A failed import says why and can be ticked again.
    const failed = row(
      run({
        run_id: 3,
        execution_id: 'plan-f',
        execution_state: 'failed',
        execution_error: 'gh: HTTP 410: artifact expired',
      }),
    )
    expect(text(failed)).toContain('Failed')
    expect(text(failed)).not.toContain('Imported')
    expect(text(failed)).toContain(
      'The import failed: gh: HTTP 410: artifact expired',
    )
    expect(failed).not.toMatch(/<input[^>]*disabled/)
  })
})

describe('GitHub import dialog', () => {
  it('opens asking GitHub, with nothing to import yet', () => {
    const html = renderToStaticMarkup(
      <GithubImportDialog
        bridge={null}
        open
        onClose={() => {}}
        onImported={() => {}}
      />,
    )
    const words = text(html)
    expect(words).toContain('Import from GitHub')
    expect(words).toContain('exact-stack-e2e.yml')
    expect(html).toContain('aria-label="Loading runs"')
    expect(words).toContain('Asking GitHub for completed runs…')
    expect(words).toContain('Runs appear as soon as GitHub answers.')
    expect(html).toMatch(/<button[^>]*class="rd-primary"[^>]*disabled/)
  })
})
