import type {
  DashboardExecutionDetail,
  DashboardRunProjection,
  JsonObject,
  StackWorker,
} from '@/lib/dashboard-data-source'
import { runTotalTokens } from '@/lib/execution-metrics'
import {
  buildExecutionPresentation,
  executionTitle,
  providerModel,
  suiteText,
} from '@/lib/execution-view'
import {
  comparisonMetric,
  formatMetricDelta,
  formatMetricValue,
  type MetricComparison,
  type MetricFormat,
} from '@/lib/metric-comparison'
import { scenarioReruns } from '@/lib/plan-execution'
import { primaryRunValues } from '@/lib/primary-metrics'

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
  status: string | null
  /** The first failure message the run reports. */
  failure: string | null
  /** The scenario definition digest, which moves with the runner. */
  behavior: string | null
  /** Input and output tokens, retries included, as the execution page counts them. */
  subjectTokens: number | null
  cacheRead: number | null
  cacheWrite: number | null
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
export type ComparedMetric = MetricComparison & {
  partial: { baseline: boolean; candidate: boolean }
  /** For figures over every run: how many of those runs are out of the totals. */
  outside?: { baseline: number; candidate: number }
}

/** One side of a scenario: its runs, and what kept it from a result when anything did. */
export type ScenarioSide = {
  runs: number
  /** `no run`, or the status of the first run that is technically invalid, undetermined or unscored. */
  state: string | null
  /** That run's first failure message. */
  failure: string | null
  /** Times the scenario ran again; only its last attempt is compared. */
  reruns: number
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
  sides: { a: ScenarioSide; b: ScenarioSide }
}

export type ComparisonChange = { field: string; a: string; b: string }

/** The stack in groups a reader can take in at once. */
export type StackComparison = {
  /** A side without a recorded stack is not compared worker by worker. */
  recorded: { a: boolean; b: boolean }
  /** Workers run from a local checkout, one entry per side and commit. */
  yourCode: Array<{
    side: 'a' | 'b'
    commit: string | null
    dirty: boolean
    workers: string[]
    /** Workers of this group the other side does not run. */
    onlyHere: string[]
  }>
  /** Workers on both sides, neither from a checkout, whose observed version differs. */
  versions: ComparisonChange[]
  /** Workers on one side only, other than that side's own code. */
  onlyA: string[]
  onlyB: string[]
}

/** The runner that measured each side, and the scenarios whose definition moved with it. */
export type RunnerComparison = {
  a: string | null
  b: string | null
  differs: boolean
  /** Scenarios whose behavior digest differs between A and B. */
  definitionsChanged: string[]
}

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
  stack: StackComparison
  runner: RunnerComparison
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
  | 'cache_read'
  | 'cache_write'
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
  ['tokens', 'Total tokens', 'tokens'],
  ['cache_read', 'Cache read', 'tokens'],
  ['cache_write', 'Cache written', 'tokens'],
  ['tokens_per_completion', 'Tokens per completed task', 'tokens'],
  ['cost', 'Reported cost', 'usd'],
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

