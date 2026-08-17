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
  numericMetrics: Record<string, number>
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
  return Object.entries(metrics.numericMetrics).sort(([left], [right]) =>
    left.localeCompare(right),
  )
}

function finiteNumber(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0
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
