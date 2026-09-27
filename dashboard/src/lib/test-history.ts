// A test's history as the redesign canvas draws it (TestHistory.dc.html and
// RunCompare.dc.html): each retained observation is a run of the test, with
// the sessions it spawned, the root session's calls by worker and how each
// criterion scored, read by the worker from the run's native results.

import type { CompletionState } from '@/lib/dashboard-data-source'
import { shortDefinition } from '@/lib/definition-digest'
import {
  formatDuration,
  formatTokens,
  NOT_REPORTED,
  plural,
} from '@/lib/format'
import {
  RESULT_STATES,
  type ResultState,
  runResultState,
} from '@/lib/result-status'
import type {
  TestHistoryResponse,
  TestObservation,
  TestSpec,
} from '@/lib/test-catalog'

/** Asks the worker for the runs of every definition at once. */
export const ALL_DEFINITIONS = 'all'

/** Copies text to the clipboard; false when the browser refused. */
export async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text)
    return true
  } catch {
    return false
  }
}

/** What a copy action says after it ran. */
export function copiedText(what: string, ok: boolean) {
  return ok
    ? `${what} copied`
    : 'Could not copy: the browser refused the clipboard.'
}

export type HistorySession = {
  session_id: string
  parent_session_id: string | null
  depth: number
  /** The agent profile the root spawned the session with. */
  label: string | null
  turns: number
  function_calls: number
  function_call_errors: number
  duration_ms: number | null
}

export type WorkerCalls = { worker: string; calls: number }

export type RunCriterion = {
  id: string
  possible: number
  awarded: number | null
  reason: string
}

export type RunDetails = {
  /** Root first, each session followed by the ones it spawned. */
  sessions: HistorySession[]
  child_sessions: number
  /** The root session's calls, most called worker first. */
  calls_by_worker: WorkerCalls[]
  input_tokens: number | null
  output_tokens: number | null
  cache_read_tokens: number | null
  cache_write_tokens: number | null
  criteria: RunCriterion[]
}

export type HistoryRun = {
  run_id: string
  attempt_id: string
  status: string
  completion: CompletionState
  score: number | null
  duration_seconds: number | null
  turns: number | null
  function_calls: number | null
  function_call_errors: number | null
  /** Absent when the run's native results could not be read. */
  details: RunDetails | null
}

export type HistoryObservation = TestObservation & {
  runs?: HistoryRun[]
  plan_execution_id?: string | null
  execution_label?: string | null
  agent_profile?: string | null
}

export type HistoryResponse = Omit<TestHistoryResponse, 'observations'> & {
  observations: HistoryObservation[]
}

/* ---------------------------------------------------------------- a run */

/** A run of the history: its execution and its case, since an execution
 *  with rotating seeds runs one test on more than one case. */
export function runKey(observation: HistoryObservation) {
  return `${observation.execution_id}:${observation.case_id}`
}

/** The execution a key names; execution ids never hold a colon. */
export function keyExecution(key: string) {
  const colon = key.indexOf(':')
  return colon === -1 ? key : key.slice(0, colon)
}

/** The run a key names. A key with the execution alone (an older link)
 *  names its first case. */
export function findRun(observations: HistoryObservation[], key: string) {
  return (
    observations.find((item) => runKey(item) === key) ??
    (key.includes(':')
      ? undefined
      : observations.find((item) => item.execution_id === key))
  )
}

/** The execution the run is listed under: its plan execution, when any. */
export function listedExecution(observation: HistoryObservation) {
  return observation.plan_execution_id || observation.execution_id
}

export function profileText(observation: HistoryObservation) {
  return observation.agent_profile
    ? `profile ${observation.agent_profile}`
    : 'no profile'
}

export function modelText(observation: HistoryObservation) {
  const { subject_provider: provider, subject_model: model } = observation
  // A model id may already carry its provider (`claude-code/claude-opus-5-5`).
  if (model && provider && !model.startsWith(`${provider}/`))
    return `${provider}/${model}`
  return model || provider || 'unknown model'
}

