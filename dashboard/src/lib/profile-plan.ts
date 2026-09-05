import type {
  DashboardDataBridge,
  JsonObject,
  MasterTestProfile,
} from './dashboard-data-source'

export type PlanConfiguration = {
  label: string
  profile_id: string
  url: string
  model: string
  provider: string
  judge_model: string
  judge_provider: string
}
export type ProfileSnapshot = {
  version: number
  profile: MasterTestProfile
  scenario_ids: string[]
  cases: Array<{
    scenario_id: string
    judge_required: boolean
    requirements: string[]
  }>
  budget: MasterTestProfile['budget']
  protected_supervisor_required: boolean
  definition_sha256: string
  profile_sha256: string
}
export type PlanSlot = {
  round: number
  group_id: string
  scenario_id: string
  execution_id: string
  state: string
  observed: number
  completed: number
  passed: number
  technical_valid: number
  result_path: string | null
  error: string | null
}
export type PlanExecutionSummary = {
  id: string
  plan_id: string
  state: string
  role: 'run' | 'baseline' | 'candidate'
  started_at: string
  finished_at: string | null
  planned: number
  finished: number
  observed: number
  completed: number
  passed: number
  technical_valid: number
  baseline_eligible: boolean
  error: string | null
  active_slot: PlanSlot | null
}
export type ProfilePlan = {
  schema_version: 2
  id: string
  created_at: string
  updated_at: string
  locked: boolean
  compatible: boolean
  state: string
  configuration: PlanConfiguration
  snapshot: ProfileSnapshot
  history: PlanExecutionSummary[]
  last_execution: PlanExecutionSummary | null
  baseline_execution_id: string | null
}
export type PlanExecution = {
  id: string
  plan_id: string
  role: PlanExecutionSummary['role']
  state: string
  started_at: string
  finished_at: string | null
  error: string | null
  baseline_eligible: boolean
  slots: PlanSlot[]
  measurements: null | {
    cohorts: Array<{
      cohort_sha256: string
      scenario_id: string
      aggregate: {
        observed_runs: number
        completed_runs: number
        passed_runs: number
        technical_valid_runs: number
      }
      consumption: Record<string, unknown>
    }>
  }
}
export type PlanRequirements = {
  ready: boolean
  checks: Array<{
    id: string
    status: 'ready' | 'blocked' | 'pending'
    message: string
  }>
  active_execution: null | {
    id: string
    kind: 'native' | 'plan'
    plan_id?: string
  }
}
export type PlanAdmission = {
  execution_id?: string
  blocked?: boolean
  requirements?: PlanRequirements
}
export async function profileAction<T>(
  bridge: DashboardDataBridge,
  request: JsonObject,
): Promise<T> {
  if (!bridge.profilePlan)
    throw new Error(
      'Profile plan controls are unavailable. Refresh the dashboard and worker.',
    )
  return (await bridge.profilePlan(request)) as T
}
export function running(state: string) {
  return state === 'running' || state === 'cancelling'
}
export function planErrors(
  config: PlanConfiguration,
  judgeRequired: boolean,
): Record<string, string> {
  const errors: Record<string, string> = {}
  if (!config.label.trim()) errors.label = 'Enter a plan name.'
  if (!config.model || !config.provider)
    errors.model = 'Select an execution model.'
  if (judgeRequired && (!config.judge_model || !config.judge_provider))
    errors.judge = 'This profile requires an evaluator.'
  return errors
}
export function downloadJson(value: unknown, name: string) {
  const url = URL.createObjectURL(
    new Blob([`${JSON.stringify(value, null, 2)}\n`], {
      type: 'application/json',
    }),
  )
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = name
  anchor.click()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}

export type ProfileComparison = {
  comparisons: Array<{
    cohort_sha256: string
    scenario_id: string
    metrics: {
      from: {
        included_runs: number
        consumption: {
          tokens_per_completion: number | null
          total_tokens_consumed: number | null
        }
      }
      to: {
        included_runs: number
        consumption: {
          tokens_per_completion: number | null
          total_tokens_consumed: number | null
        }
      }
      delta: null | {
        consumption: Record<
          string,
          { absolute: number; relative_ratio: number | null }
        >
      }
    }
  }>
  excluded: Array<{ cohort_sha256: string; side: string; reason: string }>
  unavailable?: string
}
