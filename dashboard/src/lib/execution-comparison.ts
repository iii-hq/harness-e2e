import type {
  DashboardExecutionDetail,
  DashboardRunProjection,
  JsonObject,
  StackWorker,
} from '@/lib/dashboard-data-source'
import {
  buildExecutionPresentation,
  executionTitle,
} from '@/lib/execution-view'
import {
  comparisonMetric,
  formatPlanMetricDelta,
  formatPlanMetricValue,
  type MetricFormat,
  type PlanMetricComparison,
} from '@/lib/plan-comparison'

/**
 * Two executions, any two. A is the base only because the reader chose it,
 * and every difference is an observation, never a verdict.
 *
 * Every figure is Release Control's, so the Console and Release Control give
 * the same number for the same pair: runs are projected as its run ledger
 * does (`api/src/lib/run-ledger.ts`, `projectRun`), pair by scenario and then
 * by slot — scenario, case seed and the runner's repetition — and the fair
 * rule and each side's consolidation are those of `api/src/lib/version-compare.ts`
 * (`automaticExclusions`, `compareFairly`, `consolidated`).
 */

/** One run as the fair rule reads it. */
export type CompareRun = {
  scenarioId: string
  slotId: string
  runId: string | null
  /** The case definition the run measured: `case.inputs_sha256`. */
  definition: string | null
  technical: string | null
  completion: string | null
  score: number | null
}

/** A run with Release Control's ledger columns. */
type LedgerRun = CompareRun & {
  run: DashboardRunProjection
  totalTokens: number | null
  turns: number | null
  functionCalls: number | null
  functionCallErrors: number | null
  costUsd: number | null
  wallTimeMs: number | null
  attemptsComplete: boolean
}

/**
 * Why a test left the totals without the reader asking: a gap in the
 * measurement the subject did not cause. A low score, a zero or an
 * incomplete task is the subject's result and never lands here.
 */
export type ExclusionReason =
  | 'missing'
  | 'redefined'
  | 'technical_invalid'
  | 'undetermined'
  | 'no_score'

export type TestExclusion = {
  scenario_id: string
  reason: ExclusionReason
  /** The side or sides whose runs left the gap; both for a changed definition. */
  sides: ('a' | 'b')[]
  /** False when the reader brought the test back into the totals. */
  applied: boolean
}

export type ComparisonChoice = {
  /** Scenarios the reader takes out of the totals. */
  exclude?: Iterable<string>
  /** Automatic exclusions the reader brings back. */
  include?: Iterable<string>
}

/** A figure, and whether a side's value is short of runs (then no difference is given). */
export type ComparedMetric = PlanMetricComparison & {
  partial: { baseline: boolean; candidate: boolean }
}

export type CriterionChange = {
  key: string
  label: string
  possible: number
  /** Mean points over the slots both sides scored. */
  a: number
  b: number
  delta: number
  /** Distinct reasons the evaluator gave on each side. */
  reasons: { a: string[]; b: string[] }
}

export type ScenarioComparison = {
  id: string
  /** Counted in the totals. */
  counted: boolean
  exclusion: TestExclusion | null
  /** Taken out by the reader rather than by the rule. */
  leftOut: boolean
  metrics: ComparedMetric[]
  /** Only the criteria whose points moved. */
  criteria: CriterionChange[]
  /** Anything moved: presence, a run's state, a metric or a criterion. */
  differs: boolean
}

export type ComparisonChange = { field: string; a: string; b: string }

export type ComparisonSide = {
  id: string
  title: string
  /** Where it ran: GitHub run or local, harness version, path workers. */
  origin: string
  subject: string
  /** Null when the execution recorded no parameters. */
  profile: string | null
}

export type ExecutionComparison = {
  a: ComparisonSide
  b: ComparisonSide
  parameters: ComparisonChange[]
  stack: ComparisonChange[]
  /** A side without a recorded stack is not compared worker by worker. */
  stackRecorded: { a: boolean; b: boolean }
  totals: ComparedMetric[]
  scenarios: ScenarioComparison[]
  exclusions: TestExclusion[]
}

