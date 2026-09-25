import type { CompletionState } from '@/lib/dashboard-data-source'

// The one vocabulary for how a test, a run or an execution stands, as the
// redesign canvas names it (RESULTS / RESULT / verdict()). Every screen
// that shows a result dot and label reads it from here.

export type ResultState =
  | 'passed'
  | 'lost_points'
  | 'incomplete'
  | 'inconclusive'
  | 'failed_gate'
  | 'not_run'
  | 'running'
  | 'waiting'
  | 'queued'
  | 'cancelled'
  | 'infra_error'
  | 'subject_error'
  | 'hit_limit'

/** Host status colors, plus ghost for what has not started or was stopped. */
export type ResultTone = 'ok' | 'warn' | 'alert' | 'accent' | 'ghost'

export type ResultPresentation = {
  label: string
  tone: ResultTone
  /** Still moving: the dot pulses. */
  live: boolean
}

function state(label: string, tone: ResultTone, live = false) {
  return { label, tone, live }
}

export const RESULT_STATES: Record<ResultState, ResultPresentation> = {
  passed: state('Passed', 'ok'),
  lost_points: state('Lost points', 'warn'),
  incomplete: state('Incomplete', 'warn'),
  inconclusive: state('Inconclusive', 'warn'),
  failed_gate: state('Failed a gate', 'alert'),
  not_run: state('Not run', 'alert'),
  running: state('Running', 'accent', true),
  waiting: state('Waiting for a slot', 'ghost'),
  queued: state('Queued', 'ghost'),
  cancelled: state('Cancelled', 'ghost'),
  infra_error: state('Infra error', 'alert'),
  subject_error: state('Subject error', 'alert'),
  hit_limit: state('Hit a limit', 'warn'),
}

export type RunOutcome = {
  /** The run's system status (`passed`, `hard_gate_failed`, …). */
  status: string
  completion?: CompletionState | null
  score?: number | null
}

/** The canvas's verdict() for a finished run. */
export function runResultState({
  status,
  completion,
  score,
}: RunOutcome): ResultState {
  switch (status) {
    case 'passed':
      if (completion === 'task_incomplete') return 'incomplete'
      if (completion === 'undetermined') return 'inconclusive'
      return typeof score === 'number' && score < 100 ? 'lost_points' : 'passed'
    case 'hard_gate_failed':
      return 'failed_gate'
    case 'resource_limit':
      return 'hit_limit'
    case 'subject_error':
      return 'subject_error'
    case 'infrastructure_error':
      return 'infra_error'
    default:
      // `unavailable`, or a status newer than this bundle: nothing says the
      // infrastructure failed, so the result is only undetermined.
      return 'inconclusive'
  }
}
