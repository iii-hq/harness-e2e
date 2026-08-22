import type {
  DashboardExecutionDetail,
  DashboardExecutionSummary,
  DashboardRunProjection,
  DashboardScenarioMetricSummary,
  DashboardScenarioSummary,
  ExecutionTotals,
  JsonObject,
} from '@/lib/dashboard-data-source'
import {
  buildExecutionPresentation,
  failureBreakdown,
} from '@/lib/execution-view'
import {
  generalRunMetrics,
  workflowMetricEntriesFromRecord,
  workflowMetricLabel,
  workflowMetricUnit,
} from '@/lib/workflow-metrics'

export type PlanVerdict = 'improved' | 'stable' | 'regressed' | 'inconclusive'
export type MetricTone = 'positive' | 'negative' | 'neutral' | 'unavailable'
export type MetricDirection = 'higher' | 'lower' | 'context'
export type MetricFormat =
  | 'percent_points'
  | 'score'
  | 'count'
  | 'tokens'
  | 'seconds'
  | 'milliseconds'
  | 'usd'

export type PlanMetricId =
  | 'pass_rate'
  | 'coverage'
  | 'hard_gates'
  | 'technical_failures'
  | 'quality'
  | 'confidence'
  | 'tokens'
  | 'duration'
  | 'cost'
  | 'function_calls'
  | 'function_errors'
  | 'turns'

export type PlanMetricComparison = {
  id: PlanMetricId | `workflow:${string}`
  label: string
  baseline: number | null
  candidate: number | null
  delta: number | null
  delta_percent: number | null
  direction: MetricDirection
  format: MetricFormat
  tone: MetricTone
}

export type PlanScenarioComparison = {
  id: string
  compatible: boolean
  reason: string | null
  baseline_status: string
  candidate_status: string
  metrics: PlanMetricComparison[]
  execution_metrics: PlanMetricComparison[]
  workflow_metrics: PlanMetricComparison[]
}

export type PlanComparison = {
  verdict: PlanVerdict
  headline: string
  detail: string
  baseline: DashboardExecutionSummary | null
  candidate: DashboardExecutionSummary | null
  metrics: PlanMetricComparison[]
  scenarios: PlanScenarioComparison[]
}

export const PLAN_CORE_METRICS: PlanMetricId[] = [
  'pass_rate',
  'quality',
  'tokens',
  'duration',
  'turns',
]

export const PLAN_DETAIL_METRICS: PlanMetricId[] = [
  'pass_rate',
  'coverage',
  'hard_gates',
  'technical_failures',
  'quality',
  'confidence',
  'tokens',
  'duration',
  'cost',
  'function_calls',
  'function_errors',
  'turns',
]

function finite(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function objectValue(value: unknown): JsonObject {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as JsonObject)
    : {}
}

function totals(execution: DashboardExecutionSummary): ExecutionTotals {
  return objectValue(execution.totals) as ExecutionTotals
}

function percentPoints(value: number | null): number | null {
  if (value === null) return null
  return Math.abs(value) <= 1 ? value * 100 : value
}

function scenarioMetricTotal(
  execution: DashboardExecutionSummary,
  key: 'function_calls' | 'function_call_errors' | 'turns',
): number | null {
  const metrics = execution.scenario_metrics ?? []
  if (metrics.length === 0) return null
  let total = 0
  for (const metric of metrics) {
    const explicitAverage = finite(metric.averages?.[key])
    const average = scenarioAverage(metric, key)
    const reportedSamples = finite(metric.samples?.[key])
    const runCount = finite(metric.run_count)
    const samples =
      explicitAverage !== null
        ? reportedSamples
        : average !== null
          ? runCount
          : null
    if (
      average === null ||
      samples === null ||
      runCount === null ||
      samples !== runCount
    ) {
      return null
    }
    total += average * samples
  }
  return total
}

function derivedSecurityMetricTotal(
  execution: DashboardExecutionSummary,
  key: 'function_calls' | 'function_call_errors',
): number | null {
  let total = 0
  let found = false
  for (const metric of execution.scenario_metrics ?? []) {
    if (
      metric.scenario_id !== 'security_review' ||
      finite(metric.averages?.[key]) !== null
    ) {
      continue
    }
    const average = scenarioAverage(metric, key)
    const runCount = finite(metric.run_count)
    if (average === null || runCount === null) continue
    total += average * runCount
    found = true
  }
  return found ? total : null
}

