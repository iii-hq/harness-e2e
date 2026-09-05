import type {
  AssessmentContract,
  AssessmentSummary,
  RunAssessmentContract,
} from '@/lib/assessment-contract'
import { getDashboardIiiClient } from '@/lib/iii-client'
import type { RESULTS_SCHEMA_VERSION } from '@/lib/result-contract.generated'
import type {
  EvaluatedVersionsResponse,
  TestCatalogRow,
  TestHistoryInput,
  TestHistoryResponse,
  TestObservation,
  TestSideSummary,
  TestsListInput,
  TestsListResponse,
  TestVersionInput,
  TestVersionResult,
} from '@/lib/test-catalog'

export type JsonObject = Record<string, unknown>
export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue }

export type LocalPlanState =
  | 'draft'
  | 'baseline_running'
  | 'baseline_ready'
  | 'candidate_running'
  | 'comparison_ready'

export type LocalPlan = {
  schema_version: number
  id: string
  label: string
  purpose: string
  created_at: string
  updated_at: string
  state: LocalPlanState
  locked: boolean
  scope_hash: string
  url: string
  model: string
  provider: string
  judge_model: string
  judge_provider: string
  scenarios: Array<{
    scenario_id: string
    scenario_version: number
    case_id: string
    seed: number
    inputs_sha256: string
    contract_sha256: string
    complexity_tier: string
  }>
  scenario_ids: string[]
  runs: number
  technical_retries: number
  seed: number | null
  baseline_execution_id: string | null
  candidate_execution_ids: string[]
  candidate_labels?: Record<string, string>
  incomplete_execution_ids: string[]
  last_attempt_id: string | null
  template_id?: string | null
  protected_executor_required?: boolean
  compatible?: boolean
}

export type LocalPlansResponse = {
  mode: 'local'
  plans: LocalPlan[]
  master_plan?: MasterTestPlan
}

export type MasterTestProfile = {
  id: string
  label: string
  purpose: string
  metrics: string[]
  judge_required?: boolean
  cases?: Array<{
    scenario_id: string
    judge_required: boolean
    requirements: string[]
  }>
  scenario_ids: string[]
  repetitions: number
  technical_retries: number
  profile_sha256: string
  protected_supervisor_required: boolean
  campaigns: JsonObject[]
  budget: {
    planned_runs: number
    scenario_runs: number
    fault_runs: number
    session_turn_limit_sum: number
    subject_token_limit: number | null
    unbounded_token_cases: string[]
  }
}

export type MasterTestPlan = {
  plan_id: string
  version: number
  definition_sha256: string
  profiles: MasterTestProfile[]
}

export type ExecutionTotals = JsonObject & {
  expected_reports?: number | null
  received_reports?: number | null
  missing_reports?: number | null
  passed_scenarios?: number | null
  hard_gate_failures?: number | null
  technical_failures?: number | null
  infra_failures?: number | null
  resource_limit_failures?: number | null
  scenario_pass_rate?: number | null
  report_coverage?: number | null
  total_cost_usd?: number | null
  wall_time_seconds?: number | null
  workflow_duration_seconds?: number | null
  total_tokens?: number | null
  function_calls?: number | null
  function_call_errors?: number | null
  turns?: number | null
  /** Retry plus non-completed consumption, pooled across scenarios. */
  failed_attempt_tokens?: number | null
  /** Subject tokens over completed logical runs, pooled across scenarios. */
  tokens_per_completion?: number | null
}

export type DashboardModelIdentity = JsonObject & {
  id?: string
  model?: string
  provider?: string
  judge?: DashboardModelIdentity | null
}

export type DashboardScenarioSummary = JsonObject & {
  id: string
  scenario_version?: number
  case_id?: string
  status?: string
  passed?: boolean
  pass_rate?: number | null
  median_score?: number | null
  hard_gate_failures?: number | null
  technical_failures?: number | null
  wall_time_seconds?: number | null
  total_cost_usd?: number | null
  assessment_summary?: AssessmentSummary
}

