import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { routeFromHash } from '@/hooks/use-hash-route'
import { type PlanExecution, planErrors } from '@/lib/profile-plan'
import { PlanProgress, Requirements } from './ProfilePlanPage'

describe('executable profile plan journey', () => {
  it('requires an explicit execution model and the required evaluator', () => {
    const config = {
      label: 'Smoke',
      profile_id: 'smoke',
      url: 'ws://localhost',
      model: '',
      provider: '',
      judge_model: '',
      judge_provider: '',
    }
    expect(planErrors(config, true)).toEqual({
      model: 'Select an execution model.',
      judge: 'This profile requires an evaluator.',
    })
    expect(
      planErrors({ ...config, model: 'subject', provider: 'provider' }, false),
    ).toEqual({})
  })
  it('keeps refreshable profile, duplication and manual routes', () => {
    expect(routeFromHash('#/plans/new/profile/smoke')).toEqual({
      page: 'plan-create',
      profileId: 'smoke',
    })
    expect(routeFromHash('#/plans/new/duplicate/profile-example')).toEqual({
      page: 'plan-create',
      duplicateId: 'profile-example',
    })
    expect(routeFromHash('#/plans/new/manual')).toEqual({
      page: 'plan-create',
      manual: true,
    })
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
