import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { validateExecutionSetup } from '@/components/ExecutionSetup'
import { routeFromHash } from '@/hooks/use-hash-route'
import type { PlanExecution } from '@/lib/plan-execution'
import { executionHeading, PlanProgress, Requirements } from './PlanStatus'

describe('executable plan journey', () => {
  it('requires an explicit execution model', () => {
    const config = {
      mode: 'plan' as const,
      label: 'Smoke',
      subject: '',
      selectedScenarios: ['minimal_path'],
      url: 'ws://localhost',
    }
    expect(validateExecutionSetup(config)).toEqual({
      subject: 'Choose an execution model.',
    })
    expect(validateExecutionSetup({ ...config, subject: 'model' })).toEqual({})
  })
  it('routes templates and duplication, and rejects the old manual URL', () => {
    expect(routeFromHash('#/ext/harness-e2e/plans/new/profile/smoke')).toEqual({
      page: 'plan-create',
      profileId: 'smoke',
    })
    expect(
      routeFromHash('#/ext/harness-e2e/plans/new/duplicate/profile-example'),
    ).toEqual({
      page: 'plan-create',
      duplicateId: 'profile-example',
    })
    expect(routeFromHash('#/ext/harness-e2e/plans/new/manual')).toBeNull()
  })
  it('uses every planned slot as progress denominator and separates result axes', () => {
    const execution = {
      state: 'running',
      role: 'baseline',
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
    const html = renderToStaticMarkup(<PlanProgress execution={execution} />)
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
      <PlanProgress
        execution={
          {
            state: 'running',
            role: null,
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
  it('preserves the draft and links to the active execution when admission is busy', () => {
    const html = renderToStaticMarkup(
      <Requirements
        value={{
          ready: false,
          checks: [
            {
              id: 'fixture',
              status: 'pending',
              message: 'Native setup verifies the fixture.',
            },
          ],
          active_execution: {
            id: 'plan-run',
            kind: 'plan',
            plan_id: 'profile-active',
          },
        }}
      />,
    )
    expect(html).toContain('Your saved draft is preserved.')
    expect(html).toContain('#/ext/harness-e2e/plans/profile-active')
    expect(html).toContain('Pending')
  })
})

describe('execution progress heading', () => {
  it('names a role only for a plan execution that has one', () => {
    expect(executionHeading({ role: 'baseline' })).toBe('Baseline execution')
    expect(executionHeading({ role: 'candidate' })).toBe('Candidate execution')
    expect(executionHeading({ role: null })).toBe('Execution')
  })
})