export type DashboardScenarioMetricSummary = JsonObject & {
  scenario_id: string
  scenario_version?: number
  subject_id?: string
  contract_fingerprint?: string
  run_count?: number
  averages?: JsonObject & {
    cost_usd?: number | null
    duration_seconds?: number | null
    function_call_errors?: number | null
    function_calls?: number | null
    turns?: number | null
    tokens?: number | null
    failed_attempt_tokens?: number | null
    tokens_per_completion?: number | null
    work_amplification?: number | null
  }
  samples?: JsonObject & {
    cost_usd?: number | null
    duration_seconds?: number | null
    function_call_errors?: number | null
    function_calls?: number | null
    turns?: number | null
    failed_attempt_tokens?: number | null
    tokens_per_completion?: number | null
    tokens?: number | null
    work_amplification?: number | null
  }
  workflow?: DashboardWorkflowMetricSummary | null
}

/** Consolidated workflow metrics. Usage totals are sourced exclusively from
 * persisted step metrics and may cover only part of a composite workflow. */
export type DashboardWorkflowMetricSummary = JsonObject & {
  step_count?: number
  succeeded_steps?: number
  failed_steps?: number
  hard_gate_failed_steps?: number
  skipped_steps?: number
  cancelled_steps?: number
  running_steps?: number
  pending_steps?: number
  duration_ms?: number
  asset_count?: number
  hard_gate_count?: number
  passed_hard_gate_count?: number
  evaluation_count?: number
  failure_count?: number
  input_tokens?: number | null
  output_tokens?: number | null
  total_tokens?: number | null
  function_calls?: number | null
  function_call_errors?: number | null
  input_token_metric_steps?: number
  output_token_metric_steps?: number
  token_metric_steps?: number
  function_call_metric_steps?: number
  function_call_error_metric_steps?: number
  numeric_metrics?: JsonObject & Record<string, number>
}

export type DashboardSubjectSummary = JsonObject & {
  id: string
  model?: string
  provider?: string
  judge?: DashboardModelIdentity | null
  assessment_summary?: AssessmentSummary
  scenarios: DashboardScenarioSummary[]
}

export type DashboardExecutionSummary = JsonObject & {
  id: string
  label?: string
  run_id?: string
  attempt?: number
  status: string
  started_at?: string
  completed_at?: string
  generated_at?: string
  workflow_name?: string
  workflow_url?: string | null
  event?: string
  actor?: string
  conclusion?: string
  availability?: 'full' | 'aggregate' | 'unavailable' | string
  source?: JsonObject
  release?: JsonObject
  lane?: string
  subjects: DashboardSubjectSummary[]
  scenario_metrics?: DashboardScenarioMetricSummary[]
  workflow_metrics?: DashboardWorkflowMetricSummary | null
  totals?: ExecutionTotals
  assessment_summary?: AssessmentSummary
  live_progress?: LiveProgress | null
  live_progress_error?: string | null
  persistence_errors?: string[]
  slot_start_deadline_seconds?: number | null
  baseline_comparable?: boolean
}

export type LiveProgress = {
  updated_at: string
  phase?: string | null
  terminal: boolean
  terminal_reason?: string | null
  committed_events: number
  planned_slots: number
  runs_committed: number
  slots_deferred: number
  attempts_started: number
  attempts_finished: number
  subject_observations_committed: number
  active_attempt: {
    scenario_id: string
    run_id: string
    attempt_id: string
    session_id: string
    started_at: string
  } | null
  slots: Array<{
    slot_id: string
    scenario_id: string
    repetition: number
    state: 'pending' | 'committed' | 'deferred'
    reason: string | null
    run_id: string | null
    completion: CompletionState | null
    technical: TechnicalState | null
    objective_score: number | null
    quality_score_completed: number | null
  }>
  completed_runs: number
  task_incomplete_runs: number
  undetermined_runs: number
  technical_invalid_runs: number
  completion_rate: number | null
  quality_score_completed: number | null
  quality_scored_completed_runs: number
  observed_tokens: number | null
  token_observed_attempts: number
  observed_cost_usd: number | null
  cost_observed_runs: number
}

