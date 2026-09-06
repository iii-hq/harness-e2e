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
  generalRunMetrics,
  workflowMetricEntriesFromRecord,
  workflowMetricLabel,
  workflowMetricUnit,
} from '@/lib/workflow-metrics'

export type MetricTone = 'neutral' | 'unavailable'
export type MetricFormat =
  | 'percent_points'
  | 'score'
  | 'count'
  | 'tokens'
  | 'seconds'
  | 'milliseconds'
  | 'usd'

export type PlanMetricId =
  | 'coverage'
  | 'technical_failures'
  | 'quality'
  | 'confidence'
  | 'tokens'
  | 'tokens_per_completion'
  | 'failed_attempt_tokens'
  | 'duration'
  | 'cost'
  | 'function_calls'
  | 'function_errors'
  | 'turns'

export type PlanMetricComparison = {
  id: PlanMetricId | `workflow:${string}` | `criterion:${string}`
  label: string
  baseline: number | null
  candidate: number | null
  delta: number | null
  delta_percent: number | null
  format: MetricFormat
  tone: MetricTone
  evidence?: {
    baseline_observed: number
    candidate_observed: number
    baseline_planned: number | null
    candidate_planned: number | null
    paired: number
    paired_baseline: number | null
    paired_candidate: number | null
  }
}

export type PlanScenarioComparison = {
  id: string
  compatible: boolean
  reason: string | null
  metrics: PlanMetricComparison[]
  execution_metrics: PlanMetricComparison[]
  workflow_metrics: PlanMetricComparison[]
}

export type PlanComparison = {
  headline: string
  detail: string
  baseline: DashboardExecutionSummary | null
  candidate: DashboardExecutionSummary | null
  metrics: PlanMetricComparison[]
  scenarios: PlanScenarioComparison[]
}

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