/** The model without its provider, for a legend. */
export function shortModel(observation: HistoryObservation) {
  return modelText(observation).split('/').at(-1) ?? modelText(observation)
}

/** A run as the canvas's verdict() reads it; one that scored nothing reads
 *  as failed, as the canvas paints it. */
export function runState(run: HistoryRun): ResultState {
  const state = runResultState({
    status: run.status,
    completion: run.completion,
    score: run.score,
  })
  return state === 'lost_points' && run.score === 0 ? 'failed' : state
}

/** The run as the canvas's verdict() reads it: a single run by its own
 *  status, several (or a legacy observation) by their mean score. */
export function observationState(observation: HistoryObservation): ResultState {
  const runs = observation.runs ?? []
  if (runs.length === 1) return runState(runs[0])
  const score = observation.mean_score
  if (typeof score !== 'number') return 'inconclusive'
  return score >= 100 ? 'passed' : 'lost_points'
}

/** `full marks`, `lost 10`, `no points`, or the state when unscored. */
export function scoreStatus(observation: HistoryObservation) {
  const score = observation.mean_score
  if (typeof score !== 'number')
    return RESULT_STATES[observationState(observation)].label.toLowerCase()
  if (score >= 100) return 'full marks'
  if (score <= 0) return 'no points'
  return `lost ${Number((100 - score).toFixed(1))}`
}

export function scoreText(observation: HistoryObservation) {
  const score = observation.mean_score
  return typeof score === 'number' ? String(Math.round(score)) : NOT_REPORTED
}

export type Result = 'all' | 'full' | 'lost' | 'none'

export function resultOf(observation: HistoryObservation): Result {
  const score = observation.mean_score
  if (typeof score !== 'number') return 'none'
  return score >= 100 ? 'full' : 'lost'
}

export type LostCriterion = { id: string; points: number; reason: string }

/** The criteria a run did not get every point of, with the points lost. */
export function lostCriteria(run: HistoryRun): LostCriterion[] {
  return (run.details?.criteria ?? [])
    .filter((criterion) => criterion.awarded !== criterion.possible)
    .map((criterion) => ({
      id: criterion.id,
      points: criterion.possible - (criterion.awarded ?? 0),
      reason: criterion.reason,
    }))
}

/** `runtime_contract −10 · evidence_complete −5` for the row's cell. */
export function lostText(observation: HistoryObservation) {
  return (observation.runs ?? [])
    .flatMap(lostCriteria)
    .map((criterion) => `${criterion.id} −${criterion.points}`)
    .join(' · ')
}

export function childSessions(observation: HistoryObservation) {
  return (observation.runs ?? []).reduce(
    (total, run) => total + (run.details?.child_sessions ?? 0),
    0,
  )
}

/** Under the Sub-agents heading: the tree's shape, or that there was none. */
export function subAgentsNote(details: RunDetails, turns: number | null) {
  const children = details.child_sessions
  if (children === 0) return 'None. The run stayed in one session.'
  const depth = Math.max(...details.sessions.map((session) => session.depth))
  return [
    `1 root session + ${plural(children, 'sub-agent')}, ${plural(depth, 'level')} deep`,
    turns === null ? null : `${turns} turns in all`,
  ]
    .filter(Boolean)
    .join(' · ')
}

/** What a session was: the root, or who spawned it and as which profile. */
export function sessionRole(session: HistorySession, rootId: string | null) {
  if (session.depth === 0 || session.parent_session_id === null)
    return 'root session · the subject'
  return [
    session.label,
    session.parent_session_id === rootId
      ? 'spawned by the root'
      : `spawned by ${session.parent_session_id}`,
  ]
    .filter(Boolean)
    .join(' · ')
}

/** A session id short enough for its row; the root's is a long digest. */
export function sessionName(session: HistorySession) {
  const id = session.session_id
  return id.length > 28 ? `${id.slice(0, 12)}…` : id
}

/* ---------------------------------------------------------- definitions */

export type DefinitionChoice = {
  version: string
  label: string
  runs: number
  current: boolean
}

