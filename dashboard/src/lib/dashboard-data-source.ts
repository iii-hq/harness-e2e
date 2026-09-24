import type {
  AssessmentContract,
  AssessmentSummary,
  RunAssessmentContract,
} from '@/lib/assessment-contract'
import { getDashboardIiiClient } from '@/lib/iii-client'
import type { PlanExecution } from '@/lib/plan-execution'
import type {
  EvaluatedVersionsResponse,
  TestHistoryInput,
  TestHistoryResponse,
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
  scenarios: Array<{
    scenario_id: string
    behavior_sha256: string
    case_id: string
    seed: number
    inputs_sha256: string
    contract_sha256: string
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
  compatible?: boolean
}

export type LocalPlansResponse = {
  mode: 'unified'
  plans: LocalPlan[]
  master_plan?: MasterTestPlan
}

/** What running an execution again would need. */
export type ExecutionParameters = {
  scenarios: string[]
  runs: number
  technical_retries: number
  model: string
  provider: string
  /** Agent profile the subject ran under. */
  agent: string | null
}

/** Where an execution came from; data only, every execution reads alike. */
export type ExecutionSource =
  | { kind: 'local' }
  | {
      kind: 'github'
      repository: string
      run_id: number
      run_attempt: number
      url: string
      release_control_execution_id: string | null
    }

export type StackWorker = {
  name: string
  source: 'package' | 'path'
  requested: string | null
  observed: string | null
  commit: string | null
  dirty: boolean | null
  /** Groups that ran this version, listed only when groups disagree. */
  groups?: string[]
}

export type GithubRun = {
  run_id: number
  run_attempt: number
  title: string
  /** When the run was created; the list is ordered and dated by it. */
  created_at: string | null
  /** When its latest attempt started. */
  attempt_started_at?: string | null
  conclusion: string | null
  url: string
  release_control_execution_id: string | null
  suite?: string | null
  suite_label?: string | null
  model?: string | null
  provider?: string | null
  agent?: string | null
  runner_version?: string | null
  contract_error?: string
  /** Listed before its contract was read; the dialog reads it next. */
  contract_pending?: boolean
  execution_id: string | null
  execution_state: string | null
}

export type GithubRunsResponse = {
  repository: string
  page: number
  runs: GithubRun[]
  next_page: number | null
}

export type MasterTestProfile = {
  id: string
  label: string
  purpose: string
  metrics: string[]
  cases?: Array<{
    scenario_id: string
    requirements: string[]
  }>
  scenario_ids: string[]
  repetitions: number
  technical_retries: number
  profile_sha256: string
  budget: {
    planned_runs: number
    scenario_runs: number
    session_turn_limit_sum: number
    subject_token_limit: number | null
    unbounded_token_cases: string[]
  }
}

export type MasterTestPlan = {
  plan_id: string
  definition_sha256: string
  profiles: MasterTestProfile[]
}

export type ExecutionTotals = JsonObject & {
  expected_reports?: number | null
  received_reports?: number | null
  missing_reports?: number | null
  passed_scenarios?: number | null
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
}

export type DashboardScenarioSummary = JsonObject & {
  id: string
  behavior_sha256?: string
  case_id?: string
  status?: string
  passed?: boolean
  pass_rate?: number | null
  mean_score?: number | null
  technical_failures?: number | null
  wall_time_seconds?: number | null
  total_cost_usd?: number | null
  assessment_summary?: AssessmentSummary
}

export type DashboardScenarioMetricSummary = JsonObject & {
  scenario_id: string
  behavior_sha256?: string
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
  }
  workflow?: DashboardWorkflowMetricSummary | null
}

/** Consolidated workflow metrics. Usage totals are sourced exclusively from
 * persisted step metrics and may cover only part of a composite workflow. */
export type DashboardWorkflowMetricSummary = JsonObject & {
  step_count?: number
  succeeded_steps?: number
  failed_steps?: number
  skipped_steps?: number
  cancelled_steps?: number
  running_steps?: number
  pending_steps?: number
  duration_ms?: number
  asset_count?: number
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
  /** A native run's stack source, or an execution's origin. */
  source?: JsonObject
  release?: JsonObject
  lane?: string
  /** Execution state (`importing`, `completed`, …) of a composed execution. */
  state?: string
  parameters?: ExecutionParameters | null
  stack?: JsonObject | StackWorker[]
  plan_id?: string | null
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
    score: number | null
  }>
  completed_runs: number
  task_incomplete_runs: number
  undetermined_runs: number
  technical_invalid_runs: number
  completion_rate: number | null
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
  complete?: boolean
  totals?: DashboardRunMetricTotals | null
}

