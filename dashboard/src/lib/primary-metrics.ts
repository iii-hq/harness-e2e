import type {
  DashboardExecutionDetail,
  DashboardRunProjection,
} from '@/lib/dashboard-data-source'

export type MetricId =
  | 'score'
  | 'totalTokens'
  | 'inputTokens'
  | 'outputTokens'
  | 'cacheRead'
  | 'cacheWrite'
  | 'turns'
  | 'functionCalls'
  | 'functionErrors'
  | 'durationMs'
  | 'costUsd'

export type MetricValue = {
  value: number | null
  observed: number | null
  samples: number
  expected: number
}

export type PrimaryTest = {
  key: string
  label: string
  /** The definition digest the retained runs were evaluated by, or null when
   *  the pooled runs do not agree on one. */
  definition: string | null
  metrics: Record<MetricId, MetricValue>
  repetitions: number
  executedRuns: number
  scopeKnown: boolean
}

export type PrimaryMetrics = {
  tests: PrimaryTest[]
  metrics: Record<MetricId, MetricValue>
}

export type PrimaryMetricsComparison = {
  baseline: PrimaryMetrics
  candidate: PrimaryMetrics
  tests: Array<{
    key: string
    label: string
    definition: string | null
    baseline: PrimaryTest | null
    candidate: PrimaryTest | null
  }>
  excluded: number
  totalTests: number
  deltas: Record<MetricId, number | null>
}

export type PrimaryTestValues = {
  key: string
  label: string
  definition: string | null
  expected: number
  scopeKnown: boolean
  values: Record<MetricId, Array<number | null>>
}

const metricIds: MetricId[] = [
  'score',
  'totalTokens',
  'inputTokens',
  'outputTokens',
  'cacheRead',
  'cacheWrite',
  'turns',
  'functionCalls',
  'functionErrors',
  'durationMs',
  'costUsd',
]

type AttemptValues = Record<Exclude<MetricId, 'score'>, number | null>

type TestAccumulator = {
  key: string
  label: string
  definitions: Set<string>
  expected: number
  scopeKnown: boolean
  values: Record<MetricId, Array<number | null>>
}

export function buildPrimaryMetrics(
  detail: DashboardExecutionDetail,
): PrimaryMetrics {
  const tests = new Map<string, TestAccumulator>()
  const seenRuns = new Set<string>()
  const seenScenarios = new Set<string>()
  const seenUnavailable = new Set<string>()

  for (const [reportIndex, record] of detail.reports.entries()) {
    const subject = string(record.subject_id) ?? ''
    if (!record.available || !record.report) {
      const definition =
        string(field(record, 'behavior_sha256')) ??
        subjectDefinition(detail, subject, record.scenario_id)
      const key = testKey(record.scenario_id)
      const sourceIdentity = [
        field(record, 'native_execution_id'),
        field(record, 'group_id'),
        field(record, 'round'),
      ]
      const source = stable([
        subject,
        key,
        sourceIdentity.some((value) => value !== undefined && value !== null)
          ? sourceIdentity
          : reportIndex,
      ])
      if (seenUnavailable.has(source)) continue
      seenUnavailable.add(source)
      const test = accumulator(tests, key, record.scenario_id, definition)
      test.expected += 1
      continue
    }

    for (const scenario of record.report.scenarios) {
      const definition = string(scenario.behavior_sha256)
      const key = testKey(scenario.scenario_id)
      const runKeys = scenario.runs.map((run) => stable([subject, run.run_id]))
      const source = stable([
        field(record, 'native_execution_id'),
        subject,
        key,
        caseIdentity(scenario, record.report),
        [...runKeys].sort(),
      ])
      if (
        seenScenarios.has(source) ||
        new Set(runKeys).size !== runKeys.length ||
        runKeys.some((runKey) => seenRuns.has(runKey))
      )
        continue
      seenScenarios.add(source)
      for (const runKey of runKeys) seenRuns.add(runKey)

      const test = accumulator(tests, key, scenario.scenario_id, definition)
      const planned = counter(scenario.aggregate?.planned_runs)
      const observed = counter(scenario.aggregate?.observed_runs)
      const deferred = counter(scenario.aggregate?.deferred_runs)
      if (
        planned === null ||
        observed !== scenario.runs.length ||
        deferred === null ||
        planned !== observed + deferred
      ) {
        test.scopeKnown = false
        test.expected += Math.max(planned ?? 0, scenario.runs.length)
      } else {
        test.expected += planned
      }
      for (const run of scenario.runs) {
        test.values.score.push(score(run.score))
        const values = primaryRunValues(run)
        for (const metric of metricIds) {
          if (metric !== 'score') test.values[metric].push(values[metric])
        }
      }
    }
  }

  const projected = [...tests.values()]
    .map(projectTest)
    .sort((left, right) => left.label.localeCompare(right.label))
  return aggregatePrimaryMetrics(projected)
}

export function buildPrimaryMetricsFromValues(
  tests: PrimaryTestValues[],
): PrimaryMetrics {
  return aggregatePrimaryMetrics(
    tests
      .map(projectValues)
      .sort((left, right) => left.label.localeCompare(right.label)),
  )
}

