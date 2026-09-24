import type {
  ExecutionParameters,
  ExecutionSource,
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
/** One execution, run here or imported: the origin is `source`. */
export type PlanExecution = {
  id: string
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
  slots: PlanSlot[]
  /** A scenario of this finished execution running again, and the finished
   *  state it returns to if the rerun stops before replacing anything. */
  rerun?: {
    scenarios: string[]
    runs: string[]
    started_at: string
    state: string
    error: string | null
    finished_at: string | null
  } | null
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