function metricTone(
  baseline: number | null,
  candidate: number | null,
  direction: MetricDirection,
): MetricTone {
  if (baseline === null || candidate === null) return 'unavailable'
  const delta = candidate - baseline
  if (Math.abs(delta) < 1e-9 || direction === 'context') return 'neutral'
  const improved = direction === 'higher' ? delta > 0 : delta < 0
  return improved ? 'positive' : 'negative'
}

function comparisonMetric(
  id: PlanMetricId | `workflow:${string}`,
  label: string,
  baseline: number | null,
  candidate: number | null,
  direction: MetricDirection,
  format: MetricFormat,
): PlanMetricComparison {
  const delta =
    baseline === null || candidate === null ? null : candidate - baseline
  const baselineMagnitude = baseline === null ? null : Math.abs(baseline)
  return {
    id,
    label,
    baseline,
    candidate,
    delta,
    delta_percent:
      delta === null || !baselineMagnitude
        ? null
        : (delta / baselineMagnitude) * 100,
    direction,
    format,
    tone: metricTone(baseline, candidate, direction),
  }
}

export function executionMetricValue(
  execution: DashboardExecutionSummary | null | undefined,
  id: PlanMetricId,
): number | null {
  if (!execution) return null
  const executionTotals = totals(execution)
  const assessment = objectValue(execution.assessment_summary)
  switch (id) {
    case 'pass_rate':
      return percentPoints(finite(executionTotals.scenario_pass_rate))
    case 'coverage':
      return percentPoints(finite(executionTotals.report_coverage))
    case 'hard_gates':
      return finite(executionTotals.hard_gate_failures)
    case 'technical_failures':
      return (
        finite(executionTotals.technical_failures) ??
        finite(executionTotals.infra_failures)
      )
    case 'quality':
      return finite(assessment.median_quality_score)
    case 'confidence':
      return percentPoints(finite(assessment.median_confidence))
    case 'tokens':
      return finite(executionTotals.total_tokens)
    case 'duration':
      return finite(executionTotals.wall_time_seconds)
    case 'cost':
      return finite(executionTotals.total_cost_usd)
    case 'function_calls':
      return addDerivedSecurityMetric(
        finite(executionTotals.function_calls),
        derivedSecurityMetricTotal(execution, 'function_calls'),
        scenarioMetricTotal(execution, 'function_calls'),
      )
    case 'function_errors':
      return addDerivedSecurityMetric(
        finite(executionTotals.function_call_errors),
        derivedSecurityMetricTotal(execution, 'function_call_errors'),
        scenarioMetricTotal(execution, 'function_call_errors'),
      )
    case 'turns':
      return (
        finite(executionTotals.turns) ?? scenarioMetricTotal(execution, 'turns')
      )
  }
}

function addDerivedSecurityMetric(
  reported: number | null,
  derivedSecurity: number | null,
  fullyReportedFallback: number | null,
): number | null {
  if (reported !== null) return reported + (derivedSecurity ?? 0)
  return fullyReportedFallback ?? derivedSecurity
}

function allMetrics(
  baseline: DashboardExecutionSummary,
  candidate: DashboardExecutionSummary,
): PlanMetricComparison[] {
  const value = (id: PlanMetricId) =>
    [
      executionMetricValue(baseline, id),
      executionMetricValue(candidate, id),
    ] as const
  const build = (
    id: PlanMetricId,
    label: string,
    direction: MetricDirection,
    format: MetricFormat,
  ) => {
    const [left, right] = value(id)
    return comparisonMetric(id, label, left, right, direction, format)
  }
  return [
    build('pass_rate', 'Pass rate', 'higher', 'percent_points'),
    build('coverage', 'Coverage', 'higher', 'percent_points'),
    build('hard_gates', 'Hard gates', 'lower', 'count'),
    build('technical_failures', 'Technical failures', 'lower', 'count'),
    build('quality', 'Quality score', 'higher', 'score'),
    build('confidence', 'Confidence', 'context', 'percent_points'),
    build('tokens', 'Total tokens', 'lower', 'tokens'),
    build('duration', 'Duration', 'lower', 'seconds'),
    build('cost', 'Cost', 'lower', 'usd'),
    build('function_calls', 'Function calls', 'context', 'count'),
    build('function_errors', 'Function errors', 'lower', 'count'),
    build('turns', 'Turns', 'lower', 'count'),
  ]
}

