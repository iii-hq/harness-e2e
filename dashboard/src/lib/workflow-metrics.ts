import type {
  DashboardExecutionDetail,
  JsonValue,
  SemanticTestReport,
} from '@/lib/dashboard-data-source'

export type WorkflowMetricsSummary = {
  stepCount: number
  succeededSteps: number
  failedSteps: number
  hardGateFailedSteps: number
  skippedSteps: number
  cancelledSteps: number
  runningSteps: number
  pendingSteps: number
  durationMs: number
  assetCount: number
  hardGateCount: number
  passedHardGateCount: number
  evaluationCount: number
  failureCount: number
  inputTokens: number
  outputTokens: number
  totalTokens: number
  functionCalls: number
  functionCallErrors: number
  inputTokenMetricSteps: number
  outputTokenMetricSteps: number
  tokenMetricSteps: number
  functionCallMetricSteps: number
  functionCallErrorMetricSteps: number
  numericMetrics: Record<string, number>
}

export type WorkflowStepUsage = {
  inputTokens: number | null
  outputTokens: number | null
  totalTokens: number | null
  functionCalls: number | null
  functionCallErrors: number | null
}

export const emptyWorkflowMetrics = (): WorkflowMetricsSummary => ({
  stepCount: 0,
  succeededSteps: 0,
  failedSteps: 0,
  hardGateFailedSteps: 0,
  skippedSteps: 0,
  cancelledSteps: 0,
  runningSteps: 0,
  pendingSteps: 0,
  durationMs: 0,
  assetCount: 0,
  hardGateCount: 0,
  passedHardGateCount: 0,
  evaluationCount: 0,
  failureCount: 0,
  inputTokens: 0,
  outputTokens: 0,
  totalTokens: 0,
  functionCalls: 0,
  functionCallErrors: 0,
  inputTokenMetricSteps: 0,
  outputTokenMetricSteps: 0,
  tokenMetricSteps: 0,
  functionCallMetricSteps: 0,
  functionCallErrorMetricSteps: 0,
  numericMetrics: {},
})

export function semanticTestsFromDetail(
  detail: DashboardExecutionDetail,
): SemanticTestReport[] {
  return (detail.reports ?? []).flatMap((record) =>
    (record.report?.scenarios ?? []).flatMap((scenario) =>
      (scenario.runs ?? []).flatMap((run) => run.semantic_tests ?? []),
    ),
  )
}

export function aggregateWorkflowMetrics(
  tests: readonly SemanticTestReport[],
): WorkflowMetricsSummary {
  const summary = emptyWorkflowMetrics()
  summary.stepCount = tests.length

  for (const test of tests) {
    const status = test.status.toLowerCase()
    if (status === 'succeeded') summary.succeededSteps += 1
    else if (status === 'failed') summary.failedSteps += 1
    else if (status === 'hard_gate_failed') summary.hardGateFailedSteps += 1
    else if (status === 'skipped') summary.skippedSteps += 1
    else if (status === 'cancelled') summary.cancelledSteps += 1
    else if (status === 'running') summary.runningSteps += 1
    else if (status === 'pending') summary.pendingSteps += 1

    summary.durationMs += finiteNumber(test.duration_ms)
    summary.assetCount += test.assets?.length ?? 0
    summary.hardGateCount += test.hard_gates?.length ?? 0
    summary.passedHardGateCount +=
      test.hard_gates?.filter((gate) => gate.passed).length ?? 0
    summary.evaluationCount += test.evaluations?.length ?? 0
    summary.failureCount += test.failures?.length ?? 0
    const usage = workflowStepUsage(test.metrics)
    if (usage.inputTokens != null) {
      summary.inputTokens += usage.inputTokens
      summary.inputTokenMetricSteps += 1
    }
    if (usage.outputTokens != null) {
      summary.outputTokens += usage.outputTokens
      summary.outputTokenMetricSteps += 1
    }
    if (usage.totalTokens != null) {
      summary.totalTokens += usage.totalTokens
      summary.tokenMetricSteps += 1
    }
    if (usage.functionCalls != null) {
      summary.functionCalls += usage.functionCalls
      summary.functionCallMetricSteps += 1
    }
    if (usage.functionCallErrors != null) {
      summary.functionCallErrors += usage.functionCallErrors
      summary.functionCallErrorMetricSteps += 1
    }
    collectNumericMetricLeaves(test.metrics, '', summary.numericMetrics)
  }

  return summary
}

