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
  it('carries current plan controls and idempotent starts through iii and HTTP', async () => {
    const trigger = vi.fn(async () => ({ ready: true }))
    installDashboardIiiClient({ trigger } as unknown as DashboardIiiClient)
    const request = {
      action: 'requirements',
      plan_id: 'plan-1',
    }
    installDashboardRuntimeConfig({
      mode: 'local',
      transport: 'iii',
      functions: { plan_control: 'plan-control', plan_run_start: 'plan-start' },
    } as RuntimeConfig)
    const live = await getDashboardDataBridge()
    await expect(live.planControl?.(request)).resolves.toEqual({
      ready: true,
    })
    expect(trigger).toHaveBeenCalledWith('plan-control', request)
    await live.startPlan('plan-1', 'baseline')
    expect(trigger).toHaveBeenCalledWith('plan-start', {
      plan_id: 'plan-1',
      role: 'baseline',
      idempotency_key: expect.any(String),
    })
    const fetch = vi.fn(
      async (_url: string, _options?: RequestInit) =>
        new Response(JSON.stringify({ id: 'plan-1' }), { status: 200 }),
    )
    vi.stubGlobal('fetch', fetch)
    try {
      installDashboardRuntimeConfig({
        mode: 'local',
        transport: 'static',
        functions: {},
      } as RuntimeConfig)
      const http = await getDashboardDataBridge()
      await expect(
        http.planControl?.({
          action: 'export',
          plan_id: 'plan-1',
        }),
      ).resolves.toEqual({ id: 'plan-1' })
      expect(fetch.mock.calls[0]?.[0]).toBe('./api/dashboard/plans/control')
      await http.startPlan('plan-1', 'baseline')
      expect(fetch.mock.calls.at(-1)?.[0]).toBe(
        './api/dashboard/plans/plan-1/runs',
      )
      expect(JSON.parse(String(fetch.mock.calls.at(-1)?.[1]?.body))).toEqual({
        plan_id: 'plan-1',
        role: 'baseline',
        idempotency_key: expect.any(String),
      })
    } finally {
      vi.unstubAllGlobals()
    }
  })

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
