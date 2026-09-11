import type {
  AnalyzerIdentity,
  AnalyzerUsage,
  AssessmentContract,
  AssessmentSummary,
  RunAssessmentContract,
} from '@/lib/assessment-contract'
import { getDashboardIiiClient } from '@/lib/iii-client'
import type { PlanExecution } from '@/lib/plan-execution'
import type { RESULTS_SCHEMA_VERSION } from '@/lib/result-contract.generated'
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
  origin?: 'local'
  reference_execution_id?: string
  reference_differences?: string[]
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

export type ImportedPlan = {
  origin: 'remote'
  id: string
  label: string
  purpose: string
  created_at: string | null
  updated_at: string
  template_id: string | null
  source: {
    instance_id: string
    plan_key: string
    captured_at: string
    active: boolean
    limitation: string | null
  }
  configuration: JsonObject | null
  execution_ids: string[]
}

export type Plan = LocalPlan | ImportedPlan

export type LocalPlansResponse = {
  mode: 'unified'
  plans: Plan[]
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
  judge?: DashboardModelIdentity | null
  assessment_summary?: AssessmentSummary
  scenarios: DashboardScenarioSummary[]
}

export type ReleaseControlIdentity = {
  execution_id: string
  attempt: number | null
  profile: string | null
  campaign_id: string | null
  group_id: string | null
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
  /** Set when Release Control dispatched the run; groups the ledger by plan. */
  release_control?: ReleaseControlIdentity | null
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
  quality: EvaluatorAvailability
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
  /** Markdown scenarios only: the judge-scored prompt-following pass, with
   *  the analyzer identity and usage it recorded. */
  instruction_adherence?:
    | (JsonObject & {
        availability: 'available' | 'unavailable' | 'failed'
        score?: number | null
        summary?: string
        requirements?: JsonValue[]
        analyzer?: AnalyzerIdentity
        analyzer_usage?: AnalyzerUsage
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
      technical_failures?: number
      aggregate: DashboardScenarioAggregate
      runs: DashboardRunProjection[]
    }
  >
}

export type DashboardExecutionDetail = DashboardExecutionSummary & {
  origin?: 'local' | 'remote'
  remote_reference?: JsonObject
  retained_runs?: JsonValue[]
  retained_reports?: JsonValue[]
  history_source?: JsonObject
  plan_execution?: PlanExecution
  plan_id?: string
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
    execution_evidence_open: string
    execution_delete: string
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
  openEvidence(input: { execution_id: string; report_id: string; path: string }): Promise<{ availability: string; content_base64?: string; mime_type?: string; reason?: string }>
  deleteExecution(executionId: string): Promise<void>
  listEvaluatedVersions(input?: {
    cohort_id?: string
  }): Promise<EvaluatedVersionsResponse>
  listTests(input?: TestsListInput): Promise<TestsListResponse>
  getTestVersion(input: TestVersionInput): Promise<TestVersionResult>
  getTestHistory(input: TestHistoryInput): Promise<TestHistoryResponse>
  planControl(request: JsonObject): Promise<JsonObject>
  listPlans(): Promise<LocalPlansResponse>
  getPlan(planId: string): Promise<Plan>
  createPlan(request: JsonObject): Promise<LocalPlan>
  updatePlan(planId: string, request: JsonObject): Promise<LocalPlan>
  deletePlan(planId: string): Promise<void>
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
    openEvidence: (input) => call(runtime.functions.execution_evidence_open, input),
    deleteExecution: (executionId) =>
      call(runtime.functions.execution_delete, {
        execution_id: executionId,
      }).then(() => undefined),
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
    createLocalScenario: (request) =>
      call(runtime.functions.local_scenario_create, request),
    getRunSnapshot: (after) =>
      call(runtime.functions.run_status, after === undefined ? {} : { after }),
    startRun: (request) => call(runtime.functions.run_start, request),
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

function normalizeBridgeError(cause: unknown) {
  if (cause instanceof Error) return cause
  if (typeof cause === 'object' && cause !== null && 'message' in cause) {
    return new Error(String(cause.message))
  }
  return new Error(String(cause))
}