export type DashboardRunCost = JsonObject & {
  subject_usd?: number | null
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
  /** Technically valid runs that carry a score. */
  scored_runs: number
  /** Mean score of the scored runs; null when none was scored. */
  mean_score: number | null
  total_tokens_consumed: number | null
  tokens_completed_p50: number | null
  failed_attempt_tokens: number | null
  tokens_per_completion: number | null
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
  /** Plain sum of the points the evaluated criteria awarded; null when the
   *  run evaluated nothing. */
  score: number | null
  failures?: Array<JsonObject & { phase?: string; message?: string }>
  wall_time_ms?: number | null
  metrics?: DashboardRunMetrics | null
  cost?: DashboardRunCost | null
  efficiency?: DashboardRunEfficiency | null
  semantic_tests?: SemanticTestReport[]
  scenario_flow?: ScenarioFlowEvidence | null
  retry_attempts?: DashboardRetryAttemptProjection[]
}

export type DashboardRetryAttemptProjection = JsonObject & {
  transcript?: JsonObject
  metrics?: DashboardRunMetrics | null
  run_id: string
  attempt_id: string
  attempt_number: number
  session_id: string
  status: string
  completion: CompletionState
  technical: TechnicalState
  evaluators: DashboardEvaluatorStates
  score: number | null
  wall_time_ms?: number | null
}

export type DashboardReportProjection = JsonObject & {
  result_contract_sha256: string
  report_state: 'complete' | 'partial'
  objective_outcome: 'passed' | 'failed' | 'inconclusive'
  assessment_availability?: 'available' | 'unavailable'
  assessment_contract: AssessmentContract
  assessment_summary: AssessmentSummary
  scenarios: Array<
    JsonObject & {
      scenario_id: string
      /** Digest of the definition the case was materialized from; absent only
       *  when no case could be materialized for the slot. */
      behavior_sha256?: string
      case_id?: string
      case?: JsonObject | null
      assessment_summary?: AssessmentSummary
      status?: string
      passed?: boolean
      pass_rate?: number | null
      mean_score?: number | null
      technical_failures?: number
      aggregate: DashboardScenarioAggregate
      runs: DashboardRunProjection[]
    }
  >
}

