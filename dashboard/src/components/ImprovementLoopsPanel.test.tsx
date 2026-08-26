import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  AdvisorAnswer,
  ImprovementLoopsPanel,
} from '@/components/ImprovementLoopsPanel'
import type {
  DashboardDataBridge,
  ImprovementLoopReport,
} from '@/lib/dashboard-data-source'

describe('Harness improvement loop panel', () => {
  it('keeps host mutations visibly disabled without the explicit dashboard flag', () => {
    const html = renderToStaticMarkup(
      <ImprovementLoopsPanel
        bridge={
          {
            mode: 'local',
            improvementLoopEnabled: false,
          } as DashboardDataBridge
        }
      />,
    )
    expect(html).toContain('Host mutations are disabled')
    expect(html).toContain('--enable-improvement-loop')
  })

  it('renders the direct Advisor answer and immutable evidence identities', () => {
    const report = {
      record: {
        id: 'loop-1',
        phase: 'advising',
        created_at: '2026-08-26T00:00:00Z',
        updated_at: '2026-08-26T00:00:00Z',
        deadline_at: '2026-08-26T06:00:00Z',
        consumed_cost_usd: 0,
        error: '',
        spec: {
          label: 'Tool recovery',
          base_revision: 'a'.repeat(40),
          target_scenario: 'tool_contract_recovery',
          runs: 5,
          scenarios: [],
        },
        transitions: [],
        iterations: [],
      },
      artifacts: {
        iterations: [
          {
            number: 1,
            proposal: {
              hypothesis: {
                root_cause: 'tool_discovery',
                summary: 'Global discovery amplifies recovery work.',
                confidence: 0.82,
                evidence: [
                  {
                    artifact_id: 'trace-1',
                    artifact_sha256: `sha256:${'b'.repeat(64)}`,
                  },
                ],
              },
              action: {
                behavior_change: 'Discover only after unknown_function.',
                surfaces: ['harness/src/turn_loop.rs'],
              },
              objective: {
                metric: 'function_call_errors',
                direction: 'decrease',
                minimum_change: 1,
              },
              validation_method: 'Five frozen samples.',
            },
            checks: [],
            branch: 'feat/e2e-improve-loop-1-i01',
          },
        ],
      },
    } as ImprovementLoopReport

    const html = renderToStaticMarkup(<AdvisorAnswer report={report} />)
    expect(html).toContain('What should change in Harness')
    expect(html).toContain('Global discovery amplifies recovery work.')
    expect(html).toContain('Discover only after unknown_function.')
    expect(html).toContain('trace-1')
  })
})
