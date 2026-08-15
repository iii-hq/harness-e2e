import { currentDashboardRoute } from '@/hooks/use-hash-route'
import { getDashboardIiiClient } from '@/lib/iii-client'
import type {
  EvaluatedVersionsResponse,
  TestCatalogRow,
  TestObservation,
  TestSideSummary,
  TestsListInput,
  TestsListResponse,
  TestVersionInput,
  TestVersionResult,
} from '@/lib/test-catalog'

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
    evaluated_versions_list: string
    tests_list: string
    test_version_get: string
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
  listEvaluatedVersions(input?: {
    cohort_id?: string
  }): Promise<EvaluatedVersionsResponse>
  listTests(input?: TestsListInput): Promise<TestsListResponse>
  getTestVersion(input: TestVersionInput): Promise<TestVersionResult>
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
  page: 'overview' | 'execution',
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

  return false
}

async function dashboardBridge(
  runtime: RuntimeConfig,
): Promise<DashboardDataBridge | null> {
  bridgePromise ??= Promise.resolve(makeBridge(runtime))
  return bridgePromise
}

export async function getDashboardDataBridge(): Promise<DashboardDataBridge> {
  const runtime = await runtimeConfig()
  if (runtime) return (await dashboardBridge(runtime)) ?? makeBridge(runtime)
  return makeStaticBridge()
}

