import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { MasterTestPlan } from '@/lib/dashboard-data-source'
import { MasterTestProfiles } from './MasterTestProfiles'

const plan: MasterTestPlan = {
  plan_id: 'harness',
  definition_sha256: `sha256:${'d4'.repeat(32)}`,
  profiles: [
    {
      id: 'smoke',
      label: 'Smoke',
      purpose: 'Recover without duplicate effects.',
      metrics: ['deliverable_success'],
      scenario_ids: ['cleanup_under_failure'],
      repetitions: 1,
      technical_retries: 0,
      profile_sha256: 'sha256:profile',
      budget: {
        planned_runs: 10,
        scenario_runs: 1,
        session_turn_limit_sum: 100,
        subject_token_limit: null,
        unbounded_token_cases: ['cleanup_under_failure'],
      },
    },
  ],
}

describe('master test profiles', () => {
  it('shows scope and incomplete budget without implying a successful run', () => {
    const html = renderToStaticMarkup(<MasterTestProfiles plan={plan} />)
    // The plan is identified by its definition digest, never by a version.
    expect(html).toContain('Master test plan · d4d4d4d4d4d4')
    expect(html).toContain('Smoke')
    expect(html).toContain('not available for all cases')
    expect(html).toContain('Create Smoke plan')
    expect(html).toContain('deliverable_success')
    expect(html).not.toContain('run baseline')
  })
})