function comparisonMetric(
  id: PlanMetricId | `workflow:${string}` | `criterion:${string}`,
  label: string,
  baseline: number | null,
  candidate: number | null,
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
    format,
    tone: delta === null ? 'unavailable' : 'neutral',
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
    case 'coverage':
      return percentPoints(finite(executionTotals.report_coverage))
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
    case 'tokens_per_completion':
      return finite(executionTotals.tokens_per_completion)
    case 'failed_attempt_tokens':
      return finite(executionTotals.failed_attempt_tokens)
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
  const build = (id: PlanMetricId, label: string, format: MetricFormat) => {
    const [left, right] = value(id)
    return comparisonMetric(id, label, left, right, format)
  }
  return [
    build('coverage', 'Coverage', 'percent_points'),
    build('technical_failures', 'Technical failures', 'count'),
    build('quality', 'Advisory quality', 'score'),
    build('confidence', 'Confidence', 'percent_points'),
    build('tokens', 'Tokens', 'tokens'),
    build('tokens_per_completion', 'Tokens per completion', 'tokens'),
    build('failed_attempt_tokens', 'Failed attempt tokens', 'tokens'),
    build('duration', 'Duration', 'seconds'),
    build('cost', 'Cost', 'usd'),
    build('function_calls', 'Function calls', 'count'),
    build('function_errors', 'Function errors', 'count'),
    build('turns', 'Turns', 'count'),
  ]
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

function scenarioAverage(
  metric: DashboardScenarioMetricSummary | undefined,
  key:
    | 'tokens'
    | 'tokens_per_completion'
    | 'failed_attempt_tokens'
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
  const left = generalRunMetrics(baseline)
  const right = generalRunMetrics(candidate)
  const metric = (
    id: PlanMetricId,
    label: string,
    leftValue: number | null,
    rightValue: number | null,
    format: MetricFormat,
  ) =>
    comparisonMetric(
      id,
      label,
      compatible ? leftValue : null,
      compatible ? rightValue : null,
      format,
    )
  return [
    metric(
      'cost',
      'Cost',
      left.costUsd ?? scenarioAverage(baselineSummary, 'cost_usd'),
      right.costUsd ?? scenarioAverage(candidateSummary, 'cost_usd'),
      'usd',
    ),
    metric(
      'tokens',
      'Tokens',
      left.totalTokens ?? scenarioAverage(baselineSummary, 'tokens'),
      right.totalTokens ?? scenarioAverage(candidateSummary, 'tokens'),
      'tokens',
    ),
    metric(
      'function_calls',
      'Function calls',
      left.functionCalls ?? scenarioAverage(baselineSummary, 'function_calls'),
      right.functionCalls ??
        scenarioAverage(candidateSummary, 'function_calls'),
      'count',
    ),
    metric(
      'function_errors',
      'Function errors',
      left.functionCallErrors ??
        scenarioAverage(baselineSummary, 'function_call_errors'),
      right.functionCallErrors ??
        scenarioAverage(candidateSummary, 'function_call_errors'),
      'count',
    ),
    metric(
      'duration',
      'Duration',
      scenarioAverage(baselineSummary, 'duration_seconds') ??
        runDurationSeconds(baseline),
      scenarioAverage(candidateSummary, 'duration_seconds') ??
        runDurationSeconds(candidate),
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
        workflowMetricUnit(path),
      ),
    )
}

function criterionPoints(
  execution: DashboardExecutionSummary,
  scenarioId: string,
) {
  const detail = execution as DashboardExecutionDetail
  const records = (detail.reports ?? []).filter(
    (record) => record.scenario_id === scenarioId,
  )
  const samples = records.flatMap((record) => {
    if (!record.available || !record.report) return []
    const report = record.report
    return report.scenarios
      .filter((scenario) => scenario.scenario_id === scenarioId)
      .flatMap((scenario) => {
        const subject = objectValue(report.subject)
        const judge = objectValue(report.judge)
        const caseValue = objectValue(scenario.case)
        const policy = objectValue(scenario.execution_policy)
        const identity = [
          report.result_contract_sha256,
          report.scoring_profile_sha256,
          scenario.case_id,
          caseValue.inputs_sha256,
          subject.model,
          subject.provider,
          judge.model,
          judge.provider,
          report.judge_protocol,
        ]
        return scenario.runs.flatMap((run, index) => {
          const round =
            finite(record.round) ??
            (scenario.aggregate?.planned_runs === scenario.runs.length
              ? index + 1
              : null)
          const pair =
            round !== null &&
            identity.every(
              (value) => typeof value === 'string' && value.length > 0,
            ) &&
            Object.keys(policy).length > 0
              ? JSON.stringify([
                  ...identity,
                  Object.keys(policy)
                    .sort()
                    .map((key) => [key, policy[key]]),
                  round,
                ])
              : null
          return (Array.isArray(run.criteria) ? run.criteria : [])
            .map(objectValue)
            .map((criterion) => ({
              id: `${criterion.id}:${criterion.possible}`,
              label: `Criterion ${criterion.id} · mean points / ${criterion.possible}`,
              runId: run.run_id,
              pair,
              value:
                run.technical === 'valid' ? finite(criterion.awarded) : null,
            }))
        })
      })
  })
  const slots = Array.isArray(detail.plan_execution?.slots)
    ? detail.plan_execution.slots
    : null
  const planned = slots
    ? slots.filter((slot) => slot.scenario_id === scenarioId).length
    : finite(scenarioMap(execution).get(scenarioId)?.runs)
  const criteria = new Map(
    [...new Set(samples.map((sample) => sample.id))].map((id) => {
      const values = samples.filter((sample) => sample.id === id)
      const unique = values.filter(
        (sample) =>
          values.filter((other) => other.runId === sample.runId).length === 1,
      )
      return [
        id,
        {
          label: values[0].label,
          samples: unique.filter((sample) => sample.value !== null),
        },
      ]
    }),
  )
  return { criteria, planned }
}

function criterionComparisons(
  baseline: DashboardExecutionSummary,
  candidate: DashboardExecutionSummary,
  scenarioId: string,
  compatible: boolean,
): PlanMetricComparison[] {
  const left = criterionPoints(baseline, scenarioId)
  const right = criterionPoints(candidate, scenarioId)
  const mean = (values: Array<number | null>) =>
    values.length
      ? values.reduce<number>((sum, value) => sum + (value ?? 0), 0) /
        values.length
      : null
  return [...new Map([...left.criteria, ...right.criteria])]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([id, descriptor]) => {
      const before = left.criteria.get(id)?.samples ?? []
      const after = right.criteria.get(id)?.samples ?? []
      const pairs = compatible
        ? before.flatMap((sample) => {
            const matches = after.filter(
              (other) => sample.pair !== null && other.pair === sample.pair,
            )
            return matches.length === 1 &&
              before.filter((other) => other.pair === sample.pair).length === 1
              ? [[sample.value, matches[0].value]]
              : []
          })
        : []
      const pairedBaseline = mean(pairs.map((pair) => pair[0]))
      const pairedCandidate = mean(pairs.map((pair) => pair[1]))
      return {
        ...comparisonMetric(
          `criterion:${id}`,
          descriptor.label,
          pairedBaseline,
          pairedCandidate,
          'score',
        ),
        baseline: mean(before.map((sample) => sample.value)),
        candidate: mean(after.map((sample) => sample.value)),
        evidence: {
          baseline_observed: before.length,
          candidate_observed: after.length,
          baseline_planned: left.planned,
          candidate_planned: right.planned,
          paired: pairs.length,
          paired_baseline: pairedBaseline,
          paired_candidate: pairedCandidate,
        },
      }
    })
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
    const leftGeneral = generalRunMetrics(leftRun)
    const rightGeneral = generalRunMetrics(rightRun)
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
      format: MetricFormat,
    ) =>
      comparisonMetric(
        metricId,
        label,
        compatible ? baselineValue : null,
        compatible ? candidateValue : null,
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
      metrics: [
        ...criterionComparisons(baseline, candidate, id, compatible),
        metric(
          'quality',
          'Advisory quality',
          finite(left?.assessment_summary?.median_quality_score),
          finite(right?.assessment_summary?.median_quality_score),
          'score',
        ),
        metric(
          'tokens',
          'Tokens',
          scenarioAverage(leftMetrics, 'tokens') ?? leftGeneral.totalTokens,
          scenarioAverage(rightMetrics, 'tokens') ?? rightGeneral.totalTokens,
          'tokens',
        ),
        metric(
          'tokens_per_completion',
          'Tokens per completion',
          scenarioAverage(leftMetrics, 'tokens_per_completion'),
          scenarioAverage(rightMetrics, 'tokens_per_completion'),
          'tokens',
        ),
        metric(
          'failed_attempt_tokens',
          'Failed attempt tokens',
          scenarioAverage(leftMetrics, 'failed_attempt_tokens'),
          scenarioAverage(rightMetrics, 'failed_attempt_tokens'),
          'tokens',
        ),
        metric(
          'duration',
          'Duration',
          scenarioAverage(leftMetrics, 'duration_seconds'),
          scenarioAverage(rightMetrics, 'duration_seconds'),
          'seconds',
        ),
        metric(
          'cost',
          'Cost',
          scenarioAverage(leftMetrics, 'cost_usd') ?? leftGeneral.costUsd,
          scenarioAverage(rightMetrics, 'cost_usd') ?? rightGeneral.costUsd,
          'usd',
        ),
        metric(
          'turns',
          'Turns',
          scenarioAverage(leftMetrics, 'turns') ?? runTurns(leftRun),
          scenarioAverage(rightMetrics, 'turns') ?? runTurns(rightRun),
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
    headline: 'Retained observations',
    detail:
      'Review criterion points, coverage and consumption together. Differences describe observations; they do not establish a winner or equivalence.',
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
