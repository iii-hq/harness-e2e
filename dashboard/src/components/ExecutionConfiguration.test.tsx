import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { ExecutionConfiguration } from '@/components/ExecutionConfiguration'
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

describe('execution configuration', () => {
  it('shows the suite, parameters, the linked origin and the stack', () => {
    const html = renderToStaticMarkup(
      <ExecutionConfiguration execution={execution} />,
    )
    expect(html).toContain(
      'href="https://github.com/iii-hq/harness-e2e/actions/runs/35823421664"',
    )
    const text = html.replace(/<[^>]*>/g, '')
    expect(text).toContain('GitHub #35823421664 · attempt 2')
    // The suite by name and digest, and the stack its contract names.
    expect(text).toContain('suiteSoftware engineering · 8c0cde58a134')
    expect(text).toContain('stackdefault')
    expect(html).toContain('deepseek/deepseek-flash')
    expect(html).toContain('tech-lead')
    expect(html).toContain('kanban_c1_foundation, kanban_c2_persistence')
    expect(html).toContain('2 workers · 1 differ between groups')
    expect(html.match(/data-label="worker"/g)).toHaveLength(3)
    expect(html).toContain('case-b')
  })

  it('reads a local execution as local', () => {
    const html = renderToStaticMarkup(
      <ExecutionConfiguration
        execution={{ ...execution, source: { kind: 'local' }, stack: [] }}
      />,
    )
    expect(html).toContain('>local<')
    expect(html).not.toContain('data-execution-stack')
    expect(html).not.toContain('data-execution-warnings')
  })

  it('shows the checkout a local worker ran from and what was not recorded', () => {
    const html = renderToStaticMarkup(
      <ExecutionConfiguration
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
          warnings: ['harness-e2e: /missing is not a readable Git checkout'],
        }}
      />,
    )
    expect(html).toContain('path · 0123456789ab · dirty')
    expect(html).toContain('data-execution-warnings')
    expect(html).toContain('/missing is not a readable Git checkout')
  })
})
