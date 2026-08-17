import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { LocalPlan } from '@/lib/dashboard-data-source'
import { PlanLifecycle } from '@/pages/LocalPlanPage'

const candidateRunningPlan: LocalPlan = {
  schema_version: 1,
  id: 'plan-1',
  label: 'Focused regression check',
  purpose: 'Confirm the affected local flow.',
  created_at: '2026-08-17T00:00:00Z',
  updated_at: '2026-08-17T00:01:00Z',
  state: 'candidate_running',
  locked: true,
  scope_hash: 'sha256:scope',
  policy_hash: 'sha256:policy',
  url: 'https://example.invalid/catalog',
  model: 'codex/gpt-5.6-terra',
  provider: 'openai-codex',
  judge_model: 'codex/gpt-5.6-sol',
  judge_provider: 'openai-codex',
  scenarios: [],
  scenario_ids: ['direct_answer'],
  runs: 1,
  technical_retries: 0,
  seed: null,
  baseline_execution_id: 'baseline-1',
  candidate_execution_ids: [],
  incomplete_execution_ids: [],
  last_attempt_id: 'candidate-1',
}

describe('local plan lifecycle', () => {
  it('keeps a started candidate visible with one clear active-execution action', () => {
    const html = renderToStaticMarkup(
      <PlanLifecycle
        plan={candidateRunningPlan}
        starting={null}
        feedback={{
          role: 'candidate',
          phase: 'running',
          message:
            'Candidate is running. This page refreshes automatically while the report is collected.',
          executionId: 'candidate-1',
        }}
        onStart={() => undefined}
      />,
    )

    expect(html).toContain('Candidate is running')
    expect(html).toContain('View active execution')
    expect(html).not.toContain('Open active execution')
    expect(html).not.toContain('disabled=""')
    expect(html).toContain('aria-live="polite"')
    expect(html).not.toContain('Frozen test scope')
  })

  it('makes the first incomplete lifecycle action a baseline, not a candidate', () => {
    const html = renderToStaticMarkup(
      <PlanLifecycle
        plan={{
          ...candidateRunningPlan,
          state: 'draft',
          locked: false,
          baseline_execution_id: null,
          candidate_execution_ids: [],
          last_attempt_id: null,
        }}
        starting={null}
        feedback={null}
        onStart={() => undefined}
      />,
    )

    expect(html).toContain('Capture the baseline')
    expect(html).toContain('Run baseline')
    expect(html).not.toContain('>Run candidate<')
  })

  it('prioritizes reviewing a completed candidate before offering another run', () => {
    const html = renderToStaticMarkup(
      <PlanLifecycle
        plan={{
          ...candidateRunningPlan,
          state: 'comparison_ready',
          candidate_execution_ids: ['candidate-1'],
          last_attempt_id: 'candidate-1',
        }}
        starting={null}
        feedback={null}
        onStart={() => undefined}
      />,
    )

    expect(html).toContain('Candidate results are ready')
    expect(html).toContain('View latest candidate')
    expect(html).toContain('Run another candidate')
    expect(html).toContain('View baseline execution')
  })
})