function distinct<T>(values: (T | null | undefined)[]): T[] {
  const seen: T[] = []
  for (const value of values)
    if (value != null && !seen.includes(value)) seen.push(value)
  return seen
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
        const usage = primaryRunValues(run)
        runs.push({
          ...columns,
          status: text(run.status),
          failure:
            (Array.isArray(run.failures) ? run.failures : [])
              .map((failure) => text(objectValue(failure).message))
              .find((message) => message !== null) ?? null,
          behavior:
            text(scenario.behavior_sha256) ??
            text(scenarioCase.behavior_sha256),
          subjectTokens: runTotalTokens(run),
          cacheRead: usage.cacheRead,
          cacheWrite: usage.cacheWrite,
          // The reported cost, as the execution page and its evidence table sum it.
          costUsd: usage.costUsd,
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
  // The execution page's definitions: input + output, retries included, and
  // cache reads and writes apart.
  const tokens = aggregate((run) => run.subjectTokens, false)
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
    cache_read: aggregate((run) => run.cacheRead, false),
    cache_write: aggregate((run) => run.cacheWrite, false),
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

/** What running the execution again would take: parameters when recorded,
 *  else what the report says. A suite and a profile are known only from
 *  parameters. */
function parametersOf(detail: DashboardExecutionDetail) {
  const recorded = detail.parameters ?? detail.plan_execution?.parameters
  const subject = detail.subjects[0]
  return {
    suite: suiteText(recorded?.suite),
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
  } else if (source.kind === 'docker')
    parts.push(`Docker attempt ${String(source.attempt ?? 1)}`)
  else parts.push('local')
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
    subject: providerModel(parameters),
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
  // A suite not recorded is unknown, not a difference.
  if (a.suite !== null && b.suite !== null && a.suite !== b.suite)
    changes.push({ field: 'suite', a: a.suite, b: b.suite })
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

const RUNNER = 'harness-e2e'

/**
 * The stack in groups: workers run from a checkout (one line per commit),
 * versions that differ between packaged workers, and workers on one side
 * only. The runner is compared on its own.
 */
function stackComparison(
  left: DashboardExecutionDetail,
  right: DashboardExecutionDetail,
): StackComparison {
  const stacks = { a: stackOf(left), b: stackOf(right) }
  const recorded = { a: stacks.a.length > 0, b: stacks.b.length > 0 }
  const empty = { recorded, yourCode: [], versions: [], onlyA: [], onlyB: [] }
  if (!recorded.a || !recorded.b) return empty
  const yourCode: StackComparison['yourCode'] = []
  for (const side of SIDES)
    for (const worker of stacks[side].filter(
      (entry) => entry.source === 'path',
    )) {
      const commit = worker.commit ? worker.commit.slice(0, 7) : null
      const dirty = worker.dirty === true
      const group = yourCode.find(
        (entry) =>
          entry.side === side &&
          entry.commit === commit &&
          entry.dirty === dirty,
      )
      if (group) group.workers = distinct([...group.workers, worker.name])
      else
        yourCode.push({
          side,
          commit,
          dirty,
          workers: [worker.name],
          onlyHere: [],
        })
    }
  const names = (side: 'a' | 'b') =>
    distinct(stacks[side].map((worker) => worker.name))
  const only = (side: 'a' | 'b') =>
    names(side)
      .filter((name) => !names(side === 'a' ? 'b' : 'a').includes(name))
      .sort()
  // A worker of your code on one side only stays on its your-code line,
  // marked there, and is not listed again.
  for (const group of yourCode)
    group.onlyHere = group.workers.filter((name) =>
      only(group.side).includes(name),
    )
  const ownCode = (side: 'a' | 'b') =>
    yourCode
      .filter((group) => group.side === side)
      .flatMap((group) => group.workers)
  const versions = (side: 'a' | 'b', name: string) =>
    distinct(
      stacks[side]
        .filter((worker) => worker.name === name)
        .map((worker) => worker.observed ?? 'version not observed'),
    )
      .sort()
      .join(' | ')
  const fromCheckout = (name: string) =>
    [...stacks.a, ...stacks.b].some(
      (worker) => worker.name === name && worker.source === 'path',
    )
  return {
    recorded,
    yourCode,
    versions: names('a')
      .filter(
        (name) =>
          name !== RUNNER && names('b').includes(name) && !fromCheckout(name),
      )
      .sort()
      .flatMap((name) => {
        const a = versions('a', name)
        const b = versions('b', name)
        return a === b ? [] : [{ field: name, a, b }]
      }),
    onlyA: only('a').filter((name) => !ownCode('a').includes(name)),
    onlyB: only('b').filter((name) => !ownCode('b').includes(name)),
  }
}

/** The workers of a your-code line, each one-side-only worker marked. */
export function yourCodeWorkers(
  group: StackComparison['yourCode'][number],
): string {
  return group.workers
    .map((name) =>
      group.onlyHere.includes(name)
        ? `${name} (only in ${group.side.toUpperCase()})`
        : name,
    )
    .join(', ')
}

/** How many workers run on this side only, your code included. */
function onlyCount(stack: StackComparison, side: 'a' | 'b'): number {
  return (
    (side === 'a' ? stack.onlyA : stack.onlyB).length +
    stack.yourCode
      .filter((group) => group.side === side)
      .reduce((count, group) => count + group.onlyHere.length, 0)
  )
}

/** "14 workers from your code @852b87e · 2 version differences · 5 only in B". */
export function stackSummary(stack: StackComparison): string {
  const unrecorded = SIDES.filter((side) => !stack.recorded[side])
  if (unrecorded.length > 0)
    return `no stack recorded for ${sidesLabel(unrecorded)}`
  const parts = [
    ...stack.yourCode.map(
      (group) =>
        `${group.workers.length} worker${group.workers.length === 1 ? '' : 's'} from your code${
          stack.yourCode.some((other) => other.side !== group.side)
            ? ` in ${group.side.toUpperCase()}`
            : ''
        } ${group.commit ? `@${group.commit}` : '(commit not recorded)'}${group.dirty ? ' (uncommitted changes)' : ''}`,
    ),
    ...(stack.versions.length > 0
      ? [
          `${stack.versions.length} version difference${stack.versions.length === 1 ? '' : 's'}`,
        ]
      : []),
    ...SIDES.flatMap((side) => {
      const count = onlyCount(stack, side)
      return count > 0 ? [`${count} only in ${side.toUpperCase()}`] : []
    }),
  ]
  return parts.length > 0 ? parts.join(' · ') : 'same stack'
}

function runnerComparison(
  left: DashboardExecutionDetail,
  right: DashboardExecutionDetail,
  runs: { a: LedgerRun[]; b: LedgerRun[] },
  ids: string[],
): RunnerComparison {
  const version = (detail: DashboardExecutionDetail) =>
    distinct(
      stackOf(detail)
        .filter((worker) => worker.name === RUNNER)
        .map((worker) => worker.observed),
    ).join(' | ') || null
  const a = version(left)
  const b = version(right)
  const behaviors = (side: LedgerRun[], id: string) =>
    distinct(
      side.filter((run) => run.scenarioId === id).map((run) => run.behavior),
    )
      .sort()
      .join(',')
  return {
    a,
    b,
    differs: a !== null && b !== null && a !== b,
    definitionsChanged: ids.filter((id) => {
      const one = behaviors(runs.a, id)
      const two = behaviors(runs.b, id)
      return one !== '' && two !== '' && one !== two
    }),
  }
}

/** The runner warning, when the runners or the scenario definitions differ. */
export function runnerWarning(runner: RunnerComparison): string | null {
  const changed = runner.definitionsChanged.join(', ')
  if (runner.differs)
    return `Different runners: ${runner.a} → ${runner.b} — scenario definitions and scoring may differ.${changed ? ` Definitions changed: ${changed}.` : ''}`
  return changed ? `Scenario definitions differ: ${changed}.` : null
}

const FAILURE_LENGTH = 240

/** What kept one side of a scenario from a result, when anything did. */
function scenarioSide(runs: LedgerRun[], reruns: number): ScenarioSide {
  if (runs.length === 0)
    return { runs: 0, state: 'no run', failure: null, reruns }
  const gap =
    runs.find((run) => run.technical === 'technical_invalid') ??
    runs.find((run) => run.completion === 'undetermined') ??
    runs.find((run) => run.score === null)
  const failure = gap?.failure?.split(/\r?\n|\r/)[0]?.trim() ?? null
  return {
    runs: runs.length,
    state: gap ? (gap.status ?? gap.technical ?? gap.completion) : null,
    failure:
      failure && failure.length > FAILURE_LENGTH
        ? `${failure.slice(0, FAILURE_LENGTH - 1)}…`
        : failure || null,
    reruns,
  }
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
      sides: {
        a: scenarioSide(
          runs.a.filter((run) => run.scenarioId === id),
          scenarioReruns(a.plan_execution, id),
        ),
        b: scenarioSide(
          runs.b.filter((run) => run.scenarioId === id),
          scenarioReruns(b.plan_execution, id),
        ),
      },
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
  // Invalid runs and coverage are counted over every run, so leaving a test
  // out of the totals never hides them; the rows say how many are out.
  const outside = (which: 'a' | 'b', pick: (run: LedgerRun) => boolean) =>
    runs[which].filter((run) => !counted.includes(run.scenarioId) && pick(run))
      .length
  const everyRun = (
    id: MetricId,
    value: (which: 'a' | 'b') => number | null,
    pick: (run: LedgerRun) => boolean,
  ) => {
    const [, label, format] = METRICS.find(([metric]) => metric === id) ?? []
    return {
      ...comparisonMetric(
        id,
        label ?? id,
        value('a'),
        value('b'),
        format ?? 'count',
      ),
      partial: { baseline: false, candidate: false },
      outside: { baseline: outside('a', pick), candidate: outside('b', pick) },
    }
  }
  const planned = (which: 'a' | 'b') =>
    ids.reduce((total, id) => total + plannedRuns(details[which], id), 0)
  const totals = metricRows(measure('a', counted), measure('b', counted)).map(
    (metric): ComparedMetric =>
      metric.id === 'technical_failures'
        ? everyRun(
            'technical_failures',
            (which) =>
              runs[which].filter((run) => run.technical === 'technical_invalid')
                .length,
            (run) => run.technical === 'technical_invalid',
          )
        : metric.id === 'coverage'
          ? everyRun(
              'coverage',
              (which) =>
                planned(which) > 0
                  ? (runs[which].length / planned(which)) * 100
                  : null,
              () => true,
            )
          : metric,
  )
  return {
    a: sideFacts(a),
    b: sideFacts(b),
    parameters: parameterChanges(a, b),
    stack: stackComparison(a, b),
    runner: runnerComparison(a, b, runs, ids),
    totals,
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
  const value = formatMetricValue(metric, side)
  const outside = metric.outside?.[side] ?? 0
  return metric.partial[side]
    ? `${value} (partial)`
    : outside > 0
      ? `${value} (${outside} run${outside === 1 ? '' : 's'} out of the totals)`
      : value
}

function markdownDelta(metric: ComparedMetric) {
  if (metric.delta === null) return '—'
  const [absolute, relative] = formatMetricDelta(metric).split(' · ')
  return relative ? `${absolute} (${relative})` : absolute
}

function sidesLabel(sides: ('a' | 'b')[]) {
  return sides.map((which) => which.toUpperCase()).join(' and ')
}

/** Why a gap took the scenario out, in the runs' own words. */
export function gapPhrase(scenario: ScenarioComparison): string | null {
  const exclusion = scenario.exclusion
  if (!exclusion) return null
  if (exclusion.reason === 'missing')
    return `no run in ${sidesLabel(exclusion.sides)}`
  if (exclusion.reason === 'redefined')
    return 'redefined: the case inputs differ between A and B'
  const reason = exclusion.reason === 'no_score' ? 'no score' : exclusion.reason
  return exclusion.sides
    .map((which) => {
      const side = scenario.sides[which]
      return `${reason} in ${which.toUpperCase()}${side.state ? `: ${side.state}` : ''}${side.failure ? ` — ${side.failure}` : ''}`
    })
    .join('; ')
}

/** Out of the totals, and why, in one phrase per scenario. */
export function exclusionPhrase(scenario: ScenarioComparison): string | null {
  if (scenario.leftOut) return 'left out by the reader'
  return scenario.exclusion?.applied ? gapPhrase(scenario) : null
}

/** "rerun ×2 in A, ×1 in B": the sides whose scenario ran again, compared
 *  on its last attempt. */
export function rerunPhrase(scenario: ScenarioComparison): string | null {
  const parts = SIDES.filter((which) => scenario.sides[which].reruns > 0).map(
    (which) => `×${scenario.sides[which].reruns} in ${which.toUpperCase()}`,
  )
  return parts.length > 0 ? `rerun ${parts.join(', ')}` : null
}

/** A scenario's score on one side; a run without one says what it was. */
export function scenarioScore(
  scenario: ScenarioComparison,
  which: 'a' | 'b',
): string {
  const score = scenario.metrics[0]
  const side = which === 'a' ? 'baseline' : 'candidate'
  if (score[side] !== null) return comparedValue(score, side)
  return scenario.sides[which].state ?? '—'
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
  const warning = runnerWarning(comparison.runner)
  if (warning) lines.push('', `> **${cell(warning)}**`)
  if (comparison.parameters.length > 0)
    lines.push(
      '',
      'Parameters that differ:',
      ...comparison.parameters.map(
        (change) =>
          `- ${cell(change.field)}: ${cell(change.a)} → ${cell(change.b)}`,
      ),
    )
  const { stack } = comparison
  lines.push('', `Stack: ${cell(stackSummary(stack))}`)
  for (const group of stack.yourCode)
    lines.push(
      `- Your code in ${group.side.toUpperCase()} ${group.commit ? `@${group.commit}` : '(commit not recorded)'}${group.dirty ? ' (uncommitted changes)' : ''}: ${yourCodeWorkers(group)}`,
    )
  if (stack.versions.length > 0)
    lines.push(
      `- Version differences: ${stack.versions.map((change) => `${change.field} ${change.a} → ${change.b}`).join('; ')}`,
    )
  if (stack.onlyA.length > 0)
    lines.push(`- Only in A: ${stack.onlyA.join(', ')}`)
  if (stack.onlyB.length > 0)
    lines.push(`- Only in B: ${stack.onlyB.join(', ')}`)
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
        const criteria = scenario.criteria
          .map(
            (criterion) =>
              `${criterion.delta < 0 ? '−' : '+'} ${cell(criterion.label)}`,
          )
          .join('; ')
        return `| ${cell(scenario.id)} | ${cell(scenarioScore(scenario, 'a'))} → ${cell(scenarioScore(scenario, 'b'))} | ${criteria || '—'} |`
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
  const reran = comparison.scenarios.flatMap((scenario) => {
    const phrase = rerunPhrase(scenario)
    return phrase ? [`- ${cell(scenario.id)}: ${phrase}`] : []
  })
  if (reran.length > 0)
    lines.push('', 'Run again (only the last attempt is compared):', ...reran)
  const out = comparison.scenarios.flatMap((scenario) => {
    const phrase = exclusionPhrase(scenario)
    return phrase ? [`- ${cell(scenario.id)}: ${cell(phrase)}`] : []
  })
  if (out.length > 0) lines.push('', 'Out of the totals:', ...out)
  const back = comparison.scenarios.filter(
    (scenario) =>
      scenario.counted && scenario.exclusion && !scenario.exclusion.applied,
  )
  if (back.length > 0)
    lines.push(
      '',
      'Counted despite a gap:',
      ...back.map(
        (scenario) =>
          `- ${cell(scenario.id)}: ${cell(gapPhrase(scenario) ?? '')}`,
      ),
    )
  return `${lines.join('\n')}\n`
}
