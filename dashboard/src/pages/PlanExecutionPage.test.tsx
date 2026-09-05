import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { validateExecutionSetup } from '@/components/ExecutionSetup'
import { routeFromHash } from '@/hooks/use-hash-route'
import type { PlanExecution } from '@/lib/plan-execution'
import { PlanProgress, Requirements } from './PlanExecutionPage'

describe('executable plan journey', () => {
  it('requires an explicit execution model and the required evaluator', () => {
    const config = {
      mode: 'plan' as const,
      label: 'Smoke',
      subject: '',
      judge: '',
      judgeRequired: true,
      selectedScenarios: ['minimal_path'],
      url: 'ws://localhost',
    }
    expect(validateExecutionSetup(config)).toEqual({
      subject: 'Choose an execution model.',
      judge: 'Choose a judge model for the selected tests.',
    })
    expect(
      validateExecutionSetup({ ...config, subject: 'model', judge: 'judge' }),
    ).toEqual({})
  })
  it('uses the shared form for templates, duplication and the old manual URL', () => {
    expect(routeFromHash('#/plans/new/profile/smoke')).toEqual({
      page: 'plan-create',
      profileId: 'smoke',
    })
    expect(routeFromHash('#/plans/new/duplicate/profile-example')).toEqual({
      page: 'plan-create',
      duplicateId: 'profile-example',
    })
    expect(routeFromHash('#/plans/new/manual')).toEqual(
      routeFromHash('#/plans/new'),
    )
  })
  it('uses every planned slot as progress denominator and separates result axes', () => {
    const execution = {
      state: 'running',
      role: 'run',
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
    expect(html).toContain('#/plans/profile-active')
    expect(html).toContain('Pending')
  })
})