function unreliable(execution: DashboardExecutionSummary): boolean {
  const presentation = buildExecutionPresentation(execution)
  const breakdown = presentation.breakdown
  return (
    !presentation.available ||
    executionMetricValue(execution, 'pass_rate') === null ||
    executionMetricValue(execution, 'coverage') === null ||
    [
      'running',
      'cancelling',
      'cancelled',
      'incomplete',
      'unavailable',
    ].includes(presentation.attention) ||
    breakdown.infrastructure > 0 ||
    breakdown.judge > 0 ||
    breakdown.inconclusive > 0 ||
    (executionMetricValue(execution, 'technical_failures') ?? 0) > 0
  )
}

function objectiveVerdict(
  baseline: DashboardExecutionSummary,
  candidate: DashboardExecutionSummary,
): Pick<PlanComparison, 'verdict' | 'headline' | 'detail'> {
  if (unreliable(baseline) || unreliable(candidate)) {
    return {
      verdict: 'inconclusive',
      headline: 'Comparison is inconclusive',
      detail:
        'Infrastructure, evaluator, coverage, or retained evidence prevents a reliable objective comparison.',
    }
  }

  const baselineBreakdown = failureBreakdown(baseline)
  const candidateBreakdown = failureBreakdown(candidate)
  const baselinePassRate = executionMetricValue(baseline, 'pass_rate')
  const candidatePassRate = executionMetricValue(candidate, 'pass_rate')
  const baselineCoverage = executionMetricValue(baseline, 'coverage')
  const candidateCoverage = executionMetricValue(candidate, 'coverage')
  const comparisons = [
    [baselineBreakdown.hard_gate, candidateBreakdown.hard_gate, 'lower'],
    [baselineBreakdown.subject, candidateBreakdown.subject, 'lower'],
    [
      baselineBreakdown.resource_limit,
      candidateBreakdown.resource_limit,
      'lower',
    ],
    [baselinePassRate, candidatePassRate, 'higher'],
    [baselineCoverage, candidateCoverage, 'higher'],
  ] as const
  let improved = false
  let regressed = false
  for (const [left, right, direction] of comparisons) {
    if (left === null || right === null || left === right) continue
    const better = direction === 'higher' ? right > left : right < left
    improved ||= better
    regressed ||= !better
  }
  if (regressed) {
    return {
      verdict: 'regressed',
      headline: 'Objective regression detected',
      detail:
        'The candidate worsened at least one blocking outcome, pass-rate, or coverage signal relative to the baseline.',
    }
  }
  if (improved) {
    return {
      verdict: 'improved',
      headline: 'Objective results improved',
      detail:
        'The candidate improved an objective outcome without worsening another blocking signal.',
    }
  }
  const remainingBlockers =
    candidateBreakdown.hard_gate +
    candidateBreakdown.subject +
    candidateBreakdown.resource_limit
  return {
    verdict: 'stable',
    headline: 'Objective results are stable',
    detail: remainingBlockers
      ? `The candidate matches the baseline, but ${remainingBlockers} blocking ${remainingBlockers === 1 ? 'event remains' : 'events remain'}.`
      : 'Pass rate, coverage, and blocking outcomes match the baseline.',
  }
}

function scenarioMap(detail: DashboardExecutionSummary) {
  const values = new Map<string, DashboardScenarioSummary>()
  for (const subject of detail.subjects ?? []) {
    for (const scenario of subject.scenarios ?? []) {
      if (!values.has(scenario.id)) values.set(scenario.id, scenario)
    }
  }
  return values
}

function scenarioMetricMap(detail: DashboardExecutionSummary) {
  return new Map(
    (detail.scenario_metrics ?? []).map((metric) => [
      metric.scenario_id,
      metric,
    ]),
  )
}

function scenarioStatus(summary: DashboardScenarioSummary | undefined) {
  if (!summary) return 'Not reported'
  if (typeof summary.status === 'string' && summary.status)
    return summary.status
  if (summary.passed === true) return 'passed'
  if (summary.passed === false) return 'failed'
  return 'Not reported'
}

