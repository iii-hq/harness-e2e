import { describe, expect, it, vi } from 'vitest'
import {
  getDashboardDataBridge,
  installDashboardRuntimeConfig,
  type RuntimeConfig,
  type StaticVersionSide,
  staticCompatibility,
  staticSideKey,
} from '@/lib/dashboard-data-source'
import {
  type DashboardIiiClient,
  installDashboardIiiClient,
} from '@/lib/iii-client'

describe('live dashboard transport', () => {
  it('fetches fresh execution summaries and invalidates read caches on progress', async () => {
    const runtime = {
      mode: 'local',
      transport: 'iii',
      page_size: 25,
      http_fallback: false,
      functions: {
        executions_list: 'executions',
        tests_list: 'tests',
        changed_trigger: 'changed',
      },
    } as RuntimeConfig
    let handler: (value: Record<string, unknown>) => void = () => {}
    const trigger = vi.fn(async (_functionId: string) => ({}))
    const client = {
      browserId: 'test-browser',
      trigger,
      on: vi.fn((_id, callback) => {
        handler = callback
        return () => {}
      }),
      registerTrigger: vi.fn(() => () => {}),
    } as unknown as DashboardIiiClient
    installDashboardRuntimeConfig(runtime)
    installDashboardIiiClient(client)
    const bridge = await getDashboardDataBridge()
    const stop = await bridge.subscribeRunChanges(() => {})
    await bridge.listExecutions({ ids: ['execution-1'] })
    await bridge.listExecutions({ ids: ['execution-1'] })
    expect(
      trigger.mock.calls.filter(([id]) => id === 'executions'),
    ).toHaveLength(2)
    await bridge.listTests()
    await bridge.listTests()
    expect(trigger.mock.calls.filter(([id]) => id === 'tests')).toHaveLength(1)
    handler({ kind: 'progress', execution_id: 'execution-1' })
    await bridge.listTests()
    expect(trigger.mock.calls.filter(([id]) => id === 'tests')).toHaveLength(2)
    stop()
  })
})

function side(
  assessment = 'assessment-a',
  analyzer: string | null = 'analyzer-a',
): StaticVersionSide {
  return {
    summary: {} as StaticVersionSide['summary'],
    contracts: { case: 'contract-a' },
    assessment_profiles: { case: assessment },
    analyzer_profiles: { case: analyzer },
  }
}

describe('static dashboard assessment parity', () => {
  it('keys retained comparison data by cohort and evaluated version', () => {
    expect(staticSideKey('cohort-a', 'version-a')).toBe('cohort-a::version-a')
  })

  it('keeps assessment and analyzer incompatibilities distinct', () => {
    expect(staticCompatibility(side(), side())).toEqual({
      compatibility: 'compatible',
      reasons: [],
    })
    expect(staticCompatibility(side(), side('assessment-b'))).toEqual({
      compatibility: 'assessment_changed',
      reasons: ['assessment_profile_changed'],
    })
    expect(staticCompatibility(side(), side('assessment-a', null))).toEqual({
      compatibility: 'analyzer_conflict',
      reasons: ['analyzer_profile_conflict'],
    })
  })
})