type MetricId =
  | 'score'
  | 'completed'
  | 'coverage'
  | 'technical_failures'
  | 'tokens'
  | 'tokens_per_completion'
  | 'cost'
  | 'duration'
  | 'turns'
  | 'function_calls'
  | 'function_errors'

const METRICS: Array<[MetricId, string, MetricFormat]> = [
  ['score', 'Score', 'score'],
  ['completed', 'Completed tasks', 'count'],
  ['coverage', 'Observed / planned runs', 'percent_points'],
  ['technical_failures', 'Technically invalid runs', 'count'],
  ['tokens', 'Tokens (incl. cache)', 'tokens'],
  ['tokens_per_completion', 'Tokens per completed task', 'tokens'],
  ['cost', 'Subject cost', 'usd'],
  ['duration', 'Total run duration', 'seconds'],
  ['turns', 'Total turns', 'count'],
  ['function_calls', 'Function calls', 'count'],
  ['function_errors', 'Function call errors', 'count'],
]

const SIDES = ['a', 'b'] as const
const COMPLETION = ['completed', 'task_incomplete', 'undetermined']
const TECHNICAL = ['valid', 'technical_invalid']

function objectValue(value: unknown): JsonObject {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as JsonObject)
    : {}
}

function isObject(value: unknown): boolean {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value)
}

function text(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value : null
}