export type DashboardRunMetricTotals = JsonObject & {
  input_tokens?: number | null
  output_tokens?: number | null
  cache_read_tokens?: number | null
  cache_write_tokens?: number | null
  reasoning_tokens?: number | null
  function_calls?: number | null
  function_call_errors?: number | null
  sessions?: number | null
  turns?: number | null
}

export type DashboardRunMetrics = JsonObject & {
  totals?: DashboardRunMetricTotals | null
}

export type DashboardRunCost = JsonObject & {
  total_usd?: number | null
}

export type CompletionState = 'completed' | 'task_incomplete' | 'undetermined'
export type TechnicalState = 'valid' | 'technical_invalid'
export type EvaluatorAvailability =
  | 'not_required'
  | 'pending'
  | 'available'
  | 'unavailable'

export type DashboardEvaluatorStates = {
  completion: EvaluatorAvailability
  quality: EvaluatorAvailability
  final_advisory: EvaluatorAvailability
}

export type DashboardScenarioAggregate = JsonObject & {
  planned_runs: number
  observed_runs: number
  deferred_runs: number
  completed_runs: number
  task_incomplete_runs: number
  undetermined_runs: number
  technical_valid_runs: number
  technical_invalid_runs: number
  execution_reliability: number | null
  completion_evidence_coverage: number | null
  completion_rate: number | null
  objective_scored_runs: number
  objective_median_score: number | null
  objective_score_coverage: number | null
  quality_scored_completed_runs: number
  quality_score_completed: number | null
  quality_coverage: number | null
  total_tokens_consumed: number | null
  judge_tokens_consumed: number | null
  tokens_completed_p50: number | null
  failed_attempt_tokens: number | null
  tokens_per_completion: number | null
  hard_gate_failures: number
  technical_failures: number
}

export type DashboardRunEfficiency = JsonObject & {
  wall_time_ms?: number | null
  total_tokens?: number | null
  function_calls?: number | null
  function_call_errors?: number | null
  sessions?: number | null
  turns?: number | null
}

export type SemanticTestAsset = JsonObject & {
  id: string
  namespaced_id?: string
  kind?: string
  media_type?: string
  size_bytes?: number
  artifact: JsonObject & { path: string; sha256?: string }
}

export type SemanticTestReport = JsonObject & {
  node_id: string
  step_type: string
  step_version: number
  required: boolean
  dependencies: string[]
  status: string
  duration_ms: number
  metrics?: JsonValue | null
  cost_usd?: number | null
  assets?: SemanticTestAsset[]
  hard_gates?: Array<
    JsonObject & {
      id: string
      passed: boolean
      reason: string
      evidence_ids?: string[]
    }
  >
  evaluations?: Array<
    JsonObject & {
      id: string
      outcome: string
      summary: string
      score?: number | null
      evidence_ids?: string[]
    }
  >
  failures?: Array<
    JsonObject & { phase: string; message: string; technical?: boolean }
  >
  skip_reason?: string | null
}

export type ScenarioFlowEvidence = JsonObject & {
  definition_sha256: string
  snapshot: JsonObject & {
    executable: false
    scenario_id?: string
    scenario_version?: number
  }
  checkpoint: JsonObject & { path: string; sha256?: string }
  cleanup: JsonObject & {
    status: 'succeeded' | 'failed'
    duration_ms: number
    failure?: string | null
  }
}

