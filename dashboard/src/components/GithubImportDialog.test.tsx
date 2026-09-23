import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  GithubRunsTable,
  githubRunAction,
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
    expect(html.replace(/<[^>]*>/g, '')).toContain('#2 · attempt 2')
    expect(html).toContain('href="#/ext/harness-e2e/execution/plan-imported"')
    expect(html).toContain('import again')
    expect(html).toContain('>imported<')
    expect(html).toContain('contract unavailable')
    expect(html).toContain('title="HTTP 410: artifact expired"')
  })
})