export function excludeUnsuccessfulTests(
  detail: DashboardExecutionDetail,
): DashboardExecutionDetail {
  const excluded = new Set(
    buildPrimaryMetrics(detail)
      .tests.filter((test) => test.metrics.score.value === 0)
      .map((test) => test.label),
  )
  for (const record of detail.reports) {
    for (const scenario of record.report?.scenarios ?? []) {
      if (
        scenario.passed === false ||
        scenario.runs.some(
          (run) =>
            [
              'hard_gate_failed',
              'subject_error',
              'resource_limit',
              'infrastructure_error',
            ].includes(run.status) || run.technical === 'technical_invalid',
        )
      )
        excluded.add(scenario.scenario_id)
    }
  }
  return {
    ...detail,
    reports: detail.reports.flatMap((record) => {
      if (!record.report)
        return excluded.has(record.scenario_id) ? [] : [record]
      const scenarios = record.report.scenarios.filter(
        (scenario) => !excluded.has(scenario.scenario_id),
      )
      return scenarios.length > 0
        ? [{ ...record, report: { ...record.report, scenarios } }]
        : []
    }),
  }
}

export function comparePrimaryMetrics(
  baselineInput: PrimaryMetrics,
  candidateInput: PrimaryMetrics,
  excludeZeroOrMissing: boolean,
): PrimaryMetricsComparison {
  const baselineByKey = new Map(
    baselineInput.tests.map((test) => [test.key, test]),
  )
  const candidateByKey = new Map(
    candidateInput.tests.map((test) => [test.key, test]),
  )
  const keys = [...new Set([...baselineByKey.keys(), ...candidateByKey.keys()])]
  const rows = keys
    .map((key) => {
      const baseline = baselineByKey.get(key) ?? null
      const candidate = candidateByKey.get(key) ?? null
      return {
        key,
        label: baseline?.label ?? candidate?.label ?? key,
        definition: baseline?.definition ?? candidate?.definition ?? null,
        baseline,
        candidate,
      }
    })
    .sort((left, right) => left.label.localeCompare(right.label))
  const selected = rows.filter(({ baseline, candidate }) => {
    if (!excludeZeroOrMissing) return true
    const a = baseline?.metrics.score.value ?? baseline?.metrics.score.observed
    const b =
      candidate?.metrics.score.value ?? candidate?.metrics.score.observed
    return (
      a !== null &&
      a !== undefined &&
      a > 0 &&
      b !== null &&
      b !== undefined &&
      b > 0
    )
  })
  const baseline = aggregatePrimaryMetrics(
    selected.map(
      ({ baseline, candidate }) =>
        baseline ?? missingTest(candidate as PrimaryTest),
    ),
  )
  const candidate = aggregatePrimaryMetrics(
    selected.map(
      ({ baseline, candidate }) =>
        candidate ?? missingTest(baseline as PrimaryTest),
    ),
  )
  return {
    baseline,
    candidate,
    tests: selected,
    excluded: rows.length - selected.length,
    totalTests: rows.length,
    deltas: Object.fromEntries(
      metricIds.map((metric) => [
        metric,
        metricDelta(baseline.metrics[metric], candidate.metrics[metric]),
      ]),
    ) as Record<MetricId, number | null>,
  }
}

export function metricDelta(baseline?: MetricValue, candidate?: MetricValue) {
  const a = baseline?.value ?? baseline?.observed ?? null
  const b = candidate?.value ?? candidate?.observed ?? null
  return a === null || b === null ? null : b - a
}

function missingTest(source: PrimaryTest): PrimaryTest {
  return {
    ...source,
    executedRuns: 0,
    scopeKnown: false,
    metrics: Object.fromEntries(
      metricIds.map((metric) => [
        metric,
        {
          value: null,
          observed: null,
          samples: 0,
          expected: source.metrics[metric].expected,
        },
      ]),
    ) as Record<MetricId, MetricValue>,
  }
}

function accumulator(
  tests: Map<string, TestAccumulator>,
  key: string,
  label: string,
  definition: string | null,
): TestAccumulator {
  const existing = tests.get(key)
  if (existing) {
    if (definition) existing.definitions.add(definition)
    return existing
  }
  const created: TestAccumulator = {
    key,
    label,
    definitions: new Set(definition ? [definition] : []),
    expected: 0,
    scopeKnown: true,
    values: Object.fromEntries(
      metricIds.map((id) => [id, []]),
    ) as unknown as Record<MetricId, Array<number | null>>,
  }
  tests.set(key, created)
  return created
}

function projectTest(test: TestAccumulator): PrimaryTest {
  return projectValues({
    ...test,
    definition: test.definitions.size === 1 ? [...test.definitions][0] : null,
  })
}

function projectValues(test: PrimaryTestValues): PrimaryTest {
  const expected = Math.max(test.expected, test.values.score.length)
  const metrics = Object.fromEntries(
    metricIds.map((metric) => [
      metric,
      metricValue(
        test.values[metric],
        expected,
        metric === 'score',
        test.scopeKnown,
        metric !== 'costUsd',
      ),
    ]),
  ) as Record<MetricId, MetricValue>
  return {
    key: test.key,
    label: test.label,
    definition: test.definition,
    metrics,
    repetitions: expected,
    executedRuns: test.values.score.length,
    scopeKnown: test.scopeKnown,
  }
}

