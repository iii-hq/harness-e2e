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
import {
  aggregatePrimaryMetrics,
  buildPrimaryMetrics,
  type PrimaryTest,
} from '@/lib/primary-metrics'

/**
 * Two executions, any two. A is the base only because the reader chose it,
 * and every difference is an observation, never a verdict.
 *
 * Runs pair by scenario and then by slot (case × repetition). The fair rule is
 * Release Control's (`api/src/lib/version-compare.ts`, `automaticExclusions`
 * and `compareFairly`): a scenario either side cannot measure leaves the
 * totals of both, so neither side is summed over work the other did not do.
 */

/** One run as the fair rule reads it. */
export type CompareRun = {
  scenarioId: string
  slotId: string
  runId: string | null
  /** The scenario definition the run measured (`behavior_sha256`). */
  definition: string | null
  technical: string | null
  completion: string | null
  score: number | null
}

type ProjectedRun = CompareRun & { run: DashboardRunProjection }

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
  metrics: PlanMetricComparison[]
  /** Only the criteria whose points moved. */
  criteria: CriterionChange[]
  /** Present on one side only, a different score, or a criterion that moved. */
  differs: boolean
}

export type ComparisonChange = { field: string; a: string; b: string }

export type ComparisonSide = {
  id: string
  title: string
  /** Where it ran: GitHub run or local, harness version, path workers. */
  origin: string
  subject: string
  profile: string
}

export type ExecutionComparison = {
  a: ComparisonSide
  b: ComparisonSide
  parameters: ComparisonChange[]
  stack: ComparisonChange[]
  /** A side without a recorded stack is not compared worker by worker. */
  stackRecorded: { a: boolean; b: boolean }
  totals: PlanMetricComparison[]
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
  ['score', 'Mean score', 'score'],
  ['completed', 'Completed', 'count'],
  ['coverage', 'Coverage', 'percent_points'],
  ['technical_failures', 'Technical failures', 'count'],
  ['tokens', 'Tokens', 'tokens'],
  ['tokens_per_completion', 'Tokens per completion', 'tokens'],
  ['cost', 'Cost', 'usd'],
  ['duration', 'Duration', 'seconds'],
  ['turns', 'Turns', 'count'],
  ['function_calls', 'Function calls', 'count'],
  ['function_errors', 'Function errors', 'count'],
]

const SIDES = ['a', 'b'] as const

function objectValue(value: unknown): JsonObject {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as JsonObject)
    : {}
}

function text(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value : null
}

function distinct<T>(values: (T | null | undefined)[]): T[] {
  const seen: T[] = []
  for (const value of values)
    if (value != null && !seen.includes(value)) seen.push(value)
  return seen
}