function scenarioAverage(
  metric: DashboardScenarioMetricSummary | undefined,
  key:
    | 'tokens'
    | 'duration_seconds'
    | 'cost_usd'
    | 'function_calls'
    | 'function_call_errors'
    | 'turns',
) {
  const explicit = finite(metric?.averages?.[key])
  if (explicit !== null || metric?.scenario_id !== 'security_review') {
    return explicit
  }

  const runCount = finite(metric.run_count)
  if (!runCount) return null
  if (key === 'function_call_errors') {
    const failures = finite(metric.workflow?.failure_count)
    return failures === null ? null : failures / runCount
  }
  if (key !== 'function_calls') return null

  const workflow = metric.workflow?.numeric_metrics
  const requests = finite(workflow?.request_count)
  const polls = finite(workflow?.['poll.poll_count'])
  const reconciliation = finite(workflow?.reconciliation_operations)
  if (requests === null || polls === null || reconciliation === null) {
    return null
  }

  // Security Review v3 persists operation counts instead of canonical Harness
  // usage totals. Include its scan and history entrypoints once per run so a
  // retained summary remains comparable before full execution detail is loaded.
  return (requests + polls + reconciliation + 2 * runCount) / runCount
}

function primaryScenarioRun(
  execution: DashboardExecutionSummary,
  scenarioId: string,
): DashboardRunProjection | null {
  const detail = execution as DashboardExecutionDetail
  for (const record of detail.reports ?? []) {
    for (const scenario of record.report?.scenarios ?? []) {
      if (scenario.scenario_id === scenarioId)
        return scenario.runs.at(-1) ?? null
    }
  }
  return null
}

function runDurationSeconds(run: DashboardRunProjection | null): number | null {
  const milliseconds = finite(
    run?.wall_time_ms ?? run?.efficiency?.wall_time_ms,
  )
  return milliseconds === null ? null : milliseconds / 1000
}

function runTurns(run: DashboardRunProjection | null): number | null {
  return finite(run?.metrics?.totals?.turns ?? run?.efficiency?.turns)
}

function generalMetricComparisons(
  baseline: DashboardRunProjection | null,
  candidate: DashboardRunProjection | null,
  baselineSummary: DashboardScenarioMetricSummary | undefined,
  candidateSummary: DashboardScenarioMetricSummary | undefined,
  compatible: boolean,
): PlanMetricComparison[] {
  const left = generalRunMetrics(baseline, baseline?.semantic_tests ?? [])
  const right = generalRunMetrics(candidate, candidate?.semantic_tests ?? [])
  const metric = (
    id: PlanMetricId,
    label: string,
    leftValue: number | null,
    rightValue: number | null,
    direction: MetricDirection,
    format: MetricFormat,
  ) =>
    comparisonMetric(
      id,
      label,
      compatible ? leftValue : null,
      compatible ? rightValue : null,
      direction,
      format,
    )
  return [
    metric(
      'cost',
      'Cost',
      left.costUsd ?? scenarioAverage(baselineSummary, 'cost_usd'),
      right.costUsd ?? scenarioAverage(candidateSummary, 'cost_usd'),
      'lower',
      'usd',
    ),
    metric(
      'tokens',
      'Tokens',
      left.totalTokens ?? scenarioAverage(baselineSummary, 'tokens'),
      right.totalTokens ?? scenarioAverage(candidateSummary, 'tokens'),
      'lower',
      'tokens',
    ),
    metric(
      'function_calls',
      'Function calls',
      left.functionCalls ?? scenarioAverage(baselineSummary, 'function_calls'),
      right.functionCalls ??
        scenarioAverage(candidateSummary, 'function_calls'),
      'context',
      'count',
    ),
    metric(
      'function_errors',
      'Function errors',
      left.functionCallErrors ??
        scenarioAverage(baselineSummary, 'function_call_errors'),
      right.functionCallErrors ??
        scenarioAverage(candidateSummary, 'function_call_errors'),
      'lower',
      'count',
    ),
    metric(
      'duration',
      'Time',
      scenarioAverage(baselineSummary, 'duration_seconds') ??
        runDurationSeconds(baseline),
      scenarioAverage(candidateSummary, 'duration_seconds') ??
        runDurationSeconds(candidate),
      'lower',
      'seconds',
    ),
  ]
}

function workflowMetricComparisons(
  baseline: DashboardScenarioMetricSummary | undefined,
  candidate: DashboardScenarioMetricSummary | undefined,
  compatible: boolean,
): PlanMetricComparison[] {
  const left = new Map(
    workflowMetricEntriesFromRecord(baseline?.workflow?.numeric_metrics),
  )
  const right = new Map(
    workflowMetricEntriesFromRecord(candidate?.workflow?.numeric_metrics),
  )
  return [...new Set([...left.keys(), ...right.keys()])]
    .sort()
    .map((path) =>
      comparisonMetric(
        `workflow:${path}`,
        workflowMetricLabel(path),
        compatible ? (left.get(path) ?? null) : null,
        compatible ? (right.get(path) ?? null) : null,
        'context',
        workflowMetricUnit(path),
      ),
    )
}

