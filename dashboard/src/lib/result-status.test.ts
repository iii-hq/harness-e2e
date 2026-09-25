import { describe, expect, it } from 'vitest'
import {
  RESULT_STATES,
  type ResultState,
  runResultState,
} from './result-status'

describe('result states', () => {
  it('gives every state of the canvas one label and one tone', () => {
    expect(
      Object.fromEntries(
        Object.entries(RESULT_STATES).map(([state, { label, tone }]) => [
          state,
          `${label} · ${tone}`,
        ]),
      ),
    ).toEqual({
      passed: 'Passed · ok',
      lost_points: 'Lost points · warn',
      incomplete: 'Incomplete · warn',
      inconclusive: 'Inconclusive · warn',
      failed_gate: 'Failed a gate · alert',
      not_run: 'Not run · alert',
      running: 'Running · accent',
      waiting: 'Waiting for a slot · ghost',
      queued: 'Queued · ghost',
      cancelled: 'Cancelled · ghost',
      infra_error: 'Infra error · alert',
      subject_error: 'Subject error · alert',
      hit_limit: 'Hit a limit · warn',
    })
  })

  it('marks only a running state as live', () => {
    const live = (Object.keys(RESULT_STATES) as ResultState[]).filter(
      (state) => RESULT_STATES[state].live,
    )
    expect(live).toEqual(['running'])
  })
})

describe('runResultState', () => {
  it('passes only a completed run that kept its points', () => {
    expect(
      runResultState({ status: 'passed', completion: 'completed', score: 100 }),
    ).toBe('passed')
    expect(
      runResultState({ status: 'passed', completion: 'completed', score: 95 }),
    ).toBe('lost_points')
    expect(
      runResultState({
        status: 'passed',
        completion: 'task_incomplete',
        score: 100,
      }),
    ).toBe('incomplete')
    expect(
      runResultState({
        status: 'passed',
        completion: 'undetermined',
        score: 100,
      }),
    ).toBe('inconclusive')
  })

  it('does not invent lost points for an unscored run', () => {
    expect(
      runResultState({
        status: 'passed',
        completion: 'completed',
        score: null,
      }),
    ).toBe('passed')
  })

  it('names why a run did not pass', () => {
    expect(runResultState({ status: 'hard_gate_failed' })).toBe('failed_gate')
    expect(runResultState({ status: 'resource_limit' })).toBe('hit_limit')
    expect(runResultState({ status: 'subject_error' })).toBe('subject_error')
    expect(runResultState({ status: 'infrastructure_error' })).toBe(
      'infra_error',
    )
  })

  it('calls an unavailable or unknown status inconclusive, not an error', () => {
    expect(runResultState({ status: 'unavailable' })).toBe('inconclusive')
    expect(runResultState({ status: 'something_new' })).toBe('inconclusive')
  })
})
