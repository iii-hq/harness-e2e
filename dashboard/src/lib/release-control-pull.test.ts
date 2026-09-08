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

describe('release control pull transport', () => {
  it('calls the dashboard function over iii and the POST route over HTTP', async () => {
    const response = {
      runs_dir: 'runs',
      repository: 'iii-hq/harness-e2e',
      workflow: 'exact-stack-e2e.yml',
      executions: [],
      remaining_runs: 0,
    }
    const trigger = vi.fn(async () => response)
    installDashboardIiiClient({ trigger } as unknown as DashboardIiiClient)
    installDashboardRuntimeConfig({
      mode: 'local',
      transport: 'iii',
      functions: {
        release_control_pull: 'e2e::dashboard::release-control-pull',
      },
    } as RuntimeConfig)
    const live = await getDashboardDataBridge()
    await expect(live.pullReleaseControl()).resolves.toEqual(response)
    expect(trigger).toHaveBeenCalledWith(
      'e2e::dashboard::release-control-pull',
      {},
    )
    await live.pullReleaseControl({ execution_id: 'abc' })
    expect(trigger).toHaveBeenLastCalledWith(
      'e2e::dashboard::release-control-pull',
      { execution_id: 'abc' },
    )

    const fetch = vi.fn(
      async (_url: string, _options?: RequestInit) =>
        new Response(JSON.stringify(response), { status: 200 }),
    )
    vi.stubGlobal('fetch', fetch)
    try {
      installDashboardRuntimeConfig({
        mode: 'local',
        transport: 'static',
        functions: {},
      } as RuntimeConfig)
      const http = await getDashboardDataBridge()
      await expect(http.pullReleaseControl({ limit: 3 })).resolves.toEqual(
        response,
      )
      expect(fetch.mock.calls[0]?.[0]).toBe(
        './api/dashboard/release-control/pull',
      )
      expect(fetch.mock.calls[0]?.[1]?.method).toBe('POST')
      expect(fetch.mock.calls[0]?.[1]?.body).toBe('{"limit":3}')
    } finally {
      vi.unstubAllGlobals()
    }
  })
})
