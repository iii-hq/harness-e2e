import type {
  DashboardExecutionDetail,
  DashboardRunProjection,
  DashboardScenarioAggregate,
} from '@/lib/dashboard-data-source'
import { buildScenarioMatrix } from '@/lib/scenario-matrix'

export type UsageCoverage = {
  total: number | null
  observed: number | null
  samples: number
  expected: number
}

export type ExecutionMetrics = {
  scenarios: number
  includedScenarios: number
  scopeComplete: boolean
  partial: boolean
  planned: number
  observed: number
  deferred: number
  completed: number
  incomplete: number
  undetermined: number
  technicalValid: number
  technicalInvalid: number
  completionRate: number | null
  completionCoverage: number | null
  executionReliability: number | null
  qualityMedian: number | null
  qualitySamples: number
  objectiveMedian: number | null
  objectiveSamples: number
  subjectTokens: UsageCoverage
  judgeTokens: UsageCoverage
  failedAttemptTokens: UsageCoverage
  cost: UsageCoverage
  durationMs: UsageCoverage
  functionCalls: UsageCoverage
  functionErrors: UsageCoverage
  completedTokenSamples: number
  tokensCompletedP50: number | null
  tokensPerCompletion: number | null
}

const countFields = [
  'planned_runs',
  'observed_runs',
  'deferred_runs',
  'completed_runs',
  'task_incomplete_runs',
  'undetermined_runs',
  'technical_valid_runs',
  'technical_invalid_runs',
  'objective_scored_runs',
  'quality_scored_completed_runs',
] as const

/** Pool logical runs, never scenario percentages or medians. Read-only Results projection. */
export function buildExecutionMetrics(
  detail: DashboardExecutionDetail,
): ExecutionMetrics {
  const matrix = buildScenarioMatrix(detail)
  const seen = new Set<string>()
  const seenScenarios = new Set<string>()
  const included = matrix.items.filter((item) => {
    if (!item.aggregate || !validCounts(item.aggregate, item.runs)) return false
    const keys = item.runs.map((run) =>
      JSON.stringify([item.subjectId, run.run_id]),
    )
    const scenario =
      item.scenarioIndex === null
        ? undefined
        : detail.reports[item.reportIndex]?.report?.scenarios[
            item.scenarioIndex
          ]
    const scenarioKey = JSON.stringify([
      item.subjectId,
      item.scenarioId,
      item.scenarioVersion,
      scenario?.case_id ?? [...keys].sort(),
    ])
    // Repeated report projections must not double usage or outcomes.
    if (
      seenScenarios.has(scenarioKey) ||
      new Set(keys).size !== keys.length ||
      keys.some((key) => seen.has(key))
    )
      return false
    seenScenarios.add(scenarioKey)
    for (const key of keys) seen.add(key)
    return true
  })
  const scopeComplete =
    included.length > 0 && included.length === matrix.items.length
  const runs = included.flatMap((item) => item.runs)
  const planned = included.reduce(
    (sum, item) => sum + (item.aggregate?.planned_runs ?? 0),
    0,
  )
  const deferred = included.reduce(
    (sum, item) => sum + (item.aggregate?.deferred_runs ?? 0),
    0,
  )
  const completed = runs.filter((run) => run.completion === 'completed')
  const incomplete = runs.filter(
    (run) => run.completion === 'task_incomplete',
  ).length
  const undetermined = runs.filter(
    (run) => run.completion === 'undetermined',
  ).length
  const technicalValid = runs.filter((run) => run.technical === 'valid').length
  const quality = completed.map((run) => score(run.quality_score_completed))
  const objective = runs.map((run) => score(run.objective_score))
  const subjectTokens = coverage(runs.map(tokens), scopeComplete)
  const completedTokens = coverage(completed.map(tokens), scopeComplete)
  const physicalAttempts = runs.flatMap((run) => [
    ...(run.retry_attempts ?? []),
    run,
  ])
  return {
    scenarios: matrix.items.length,
    includedScenarios: included.length,
    scopeComplete,
    partial:
      !scopeComplete ||
      deferred > 0 ||
      matrix.contracts.some((contract) => contract.reportState === 'partial'),
    planned,
    observed: runs.length,
    deferred,
    completed: completed.length,
    incomplete,
    undetermined,
    technicalValid,
    technicalInvalid: runs.length - technicalValid,
    completionRate: ratio(completed.length, completed.length + incomplete),
    completionCoverage: ratio(completed.length + incomplete, planned),
    executionReliability: ratio(technicalValid, planned),
    qualityMedian: median(quality),
    qualitySamples: quality.filter((value) => value !== null).length,
    objectiveMedian: median(objective),
    objectiveSamples: objective.filter((value) => value !== null).length,
    subjectTokens,
    judgeTokens: coverage(
      physicalAttempts.map((attempt) => {
        const input = counter(field(attempt.judge_usage, 'input_tokens'))
        const output = counter(field(attempt.judge_usage, 'output_tokens'))
        return input === null || output === null
          ? null
          : counter(input + output)
      }),
      scopeComplete,
    ),
    failedAttemptTokens: coverage(
      runs.map((run) => {
        // Terminal run efficiency already includes retries. For an incomplete
        // task all consumption is failed-attempt consumption, with no second sum.
        if (run.completion !== 'completed') return tokens(run)
        const retries = (run.retry_attempts ?? []).map(tokens)
        return retries.length === 0 ? 0 : coverage(retries, true).total
      }),
      scopeComplete,
    ),
    cost: coverage(
      runs.map((run) => nonnegative(run.cost?.total_usd)),
      scopeComplete,
    ),
    durationMs: coverage(
      runs.map((run) => counter(run.wall_time_ms)),
      scopeComplete,
    ),
    functionCalls: coverage(
      runs.map((run) => cumulativeCounter(run, 'function_calls')),
      scopeComplete,
    ),
    functionErrors: coverage(
      runs.map((run) => cumulativeCounter(run, 'function_call_errors')),
      scopeComplete,
    ),
    completedTokenSamples: completedTokens.samples,
    tokensCompletedP50:
      completedTokens.total === null ? null : median(completed.map(tokens)),
    tokensPerCompletion:
      subjectTokens.total === null || completed.length === 0
        ? null
        : subjectTokens.total / completed.length,
  }
}

