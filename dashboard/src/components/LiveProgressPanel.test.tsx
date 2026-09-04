import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { LiveProgressPanel } from '@/components/LiveProgressPanel'
import type { LiveProgress } from '@/lib/dashboard-data-source'

const liveProgressFixture: LiveProgress = {
  updated_at: '2026-09-04T12:00:00Z',
  phase: 'execute',
  terminal: false,
  committed_events: 16,
  terminal_reason: null,
  planned_slots: 3,
  runs_committed: 2,
  slots_deferred: 0,
  attempts_started: 4,
  attempts_finished: 3,
  subject_observations_committed: 3,
  active_attempt: {
    scenario_id: 'scenario_c',
    run_id: 'c',
    attempt_id: 'attempt-c',
    session_id: 'c',
    started_at: '2026-09-04T12:00:00Z',
  },
  slots: [
    {
      slot_id: 'a',
      scenario_id: 'scenario_a',
      repetition: 0,
      state: 'committed',
      reason: null,
      run_id: 'a',
      completion: 'completed',
      technical: 'valid',
      objective_score: 100,
      quality_score_completed: 80,
    },
    {
      slot_id: 'b',
      scenario_id: 'scenario_b',
      repetition: 0,
      state: 'committed',
      reason: null,
      run_id: 'b',
      completion: 'task_incomplete',
      technical: 'valid',
      objective_score: 20,
      quality_score_completed: null,
    },
    {
      slot_id: 'c',
      scenario_id: 'scenario_c',
      repetition: 0,
      state: 'pending',
      reason: null,
      run_id: null,
      completion: null,
      technical: null,
      objective_score: null,
      quality_score_completed: null,
    },
  ],
  completed_runs: 1,
  task_incomplete_runs: 1,
  undetermined_runs: 0,
  technical_invalid_runs: 0,
  completion_rate: 0.5,
  quality_score_completed: 80,
  quality_scored_completed_runs: 1,
  observed_tokens: 400,
  token_observed_attempts: 3,
  observed_cost_usd: 0.4,
  cost_observed_runs: 2,
}

describe('live progress panel', () => {
  it('shows provisional coverage without treating pending slots as failures', () => {
    const html = renderToStaticMarkup(
      <LiveProgressPanel progress={liveProgressFixture} running />,
    )
    expect(html).toContain('Live progress')
    expect(html).toContain('Provisional results')
    expect(html).toContain('2/3')
    expect(html).toContain('1 pending · 0 deferred')
    expect(html).toContain('50%')
    expect(html).toContain('1/2 determined runs')
    expect(html).toContain('3/4 attempts with complete token telemetry')
    expect(html).toContain('$0.4000')
    expect(html).toContain('1/1 completed runs scored')
    expect(html).toContain('Pending runs have no outcome')
    expect(html).toContain('Scenario C')
    expect(html).not.toContain('Failed')
  })

  it('keeps unknown usage unknown and interrupted evidence explicitly partial', () => {
    const html = renderToStaticMarkup(
      <LiveProgressPanel
        progress={{
          ...liveProgressFixture,
          terminal: true,
          phase: 'cancelled',
          active_attempt: null,
          observed_tokens: null,
          observed_cost_usd: null,
          quality_score_completed: null,
          token_observed_attempts: 0,
          cost_observed_runs: 0,
          terminal_reason: 'user_cancelled',
        }}
        running={false}
      />,
    )
    expect(html).toContain('Preserved progress')
    expect(html).toContain('Partial evidence')
    expect(html).toContain('user_cancelled')
    expect(html).toContain('0/4 attempts with complete token telemetry')
    expect(html).not.toContain('$0.0000')
    expect(html).not.toContain('Live progress')
  })
})
