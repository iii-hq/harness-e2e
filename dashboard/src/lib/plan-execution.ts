import type { DashboardDataBridge, JsonObject } from './dashboard-data-source'

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
export type PlanExecution = {
  id: string
  plan_id: string
  role: 'run' | 'baseline' | 'candidate'
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
