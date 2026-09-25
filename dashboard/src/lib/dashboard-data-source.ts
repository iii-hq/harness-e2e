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

/** A suite: only what to test. `repository` ones come from the master
 *  plan and are read-only; `local` ones are this Console's. */
export type Suite = {
  id: string
  label: string
  source: 'repository' | 'local'
  purpose: string
  scenarios: string[]
  repetitions: number
  technical_retries: number
  /** Digest of the snapshot this runner materializes it to. */
  sha256: string | null
  updated_at: string | null
}

/** A container a stack declares, with the version or commit it pins. */
export type StackContainer = {
  name: string
  version: string | null
  commit: string | null
}

/** A stack: where a suite runs, an iii Compose project plus `iii` and an
 *  optional `template`. `repository` ones come from `stacks/` and are
 *  read-only; `local` ones are this Console's. */
export type Stack = {
  id: string
  label: string
  source: 'repository' | 'local'
  /** The stack as written, comments included. */
  yaml: string
  iii: string | null
  template: string | null
  containers: StackContainer[]
  /** What may not run as written; never blocking. */
  warnings: string[]
  updated_at: string | null
}

/** The suite an execution ran; `id` is absent for an unnamed suite. The
 *  runner sets `sha256`, the digest of what it materialized; a request never
 *  does. */
export type ExecutionSuite = {
  id?: string | null
  label: string
  sha256?: string
}

/** Where an execution runs: on this harness, in Docker from this worker,
 *  or on GitHub (imported runs). */
export type ExecutionWhere = 'harness' | 'docker' | 'github'

/** The stack an execution ran on in Docker or on GitHub: once imported, the
 *  final `stack.yaml` its contract recorded. The worker sets `sha256`. */
export type ExecutionStack = {
  name: string
  yaml: string
  sha256?: string
}

/** What running an execution again would need. */
export type ExecutionParameters = {
  /** Absent for an execution from before suites. */
  suite?: ExecutionSuite | null
  scenarios: string[]
  runs: number
  technical_retries: number
  model: string
  provider: string
  /** Agent profile the subject ran under. */
  agent: string | null
  /** Absent for an execution from before Docker: this harness. */
  where?: ExecutionWhere
  /** Docker and GitHub only; this harness runs on its own stack. */
  stack?: ExecutionStack | null
}

/** One group of a Docker execution and where it is. */
export type DockerGroup = {
  round: number
  /** Empty until the stack's runner materialized the suite. */
  campaign_id: string
  group_id: string
  scenarios: string[]
  state: 'queued' | 'running' | 'done' | 'failed' | 'cancelled' | 'interrupted'
  attempt: number
  error?: string | null
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
      /** The stack its contract names. */
      stack?: string | null
    }
  | {
      kind: 'docker'
      /** The last root bundle's attempt: 1, then one more per re-run. */
      attempt: number
      phase: 'prepare' | 'groups' | 'finalize' | 'import' | 'done'
      image?: string | null
      groups: DockerGroup[]
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
  subjects: DashboardSubjectSummary[]
  scenario_metrics?: DashboardScenarioMetricSummary[]
  workflow_metrics?: DashboardWorkflowMetricSummary | null
  totals?: ExecutionTotals
  assessment_summary?: AssessmentSummary
  live_progress?: LiveProgress | null
  live_progress_error?: string | null
  persistence_errors?: string[]
  slot_start_deadline_seconds?: number | null
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

/** What one native run reports for one slot, or why it reports nothing. */
export type DashboardReportRecord = JsonObject & {
  subject_id: string
  scenario_id: string
  available: boolean
  report?: DashboardReportProjection
}

export type DashboardExecutionDetail = DashboardExecutionSummary & {
  plan_execution?: PlanExecution
  evidence_error?: string
  reports: DashboardReportRecord[]
  /** The attempts a scenario ran before its current one, per slot: shown,
   *  counted nowhere. */
  previous_reports?: DashboardReportRecord[]
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
    execution_slot_rerun: string
    execution_cancel: string
    run_cancel: string
    suites_list: string
    suite_create: string
    suite_update: string
    suite_delete: string
    stacks_list: string
    stack_create: string
    stack_update: string
    stack_delete: string
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
  /** The master plan's suites, then this Console's. */
  listSuites(): Promise<{ suites: Suite[] }>
  /** A suite of this Console that starts as a copy of `from`. */
  createSuite(from: string, label?: string): Promise<Suite>
  updateSuite(
    suiteId: string,
    changes: Partial<
      Pick<Suite, 'label' | 'scenarios' | 'repetitions' | 'technical_retries'>
    >,
  ): Promise<Suite>
  deleteSuite(suiteId: string): Promise<void>
  /** The repository's stacks, then this Console's. */
  listStacks(): Promise<{ stacks: Stack[] }>
  /** A stack of this Console that starts as a copy of `from`. */
  createStack(from: string, label?: string): Promise<Stack>
  /** Refused only when the YAML does not parse or declares no containers. */
  updateStack(
    stackId: string,
    changes: Partial<Pick<Stack, 'label' | 'yaml'>>,
  ): Promise<Stack>
  deleteStack(stackId: string): Promise<void>
  getCatalog(url?: string): Promise<JsonObject>
  /** Starts an execution on this harness or in Docker; Run tests and Run
   *  again alike. */
  startExecution(request: {
    parameters: ExecutionParameters
    label: string
  }): Promise<{ execution_id: string }>
  /** Runs one scenario of a finished execution again, its group whole: here,
   *  or in Docker as the execution's next attempt. */
  rerunScenario(
    executionId: string,
    scenarioId: string,
  ): Promise<{ execution_id: string }>
  /** Stops an execution: no next scenario is admitted. */
  cancelExecution(executionId: string): Promise<JsonObject>
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
    listSuites: () => call(runtime.functions.suites_list, {}),
    createSuite: (from, label = '') =>
      call(runtime.functions.suite_create, { from, label }),
    updateSuite: (suiteId, changes) =>
      call(runtime.functions.suite_update, { ...changes, suite_id: suiteId }),
    deleteSuite: (suiteId) =>
      call(runtime.functions.suite_delete, { suite_id: suiteId }).then(
        () => undefined,
      ),
    listStacks: () => call(runtime.functions.stacks_list, {}),
    createStack: (from, label = '') =>
      call(runtime.functions.stack_create, { from, label }),
    updateStack: (stackId, changes) =>
      call(runtime.functions.stack_update, { ...changes, stack_id: stackId }),
    deleteStack: (stackId) =>
      call(runtime.functions.stack_delete, { stack_id: stackId }).then(
        () => undefined,
      ),
    getCatalog: (url) =>
      call(runtime.functions.catalog_get, url ? { url } : {}),
    startExecution: (request) =>
      call(runtime.functions.execution_start, request),
    rerunScenario: (executionId, scenarioId) =>
      call(runtime.functions.execution_slot_rerun, {
        execution_id: executionId,
        scenario_id: scenarioId,
      }),
    cancelExecution: (executionId) =>
      call(runtime.functions.execution_cancel, { execution_id: executionId }),
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
