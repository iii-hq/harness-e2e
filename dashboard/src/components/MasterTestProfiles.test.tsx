import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { MasterTestPlan } from '@/lib/dashboard-data-source'
import { MasterTestProfiles, profileExport } from './MasterTestProfiles'

const plan: MasterTestPlan = {
  plan_id: 'harness',
  version: 1,
  definition_sha256: 'sha256:definition',
  profiles: [
    {
      id: 'resilience',
      label: 'Resilience',
      purpose: 'Recover without duplicate effects.',
      metrics: ['work_amplification'],
      scenario_ids: ['cleanup_under_failure'],
      repetitions: 1,
      technical_retries: 0,
      profile_sha256: 'sha256:profile',
      protected_supervisor_required: true,
      campaigns: [
        { campaign_id: 'resilience-r01', test_plan: { repetition: 1 } },
      ],
      budget: {
        planned_runs: 10,
        scenario_runs: 1,
        fault_runs: 9,
        session_turn_limit_sum: 100,
        subject_token_limit: null,
        unbounded_token_cases: ['cleanup_under_failure'],
      },
    },
  ],
}

describe('master test profiles', () => {
  it('shows scope, incomplete budget and protected executor without implying a successful run', () => {
    const html = renderToStaticMarkup(<MasterTestProfiles plan={plan} />)
    expect(html).toContain('Resilience')
    expect(html).toContain('Protected fault executor')
    expect(html).toContain('not available for all cases')
    expect(html).toContain('Export Resilience profile')
    expect(html).toContain('work_amplification')
    expect(html).not.toContain('run baseline')
  })

  it('exports the exact campaign policy and frozen identity for the CLI importer', () => {
    const exported = profileExport(plan, plan.profiles[0])
    expect(exported.schema).toBe('harness-e2e-profile-campaigns/v1')
    expect(exported.definition_sha256).toBe(plan.definition_sha256)
    expect(exported.profile.campaigns).toEqual(plan.profiles[0].campaigns)
    expect(exported.profile.profile_sha256).toBe('sha256:profile')
  })
})