/** The current definition first, then the most recently run; a definition
 *  that never ran is listed only when it is the current one. */
export function definitionChoices(
  history: Pick<HistoryResponse, 'available_versions' | 'current_version'>,
): DefinitionChoice[] {
  const current = history.current_version ?? null
  return history.available_versions
    .filter((item) => item.execution_count > 0 || item.version === current)
    .sort(
      (left, right) =>
        Number(right.version === current) - Number(left.version === current) ||
        (right.last_seen ?? '').localeCompare(left.last_seen ?? ''),
    )
    .map((item) => ({
      version: item.version,
      label: shortDefinition(item.version) ?? item.version,
      runs: item.observation_count ?? item.run_count,
      current: item.version === current,
    }))
}

/** When the current definition has not run but earlier ones have: what the
 *  runs shown can and cannot say. */
export function staleNotice(
  history: Pick<HistoryResponse, 'available_versions' | 'current_version'>,
) {
  const current = history.current_version
  if (!current) return null
  const choices = definitionChoices(history)
  const earlier = choices.filter((choice) => !choice.current)
  if (choices.some((choice) => choice.current && choice.runs > 0)) return null
  if (earlier.length === 0) return null
  const runs = earlier.reduce((total, choice) => total + choice.runs, 0)
  const where =
    earlier.length === 1
      ? earlier[0].label
      : `${earlier.length} earlier definitions`
  return `The current definition ${shortDefinition(current)} hasn’t run yet. The ${plural(runs, 'run')} below belong to ${where}, so they don’t say how the test behaves now.`
}

/* ---------------------------------------------------------------- chart */

export type ChartMetric = 'score' | 'duration' | 'tokens' | 'turns'

export const CHART_METRICS: Array<{
  id: ChartMetric
  label: string
  note: string
}> = [
  { id: 'score', label: 'Score', note: '' },
  {
    id: 'duration',
    label: 'Duration',
    note: 'Wall time per run, retries included. Lower is better only when the score holds.',
  },
  {
    id: 'tokens',
    label: 'Tokens',
    note: 'Input + output per run; cache is left out. Runs that reported no tokens sit on the axis.',
  },
  { id: 'turns', label: 'Turns', note: 'Model turns per run.' },
]

/** The run's value for a chart metric: duration in milliseconds. A zero
 *  duration, token count or turn count was not reported. */
export function metricValue(
  observation: HistoryObservation,
  metric: ChartMetric,
): number | null {
  const value =
    metric === 'score'
      ? observation.mean_score
      : metric === 'duration'
        ? observation.median_duration_seconds == null
          ? null
          : observation.median_duration_seconds * 1000
        : metric === 'tokens'
          ? observation.median_tokens
          : observation.median_turns
  if (typeof value !== 'number' || !Number.isFinite(value)) return null
  return metric !== 'score' && value === 0 ? null : value
}

export function metricFigure(metric: ChartMetric, value: number) {
  if (metric === 'duration') return formatDuration(value)
  if (metric === 'tokens') return formatTokens(value)
  return String(Math.round(value))
}

/** 1, 1.5, 2, 2.5 … times a power of ten: the axis top over a peak. */
export function niceMax(value: number) {
  if (!(value > 0)) return 1
  const magnitude = 10 ** Math.floor(Math.log10(value))
  return (Math.ceil((value / magnitude) * 2) / 2) * magnitude
}

/** The axis top for a metric: 100 for scores; durations past two minutes
 *  scale in whole minutes. */
export function axisTop(metric: ChartMetric, peak: number) {
  if (metric === 'score') return 100
  if (metric === 'duration' && peak >= 120_000)
    return niceMax(peak / 60_000) * 60_000
  return niceMax(peak)
}

/* ---------------------------------------------------------- aggregates */

function median(values: number[]) {
  if (values.length === 0) return null
  const sorted = [...values].sort((left, right) => left - right)
  const middle = Math.floor(sorted.length / 2)
  return sorted.length % 2
    ? sorted[middle]
    : (sorted[middle - 1] + sorted[middle]) / 2
}

