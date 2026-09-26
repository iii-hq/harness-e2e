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
  it('carries suite, stack and execution controls through the Console client', async () => {
    const trigger = vi.fn(async () => ({ suites: [] }))
    installDashboardIiiClient({ trigger } as unknown as DashboardIiiClient)
    installDashboardRuntimeConfig({
      functions: {
        suites_list: 'suites-list',
        suite_create: 'suite-create',
        suite_update: 'suite-update',
        suite_delete: 'suite-delete',
        stacks_list: 'stacks-list',
        stack_create: 'stack-create',
        stack_update: 'stack-update',
        stack_delete: 'stack-delete',
        credentials_list: 'credentials-list',
        credential_set: 'credential-set',
        credential_delete: 'credential-delete',
        credentials_import: 'credentials-import',
        execution_cancel: 'execution-cancel',
      },
    } as RuntimeConfig)
    const live = await getDashboardDataBridge()
    await expect(live.listSuites()).resolves.toEqual({ suites: [] })
    expect(trigger).toHaveBeenCalledWith('suites-list', {})
    await live.createSuite('regression')
    expect(trigger).toHaveBeenCalledWith('suite-create', {
      from: 'regression',
      label: '',
    })
    await live.updateSuite('suite-1', { repetitions: 2 })
    expect(trigger).toHaveBeenCalledWith('suite-update', {
      suite_id: 'suite-1',
      repetitions: 2,
    })
    await live.deleteSuite('suite-1')
    expect(trigger).toHaveBeenCalledWith('suite-delete', {
      suite_id: 'suite-1',
    })
    await live.listStacks()
    expect(trigger).toHaveBeenCalledWith('stacks-list', {})
    await live.createStack('default')
    expect(trigger).toHaveBeenCalledWith('stack-create', {
      from: 'default',
      label: '',
    })
    await live.updateStack('stack-1', { yaml: 'containers: {}\n' })
    expect(trigger).toHaveBeenCalledWith('stack-update', {
      stack_id: 'stack-1',
      yaml: 'containers: {}\n',
    })
    await live.deleteStack('stack-1')
    expect(trigger).toHaveBeenCalledWith('stack-delete', {
      stack_id: 'stack-1',
    })
    await live.listCredentials()
    expect(trigger).toHaveBeenCalledWith('credentials-list', {})
    await live.setCredential('OPENAI_API_KEY', 'sk-value')
    expect(trigger).toHaveBeenCalledWith('credential-set', {
      name: 'OPENAI_API_KEY',
      secret: 'sk-value',
    })
    await live.deleteCredential('OPENAI_API_KEY')
    expect(trigger).toHaveBeenCalledWith('credential-delete', {
      name: 'OPENAI_API_KEY',
    })
    await live.importCredentials()
    expect(trigger).toHaveBeenCalledWith('credentials-import', {})
    await live.cancelExecution('plan-1')
    expect(trigger).toHaveBeenCalledWith('execution-cancel', {
      execution_id: 'plan-1',
    })
    installDashboardRuntimeConfig({
      functions: { execution_delete: 'execution-delete' },
    } as RuntimeConfig)
    await (await getDashboardDataBridge()).deleteExecution('execution-1')
    expect(trigger).toHaveBeenCalledWith('execution-delete', {
      execution_id: 'execution-1',
    })
    installDashboardRuntimeConfig({
      functions: { execution_start: 'execution-start' },
    } as RuntimeConfig)
    const started = {
      label: '',
      parameters: {
        scenarios: ['minimal_path'],
        runs: 1,
        technical_retries: 0,
        model: 'model',
        provider: 'provider',
        agent: null,
      },
    }
    await (await getDashboardDataBridge()).startExecution(started)
    expect(trigger).toHaveBeenCalledWith('execution-start', started)
    installDashboardRuntimeConfig({
      functions: { execution_slot_rerun: 'execution-slot-rerun' },
    } as RuntimeConfig)
    await (await getDashboardDataBridge()).rerunScenario(
      'plan-1',
      'minimal_path',
    )
    expect(trigger).toHaveBeenCalledWith('execution-slot-rerun', {
      execution_id: 'plan-1',
      scenario_id: 'minimal_path',
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