export type DashboardRunProjection = JsonObject & {
  run_id: string
  attempt_id: string
  attempt_number?: number
  session_id?: string
  assessment: RunAssessmentContract
  transcript?: JsonObject
  status: string
  completion: CompletionState
  technical: TechnicalState
  evaluators: DashboardEvaluatorStates
  objective_score: number | null
  quality_score_completed: number | null
  score?: number | null
  validation_score?: number | null
  instruction_adherence?:
    | (JsonObject & {
        availability: 'available' | 'unavailable' | 'failed'
        score?: number | null
        summary?: string
        requirements?: JsonValue[]
      })
    | null
  markdown_execution?:
    | (JsonObject & {
        pipeline_complete: boolean
        source_path?: string
        source_sha256?: string
        behavior_sha256?: string
        compiled_sha256?: string
        materialized_plan_sha256?: string | null
        phases?: JsonValue[]
      })
    | null
  failures?: Array<JsonObject & { phase?: string; message?: string }>
  wall_time_ms?: number | null
  metrics?: DashboardRunMetrics | null
  cost?: DashboardRunCost | null
  efficiency?: DashboardRunEfficiency | null
  judge_usage?:
    | (JsonObject & {
        input_tokens?: number | null
        output_tokens?: number | null
        cost_usd?: number | null
      })
    | null
  semantic_tests?: SemanticTestReport[]
  scenario_flow?: ScenarioFlowEvidence | null
  retry_attempts?: DashboardRetryAttemptProjection[]
}

export type DashboardRetryAttemptProjection = JsonObject & {
  run_id: string
  attempt_id: string
  attempt_number: number
  session_id: string
  status: string
  completion: CompletionState
  technical: TechnicalState
  evaluators: DashboardEvaluatorStates
  objective_score: number | null
  quality_score_completed: number | null
  wall_time_ms?: number | null
}

export type DashboardReportProjection = JsonObject & {
  schema_version: typeof RESULTS_SCHEMA_VERSION
  result_contract_sha256: string
  scoring_profile_sha256: string
  report_state: 'complete' | 'partial'
  objective_outcome: 'passed' | 'failed' | 'inconclusive'
  assessment_availability?: 'available' | 'unavailable'
  assessment_contract: AssessmentContract
  assessment_summary: AssessmentSummary
  scenarios: Array<
    JsonObject & {
      scenario_id: string
      scenario_version: number
      assessment_summary?: AssessmentSummary
      status?: string
      passed?: boolean
      pass_rate?: number | null
      median_score?: number | null
      hard_gate_failures?: number
      technical_failures?: number
      aggregate: DashboardScenarioAggregate
      runs: DashboardRunProjection[]
    }
  >
}

export type DashboardExecutionDetail = DashboardExecutionSummary & {
  reports: Array<
    JsonObject & {
      subject_id: string
      scenario_id: string
      available: boolean
      report?: DashboardReportProjection
    }
  >
}

export type ExecutionManifest = JsonObject & {
  executions: DashboardExecutionSummary[]
  mode?: string
  total?: number
  next_cursor?: string | null
}

export type ExecutionBundle = {
  manifest: ExecutionManifest
  detail: DashboardExecutionDetail
}

