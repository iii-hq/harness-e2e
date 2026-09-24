import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { PlanExecution } from '@/lib/plan-execution'
import { ExecutionProgress } from './ExecutionProgress'

describe('execution progress', () => {
  it('uses every planned slot as progress denominator and separates result axes', () => {
    const execution = {
      state: 'running',
      slots: [
        {
          state: 'finished',
          observed: 1,
          completed: 1,
          passed: 0,
          technical_valid: 1,
        },
        {
          state: 'running',
          scenario_id: 'second',
          round: 2,
          observed: 0,
          completed: 0,
          passed: 0,
          technical_valid: 0,
        },
        {
          state: 'pending',
          observed: 0,
          completed: 0,
          passed: 0,
          technical_valid: 0,
        },
      ],
    } as PlanExecution
    const html = renderToStaticMarkup(
      <ExecutionProgress execution={execution} />,
    )
    expect(html).toContain('Execution · running')
    expect(html).toContain('1 / 3 slots finished')
    expect(html).toContain('max="3"')
    expect(html).toContain('Round 2 · second')
    for (const label of [
      'Execution completion',
      'Objective correctness',
      'Technical validity',
      'Observation coverage',
    ])
      expect(html).toContain(label)
    expect(html).not.toContain('data-rerun-progress')
  })
  it('times a rerun from when it started, not from the execution', () => {
    const started = new Date(Date.now() - 90_000).toISOString()
    const html = renderToStaticMarkup(
      <ExecutionProgress
        execution={
          {
            state: 'running',
            slots: [],
            rerun: {
              scenarios: ['timer_wake'],
              runs: ['native'],
              started_at: started,
              state: 'completed',
              error: null,
              finished_at: '2026-09-20T10:00:00Z',
            },
          } as unknown as PlanExecution
        }
      />,
    )
    expect(html).toContain('timer_wake running again since')
    expect(html).toMatch(/· 1m 3\ds elapsed/)
    expect(html).toContain('totals update when it finishes')
  })
  it('lists the groups of a Docker execution and where they are', () => {
    const group = (group_id: string, state: string, attempt = 1) => ({
      round: 1,
      campaign_id: 'pr-r01',
      group_id,
      scenarios: [group_id],
      state,
      attempt,
      error: state === 'failed' ? 'provider refused the key' : null,
    })
    const html = renderToStaticMarkup(
      <ExecutionProgress
        execution={
          {
            state: 'running',
            slots: [],
            source: {
              kind: 'docker',
              attempt: 2,
              phase: 'groups',
              groups: [
                group('case-minimal-path', 'done'),
                group('case-persistent-state', 'running', 2),
                group('case-shell-coder-sandbox', 'failed'),
                group('case-tool-contract-recovery', 'queued'),
              ],
            },
          } as unknown as PlanExecution
        }
      />,
    )
    expect(html).toContain('Running the groups…')
    for (const text of [
      'case-minimal-path',
      'running · attempt 2',
      'provider refused the key',
      'queued',
    ])
      expect(html).toContain(text)
  })
})