export function buildScenarioComparisons(
  baseline: DashboardExecutionSummary,
  candidate: DashboardExecutionSummary,
): PlanScenarioComparison[] {
  const baselineScenarios = scenarioMap(baseline)
  const candidateScenarios = scenarioMap(candidate)
  const baselineMetrics = scenarioMetricMap(baseline)
  const candidateMetrics = scenarioMetricMap(candidate)
  const ids = new Set([
    ...baselineScenarios.keys(),
    ...candidateScenarios.keys(),
    ...baselineMetrics.keys(),
    ...candidateMetrics.keys(),
  ])
  return [...ids].sort().map((id) => {
    const left = baselineScenarios.get(id)
    const right = candidateScenarios.get(id)
    const leftMetrics = baselineMetrics.get(id)
    const rightMetrics = candidateMetrics.get(id)
    const leftRun = primaryScenarioRun(baseline, id)
    const rightRun = primaryScenarioRun(candidate, id)
    const leftGeneral = generalRunMetrics(
      leftRun,
      leftRun?.semantic_tests ?? [],
    )
    const rightGeneral = generalRunMetrics(
      rightRun,
      rightRun?.semantic_tests ?? [],
    )
    const versionMismatch =
      left?.scenario_version != null &&
      right?.scenario_version != null &&
      left.scenario_version !== right.scenario_version
    const caseMismatch =
      Boolean(left?.case_id && right?.case_id) &&
      left?.case_id !== right?.case_id
    const contractMismatch =
      Boolean(
        leftMetrics?.contract_fingerprint && rightMetrics?.contract_fingerprint,
      ) &&
      leftMetrics?.contract_fingerprint !== rightMetrics?.contract_fingerprint
    const sideMissing = !left || !right
    const compatible =
      !sideMissing && !versionMismatch && !caseMismatch && !contractMismatch
    const metric = (
      metricId: PlanMetricId,
      label: string,
      baselineValue: number | null,
      candidateValue: number | null,
      direction: MetricDirection,
      format: MetricFormat,
    ) =>
      comparisonMetric(
        metricId,
        label,
        compatible ? baselineValue : null,
        compatible ? candidateValue : null,
        direction,
        format,
      )
    return {
      id,
      compatible,
      reason: sideMissing
        ? 'One execution does not contain this test.'
        : versionMismatch || caseMismatch || contractMismatch
          ? 'The retained scenario contract differs between executions.'
          : null,
      baseline_status: scenarioStatus(left),
      candidate_status: scenarioStatus(right),
      metrics: [
        metric(
          'pass_rate',
          'Pass rate',
          percentPoints(finite(left?.pass_rate)),
          percentPoints(finite(right?.pass_rate)),
          'higher',
          'percent_points',
        ),
        metric(
          'quality',
          'Quality score',
          finite(left?.assessment_summary?.median_quality_score),
          finite(right?.assessment_summary?.median_quality_score),
          'higher',
          'score',
        ),
        metric(
          'tokens',
          'Tokens',
          scenarioAverage(leftMetrics, 'tokens') ?? leftGeneral.totalTokens,
          scenarioAverage(rightMetrics, 'tokens') ?? rightGeneral.totalTokens,
          'lower',
          'tokens',
        ),
        metric(
          'duration',
          'Duration',
          scenarioAverage(leftMetrics, 'duration_seconds'),
          scenarioAverage(rightMetrics, 'duration_seconds'),
          'lower',
          'seconds',
        ),
        metric(
          'cost',
          'Cost',
          scenarioAverage(leftMetrics, 'cost_usd') ?? leftGeneral.costUsd,
          scenarioAverage(rightMetrics, 'cost_usd') ?? rightGeneral.costUsd,
          'lower',
          'usd',
        ),
        metric(
          'turns',
          'Turns',
          scenarioAverage(leftMetrics, 'turns') ?? runTurns(leftRun),
          scenarioAverage(rightMetrics, 'turns') ?? runTurns(rightRun),
          'lower',
          'count',
        ),
      ],
      execution_metrics: generalMetricComparisons(
        leftRun,
        rightRun,
        leftMetrics,
        rightMetrics,
        compatible,
      ),
      workflow_metrics: workflowMetricComparisons(
        leftMetrics,
        rightMetrics,
        compatible,
      ),
    }
  })
}

