import type {
  DashboardDataBridge,
  ExecutionParameters,
  ExecutionSource,
  JsonObject,
  StackWorker,
} from './dashboard-data-source'

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
  /** The runs this slot ran before its current one, oldest first. Only the
   *  last attempt counts; these stay visible, outside every total. */
  previous_attempts?: Array<{ execution_id: string; error: string | null }>
}
/** One execution, planned here or imported: the origin is `source`. */
export type PlanExecution = {
  id: string
  plan_id: string | null
  role: 'baseline' | 'candidate' | null
  label: string | null
  parameters: ExecutionParameters | null
  source: ExecutionSource
  stack: StackWorker[]
  /** What could not be recorded about the stack, and scenarios added to
   *  complete a sequential group; shown, never blocking. */
  warnings?: string[]
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
export async function planAction<T>(
  bridge: DashboardDataBridge,
  request: JsonObject,
): Promise<T> {
  return (await bridge.planControl(request)) as T
}
/** How many times a scenario of the execution ran again: its most rerun slot. */
export function scenarioReruns(
  execution: Pick<PlanExecution, 'slots'> | undefined,
  scenarioId: string,
): number {
  return Math.max(
    0,
    ...(execution?.slots ?? [])
      .filter((slot) => slot.scenario_id === scenarioId)
      .map((slot) => slot.previous_attempts?.length ?? 0),
  )
}

/** The scenarios that run again with this one: those sharing its native runs
 *  (a sequential group), in order, itself included. */
export function rerunGroup(
  execution: Pick<PlanExecution, 'slots'>,
  scenarioId: string,
): string[] {
  const runs = new Set(
    execution.slots
      .filter((slot) => slot.scenario_id === scenarioId && slot.execution_id)
      .map((slot) => slot.execution_id),
  )
  return [
    ...new Set(
      execution.slots
        .filter((slot) => runs.has(slot.execution_id))
        .map((slot) => slot.scenario_id),
    ),
  ]
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
