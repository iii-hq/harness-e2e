import { currentDashboardRoute } from '@/hooks/use-hash-route'
import { getDashboardIiiClient } from '@/lib/iii-client'

type JsonObject = Record<string, unknown>

export type ExecutionManifest = JsonObject & {
  executions: JsonObject[]
  mode?: string
  total?: number
  next_cursor?: string | null
}

type ExecutionBundle = {
  manifest: ExecutionManifest
  detail: JsonObject
}

type RuntimeConfig = {
  mode: 'local' | 'observed'
  transport: 'iii' | 'static'
  page_size: number
  functions: {
    executions_list: string
    execution_get: string
    catalog_get: string
    run_status: string
    run_start: string
    run_cancel: string
    changed_trigger: string
  }
}

export type ExecutionListInput = {
  cursor?: string
  limit?: number
  query?: string
  status?: string
  event?: string
  ids?: string[]
}

export type DashboardDataBridge = {
  remotePaging: boolean
  listExecutions(input?: ExecutionListInput): Promise<ExecutionManifest>
  getCatalog(url?: string): Promise<JsonObject>
  getRunSnapshot(after?: number): Promise<JsonObject>
  startRun(request: JsonObject): Promise<JsonObject>
  cancelRun(): Promise<JsonObject>
  subscribeRunChanges(
    handler: (payload: JsonObject) => void,
  ): Promise<() => void>
}

let runtimePromise: Promise<RuntimeConfig | null> | null = null
let bridgePromise: Promise<DashboardDataBridge | null> | null = null

export async function loadRuntimeExecutionData(
  page: 'overview' | 'execution' | 'compare',
): Promise<boolean> {
  const runtime = await runtimeConfig()
  if (runtime?.mode !== 'local') return false
  const bridge = await dashboardBridge(runtime)
  if (!bridge) return false
  window.HarnessDashboardData = bridge

  if (page === 'overview') {
    window.HARNESS_EXECUTIONS = await bridge.listExecutions({
      cursor: '0',
      limit: runtime.page_size,
    })
    return true
  }

  const route = currentDashboardRoute()
  if (page === 'execution') {
    const executionId =
      route.page === 'execution' ? route.executionId.trim() : ''
    if (!executionId) return false
    const bundle = await getExecution(runtime, executionId)
    window.HARNESS_EXECUTIONS = bundle.manifest
    window.HARNESS_EXECUTION_DETAILS = {
      ...(window.HARNESS_EXECUTION_DETAILS ?? {}),
      [executionId]: bundle.detail,
    }
    return true
  }

  const ids = (route.page === 'compare' ? [route.left, route.right] : [])
    .map((value) => value?.trim())
    .filter((value): value is string => Boolean(value))
  if (new Set(ids).size !== 2) {
    window.HARNESS_EXECUTIONS = await bridge.listExecutions({
      cursor: '0',
      limit: 1,
      ids: [],
    })
    window.HARNESS_EXECUTIONS.executions = []
    window.HARNESS_EXECUTIONS.total = 0
    return true
  }
  window.HARNESS_EXECUTIONS = await bridge.listExecutions({
    cursor: '0',
    limit: 2,
    ids,
  })
  return true
}

async function dashboardBridge(
  runtime: RuntimeConfig,
): Promise<DashboardDataBridge | null> {
  bridgePromise ??= Promise.resolve(makeBridge(runtime))
  return bridgePromise
}

function makeBridge(runtime: RuntimeConfig): DashboardDataBridge {
  const call = async <T extends JsonObject>(
    functionId: string,
    payload: JsonObject,
    fallback: () => Promise<T>,
  ): Promise<T> => {
    try {
      const client = await getDashboardIiiClient()
      return await client.trigger<T>(functionId, payload)
    } catch {
      return fallback()
    }
  }

  const listExecutions = (input: ExecutionListInput = {}) =>
    call<ExecutionManifest>(runtime.functions.executions_list, input, () =>
      httpExecutionList(input),
    )

  return {
    remotePaging: true,
    listExecutions,
    getCatalog: (url) =>
      call(runtime.functions.catalog_get, url ? { url } : {}, () =>
        httpJson(
          `./api/local/catalog${url ? `?url=${encodeURIComponent(url)}` : ''}`,
        ),
      ),
    getRunSnapshot: (after) =>
      call(
        runtime.functions.run_status,
        typeof after === 'number' ? { after } : {},
        () =>
          httpJson(
            `./api/local/run${typeof after === 'number' ? `?after=${after}` : ''}`,
          ),
      ),
    startRun: (request) =>
      call(runtime.functions.run_start, request, () =>
        httpJson('./api/local/run', {
          method: 'POST',
          body: JSON.stringify(request),
        }),
      ),
    cancelRun: () =>
      call(runtime.functions.run_cancel, {}, () =>
        httpJson('./api/local/run/cancel', {
          method: 'POST',
          body: '{}',
        }),
      ),
    subscribeRunChanges: async (handler) => {
      const client = await getDashboardIiiClient()
      const handlerId = 'iii::harness-e2e-dashboard::changed'
      const offHandler = client.on<JsonObject>(handlerId, handler)
      const offTrigger = client.registerTrigger({
        type: runtime.functions.changed_trigger,
        function_id: `${handlerId}::${client.browserId}`,
        config: {},
      })
      return () => {
        offTrigger()
        offHandler()
      }
    },
  }
}

async function getExecution(
  runtime: RuntimeConfig,
  executionId: string,
): Promise<ExecutionBundle> {
  try {
    const client = await getDashboardIiiClient()
    return await client.trigger<ExecutionBundle>(
      runtime.functions.execution_get,
      {
        execution_id: executionId,
      },
    )
  } catch {
    return httpJson(
      `./api/dashboard/executions/${encodeURIComponent(executionId)}`,
    )
  }
}

async function runtimeConfig(): Promise<RuntimeConfig | null> {
  runtimePromise ??= httpJson<RuntimeConfig>('./api/dashboard').catch(
    () => null,
  )
  return runtimePromise
}

async function httpExecutionList(input: ExecutionListInput) {
  const query = new URLSearchParams()
  if (input.cursor) query.set('cursor', input.cursor)
  if (input.limit) query.set('limit', String(input.limit))
  if (input.query) query.set('query', input.query)
  if (input.status) query.set('status', input.status)
  if (input.event) query.set('event', input.event)
  if (input.ids?.length) query.set('ids_csv', input.ids.join(','))
  const suffix = query.size > 0 ? `?${query}` : ''
  return httpJson<ExecutionManifest>(`./api/dashboard/executions${suffix}`)
}

async function httpJson<T extends JsonObject>(
  path: string,
  options: RequestInit = {},
): Promise<T> {
  const response = await fetch(path, {
    cache: 'no-store',
    headers: { 'Content-Type': 'application/json' },
    ...options,
  })
  const payload = (await response.json().catch(() => ({}))) as T & {
    error?: string
  }
  if (!response.ok) {
    throw new Error(payload.error || `Request failed (${response.status})`)
  }
  return payload
}
