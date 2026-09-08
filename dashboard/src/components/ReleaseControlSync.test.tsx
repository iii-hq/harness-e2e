import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  groupLine,
  ReleaseControlSyncButton,
  ReleaseControlSyncResult,
  type ReleaseControlSyncState,
  summarizeExecution,
} from '@/components/ReleaseControlSync'
import type {
  ReleaseControlPulledExecution,
  ReleaseControlPullResponse,
} from '@/lib/dashboard-data-source'

const execution: ReleaseControlPulledExecution = {
  execution_id: '5ad654c4-693e-4b8a-98f9-7df2c44e0640',
  run_id: 34086209340,
  run_attempt: 1,
  url: 'https://github.com/iii-hq/harness-e2e/actions/runs/34086209340',
  created_at: '2026-09-07T05:17:02Z',
  pulled_at: '2026-09-08T10:00:00Z',
  plan_id: 'plan-release-control-regression-deepseek-deepseek-v4-flash',
  plan_execution_id: 'plan-0123456789abcdef0123456789abcdef',
  groups: [
    {
      campaign_id: 'regression-r01',
      group_id: 'case-minimal-path',
      native_execution_id: '4fea7a941bdcc85bb5f0eda5a68429ea',
      outcome: 'imported',
      reason: null,
      runner_version: '0.8.6-experimental',
      runner_revision: 'd1cc7a42',
      schema_version: 5,
    },
    {
      campaign_id: 'regression-r01',
      group_id: 'case-timer-wake',
      native_execution_id: '870ec76b1ae60f56ec44ecbe5cf8835b',
      outcome: 'unreadable',
      reason: 'unsupported results schema_version 4; expected 5',
      runner_version: '0.8.6-experimental',
      runner_revision: 'd1cc7a42',
      schema_version: 4,
    },
    {
      campaign_id: 'regression-r01',
      group_id: 'fault-l2',
      native_execution_id: null,
      outcome: 'not_importable',
      reason: 'fault injection groups have no native run',
      runner_version: null,
      runner_revision: null,
      schema_version: null,
    },
  ],
}

const result: ReleaseControlPullResponse = {
  runs_dir: 'target/harness-e2e-local-runs',
  repository: 'iii-hq/harness-e2e',
  workflow: 'exact-stack-e2e.yml',
  executions: [execution],
  remaining_runs: 2,
}

function state(
  overrides: Partial<ReleaseControlSyncState>,
): ReleaseControlSyncState {
  return {
    pending: false,
    result: null,
    error: null,
    run: async () => undefined,
    dismiss: () => undefined,
    ...overrides,
  }
}

describe('release control sync', () => {
  it('summarizes an execution by outcome and a group by its reason', () => {
    expect(summarizeExecution(execution)).toBe(
      '5ad654c4-693e-4b8a-98f9-7df2c44e0640 · 1 imported · 1 unreadable · 1 not importable',
    )
    expect(groupLine(execution.groups[1])).toBe(
      'regression-r01/case-timer-wake · unreadable · runner 0.8.6-experimental: unsupported results schema_version 4; expected 5',
    )
    expect(groupLine(execution.groups[2])).toBe(
      'regression-r01/fault-l2 · not importable: fault injection groups have no native run',
    )
  })

  it('renders the button in its idle and busy states', () => {
    expect(
      renderToStaticMarkup(<ReleaseControlSyncButton sync={state({})} />),
    ).toContain('sync release control')
    const busy = renderToStaticMarkup(
      <ReleaseControlSyncButton sync={state({ pending: true })} />,
    )
    expect(busy).toContain('syncing release control')
    expect(busy).toContain('disabled')
  })

  it('renders every group, the GitHub run link and the server error', () => {
    const html = renderToStaticMarkup(
      <ReleaseControlSyncResult sync={state({ result })} />,
    )
    expect(html).toContain('1 execution checked · 1 run added')
    expect(html).toContain('regression-r01/case-minimal-path · imported')
    expect(html).toContain('unsupported results schema_version 4')
    expect(html).toContain('actions/runs/34086209340')
    expect(html).toContain(
      'filed under plan plan-release-control-regression-deepseek-deepseek-v4-flash',
    )
    expect(html).toContain('target/harness-e2e-local-runs')
    expect(html).toContain('2 older executions did not fit')

    const failed = renderToStaticMarkup(
      <ReleaseControlSyncResult
        sync={state({
          error:
            'GitHub artifact download needs a token: set HARNESS_E2E_GITHUB_TOKEN',
        })}
      />,
    )
    expect(failed).toContain('Release Control sync failed')
    expect(failed).toContain('HARNESS_E2E_GITHUB_TOKEN')
    expect(
      renderToStaticMarkup(<ReleaseControlSyncResult sync={state({})} />),
    ).toBe('')
  })
})
