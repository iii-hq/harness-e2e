import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { ExecutionFacts } from '@/components/ExecutionConfiguration'
import type { PlanExecution } from '@/lib/plan-execution'

const execution: PlanExecution = {
  id: 'plan-imported',
  label: 'Software engineering',
  parameters: {
    suite: {
      id: 'software-engineering',
      label: 'Software engineering',
      sha256: 'sha256:8c0cde58a134e6be0fe7',
    },
    scenarios: ['kanban_c1_foundation', 'kanban_c2_persistence'],
    runs: 1,
    technical_retries: 0,
    model: 'deepseek-flash',
    provider: 'deepseek',
    agent: 'tech-lead',
  },
  source: {
    kind: 'github',
    repository: 'iii-hq/harness-e2e',
    run_id: 35823421664,
    run_attempt: 2,
    url: 'https://github.com/iii-hq/harness-e2e/actions/runs/35823421664',
    release_control_execution_id: '366030b3',
    stack: 'default',
  },
  stack: [
    {
      name: 'harness',
      source: 'package',
      requested: '1.8.31',
      observed: '1.8.8-rc.3',
      commit: null,
      dirty: null,
      groups: ['case-a'],
    },
    {
      name: 'harness',
      source: 'package',
      requested: '1.8.31',
      observed: '1.8.9',
      commit: null,
      dirty: null,
      groups: ['case-b'],
    },
    {
      name: 'state',
      source: 'package',
      requested: '0.22.17',
      observed: '0.22.3-rc.3',
      commit: null,
      dirty: null,
    },
  ],
  state: 'completed',
  started_at: '2026-09-20T10:00:00Z',
  finished_at: '2026-09-20T11:00:00Z',
  error: null,
  slots: [],
  measurements: null,
}
describe('execution facts', () => {
  it('shows where it ran, the suite, the model, the profile and the stack button', () => {
    const html = renderToStaticMarkup(<ExecutionFacts execution={execution} />)
    expect(html).toContain(
      'href="https://github.com/iii-hq/harness-e2e/actions/runs/35823421664"',
    )
    const text = html.replace(/<[^>]*>/g, '')
    expect(text).toContain('WhereGitHub #35823421664 · attempt 2')
    // The suite by name and digest, and the stack its contract names.
    expect(text).toContain('SuiteSoftware engineering · 8c0cde58a134')
    expect(text).toContain('Stackdefault')
    expect(text).toContain('Release Control366030b3')
    expect(html).toContain('deepseek/deepseek-flash')
    expect(html).toContain('tech-lead')
    expect(text).toContain('Stack · 2 workers · 1 differ between groups')
    // The stack table stays closed until asked for.
    expect(html).not.toContain('data-execution-stack')
  })

  it('reads a local execution as local, without a stack button', () => {
    const html = renderToStaticMarkup(
      <ExecutionFacts
        execution={{ ...execution, source: { kind: 'local' }, stack: [] }}
      />,
    )
    expect(html).toContain('>This harness<')
    expect(html).not.toContain('data-stack-toggle')
  })

  it('adds the facts the page knows, without repeating a label', () => {
    const html = renderToStaticMarkup(
      <ExecutionFacts
        execution={{
          ...execution,
          source: { kind: 'local' },
          stack: [
            {
              name: 'queue',
              source: 'path',
              requested: null,
              observed: '0.4.1',
              commit: '0123456789abcdef0123456789abcdef01234567',
              dirty: true,
            },
          ],
        }}
        extra={[
          ['Suite', 'duplicate'],
          ['Started', 'Sep 20, 10:00'],
        ]}
      />,
    )
    const text = html.replace(/<[^>]*>/g, '')
    expect(text).toContain('StartedSep 20, 10:00')
    expect(text).not.toContain('duplicate')
    expect(text).toContain('Stack · 1 worker')
  })
})
