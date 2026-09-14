import { describe, expect, it, vi } from 'vitest'
import {
  getDashboardDataBridge,
  installDashboardRuntimeConfig,
  type RuntimeConfig,
} from '@/lib/dashboard-data-source'
import {
  type DashboardIiiClient,
  installDashboardIiiClient,
} from '@/lib/iii-client'

describe('live dashboard transport', () => {
  it('carries current plan controls and idempotent starts through the Console client', async () => {
    const trigger = vi.fn(async () => ({ ready: true }))
    installDashboardIiiClient({ trigger } as unknown as DashboardIiiClient)
    const request = {
      action: 'requirements',
      plan_id: 'plan-1',
    }
    installDashboardRuntimeConfig({
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
    installDashboardRuntimeConfig({
      functions: { plan_delete: 'plan-delete' },
    } as RuntimeConfig)
    await (await getDashboardDataBridge()).deletePlan('plan-1')
    expect(trigger).toHaveBeenCalledWith('plan-delete', { plan_id: 'plan-1' })
    installDashboardRuntimeConfig({
      functions: { execution_delete: 'execution-delete' },
    } as RuntimeConfig)
    await (await getDashboardDataBridge()).deleteExecution('execution-1')
    expect(trigger).toHaveBeenCalledWith('execution-delete', {
      execution_id: 'execution-1',
    })
  })

  it('does not retry iii failures through HTTP', async () => {
    const fetch = vi.fn()
    vi.stubGlobal('fetch', fetch)
    installDashboardIiiClient({
      trigger: vi.fn().mockRejectedValue(new Error('websocket disconnected')),
    } as unknown as DashboardIiiClient)
    installDashboardRuntimeConfig({
      functions: { executions_list: 'executions' },
    } as RuntimeConfig)

    await expect(
      (await getDashboardDataBridge()).listExecutions(),
    ).rejects.toThrow('websocket disconnected')
    expect(fetch).not.toHaveBeenCalled()
    vi.unstubAllGlobals()
  })

  it('fetches fresh execution summaries and invalidates read caches on progress', async () => {
    const runtime = {
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