export function buildPlanComparison(
  baseline: DashboardExecutionSummary | null | undefined,
  candidate: DashboardExecutionSummary | null | undefined,
  details?: {
    baseline: DashboardExecutionDetail
    candidate: DashboardExecutionDetail
  },
): PlanComparison {
  if (!baseline || !candidate) {
    return {
      verdict: 'inconclusive',
      headline: 'Comparison is unavailable',
      detail:
        'Both a retained baseline and a completed candidate are required before deltas can be calculated.',
      baseline: baseline ?? null,
      candidate: candidate ?? null,
      metrics: [],
      scenarios: [],
    }
  }
  return {
    ...objectiveVerdict(baseline, candidate),
    baseline,
    candidate,
    metrics: allMetrics(baseline, candidate),
    scenarios: buildScenarioComparisons(
      details?.baseline ?? baseline,
      details?.candidate ?? candidate,
    ),
  }
}

export function metricById(
  comparison: PlanComparison,
  id: PlanMetricId,
): PlanMetricComparison | null {
  return comparison.metrics.find((metric) => metric.id === id) ?? null
}

function compactNumber(value: number) {
  return new Intl.NumberFormat('en-US', {
    notation: Math.abs(value) >= 1000 ? 'compact' : 'standard',
    maximumFractionDigits: Math.abs(value) >= 100 ? 0 : 1,
  }).format(value)
}

function signed(value: number, formatted: string) {
  if (formatted.startsWith('-') || formatted.startsWith('+')) return formatted
  return `${value > 0 ? '+' : value < 0 ? '-' : ''}${formatted}`
}

export function formatPlanMetricValue(
  metric: PlanMetricComparison,
  side: 'baseline' | 'candidate',
): string {
  const value = metric[side]
  if (value === null) return 'Not reported'
  switch (metric.format) {
    case 'percent_points':
      return `${compactNumber(value)}%`
    case 'score':
      return compactNumber(value)
    case 'tokens':
      return compactNumber(value)
    case 'seconds':
      return value < 60
        ? `${value.toFixed(value < 10 ? 1 : 0)}s`
        : `${Math.floor(value / 60)}m ${Math.round(value % 60)}s`
    case 'milliseconds':
      return value < 1_000
        ? `${compactNumber(value)} ms`
        : `${(value / 1_000).toFixed(1)}s`
    case 'usd':
      return `$${value.toFixed(value < 1 ? 4 : 2)}`
    case 'count':
      return compactNumber(value)
  }
}

export function formatPlanMetricDelta(metric: PlanMetricComparison): string {
  if (metric.delta === null) return 'Not comparable'
  const value = metric.delta
  if (Math.abs(value) < 1e-9) return 'No change'
  if (metric.format === 'percent_points') {
    return `${signed(value, compactNumber(value))} pp`
  }
  if (metric.format === 'score') {
    return `${signed(value, compactNumber(value))} pts`
  }
  const absolute = (() => {
    if (metric.format === 'seconds')
      return signed(value, `${Math.abs(value).toFixed(1)}s`)
    if (metric.format === 'milliseconds')
      return signed(value, `${compactNumber(Math.abs(value))} ms`)
    if (metric.format === 'usd')
      return signed(value, `$${Math.abs(value).toFixed(4)}`)
    return signed(value, compactNumber(Math.abs(value)))
  })()
  const relative = metric.delta_percent
  return relative === null
    ? absolute
    : `${absolute} · ${signed(relative, `${Math.abs(relative).toFixed(1)}%`)}`
}

export async function loadExecutionSummaries(
  listExecutions: (input: {
    ids: string[]
    limit: number
  }) => Promise<{ executions: DashboardExecutionSummary[] }>,
  executionIds: string[],
): Promise<Record<string, DashboardExecutionSummary>> {
  const ids = [...new Set(executionIds.filter(Boolean))]
  const batches: string[][] = []
  for (let index = 0; index < ids.length; index += 100) {
    batches.push(ids.slice(index, index + 100))
  }
  const responses = await Promise.all(
    batches.map((batch) =>
      listExecutions({ ids: batch, limit: Math.max(1, batch.length) }),
    ),
  )
  return Object.fromEntries(
    responses
      .flatMap((response) => response.executions)
      .map((item) => [item.id, item]),
  )
}