export function workflowMetricsFromDetail(
  detail: DashboardExecutionDetail,
): WorkflowMetricsSummary {
  return aggregateWorkflowMetrics(semanticTestsFromDetail(detail))
}

export function workflowMetricEntries(
  metrics: WorkflowMetricsSummary,
): Array<[string, number]> {
  return Object.entries(metrics.numericMetrics)
    .filter(([key]) => !USAGE_METRIC_PATHS.has(key))
    .sort(([left], [right]) => left.localeCompare(right))
}

/**
 * Read the usage contract emitted by Harness-backed workflow steps. Direct
 * keys remain supported for reports produced before the canonical `totals`
 * envelope was introduced.
 */
export function workflowStepUsage(
  metrics: JsonValue | undefined | null,
): WorkflowStepUsage {
  const inputTokens = firstNumericMetric(metrics, [
    ['totals', 'input_tokens'],
    ['input_tokens'],
  ])
  const outputTokens = firstNumericMetric(metrics, [
    ['totals', 'output_tokens'],
    ['output_tokens'],
  ])
  const explicitTotalTokens = firstNumericMetric(metrics, [
    ['totals', 'total_tokens'],
    ['total_tokens'],
    ['tokens'],
  ])

  return {
    inputTokens,
    outputTokens,
    totalTokens:
      explicitTotalTokens ??
      (inputTokens != null && outputTokens != null
        ? inputTokens + outputTokens
        : null),
    functionCalls: firstNumericMetric(metrics, [
      ['totals', 'function_calls'],
      ['function_calls'],
    ]),
    functionCallErrors: firstNumericMetric(metrics, [
      ['totals', 'function_call_errors'],
      ['function_call_errors'],
    ]),
  }
}

const USAGE_METRIC_PATHS = new Set([
  'totals.input_tokens',
  'totals.output_tokens',
  'totals.total_tokens',
  'totals.function_calls',
  'totals.function_call_errors',
  'input_tokens',
  'output_tokens',
  'total_tokens',
  'tokens',
  'function_calls',
  'function_call_errors',
])

function finiteNumber(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0
}

function firstNumericMetric(
  value: JsonValue | undefined | null,
  paths: readonly (readonly string[])[],
): number | null {
  for (const path of paths) {
    const metric = numericMetricAtPath(value, path)
    if (metric != null) return metric
  }
  return null
}

function numericMetricAtPath(
  value: JsonValue | undefined | null,
  path: readonly string[],
): number | null {
  let cursor: unknown = value
  for (const segment of path) {
    if (!cursor || typeof cursor !== 'object' || Array.isArray(cursor)) {
      return null
    }
    cursor = (cursor as Record<string, unknown>)[segment]
  }
  return typeof cursor === 'number' && Number.isFinite(cursor) && cursor >= 0
    ? cursor
    : null
}

function collectNumericMetricLeaves(
  value: JsonValue | undefined | null,
  path: string,
  output: Record<string, number>,
) {
  if (typeof value === 'number' && Number.isFinite(value)) {
    const key = path || 'value'
    output[key] = (output[key] ?? 0) + value
    return
  }
  if (Array.isArray(value)) {
    for (const child of value) collectNumericMetricLeaves(child, path, output)
    return
  }
  if (!value || typeof value !== 'object') return
  for (const [key, child] of Object.entries(value)) {
    collectNumericMetricLeaves(child, path ? `${path}.${key}` : key, output)
  }
}