function finite(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function integer(value: unknown): number | null {
  const number = finite(value)
  return number === null ? null : Math.trunc(number)
}

function tokenCount(value: unknown): number | null {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0
    ? value
    : null
}

function sumOrNull(values: (number | null)[]): number | null {
  let total = 0
  for (const value of values) {
    if (value === null) return null
    total += value
  }
  return total
}

function distinct<T>(values: (T | null | undefined)[]): T[] {
  const seen: T[] = []
  for (const value of values)
    if (value != null && !seen.includes(value)) seen.push(value)
  return seen
}

function cacheTokens(attempt: JsonObject): number | null {
  const metrics = objectValue(attempt.metrics)
  if (metrics.complete !== true || !isObject(metrics.totals)) return null
  const totals = objectValue(metrics.totals)
  return tokenCount(
    sumOrNull([
      totals.cache_read_tokens === undefined
        ? 0
        : tokenCount(totals.cache_read_tokens),
      tokenCount(totals.cache_write_tokens ?? 0),
    ]),
  )
}

/** The ledger columns of one run, as Release Control's `projectRun` reads them. */
function ledgerColumns(run: DashboardRunProjection) {
  const efficiency = isObject(run.efficiency)
    ? objectValue(run.efficiency)
    : null
  const totals = objectValue(objectValue(run.metrics).totals)
  const retries = (
    Array.isArray(run.retry_attempts) ? run.retry_attempts : []
  ).map(objectValue)
  const status = text(run.status)
  const completion =
    (COMPLETION.includes(String(run.completion))
      ? String(run.completion)
      : null) ??
    (status === null
      ? null
      : status === 'passed'
        ? 'completed'
        : status === 'hard_gate_failed' || status === 'resource_limit'
          ? 'task_incomplete'
          : 'undetermined')
  const technical =
    (TECHNICAL.includes(String(run.technical))
      ? String(run.technical)
      : null) ??
    (status === null
      ? null
      : status === 'subject_error' || status === 'infrastructure_error'
        ? 'technical_invalid'
        : 'valid')
  const rootTurns = finite(efficiency?.root_turns)
  const technicalAttempts = integer(efficiency?.technical_attempts)
  return {
    completion,
    technical,
    score: finite(run.score),
    // Input and output of every attempt, plus each attempt's cache reads and writes.
    totalTokens: tokenCount(
      sumOrNull([
        tokenCount(efficiency?.total_tokens),
        tokenCount(sumOrNull([cacheTokens(run), ...retries.map(cacheTokens)])),
      ]),
    ),
    turns:
      rootTurns !== null
        ? rootTurns + (finite(efficiency?.child_turns) ?? 0)
        : retries.length === 0
          ? finite(totals.turns)
          : null,
    functionCalls: integer(efficiency?.function_calls),
    functionCallErrors:
      integer(efficiency?.function_call_errors) ??
      (retries.length === 0 &&
      (technicalAttempts === null || technicalAttempts <= 1)
        ? integer(totals.function_call_errors)
        : null),
    costUsd: finite(objectValue(run.cost).subject_usd),
    wallTimeMs: finite(run.wall_time_ms),
    // Usage counts once every technical attempt left evidence.
    attemptsComplete:
      efficiency !== null &&
      objectValue(efficiency.unavailable).retry_efficiency === undefined &&
      (technicalAttempts === null || technicalAttempts === retries.length + 1),
  }
}

/**
 * Every retained run, keyed by slot as the runner keys it: scenario, case
 * seed and the repetition inside its native run. Rounds of a plan, like
 * Release Control's campaigns, repeat the same slot.
 */
export function compareRuns(detail: DashboardExecutionDetail): LedgerRun[] {
  const runs: LedgerRun[] = []
  for (const record of detail.reports) {
    if (!record.available || !record.report) continue
    for (const scenario of record.report.scenarios) {
      const scenarioCase = objectValue(scenario.case)
      const seed = scenarioCase.seed == null ? null : String(scenarioCase.seed)
      scenario.runs.forEach((run, repetition) => {
        const columns = ledgerColumns(run)
        runs.push({
          ...columns,
          scenarioId: scenario.scenario_id,
          slotId: JSON.stringify([scenario.scenario_id, seed, repetition]),
          runId: text(run.run_id),
          definition: text(scenarioCase.inputs_sha256),
          run,
        })
      })
    }
  }
  return runs
}

/** A run id claimed twice cannot vouch for either copy. */
export function uniqueRuns<T extends { runId: string | null }>(runs: T[]): T[] {
  const seen = new Map<string, number>()
  for (const run of runs)
    if (run.runId !== null) seen.set(run.runId, (seen.get(run.runId) ?? 0) + 1)
  return runs.filter((run) => run.runId === null || seen.get(run.runId) === 1)
}

/**
 * The same task on both sides, or not. One definition on each side and equal
 * across them is like-for-like; anything else — a change, several within a
 * side, or a null that cannot vouch for sameness — is a redefinition.
 */
function likeForLike(left: CompareRun[], right: CompareRun[]): boolean {
  const a = distinct(left.map((run) => run.definition))
  const b = distinct(right.map((run) => run.definition))
  return a.length === 1 && b.length === 1 && a[0] === b[0]
}

/**
 * The tests a like-for-like comparison cannot hold, and which side caused it.
 *
 * Symmetric by construction: a test that either side cannot measure leaves
 * both totals. The first gap in this order names the reason: no run, a
 * changed definition, an invalid run, an undetermined completion, then a run
 * nothing evaluated.
 */
export function automaticExclusions(
  a: CompareRun[],
  b: CompareRun[],
): Map<string, Omit<TestExclusion, 'applied'>> {
  const runs = { a: uniqueRuns(a), b: uniqueRuns(b) }
  const exclusions = new Map<string, Omit<TestExclusion, 'applied'>>()
  for (const scenarioId of new Set(
    [...runs.a, ...runs.b].map((run) => run.scenarioId),
  )) {
    const of = {
      a: runs.a.filter((run) => run.scenarioId === scenarioId),
      b: runs.b.filter((run) => run.scenarioId === scenarioId),
    }
    const causedBy = (pick: (run: CompareRun) => boolean) =>
      SIDES.filter((which) => of[which].some(pick))
    const exclude = (reason: ExclusionReason, sides: ('a' | 'b')[]) =>
      exclusions.set(scenarioId, { scenario_id: scenarioId, reason, sides })
    const empty = SIDES.filter((which) => of[which].length === 0)
    if (empty.length > 0) {
      exclude('missing', empty)
      continue
    }
    const redefined = [...new Set(of.a.map((run) => run.slotId))].some(
      (slotId) => {
        const left = of.a.filter((run) => run.slotId === slotId)
        const right = of.b.filter((run) => run.slotId === slotId)
        return right.length > 0 && !likeForLike(left, right)
      },
    )
    if (redefined) {
      exclude('redefined', [...SIDES])
      continue
    }
    const gaps: [ExclusionReason, (run: CompareRun) => boolean][] = [
      ['technical_invalid', (run) => run.technical === 'technical_invalid'],
      ['undetermined', (run) => run.completion === 'undetermined'],
      ['no_score', (run) => run.score === null],
    ]
    for (const [reason, pick] of gaps) {
      const sides = causedBy(pick)
      if (sides.length > 0) {
        exclude(reason, sides)
        break
      }
    }
  }
  return exclusions
}

/** Runs a side planned for one scenario: each report's plan, one per report never retained. */
function plannedRuns(detail: DashboardExecutionDetail, scenarioId: string) {
  let planned = 0
  for (const record of detail.reports) {
    if (record.scenario_id !== scenarioId) continue
    if (!record.available || !record.report) {
      planned += 1
      continue
    }
    for (const scenario of record.report.scenarios)
      if (scenario.scenario_id === scenarioId)
        planned +=
          tokenCount(scenario.aggregate?.planned_runs) ?? scenario.runs.length
  }
  return planned
}

type Aggregate = { value: number | null; complete: boolean }

/**
 * One side over some runs, as Release Control consolidates it: a figure sums
 * the runs that reported it (usage only once every attempt left evidence), and
 * is complete when every run reported it and every planned run was observed.
 * The score is the mean of the scored runs, given only when complete.
 */
function consolidate(
  runs: LedgerRun[],
  planned: number | null,
  settled: boolean,
): Record<MetricId, Aggregate> {
  const complete = (measured: number) =>
    measured > 0 &&
    measured === runs.length &&
    planned !== null &&
    runs.length === planned &&
    settled
  const sum = (values: number[]) =>
    values.reduce((total, value) => total + value, 0)
  const aggregate = (
    pick: (run: LedgerRun) => number | null,
    attempts = true,
  ): Aggregate => {
    const values = runs.flatMap((run) => {
      const value = pick(run)
      return value === null || (attempts && !run.attemptsComplete)
        ? []
        : [value]
    })
    return {
      value: values.length === 0 ? null : sum(values),
      complete: complete(values.length),
    }
  }
  if (runs.length === 0)
    return Object.fromEntries(
      METRICS.map(([id]) => [id, { value: null, complete: false }]),
    ) as Record<MetricId, Aggregate>
  const scored = runs.flatMap((run) => (run.score === null ? [] : [run.score]))
  const completed = runs.filter((run) => run.completion === 'completed').length
  const tokens = aggregate((run) => run.totalTokens)
  const errors = runs.filter(
    (run) =>
      run.attemptsComplete &&
      run.functionCalls !== null &&
      run.functionCallErrors !== null,
  )
  const counted = (value: number): Aggregate => ({
    value,
    complete: settled,
  })
  return {
    score: {
      value: complete(scored.length) ? sum(scored) / scored.length : null,
      complete: complete(scored.length),
    },
    completed: counted(completed),
    coverage: {
      value: planned ? (runs.length / planned) * 100 : null,
      complete: planned !== null && settled,
    },
    technical_failures: counted(
      runs.filter((run) => run.technical === 'technical_invalid').length,
    ),
    tokens,
    tokens_per_completion: {
      value:
        tokens.complete && tokens.value !== null && completed > 0
          ? tokens.value / completed
          : null,
      complete: tokens.complete,
    },
    cost: aggregate((run) => run.costUsd),
    duration: (() => {
      const duration = aggregate((run) => run.wallTimeMs, false)
      return {
        ...duration,
        value: duration.value === null ? null : duration.value / 1000,
      }
    })(),
    turns: aggregate((run) => run.turns),
    function_calls: aggregate((run) => run.functionCalls),
    function_errors: {
      value:
        errors.length === 0
          ? null
          : sum(errors.map((run) => run.functionCallErrors ?? 0)),
      complete: complete(errors.length),
    },
  }
}

function metricRows(
  a: Record<MetricId, Aggregate>,
  b: Record<MetricId, Aggregate>,
): ComparedMetric[] {
  return METRICS.map(([id, label, format]) => {
    const metric = comparisonMetric(id, label, a[id].value, b[id].value, format)
    const partial = {
      baseline: a[id].value !== null && !a[id].complete,
      candidate: b[id].value !== null && !b[id].complete,
    }
    // A side short of runs is shown, but no difference is taken from it.
    return partial.baseline || partial.candidate
      ? {
          ...metric,
          delta: null,
          delta_percent: null,
          tone: 'unavailable',
          partial,
        }
      : { ...metric, partial }
  })
}

type Criterion = {
  id: string
  possible: number
  awarded: number | null
  reason: string | null
  label: string
}

function criteriaOf(run: LedgerRun): Criterion[] {
  const criteria = Array.isArray(run.run.criteria) ? run.run.criteria : []
  return criteria.map(objectValue).flatMap((criterion) => {
    const id = text(criterion.id)
    const possible = finite(criterion.possible)
    if (!id || possible === null) return []
    return [
      {
        id,
        possible,
        // Scored only when the run is technically valid and points were awarded.
        awarded: run.technical === 'valid' ? finite(criterion.awarded) : null,
        reason: text(criterion.reason),
        label: text(criterion.description) ?? id,
      },
    ]
  })
}

/** Criteria whose mean points moved over the slots both sides scored. */
function criterionChanges(
  left: LedgerRun[],
  right: LedgerRun[],
): CriterionChange[] {
  const pairs = [...new Set(left.map((run) => run.slotId))].flatMap(
    (slotId) => {
      const other = right.filter((run) => run.slotId === slotId)
      return other.length > 0
        ? [[left.filter((run) => run.slotId === slotId), other] as const]
        : []
    },
  )
  const keyOf = (criterion: Criterion) =>
    `${criterion.id}:${criterion.possible}`
  const known = new Map<string, Criterion>()
  for (const [one, two] of pairs)
    for (const run of [...one, ...two])
      for (const criterion of criteriaOf(run))
        known.set(keyOf(criterion), criterion)
  const changes: CriterionChange[] = []
  for (const [key, criterion] of known) {
    const before: Criterion[] = []
    const after: Criterion[] = []
    for (const [one, two] of pairs) {
      const scored = (runs: readonly LedgerRun[]) =>
        runs.flatMap((run) =>
          criteriaOf(run).filter(
            (entry) => keyOf(entry) === key && entry.awarded !== null,
          ),
        )
      const leftScores = scored(one)
      const rightScores = scored(two)
      if (leftScores.length === 0 || rightScores.length === 0) continue
      before.push(...leftScores)
      after.push(...rightScores)
    }
    const mean = (entries: Criterion[]) =>
      entries.length === 0
        ? null
        : entries.reduce((total, entry) => total + (entry.awarded ?? 0), 0) /
          entries.length
    const a = mean(before)
    const b = mean(after)
    if (a === null || b === null || Math.abs(b - a) < 1e-9) continue
    changes.push({
      key,
      label: criterion.label,
      possible: criterion.possible,
      a,
      b,
      delta: b - a,
      reasons: {
        a: distinct(before.map((entry) => entry.reason)),
        b: distinct(after.map((entry) => entry.reason)),
      },
    })
  }
  return changes.sort((one, two) => one.key.localeCompare(two.key))
}

function stackOf(detail: DashboardExecutionDetail): StackWorker[] {
  const stack = detail.plan_execution?.stack ?? detail.stack
  return Array.isArray(stack) ? stack : []
}

function workerState(worker: StackWorker): string {
  return [
    worker.source,
    worker.observed ?? 'version not observed',
    worker.commit ? `@ ${worker.commit.slice(0, 7)}` : null,
    worker.dirty ? '(uncommitted changes)' : null,
  ]
    .filter(Boolean)
    .join(' ')
}

/** What running the execution again would take: parameters when recorded,
 *  else what the report says. A profile is known only from parameters. */
function parametersOf(detail: DashboardExecutionDetail) {
  const recorded = detail.parameters ?? detail.plan_execution?.parameters
  const subject = detail.subjects[0]
  return {
    scenarios:
      recorded?.scenarios ??
      distinct(detail.reports.map((record) => record.scenario_id)),
    runs:
      recorded?.runs ??
      (typeof detail.requested_runs === 'number'
        ? detail.requested_runs
        : null),
    model: recorded?.model ?? text(subject?.model) ?? 'not reported',
    provider: recorded?.provider ?? text(subject?.provider) ?? 'not reported',
    profile: recorded ? (recorded.agent ?? 'no profile') : null,
  }
}

function sideFacts(detail: DashboardExecutionDetail): ComparisonSide {
  const source = objectValue(detail.plan_execution?.source ?? detail.source)
  const parts: string[] = []
  if (source.kind === 'github') {
    parts.push(`GitHub run ${String(source.run_id ?? '')}`)
    const rc = text(source.release_control_execution_id)
    if (rc) parts.push(`RC ${rc.slice(0, 8)}`)
  } else parts.push('local')
  const stack = stackOf(detail)
  // The application under test and the runner that measured it are different
  // workers; a path checkout of either is described with the others below.
  const harness = stack.find((worker) => worker.name === 'harness')
  if (harness?.observed && harness.source !== 'path')
    parts.push(`harness ${harness.observed}`)
  const runner = stack.find((worker) => worker.name === 'harness-e2e')
  if (runner?.observed) parts.push(`runner ${runner.observed}`)
  const paths = new Map<string, string[]>()
  for (const worker of stack.filter((entry) => entry.source === 'path')) {
    const state = worker.commit
      ? `@ ${worker.commit.slice(0, 7)}${worker.dirty ? ' (uncommitted changes)' : ''}`
      : '(path, commit not recorded)'
    paths.set(state, [...(paths.get(state) ?? []), worker.name])
  }
  for (const [state, names] of paths)
    parts.push(`${distinct(names).join(', ')} ${state}`)
  const parameters = parametersOf(detail)
  return {
    id: detail.id,
    title: executionTitle(buildExecutionPresentation(detail)).title,
    origin: parts.join(' · '),
    subject: `${parameters.provider}/${parameters.model}`,
    profile: parameters.profile,
  }
}

function parameterChanges(
  left: DashboardExecutionDetail,
  right: DashboardExecutionDetail,
): ComparisonChange[] {
  const a = parametersOf(left)
  const b = parametersOf(right)
  const changes: ComparisonChange[] = []
  const onlyA = a.scenarios.filter((id) => !b.scenarios.includes(id))
  const onlyB = b.scenarios.filter((id) => !a.scenarios.includes(id))
  if (onlyA.length > 0 || onlyB.length > 0) {
    const describe = (all: string[], only: string[]) =>
      `${all.length} scenario${all.length === 1 ? '' : 's'}${only.length > 0 ? ` · only here: ${only.join(', ')}` : ''}`
    changes.push({
      field: 'scenarios',
      a: describe(a.scenarios, onlyA),
      b: describe(b.scenarios, onlyB),
    })
  }
  const push = (field: string, one: string, two: string) => {
    if (one !== two) changes.push({ field, a: one, b: two })
  }
  push(
    'runs',
    String(a.runs ?? 'not recorded'),
    String(b.runs ?? 'not recorded'),
  )
  push('model', a.model, b.model)
  push('provider', a.provider, b.provider)
  // An unrecorded profile is unknown, not a difference.
  if (a.profile !== null && b.profile !== null)
    push('profile', a.profile, b.profile)
  return changes
}

function stackChanges(
  left: DashboardExecutionDetail,
  right: DashboardExecutionDetail,
): Pick<ExecutionComparison, 'stack' | 'stackRecorded'> {
  const a = stackOf(left)
  const b = stackOf(right)
  const stackRecorded = { a: a.length > 0, b: b.length > 0 }
  if (!stackRecorded.a || !stackRecorded.b) return { stack: [], stackRecorded }
  const state = (workers: StackWorker[], name: string) =>
    distinct(workers.filter((worker) => worker.name === name).map(workerState))
      .sort()
      .join(' | ') || 'absent'
  const stack = distinct([...a, ...b].map((worker) => worker.name))
    .sort()
    .flatMap((name) => {
      const one = state(a, name)
      const two = state(b, name)
      return one === two ? [] : [{ field: name, a: one, b: two }]
    })
  return { stack, stackRecorded }
}

function settled(detail: DashboardExecutionDetail) {
  return !['running', 'importing', 'cancelling'].includes(String(detail.status))
}

/**
 * The comparison a reader sees first: every automatic exclusion applied, then
 * the reader's own choices — `exclude` takes more tests out, `include` brings
 * an automatic exclusion back. An excluded scenario keeps its row and values.
 * A scenario neither side observed is missing on both.
 */
export function compareExecutions(
  a: DashboardExecutionDetail,
  b: DashboardExecutionDetail,
  choice: ComparisonChoice = {},
): ExecutionComparison {
  const details = { a, b }
  const runs = { a: uniqueRuns(compareRuns(a)), b: uniqueRuns(compareRuns(b)) }
  const ids = distinct([
    ...a.reports.map((record) => record.scenario_id),
    ...b.reports.map((record) => record.scenario_id),
    ...runs.a.map((run) => run.scenarioId),
    ...runs.b.map((run) => run.scenarioId),
  ]).sort()
  const automatic = automaticExclusions(runs.a, runs.b)
  for (const id of ids)
    if (!automatic.has(id) && !runs.a.some((run) => run.scenarioId === id))
      if (!runs.b.some((run) => run.scenarioId === id))
        automatic.set(id, {
          scenario_id: id,
          reason: 'missing',
          sides: ['a', 'b'],
        })
  const include = new Set(choice.include ?? [])
  const exclude = new Set(choice.exclude ?? [])
  const exclusions = [...automatic.values()]
    .map((exclusion) => ({
      ...exclusion,
      applied: !include.has(exclusion.scenario_id),
    }))
    .sort((one, two) => one.scenario_id.localeCompare(two.scenario_id))
  const isCounted = (id: string) =>
    !exclude.has(id) &&
    !exclusions.some((entry) => entry.scenario_id === id && entry.applied)
  // Once any test leaves the totals, what was planned narrows to what the
  // remaining tests observed, as Release Control narrows it.
  const filtered = ids.some((id) => !isCounted(id))
  const measure = (which: 'a' | 'b', scenarioIds: string[]) => {
    const kept = runs[which].filter((run) =>
      scenarioIds.includes(run.scenarioId),
    )
    const planned = filtered
      ? kept.length
      : scenarioIds.reduce(
          (total, id) => total + plannedRuns(details[which], id),
          0,
        )
    return consolidate(kept, planned, filtered || settled(details[which]))
  }
  const stateOf = (which: 'a' | 'b', id: string) =>
    runs[which]
      .filter((run) => run.scenarioId === id)
      .map((run) => `${run.completion}/${run.technical}`)
      .sort()
      .join(',')
  const scenarios = ids.map((id): ScenarioComparison => {
    const exclusion =
      exclusions.find((entry) => entry.scenario_id === id) ?? null
    const metrics = metricRows(measure('a', [id]), measure('b', [id]))
    const criteria = criterionChanges(
      runs.a.filter((run) => run.scenarioId === id),
      runs.b.filter((run) => run.scenarioId === id),
    )
    const present = (which: 'a' | 'b') =>
      details[which].reports.some((record) => record.scenario_id === id)
    return {
      id,
      counted: isCounted(id),
      exclusion,
      leftOut: exclude.has(id),
      metrics,
      criteria,
      differs:
        present('a') !== present('b') ||
        stateOf('a', id) !== stateOf('b', id) ||
        criteria.length > 0 ||
        metrics.some(
          (metric) =>
            metric.baseline !== metric.candidate ||
            metric.partial.baseline !== metric.partial.candidate,
        ),
    }
  })
  const counted = scenarios
    .filter((scenario) => scenario.counted)
    .map((scenario) => scenario.id)
  return {
    a: sideFacts(a),
    b: sideFacts(b),
    parameters: parameterChanges(a, b),
    ...stackChanges(a, b),
    totals: metricRows(measure('a', counted), measure('b', counted)),
    scenarios,
    exclusions,
  }
}

function cell(value: string): string {
  return value.replace(/\|/g, '\\|').replace(/\r?\n|\r/g, ' ')
}

/** A side's figure as text; a partial one says so. */
export function comparedValue(
  metric: ComparedMetric,
  side: 'baseline' | 'candidate',
  unavailable = '—',
) {
  if (metric[side] === null) return unavailable
  const value = formatPlanMetricValue(metric, side)
  return metric.partial[side] ? `${value} (partial)` : value
}

function markdownDelta(metric: ComparedMetric) {
  if (metric.delta === null) return '—'
  const [absolute, relative] = formatPlanMetricDelta(metric).split(' · ')
  return relative ? `${absolute} (${relative})` : absolute
}

function sidesLabel(sides: ('a' | 'b')[]) {
  return sides.map((which) => which.toUpperCase()).join(' and ')
}

/** Out of the totals, and why, in one phrase per scenario. */
export function exclusionPhrase(scenario: ScenarioComparison): string | null {
  if (scenario.leftOut) return 'left out by the reader'
  if (scenario.exclusion?.applied)
    return `${scenario.exclusion.reason} in ${sidesLabel(scenario.exclusion.sides)}`
  return null
}

/** A pull-request-ready summary of the same comparison, without a verdict. */
export function comparisonMarkdown(comparison: ExecutionComparison): string {
  const { a, b } = comparison
  const pair = (one: string, two: string) =>
    one === two ? one : `${one} → ${two}`
  const lines = [
    `### ${cell(
      [
        ...(a.title === b.title ? [a.title] : []),
        pair(a.subject, b.subject),
        ...(a.profile !== null && b.profile !== null
          ? [pair(a.profile, b.profile)]
          : []),
      ].join(' · '),
    )}`,
    '',
    `A (base): ${cell(`${a.title} · ${a.origin}`)}`,
    `B: ${cell(`${b.title} · ${b.origin}`)}`,
  ]
  const changes = [...comparison.parameters, ...comparison.stack]
  if (changes.length > 0)
    lines.push(
      '',
      'What changed:',
      ...changes.map(
        (change) =>
          `- ${cell(change.field)}: ${cell(change.a)} → ${cell(change.b)}`,
      ),
    )
  const reported = comparison.totals.filter(
    (metric) => metric.baseline !== null || metric.candidate !== null,
  )
  if (reported.length > 0)
    lines.push(
      '',
      '| Metric | A | B | Difference |',
      '| --- | --- | --- | --- |',
      ...reported.map(
        (metric) =>
          `| ${metric.label} | ${comparedValue(metric, 'baseline')} | ${comparedValue(metric, 'candidate')} | ${markdownDelta(metric)} |`,
      ),
    )
  const differing = comparison.scenarios.filter((scenario) => scenario.differs)
  if (differing.length > 0)
    lines.push(
      '',
      '| Scenario | Score A → B | Criteria that changed |',
      '| --- | --- | --- |',
      ...differing.map((scenario) => {
        const score = scenario.metrics[0]
        const criteria = scenario.criteria
          .map(
            (criterion) =>
              `${criterion.delta < 0 ? '−' : '+'} ${cell(criterion.label)}`,
          )
          .join('; ')
        return `| ${cell(scenario.id)} | ${comparedValue(score, 'baseline')} → ${comparedValue(score, 'candidate')} | ${criteria || '—'} |`
      }),
    )
  // Only a counted scenario can be said to show no difference.
  const same = comparison.scenarios.filter(
    (scenario) => scenario.counted && !scenario.differs,
  )
  if (same.length > 0)
    lines.push(
      '',
      `No difference: ${same.map((scenario) => cell(scenario.id)).join(', ')}.`,
    )
  const out = comparison.scenarios.flatMap((scenario) => {
    const phrase = exclusionPhrase(scenario)
    return phrase ? [`${cell(scenario.id)} (${phrase})`] : []
  })
  if (out.length > 0) lines.push('', `Out of the totals: ${out.join(', ')}.`)
  const back = comparison.exclusions.filter(
    (exclusion) =>
      !exclusion.applied &&
      comparison.scenarios.some(
        (scenario) => scenario.id === exclusion.scenario_id && scenario.counted,
      ),
  )
  if (back.length > 0)
    lines.push(
      '',
      `Counted despite a gap: ${back.map((exclusion) => `${cell(exclusion.scenario_id)} (${exclusion.reason} in ${sidesLabel(exclusion.sides)})`).join(', ')}.`,
    )
  return `${lines.join('\n')}\n`
}