export type RuntimeConfig = {
  mode: 'local' | 'observed'
  transport: 'iii' | 'static'
  page_size: number
  http_fallback?: boolean
  functions: {
    executions_list: string
    execution_get: string
    evaluated_versions_list: string
    tests_list: string
    test_version_get: string
    test_history_get: string
    catalog_get: string
    local_scenario_create: string
    run_status: string
    run_start: string
    run_cancel: string
    plan_control: string
    plans_list: string
    plan_get: string
    plan_create: string
    plan_update: string
    plan_run_start: string
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
  mode: 'local' | 'observed' | 'published'
  remotePaging: boolean
  listExecutions(input?: ExecutionListInput): Promise<ExecutionManifest>
  getExecution(executionId: string): Promise<DashboardExecutionDetail>
  listEvaluatedVersions(input?: {
    cohort_id?: string
  }): Promise<EvaluatedVersionsResponse>
  listTests(input?: TestsListInput): Promise<TestsListResponse>
  getTestVersion(input: TestVersionInput): Promise<TestVersionResult>
  getTestHistory(input: TestHistoryInput): Promise<TestHistoryResponse>
  planControl(request: JsonObject): Promise<JsonObject>
  listPlans(): Promise<LocalPlansResponse>
  getPlan(planId: string): Promise<LocalPlan>
  createPlan(request: JsonObject): Promise<LocalPlan>
  updatePlan(planId: string, request: JsonObject): Promise<LocalPlan>
  startPlan(planId: string, role: 'baseline' | 'candidate'): Promise<LocalPlan>
  getCatalog(url?: string): Promise<JsonObject>
  createLocalScenario(request: {
    file_name: string
    source: string
  }): Promise<JsonObject>
  getRunSnapshot(after?: number): Promise<JsonObject>
  startRun(request: JsonObject): Promise<JsonObject>
  cancelRun(): Promise<JsonObject>
  subscribeRunChanges(
    handler: (payload: JsonObject) => void,
  ): Promise<() => void>
}

let runtimePromise: Promise<RuntimeConfig | null> | null = null
let bridgePromise: Promise<DashboardDataBridge | null> | null = null
let installedRuntime: RuntimeConfig | null = null

export function installDashboardRuntimeConfig(runtime: RuntimeConfig) {
  installedRuntime = runtime
  runtimePromise = Promise.resolve(runtime)
  bridgePromise = null
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
      if (runtime.http_fallback === false || !isTransportUnavailable(cause)) {
        throw normalizeBridgeError(cause)
      }
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
    call<ExecutionManifest>(runtime.functions.executions_list, input, () =>
      httpExecutionList(input),
    )

  return {
    mode: runtime.mode,
    remotePaging: true,
    listExecutions,
    getExecution: (executionId) =>
      getExecution(runtime, executionId).then((bundle) => bundle.detail),
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
    getTestHistory: (input) =>
      cachedCall<TestHistoryResponse>(
        runtime.functions.test_history_get,
        input as unknown as JsonObject,
        () => httpTestHistory(input),
      ),
    planControl: (request) =>
      call<JsonObject>(runtime.functions.plan_control, request, () =>
        httpJson<JsonObject>('./api/dashboard/plans/control', {
          method: 'POST',
          body: JSON.stringify(request),
        }),
      ),
    listPlans: () =>
      call<LocalPlansResponse>(runtime.functions.plans_list, {}, () =>
        httpJson<LocalPlansResponse>('./api/dashboard/plans'),
      ),
    getPlan: (planId) =>
      call<LocalPlan>(runtime.functions.plan_get, { plan_id: planId }, () =>
        httpJson<LocalPlan>(
          `./api/dashboard/plans/${encodeURIComponent(planId)}`,
        ),
      ),
    createPlan: (request) =>
      call<LocalPlan>(runtime.functions.plan_create, request, () =>
        httpJson<LocalPlan>('./api/dashboard/plans', {
          method: 'POST',
          body: JSON.stringify(request),
        }),
      ),
    updatePlan: (planId, request) =>
      call<LocalPlan>(
        runtime.functions.plan_update,
        { ...request, plan_id: planId },
        () =>
          httpJson<LocalPlan>(
            `./api/dashboard/plans/${encodeURIComponent(planId)}`,
            { method: 'PATCH', body: JSON.stringify(request) },
          ),
      ),
    startPlan: (planId, role) => {
      const request = {
        plan_id: planId,
        role,
        idempotency_key: crypto.randomUUID(),
      }
      return call<LocalPlan>(runtime.functions.plan_run_start, request, () =>
        httpJson<LocalPlan>(
          `./api/dashboard/plans/${encodeURIComponent(planId)}/runs`,
          {
            method: 'POST',
            body: JSON.stringify(request),
          },
        ),
      )
    },
    getCatalog: (url) =>
      call(runtime.functions.catalog_get, url ? { url } : {}, () =>
        httpJson(
          `./api/local/catalog${url ? `?url=${encodeURIComponent(url)}` : ''}`,
        ),
      ),
    createLocalScenario: (request) =>
      call(runtime.functions.local_scenario_create, request, () =>
        httpJson('./api/local/scenarios', {
          method: 'POST',
          body: JSON.stringify(request),
        }),
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
        readCache.clear()
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

export type StaticVersionSide = {
  summary: TestSideSummary
  contracts: Record<string, string | null>
  assessment_profiles: Record<string, string | null>
  analyzer_profiles: Record<string, string | null>
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
    mode: 'published',
    remotePaging: false,
    listExecutions: async (input = {}) => {
      const manifest = await staticExecutionManifest()
      return filterStaticExecutions(manifest, input)
    },
    getExecution: async (executionId) => {
      const manifest = await staticExecutionManifest()
      const summary = manifest.executions.find(
        (candidate) => candidate.id === executionId,
      )
      if (!summary) throw new Error(`Unknown execution '${executionId}'`)
      if (!summary.detail_path) {
        return { ...summary, reports: [] }
      }
      return httpJson<DashboardExecutionDetail>(String(summary.detail_path))
    },
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
          (item) =>
            item.cohort_id === input.cohort_id &&
            item.evaluated_version_id === input.from_version_id,
        ),
        to_observations: shard.observations.filter(
          (item) =>
            item.cohort_id === input.cohort_id &&
            item.evaluated_version_id === input.to_version_id,
        ),
      }
    },
    getTestHistory: async () => {
      throw new Error(
        'Test metric history is available only in the local dashboard',
      )
    },
    planControl: async () => {
      throw new Error('Plan controls are available only in the local dashboard')
    },
    listPlans: async () => {
      throw new Error('Local plans are available only in the local dashboard')
    },
    getPlan: async () => {
      throw new Error('Local plans are available only in the local dashboard')
    },
    createPlan: async () => {
      throw new Error('Local plans are available only in the local dashboard')
    },
    updatePlan: async () => {
      throw new Error('Local plans are available only in the local dashboard')
    },
    startPlan: async () => {
      throw new Error('Local plans are available only in the local dashboard')
    },
    getCatalog: () => Promise.reject(new Error('Catalog unavailable')),
    createLocalScenario: () =>
      Promise.reject(new Error('Local scenario authoring unavailable')),
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
        input.cohort_id &&
          input.from_version_id &&
          input.to_version_id &&
          sides[staticSideKey(input.cohort_id, input.from_version_id)] &&
          sides[staticSideKey(input.cohort_id, input.to_version_id)],
      )
    })?.version ??
    row.available_versions.find((version) => {
      const sides = row.version_results[String(version.version)]?.sides ?? {}
      return Boolean(
        input.cohort_id &&
          input.to_version_id &&
          sides[staticSideKey(input.cohort_id, input.to_version_id)],
      )
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
  const from =
    version.sides[staticSideKey(input.cohort_id, input.from_version_id)] ?? null
  const to =
    version.sides[staticSideKey(input.cohort_id, input.to_version_id)] ?? null
  const { compatibility, reasons: compatibility_reasons } = staticCompatibility(
    from,
    to,
  )
  const difference = (left: number | null, right: number | null) =>
    compatibility === 'compatible' && left !== null && right !== null
      ? right - left
      : null
  return {
    test_id: row.test_id,
    test_version: input.test_version,
    compatibility,
    compatibility_reasons,
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

export function staticCompatibility(
  from: StaticVersionSide | null,
  to: StaticVersionSide | null,
): {
  compatibility: TestVersionResult['compatibility']
  reasons: string[]
} {
  if (!from || !to) {
    return {
      compatibility: 'missing_side',
      reasons: ['comparison_side_missing'],
    }
  }
  if (
    Object.values(from.contracts).some((value) => value === null) ||
    Object.values(to.contracts).some((value) => value === null)
  ) {
    return {
      compatibility: 'contract_conflict',
      reasons: ['scenario_contract_conflict'],
    }
  }
  if (JSON.stringify(from.contracts) !== JSON.stringify(to.contracts)) {
    return {
      compatibility: 'contract_changed',
      reasons: ['scenario_contract_changed'],
    }
  }
  if (
    Object.values(from.assessment_profiles).some((value) => value === null) ||
    Object.values(to.assessment_profiles).some((value) => value === null)
  ) {
    return {
      compatibility: 'assessment_conflict',
      reasons: ['assessment_profile_conflict'],
    }
  }
  if (
    JSON.stringify(from.assessment_profiles) !==
    JSON.stringify(to.assessment_profiles)
  ) {
    return {
      compatibility: 'assessment_changed',
      reasons: ['assessment_profile_changed'],
    }
  }
  if (
    Object.values(from.analyzer_profiles).some((value) => value === null) ||
    Object.values(to.analyzer_profiles).some((value) => value === null)
  ) {
    return {
      compatibility: 'analyzer_conflict',
      reasons: ['analyzer_profile_conflict'],
    }
  }
  if (
    JSON.stringify(from.analyzer_profiles) !==
    JSON.stringify(to.analyzer_profiles)
  ) {
    return {
      compatibility: 'analyzer_changed',
      reasons: ['analyzer_profile_changed'],
    }
  }
  return { compatibility: 'compatible', reasons: [] }
}

export function staticSideKey(cohortId: string, evaluatedVersionId: string) {
  return `${cohortId}::${evaluatedVersionId}`
}

async function getExecution(
  runtime: RuntimeConfig,
  executionId: string,
): Promise<ExecutionBundle> {
  if (runtime.transport === 'static') {
    return httpJson<ExecutionBundle>(
      `./api/dashboard/executions/${encodeURIComponent(executionId)}`,
    )
  }
  try {
    const client = await getDashboardIiiClient()
    return await client.trigger<ExecutionBundle>(
      runtime.functions.execution_get,
      {
        execution_id: executionId,
      },
    )
  } catch (cause) {
    if (runtime.http_fallback === false || !isTransportUnavailable(cause)) {
      throw normalizeBridgeError(cause)
    }
    return httpJson(
      `./api/dashboard/executions/${encodeURIComponent(executionId)}`,
    )
  }
}

let staticExecutionManifestPromise: Promise<ExecutionManifest> | null = null

function staticExecutionManifest() {
  staticExecutionManifestPromise ??=
    httpJson<ExecutionManifest>('./executions.json')
  return staticExecutionManifestPromise
}

function filterStaticExecutions(
  manifest: ExecutionManifest,
  input: ExecutionListInput,
): ExecutionManifest {
  const query = input.query?.trim().toLowerCase() ?? ''
  const executions = manifest.executions.filter((execution) => {
    if (
      input.status &&
      input.status !== 'all' &&
      execution.status !== input.status
    ) {
      return false
    }
    if (
      input.event &&
      input.event !== 'all' &&
      execution.event !== input.event
    ) {
      return false
    }
    if (input.ids?.length && !input.ids.includes(execution.id)) return false
    if (!query) return true
    return [
      execution.label,
      execution.id,
      execution.run_id,
      execution.completed_at,
      execution.started_at,
      execution.source?.sha,
      execution.source?.ref,
    ]
      .filter(Boolean)
      .join(' ')
      .toLowerCase()
      .includes(query)
  })
  const offset = Number(input.cursor) || 0
  const limit = input.limit && input.limit > 0 ? input.limit : executions.length
  return {
    ...manifest,
    executions: executions.slice(offset, offset + limit),
    total: executions.length,
    next_cursor:
      offset + limit < executions.length ? String(offset + limit) : null,
  }
}

async function runtimeConfig(): Promise<RuntimeConfig | null> {
  if (installedRuntime) return installedRuntime
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

function httpTestHistory(input: TestHistoryInput) {
  const { test_id, ...queryInput } = input
  return httpJson<TestHistoryResponse>(
    `./api/dashboard/tests/${encodeURIComponent(test_id)}/history${queryString(queryInput)}`,
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