export type DashboardExecutionDetail = DashboardExecutionSummary & {
  plan_execution?: PlanExecution
  evidence_error?: string
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
  functions: {
    executions_list: string
    execution_get: string
    execution_delete: string
    execution_rename: string
    evidence_read: string
    github_runs_list: string
    github_run_contracts: string
    github_run_import: string
    evaluated_versions_list: string
    tests_list: string
    test_version_get: string
    test_history_get: string
    catalog_get: string
    execution_start: string
    run_cancel: string
    plan_control: string
    plans_list: string
    plan_get: string
    plan_create: string
    plan_update: string
    plan_delete: string
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
  listExecutions(input?: ExecutionListInput): Promise<ExecutionManifest>
  getExecution(executionId: string): Promise<DashboardExecutionDetail>
  deleteExecution(executionId: string): Promise<void>
  renameExecution(executionId: string, label: string): Promise<PlanExecution>
  /** One file a run's report declares, or one screenshot inside a deliverable. */
  readEvidence(input: {
    execution_id: string
    path: string
    pointer?: string
  }): Promise<{ media_type: string; base64: string }>
  listGithubRuns(page?: number): Promise<GithubRunsResponse>
  readGithubRunContracts(
    runs: Array<Pick<GithubRun, 'run_id' | 'run_attempt'>>,
  ): Promise<{ runs: Array<Partial<GithubRun> & { run_id: number }> }>
  importGithubRun(
    runId: number,
  ): Promise<{ execution_id: string; state: string }>
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
  deletePlan(planId: string): Promise<void>
  startPlan(planId: string, role: 'baseline' | 'candidate'): Promise<LocalPlan>
  getCatalog(url?: string): Promise<JsonObject>
  /** Starts an execution on this stack; Run tests and Run again alike. */
  startExecution(request: {
    parameters: ExecutionParameters
    label: string
  }): Promise<{ execution_id: string }>
  cancelRun(): Promise<JsonObject>
  subscribeRunChanges(
    handler: (payload: JsonObject) => void,
  ): Promise<() => void>
}

let bridge: DashboardDataBridge | null = null

export function installDashboardRuntimeConfig(runtime: RuntimeConfig) {
  bridge = makeBridge(runtime)
}

export async function getDashboardDataBridge(): Promise<DashboardDataBridge> {
  if (!bridge) throw new Error('Console dashboard has not been initialized')
  return bridge
}

function makeBridge(runtime: RuntimeConfig): DashboardDataBridge {
  const readCache = new Map<string, Promise<unknown>>()
  const call = async <T>(
    functionId: string,
    payload: JsonObject,
  ): Promise<T> => {
    try {
      const client = await getDashboardIiiClient()
      return await client.trigger<T>(functionId, payload)
    } catch (cause) {
      throw normalizeBridgeError(cause)
    }
  }
  const cachedCall = <T>(
    functionId: string,
    payload: JsonObject,
  ): Promise<T> => {
    const key = `${functionId}:${JSON.stringify(payload)}`
    const existing = readCache.get(key) as Promise<T> | undefined
    if (existing) return existing
    const pending = call<T>(functionId, payload).catch((cause) => {
      readCache.delete(key)
      throw cause
    })
    readCache.set(key, pending)
    return pending
  }
  return {
    listExecutions: (input = {}) =>
      call(runtime.functions.executions_list, input),
    getExecution: (executionId) =>
      call<ExecutionBundle>(runtime.functions.execution_get, {
        execution_id: executionId,
      }).then((bundle) => bundle.detail),
    deleteExecution: (executionId) =>
      call(runtime.functions.execution_delete, {
        execution_id: executionId,
      }).then(() => undefined),
    renameExecution: (executionId, label) =>
      call(runtime.functions.execution_rename, {
        execution_id: executionId,
        label,
      }),
    readEvidence: (input) => call(runtime.functions.evidence_read, input),
    listGithubRuns: (page = 1) =>
      call(runtime.functions.github_runs_list, { page }),
    readGithubRunContracts: (runs) =>
      call(runtime.functions.github_run_contracts, {
        runs: runs.map(({ run_id, run_attempt }) => ({ run_id, run_attempt })),
      }),
    importGithubRun: (runId) =>
      call(runtime.functions.github_run_import, { run_id: runId }),
    listEvaluatedVersions: (input = {}) =>
      cachedCall(runtime.functions.evaluated_versions_list, input),
    listTests: (input = {}) => cachedCall(runtime.functions.tests_list, input),
    getTestVersion: (input) =>
      cachedCall(runtime.functions.test_version_get, input),
    getTestHistory: (input) =>
      cachedCall(
        runtime.functions.test_history_get,
        input as unknown as JsonObject,
      ),
    planControl: (request) => call(runtime.functions.plan_control, request),
    listPlans: () => call(runtime.functions.plans_list, {}),
    getPlan: (planId) => call(runtime.functions.plan_get, { plan_id: planId }),
    createPlan: (request) => call(runtime.functions.plan_create, request),
    updatePlan: (planId, request) =>
      call(runtime.functions.plan_update, { ...request, plan_id: planId }),
    deletePlan: (planId) =>
      call(runtime.functions.plan_delete, { plan_id: planId }).then(
        () => undefined,
      ),
    startPlan: (planId, role) =>
      call(runtime.functions.plan_run_start, {
        plan_id: planId,
        role,
        idempotency_key: crypto.randomUUID(),
      }),
    getCatalog: (url) =>
      call(runtime.functions.catalog_get, url ? { url } : {}),
    startExecution: (request) =>
      call(runtime.functions.execution_start, request),
    cancelRun: () => call(runtime.functions.run_cancel, {}),
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

export function normalizeBridgeError(cause: unknown) {
  if (cause instanceof Error) return cause
  if (typeof cause === 'object' && cause !== null && 'message' in cause) {
    return new Error(String(cause.message))
  }
  return new Error(String(cause))
}
