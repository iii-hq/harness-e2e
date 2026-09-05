import type {
  DashboardExecutionDetail,
  DashboardRunProjection,
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

export type GeneralRunMetrics = {
  totalTokens: number | null
  functionCalls: number | null
  functionCallErrors: number | null
  costUsd: number | null
}

export function generalRunMetrics(
  run: DashboardRunProjection | null | undefined,
): GeneralRunMetrics {
  const totals = run?.metrics?.totals
  const input = nullableNumber(totals?.input_tokens)
  const output = nullableNumber(totals?.output_tokens)
  return {
    totalTokens:
      nullableNumber(totals?.total_tokens ?? run?.efficiency?.total_tokens) ??
      (input != null && output != null ? input + output : null),
    functionCalls: nullableNumber(
      totals?.function_calls ?? run?.efficiency?.function_calls,
    ),
    functionCallErrors: nullableNumber(
      totals?.function_call_errors ?? run?.efficiency?.function_call_errors,
    ),
    costUsd: nullableNumber(run?.cost?.total_usd),
  }
}

export function summedGeneralRunMetricsFromDetail(
  detail: DashboardExecutionDetail,
): GeneralRunMetrics | null {
  const runs = (detail.reports ?? []).flatMap((record) =>
    (record.report?.scenarios ?? []).flatMap((scenario) => scenario.runs ?? []),
  )
  if (runs.length === 0) return null

  const values = runs.map(generalRunMetrics)
  const sumComplete = (
    select: (metrics: GeneralRunMetrics) => number | null,
  ): number | null => {
    const selected = values.map(select)
    return selected.every((value): value is number => value !== null)
      ? selected.reduce((total, value) => total + value, 0)
      : null
  }

  return {
    totalTokens: sumComplete((metrics) => metrics.totalTokens),
    functionCalls: sumComplete((metrics) => metrics.functionCalls),
    functionCallErrors: sumComplete((metrics) => metrics.functionCallErrors),
    costUsd: sumComplete((metrics) => metrics.costUsd),
  }
}

function nullableNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0
    ? value
    : null
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
  return workflowMetricEntriesFromRecord(metrics.numericMetrics)
}

export function workflowMetricEntriesFromRecord(
  metrics: Readonly<Record<string, unknown>> | undefined | null,
): Array<[string, number]> {
  return Object.entries(metrics ?? {})
    .filter(
      (entry): entry is [string, number] =>
        !USAGE_METRIC_PATHS.has(entry[0]) &&
        typeof entry[1] === 'number' &&
        Number.isFinite(entry[1]),
    )
    .sort(([left], [right]) => left.localeCompare(right))
}

const WORKFLOW_METRIC_LABELS: Record<string, string> = {
  finding_count: 'Findings',
  listed_run_count: 'Listed runs',
  'poll.poll_count': 'Polls',
  'poll.wait_duration_ms': 'Poll wait',
  reconciliation_operations: 'Reconciliation operations',
  request_count: 'Requests',
}

export function workflowMetricLabel(path: string): string {
  return (
    WORKFLOW_METRIC_LABELS[path] ??
    path
      .replaceAll('.', ' ')
      .replaceAll('_', ' ')
      .replace(/\b\w/g, (letter) => letter.toUpperCase())
  )
}

export function workflowMetricUnit(path: string): 'count' | 'milliseconds' {
  return path.endsWith('_ms') ? 'milliseconds' : 'count'
}

/** Read the native usage totals; absent measurements remain unavailable. */
export function workflowStepUsage(
  metrics: JsonValue | undefined | null,
): WorkflowStepUsage {
  const inputTokens = numericMetricAtPath(metrics, ['totals', 'input_tokens'])
  const outputTokens = numericMetricAtPath(metrics, ['totals', 'output_tokens'])
  return {
    inputTokens,
    outputTokens,
    totalTokens:
      numericMetricAtPath(metrics, ['totals', 'total_tokens']) ??
      (inputTokens != null && outputTokens != null
        ? inputTokens + outputTokens
        : null),
    functionCalls: numericMetricAtPath(metrics, ['totals', 'function_calls']),
    functionCallErrors: numericMetricAtPath(metrics, [
      'totals',
      'function_call_errors',
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
