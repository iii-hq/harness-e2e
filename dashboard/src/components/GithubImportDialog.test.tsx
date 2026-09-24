import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  GithubRunsTable,
  githubRunAction,
  sortGithubRuns,
  withContracts,
} from '@/components/GithubImportDialog'
import type { GithubRun } from '@/lib/dashboard-data-source'

function run(overrides: Partial<GithubRun> & { run_id: number }): GithubRun {
  return {
    run_attempt: 1,
    title: 'E2E · 366030b3',
    created_at: '2026-09-20T10:00:00Z',
    conclusion: 'success',
    url: `https://github.com/iii-hq/harness-e2e/actions/runs/${overrides.run_id}`,
    release_control_execution_id: '366030b3',
    suite: 'software-engineering',
    suite_label: 'Software engineering',
    model: 'deepseek-flash',
    provider: 'deepseek',
    agent: null,
    execution_id: null,
    execution_state: null,
    ...overrides,
  }
}

describe('GitHub import panel', () => {
  it('lists runs with suite, subject and profile, and offers the right action', () => {
    const runs = [
      run({ run_id: 1 }),
      run({
        run_id: 2,
        run_attempt: 2,
        attempt_started_at: '2026-09-21T09:00:00Z',
        release_control_execution_id: '366030b3-5f55-4c7d',
        runner_version: '0.11.28',
        agent: 'tech-lead',
        conclusion: 'failure',
        execution_id: 'plan-imported',
        execution_state: 'completed',
      }),
      run({
        run_id: 3,
        execution_id: 'plan-importing',
        execution_state: 'importing',
        suite: null,
        suite_label: null,
        contract_error: 'HTTP 410: artifact expired',
      }),
    ]
    expect(runs.map(githubRunAction)).toEqual([
      'import',
      'imported',
      'importing',
    ])
    const html = renderToStaticMarkup(
      <GithubRunsTable runs={runs} importing={null} onImport={() => {}} />,
    )
    expect(html.match(/data-github-run=/g)).toHaveLength(3)
    expect(html).toContain('Software engineering')
    expect(html).toContain('deepseek-flash')
    expect(html).toContain('tech-lead')
    const text = html.replace(/<[^>]*>/g, '')
    // Dated by creation; the latest attempt apart; the RC execution short.
    expect(text).toMatch(/attempt 2 · Sep 21, 2026/)
    expect(text).toContain('RC 366030b3')
    expect(html).toContain(
      'title="Release Control execution 366030b3-5f55-4c7d"',
    )
    expect(text).toContain('0.11.28')
    expect(html).toContain('href="#/ext/harness-e2e/execution/plan-imported"')
    expect(html).toContain('import again')
    expect(html).toContain('>imported<')
    expect(html).toContain('contract unavailable')
    expect(html).toContain('title="HTTP 410: artifact expired"')
  })
})

describe('GitHub runs while their contracts are read', () => {
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

  it('shows each row as it is read and merges what arrives', () => {
    const listed = [
      run({
        run_id: 1,
        contract_pending: true,
        suite_label: null,
        model: null,
      }),
      run({ run_id: 2 }),
    ]
    const html = renderToStaticMarkup(
      <GithubRunsTable runs={listed} importing={null} onImport={() => {}} />,
    )
    expect(html).toContain('aria-busy="true"')
    expect(html.match(/reading…/g)).toHaveLength(4)
    const read = withContracts(
      listed,
      [
        {
          run_id: 1,
          suite_label: 'Regression',
          model: 'gpt-5.6-terra',
          runner_version: '0.11.28',
        },
      ],
      [1],
    )
    expect(read[0]).toMatchObject({
      contract_pending: false,
      suite_label: 'Regression',
      runner_version: '0.11.28',
      contract_error: undefined,
    })
    expect(read[1]).toBe(listed[1])
    // A run the answer left out stops waiting.
    expect(withContracts(listed, [], [1])[0].contract_error).toBe(
      'The contract could not be read',
    )
  })
})