function validCounts(
  aggregate: DashboardScenarioAggregate,
  runs: DashboardRunProjection[],
): boolean {
  if (countFields.some((field) => counter(aggregate[field]) === null))
    return false
  if (
    aggregate.observed_runs !== runs.length ||
    aggregate.planned_runs !== runs.length + aggregate.deferred_runs
  )
    return false
  if (
    runs.some(
      (run) =>
        !run.run_id ||
        !['completed', 'task_incomplete', 'undetermined'].includes(
          run.completion,
        ) ||
        !['valid', 'technical_invalid'].includes(run.technical),
    )
  )
    return false
  return (
    aggregate.completed_runs ===
      runs.filter((run) => run.completion === 'completed').length &&
    aggregate.task_incomplete_runs ===
      runs.filter((run) => run.completion === 'task_incomplete').length &&
    aggregate.undetermined_runs ===
      runs.filter((run) => run.completion === 'undetermined').length &&
    aggregate.technical_valid_runs ===
      runs.filter((run) => run.technical === 'valid').length &&
    aggregate.technical_invalid_runs ===
      runs.filter((run) => run.technical === 'technical_invalid').length
  )
}

function field(value: unknown, key: string): unknown {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)[key]
    : undefined
}

function tokens(run: unknown): number | null {
  return cumulativeCounter(run, 'total_tokens')
}

function cumulativeCounter(run: unknown, name: string): number | null {
  const total = counter(field(field(run, 'efficiency'), name))
  const retries = field(run, 'retry_attempts')
  if (total === null || !Array.isArray(retries) || retries.length === 0)
    return total
  const retryValues = retries.map((retry) =>
    counter(field(field(retry, 'efficiency'), name)),
  )
  const retryTotal = coverage(retryValues, true).total
  // A terminal efficiency can carry a partial sum when retry efficiency was
  // unavailable. Do not present that smaller value as complete consumption.
  return retryTotal !== null && total >= retryTotal ? total : null
}

function nonnegative(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0
    ? value
    : null
}

function counter(value: unknown): number | null {
  return nonnegative(value) !== null && Number.isSafeInteger(value)
    ? (value as number)
    : null
}

function score(value: unknown): number | null {
  const result = nonnegative(value)
  return result !== null && result <= 100 ? result : null
}

function ratio(numerator: number, denominator: number): number | null {
  return denominator > 0 ? numerator / denominator : null
}

function median(values: Array<number | null>): number | null {
  const known = values
    .filter((value): value is number => value !== null)
    .sort((a, b) => a - b)
  if (known.length === 0) return null
  return (
    (known[Math.floor((known.length - 1) / 2)] +
      known[Math.floor(known.length / 2)]) /
    2
  )
}

function coverage(
  values: Array<number | null>,
  scopeComplete: boolean,
): UsageCoverage {
  const known = values.filter((value): value is number => value !== null)
  const sum = known.reduce((sum, value) => sum + value, 0)
  const observed =
    known.length > 0 && sum <= Number.MAX_SAFE_INTEGER ? sum : null
  return {
    total: scopeComplete && known.length === values.length ? observed : null,
    observed,
    samples: known.length,
    expected: values.length,
  }
}