function known(values: Array<number | null | undefined>) {
  return values.filter(
    (value): value is number =>
      typeof value === 'number' && Number.isFinite(value),
  )
}

export type Summary = {
  label: string
  value: string
  sub: string
}

/** The five figures over the runs in scope. */
export function summaryFigures(
  observations: HistoryObservation[],
  /** Every run in scope, when the page holds only the latest of them. */
  total = observations.length,
): Summary[] {
  const partial = total > observations.length
  const scored = known(observations.map((item) => item.mean_score))
  const full = scored.filter((score) => score >= 100).length
  const durations = known(
    observations.map((item) => metricValue(item, 'duration')),
  )
  const tokens = known(
    observations
      .filter((item) => typeof item.mean_score === 'number')
      .map((item) => metricValue(item, 'tokens')),
  )
  const unscored = observations.length - scored.length
  const mean = scored.length
    ? scored.reduce((total, score) => total + score, 0) / scored.length
    : null
  const medianDuration = median(durations)
  const medianTokens = median(tokens)
  return [
    {
      label: 'Mean score',
      value: mean === null ? NOT_REPORTED : String(Math.round(mean)),
      sub: partial
        ? `over the latest ${observations.length} of ${plural(total, 'run')}`
        : `over ${plural(scored.length, 'scored run')}`,
    },
    {
      label: 'Full marks',
      value: `${full}/${scored.length}`,
      sub:
        scored.length - full
          ? `${scored.length - full} lost points`
          : 'every scored run',
    },
    {
      label: 'Runs',
      value: String(total),
      sub: partial
        ? `the latest ${observations.length} summed up here`
        : unscored
          ? `${unscored} without a score`
          : 'all scored',
    },
    {
      label: 'Median duration',
      value: formatDuration(medianDuration),
      sub: 'per run',
    },
    {
      label: 'Median tokens',
      value: formatTokens(medianTokens),
      sub: 'input + output, per scored run',
    },
  ]
}

export type Loss = { id: string; share: number; text: string }

/** The criteria most often lost across the scored runs in scope. */
export function losses(
  observations: HistoryObservation[],
  spec: TestSpec | null,
): Loss[] {
  const scored = observations.filter(
    (item) => typeof item.mean_score === 'number',
  )
  const counts = new Map<string, number>()
  for (const observation of scored) {
    const ids = new Set(
      (observation.runs ?? []).flatMap(lostCriteria).map((item) => item.id),
    )
    for (const id of ids) counts.set(id, (counts.get(id) ?? 0) + 1)
  }
  return [...counts.entries()]
    .sort(
      (left, right) => right[1] - left[1] || left[0].localeCompare(right[0]),
    )
    .slice(0, 6)
    .map(([id, count]) => {
      const weight = spec?.criteria.find((item) => item.id === id)?.weight
      return {
        id,
        share: Math.round((count / scored.length) * 100),
        text: `${count} of ${scored.length}${weight === undefined ? '' : ` · ${weight} pts`}`,
      }
    })
}

/* ------------------------------------------------------------ selection */

/** Ticking a run: the first ticked is A; a third tick is refused. */
export function toggleSelection(selected: string[], id: string): string[] {
  if (selected.includes(id)) return selected.filter((item) => item !== id)
  return selected.length >= 2 ? selected : [...selected, id]
}

/** B minus A, the size and the share, never which side is better. */
export function differenceText(
  a: number | null,
  b: number | null,
  format: (value: number) => string,
  unit: 'points' | 'value' = 'value',
) {
  if (a === null || b === null) return NOT_REPORTED
  const delta = b - a
  if (Math.abs(delta) < 1e-9) return 'no change'
  const sign = delta > 0 ? '+' : '−'
  if (unit === 'points')
    return `${sign}${Number(Math.abs(delta).toFixed(1))} pts`
  const share =
    a === 0
      ? ''
      : ` · ${sign}${Math.round((Math.abs(delta) / Math.abs(a)) * 100)}%`
  return `${sign}${format(Math.abs(delta))}${share}`
}