export function aggregatePrimaryMetrics(tests: PrimaryTest[]): PrimaryMetrics {
  const scopeKnown = tests.every((test) => test.scopeKnown)
  const metrics = Object.fromEntries(
    metricIds.map((metric) => {
      if (metric === 'score') {
        const values = tests.map(
          (test) => test.metrics.score.value ?? test.metrics.score.observed,
        )
        return [
          metric,
          metricValue(
            values,
            tests.length,
            true,
            scopeKnown &&
              tests.every((test) => test.metrics.score.value !== null),
            false,
          ),
        ]
      }
      const expected = tests.reduce(
        (sum, test) => sum + test.metrics[metric].expected,
        0,
      )
      const samples = tests.reduce(
        (sum, test) => sum + test.metrics[metric].samples,
        0,
      )
      const observedValues = tests
        .map((test) => test.metrics[metric].observed)
        .filter((value): value is number => value !== null)
      const observed =
        observedValues.length === 0
          ? null
          : sum(observedValues, metric !== 'costUsd')
      return [
        metric,
        {
          value:
            scopeKnown &&
            samples === expected &&
            expected > 0 &&
            observed !== null
              ? observed
              : null,
          observed,
          samples,
          expected,
        },
      ]
    }),
  ) as Record<MetricId, MetricValue>
  return { tests, metrics }
}

function metricValue(
  values: Array<number | null>,
  expected: number,
  average: boolean,
  scopeKnown = true,
  safeInteger = true,
): MetricValue {
  const known = values.filter((value): value is number => value !== null)
  const total = known.length === 0 ? null : sum(known, safeInteger && !average)
  const observed = total === null ? null : total / (average ? known.length : 1)
  return {
    value:
      scopeKnown && known.length === expected && expected > 0 ? observed : null,
    observed,
    samples: observed === null ? 0 : known.length,
    expected,
  }
}

export function primaryRunValues(run: DashboardRunProjection): AttemptValues {
  const attempts: unknown[] = [...(run.retry_attempts ?? []), run]
  const inputTokens = sumAttempts(attempts, 'input_tokens')
  const cacheRead = sumAttempts(attempts, 'cache_read_tokens')
  const cacheWrite = sumAttempts(attempts, 'cache_write_tokens')
  const outputTokens = sumAttempts(attempts, 'output_tokens')
  return {
    totalTokens:
      inputTokens === null || outputTokens === null
        ? null
        : sum([inputTokens, outputTokens], true),
    inputTokens,
    outputTokens,
    cacheRead,
    cacheWrite,
    turns: sumAttempts(attempts, 'turns'),
    functionCalls: sumAttempts(attempts, 'function_calls'),
    functionErrors: sumAttempts(attempts, 'function_call_errors'),
    durationMs: counter(run.wall_time_ms),
    costUsd: nonnegative(run.cost?.total_usd),
  }
}

function sumAttempts(attempts: unknown[], metric: string): number | null {
  const values = attempts.map((attempt) =>
    counter(field(field(field(attempt, 'metrics'), 'totals'), metric)),
  )
  return values.every((value): value is number => value !== null)
    ? sum(values, true)
    : null
}

function sum(values: number[], safeInteger: boolean): number | null {
  const result = values.reduce((total, value) => total + value, 0)
  return Number.isFinite(result) &&
    (!safeInteger || Number.isSafeInteger(result))
    ? result
    : null
}

function caseIdentity(scenario: unknown, report: unknown): string | null {
  const caseId = string(field(scenario, 'case_id'))
  const inputs = string(field(field(scenario, 'case'), 'inputs_sha256'))
  const definition = string(field(field(scenario, 'case'), 'behavior_sha256'))
  const resultContract = string(field(report, 'result_contract_sha256'))
  const executionPolicy = field(scenario, 'execution_policy')
  if (
    !caseId ||
    !inputs ||
    !definition ||
    !resultContract ||
    !isObject(executionPolicy)
  )
    return null
  return stable([caseId, inputs, definition, executionPolicy, resultContract])
}

function subjectDefinition(
  detail: DashboardExecutionDetail,
  subjectId: string,
  scenarioId: string,
): string | null {
  const subject = detail.subjects.find((item) => item.id === subjectId)
  const scenario = subject?.scenarios.find((item) => item.id === scenarioId)
  return string(scenario?.behavior_sha256)
}

function testKey(label: string): string {
  return stable([label])
}

function stable(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stable).join(',')}]`
  if (value && typeof value === 'object') {
    return `{${Object.entries(value as Record<string, unknown>)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, item]) => `${JSON.stringify(key)}:${stable(item)}`)
      .join(',')}}`
  }
  return JSON.stringify(value) ?? 'null'
}

function field(value: unknown, key: string): unknown {
  return isObject(value) ? (value as Record<string, unknown>)[key] : undefined
}

function isObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

function string(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value : null
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