/** Every retained run, keyed by slot: scenario, case and repetition. */
export function compareRuns(detail: DashboardExecutionDetail): ProjectedRun[] {
  const runs: ProjectedRun[] = []
  for (const record of detail.reports) {
    if (!record.available || !record.report) continue
    const round = typeof record.round === 'number' ? record.round : null
    for (const scenario of record.report.scenarios) {
      const scenarioCase = objectValue(scenario.case)
      const caseId =
        text(scenario.case_id) ??
        text(scenarioCase.case_id) ??
        (scenarioCase.seed == null ? null : String(scenarioCase.seed))
      const definition =
        text(scenario.behavior_sha256) ?? text(scenarioCase.behavior_sha256)
      scenario.runs.forEach((run, index) => {
        // A plan slot is one round of one scenario; a plain run holds its
        // repetitions in order.
        const repetition =
          round === null ? index : (round - 1) * scenario.runs.length + index
        runs.push({
          scenarioId: scenario.scenario_id,
          slotId: JSON.stringify([scenario.scenario_id, caseId, repetition]),
          runId: text(run.run_id),
          definition,
          technical: text(run.technical),
          completion: text(run.completion),
          score: typeof run.score === 'number' ? run.score : null,
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

function mean(values: number[]): number | null {
  return values.length === 0
    ? null
    : values.reduce((total, value) => total + value, 0) / values.length
}

function measures(
  tests: PrimaryTest[],
  runs: CompareRun[],
): Record<MetricId, number | null> {
  if (tests.length === 0 && runs.length === 0)
    return Object.fromEntries(METRICS.map(([id]) => [id, null])) as Record<
      MetricId,
      number | null
    >
  const { metrics } = aggregatePrimaryMetrics(tests)
  const completed = runs.filter((run) => run.completion === 'completed').length
  const planned = tests.reduce((total, test) => total + test.repetitions, 0)
  const executed = tests.reduce((total, test) => total + test.executedRuns, 0)
  const tokens = metrics.totalTokens.value
  const duration = metrics.durationMs.value
  return {
    score: metrics.score.value,
    completed,
    coverage: planned > 0 ? (executed / planned) * 100 : null,
    technical_failures: runs.filter(
      (run) => run.technical === 'technical_invalid',
    ).length,
    tokens,
    tokens_per_completion:
      tokens !== null && completed > 0 ? tokens / completed : null,
    cost: metrics.costUsd.value,
    duration: duration === null ? null : duration / 1000,
    turns: metrics.turns.value,
    function_calls: metrics.functionCalls.value,
    function_errors: metrics.functionErrors.value,
  }
}

function metricRows(
  a: Record<MetricId, number | null>,
  b: Record<MetricId, number | null>,
): PlanMetricComparison[] {
  return METRICS.map(([id, label, format]) =>
    comparisonMetric(id, label, a[id], b[id], format),
  )
}

type Criterion = {
  id: string
  possible: number
  awarded: number | null
  reason: string | null
  label: string
}

function criteriaOf(run: ProjectedRun): Criterion[] {
  const criteria = Array.isArray(run.run.criteria) ? run.run.criteria : []
  return criteria.map(objectValue).flatMap((criterion) => {
    const id = text(criterion.id)
    if (!id || typeof criterion.possible !== 'number') return []
    return [
      {
        id,
        possible: criterion.possible,
        // Only a technically valid run measured anything.
        awarded:
          run.technical === 'valid' && typeof criterion.awarded === 'number'
            ? criterion.awarded
            : null,
        reason: text(criterion.reason),
        label: text(criterion.description) ?? id,
      },
    ]
  })
}

/** Criteria whose mean points moved over the slots both sides scored. */
function criterionChanges(
  left: ProjectedRun[],
  right: ProjectedRun[],
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
      const scored = (runs: readonly ProjectedRun[]) =>
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
    const a = mean(before.map((entry) => entry.awarded as number))
    const b = mean(after.map((entry) => entry.awarded as number))
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
 *  else what the report says. */
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
    profile: recorded
      ? (recorded.agent ?? 'no profile')
      : 'profile not recorded',
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
  const harness = stack.find((worker) => worker.name === 'harness-e2e')
  if (harness?.observed) parts.push(`harness ${harness.observed}`)
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

/**
 * The comparison a reader sees first: every automatic exclusion applied, then
 * the reader's own choices — `exclude` takes more tests out, `include` brings
 * an automatic exclusion back. An excluded scenario keeps its row and values.
 */
export function compareExecutions(
  a: DashboardExecutionDetail,
  b: DashboardExecutionDetail,
  choice: ComparisonChoice = {},
): ExecutionComparison {
  const runs = { a: compareRuns(a), b: compareRuns(b) }
  const tests = {
    a: new Map(buildPrimaryMetrics(a).tests.map((test) => [test.label, test])),
    b: new Map(buildPrimaryMetrics(b).tests.map((test) => [test.label, test])),
  }
  const automatic = automaticExclusions(runs.a, runs.b)
  const include = new Set(choice.include ?? [])
  const exclude = new Set(choice.exclude ?? [])
  const exclusions = [...automatic.values()]
    .map((exclusion) => ({
      ...exclusion,
      applied: !include.has(exclusion.scenario_id),
    }))
    .sort((one, two) => one.scenario_id.localeCompare(two.scenario_id))
  const side = (which: 'a' | 'b', ids: string[]) =>
    measures(
      ids.flatMap((id) => tests[which].get(id) ?? []),
      uniqueRuns(runs[which]).filter((run) => ids.includes(run.scenarioId)),
    )
  const ids = distinct([
    ...tests.a.keys(),
    ...tests.b.keys(),
    ...runs.a.map((run) => run.scenarioId),
    ...runs.b.map((run) => run.scenarioId),
  ]).sort()
  const scenarios = ids.map((id): ScenarioComparison => {
    const exclusion =
      exclusions.find((entry) => entry.scenario_id === id) ?? null
    const metrics = metricRows(side('a', [id]), side('b', [id]))
    const criteria = criterionChanges(
      runs.a.filter((run) => run.scenarioId === id),
      runs.b.filter((run) => run.scenarioId === id),
    )
    const score = metrics[0]
    return {
      id,
      counted: !exclude.has(id) && !exclusion?.applied,
      exclusion,
      leftOut: exclude.has(id),
      metrics,
      criteria,
      differs:
        tests.a.has(id) !== tests.b.has(id) ||
        score.baseline !== score.candidate ||
        criteria.length > 0,
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
    totals: metricRows(side('a', counted), side('b', counted)),
    scenarios,
    exclusions,
  }
}

function cell(value: string): string {
  return value.replace(/\|/g, '\\|').replace(/\n/g, ' ')
}

function markdownValue(
  metric: PlanMetricComparison,
  side: 'baseline' | 'candidate',
) {
  return metric[side] === null ? '—' : formatPlanMetricValue(metric, side)
}

function markdownDelta(metric: PlanMetricComparison) {
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
    `### ${[
      ...(a.title === b.title ? [a.title] : []),
      pair(a.subject, b.subject),
      pair(a.profile, b.profile),
    ].join(' · ')}`,
    '',
    `A (base): ${a.title} · ${a.origin}`,
    `B: ${b.title} · ${b.origin}`,
  ]
  const changes = [...comparison.parameters, ...comparison.stack]
  if (changes.length > 0)
    lines.push(
      '',
      'What changed:',
      ...changes.map(
        (change) => `- ${change.field}: ${change.a} → ${change.b}`,
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
          `| ${metric.label} | ${markdownValue(metric, 'baseline')} | ${markdownValue(metric, 'candidate')} | ${markdownDelta(metric)} |`,
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
        return `| ${cell(scenario.id)} | ${markdownValue(score, 'baseline')} → ${markdownValue(score, 'candidate')} | ${criteria || '—'} |`
      }),
    )
  const same = comparison.scenarios.filter((scenario) => !scenario.differs)
  if (same.length > 0)
    lines.push(
      '',
      `No difference: ${same.map((scenario) => scenario.id).join(', ')}.`,
    )
  const out = comparison.scenarios.flatMap((scenario) => {
    const phrase = exclusionPhrase(scenario)
    return phrase ? [`${scenario.id} (${phrase})`] : []
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
      `Counted despite a gap: ${back.map((exclusion) => `${exclusion.scenario_id} (${exclusion.reason} in ${sidesLabel(exclusion.sides)})`).join(', ')}.`,
    )
  return `${lines.join('\n')}\n`
}