function makeBridge(runtime: RuntimeConfig): DashboardDataBridge {
  const readCache = new Map<string, Promise<unknown>>()
  const call = async <T>(
    functionId: string,
    payload: JsonObject,
    fallback: () => Promise<T>,
  ): Promise<T> => {
    if (runtime.transport === 'static') return fallback()
    try {
      const client = await getDashboardIiiClient()
      return await client.trigger<T>(functionId, payload)
    } catch (cause) {
      if (!isTransportUnavailable(cause)) throw normalizeBridgeError(cause)
      return fallback()
    }
  }

  const cachedCall = <T>(
    functionId: string,
    payload: JsonObject,
    fallback: () => Promise<T>,
  ): Promise<T> => {
    const key = `${functionId}:${JSON.stringify(payload)}`
    const existing = readCache.get(key) as Promise<T> | undefined
    if (existing) return existing
    const pending = call(functionId, payload, fallback).catch((cause) => {
      readCache.delete(key)
      throw cause
    })
    readCache.set(key, pending)
    return pending
  }

  const listExecutions = (input: ExecutionListInput = {}) =>
    cachedCall<ExecutionManifest>(
      runtime.functions.executions_list,
      input,
      () => httpExecutionList(input),
    )

  return {
    remotePaging: true,
    listExecutions,
    listEvaluatedVersions: (input = {}) =>
      cachedCall<EvaluatedVersionsResponse>(
        runtime.functions.evaluated_versions_list,
        input,
        () => httpEvaluatedVersions(input),
      ),
    listTests: (input = {}) =>
      cachedCall<TestsListResponse>(runtime.functions.tests_list, input, () =>
        httpTests(input),
      ),
    getTestVersion: (input) =>
      cachedCall<TestVersionResult>(
        runtime.functions.test_version_get,
        input,
        () => httpTestVersion(input),
      ),
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
      const offHandler = client.on<JsonObject>(handlerId, (payload) => {
        if (payload.kind !== 'progress') readCache.clear()
        handler(payload)
      })
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

function isTransportUnavailable(cause: unknown) {
  if (cause instanceof Error) {
    return /invocation timeout|shutting down|websocket|socket|network/i.test(
      cause.message,
    )
  }
  if (typeof cause !== 'object' || cause === null) return false
  const code = 'code' in cause ? String(cause.code) : ''
  return code === 'function_not_found' || code === 'function_not_invokable'
}

function normalizeBridgeError(cause: unknown) {
  if (cause instanceof Error) return cause
  if (typeof cause === 'object' && cause !== null && 'message' in cause) {
    return new Error(String(cause.message))
  }
  return new Error(String(cause))
}

type StaticVersionSide = {
  summary: TestSideSummary
  contracts: Record<string, string | null>
}

type StaticCatalogRow = TestCatalogRow & {
  version_results: Record<string, { sides: Record<string, StaticVersionSide> }>
  shards: Record<string, string>
}

type StaticTestIndex = {
  evaluated_versions: EvaluatedVersionsResponse
  tests: Omit<TestsListResponse, 'rows'> & { rows: StaticCatalogRow[] }
}

type StaticTestShard = {
  test_id: string
  test_version: number
  observations: TestObservation[]
}

let staticTestIndexPromise: Promise<StaticTestIndex> | null = null

function makeStaticBridge(): DashboardDataBridge {
  const staticIndex = () => {
    staticTestIndexPromise ??= httpJson<StaticTestIndex>('./tests/index.json')
    return staticTestIndexPromise
  }
  return {
    remotePaging: false,
    listExecutions: async () => ({ executions: [] }),
    listEvaluatedVersions: async () => (await staticIndex()).evaluated_versions,
    listTests: async (input = {}) => {
      const source = (await staticIndex()).tests
      const query = input.query?.trim().toLowerCase() ?? ''
      const rows = source.rows
        .filter((row) => !query || row.test_id.toLowerCase().includes(query))
        .map((row) => materializeStaticRow(row, input))
      return { ...source, rows, total: rows.length, next_cursor: null }
    },
    getTestVersion: async (input) => {
      const row = (await staticIndex()).tests.rows.find(
        (candidate) => candidate.test_id === input.test_id,
      )
      if (!row) throw new Error(`Unknown test '${input.test_id}'`)
      const path = row.shards[String(input.test_version)]
      if (!path) {
        throw new Error(
          `Unknown test '${input.test_id}' version ${input.test_version}`,
        )
      }
      const result = materializeStaticResult(row, input)
      const shard = await httpJson<StaticTestShard>(path)
      return {
        ...result,
        from_observations: shard.observations.filter(
          (item) => item.evaluated_version_id === input.from_version_id,
        ),
        to_observations: shard.observations.filter(
          (item) => item.evaluated_version_id === input.to_version_id,
        ),
      }
    },
    getCatalog: () => Promise.reject(new Error('Catalog unavailable')),
    getRunSnapshot: () => Promise.reject(new Error('Runner unavailable')),
    startRun: () => Promise.reject(new Error('Runner unavailable')),
    cancelRun: () => Promise.reject(new Error('Runner unavailable')),
    subscribeRunChanges: async () => () => undefined,
  }
}

function materializeStaticRow(
  row: StaticCatalogRow,
  input: TestsListInput,
): TestCatalogRow {
  const selectedVersion =
    row.available_versions.find((version) => {
      const sides = row.version_results[String(version.version)]?.sides ?? {}
      return Boolean(
        input.from_version_id &&
          input.to_version_id &&
          sides[input.from_version_id] &&
          sides[input.to_version_id],
      )
    })?.version ??
    row.available_versions.find((version) => {
      const sides = row.version_results[String(version.version)]?.sides ?? {}
      return Boolean(input.to_version_id && sides[input.to_version_id])
    })?.version ??
    row.selected_version ??
    row.available_versions[0]?.version ??
    null
  const result =
    selectedVersion &&
    input.cohort_id &&
    input.from_version_id &&
    input.to_version_id &&
    input.from_version_id !== input.to_version_id
      ? materializeStaticResult(row, {
          test_id: row.test_id,
          test_version: selectedVersion,
          cohort_id: input.cohort_id,
          from_version_id: input.from_version_id,
          to_version_id: input.to_version_id,
        })
      : null
  const {
    version_results: _versionResults,
    shards: _shards,
    ...publicRow
  } = row
  return { ...publicRow, selected_version: selectedVersion, result }
}

function materializeStaticResult(
  row: StaticCatalogRow,
  input: TestVersionInput,
): TestVersionResult {
  const version = row.version_results[String(input.test_version)]
  if (!version) {
    throw new Error(
      `Unknown test '${input.test_id}' version ${input.test_version}`,
    )
  }
  const from = version.sides[input.from_version_id] ?? null
  const to = version.sides[input.to_version_id] ?? null
  const compatibility = staticCompatibility(from, to)
  const difference = (left: number | null, right: number | null) =>
    compatibility === 'compatible' && left !== null && right !== null
      ? right - left
      : null
  return {
    test_id: row.test_id,
    test_version: input.test_version,
    compatibility,
    from: from?.summary ?? null,
    to: to?.summary ?? null,
    delta: {
      score: difference(
        from?.summary.median_score ?? null,
        to?.summary.median_score ?? null,
      ),
      cost_usd: difference(
        from?.summary.median_cost_usd ?? null,
        to?.summary.median_cost_usd ?? null,
      ),
      tokens: difference(
        from?.summary.median_tokens ?? null,
        to?.summary.median_tokens ?? null,
      ),
      duration_seconds: difference(
        from?.summary.median_duration_seconds ?? null,
        to?.summary.median_duration_seconds ?? null,
      ),
    },
    from_observations: [],
    to_observations: [],
  }
}

function staticCompatibility(
  from: StaticVersionSide | null,
  to: StaticVersionSide | null,
): TestVersionResult['compatibility'] {
  if (!from || !to) return 'missing_side'
  if (
    Object.values(from.contracts).some((value) => value === null) ||
    Object.values(to.contracts).some((value) => value === null)
  ) {
    return 'contract_conflict'
  }
  return JSON.stringify(from.contracts) === JSON.stringify(to.contracts)
    ? 'compatible'
    : 'contract_changed'
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
  } catch (cause) {
    if (!isTransportUnavailable(cause)) throw normalizeBridgeError(cause)
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

function queryString(input: Record<string, unknown>) {
  const query = new URLSearchParams()
  for (const [key, value] of Object.entries(input)) {
    if (value !== undefined && value !== null && value !== '') {
      query.set(key, String(value))
    }
  }
  return query.size > 0 ? `?${query}` : ''
}

function httpEvaluatedVersions(input: { cohort_id?: string }) {
  return httpJson<EvaluatedVersionsResponse>(
    `./api/dashboard/evaluated-versions${queryString(input)}`,
  )
}

function httpTests(input: TestsListInput) {
  return httpJson<TestsListResponse>(
    `./api/dashboard/tests${queryString(input)}`,
  )
}

function httpTestVersion(input: TestVersionInput) {
  return httpJson<TestVersionResult>(
    `./api/dashboard/test-version${queryString(input)}`,
  )
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
