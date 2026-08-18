import { describe, expect, it } from 'vitest'
import type { SemanticTestReport } from '@/lib/dashboard-data-source'
import {
  aggregateWorkflowMetrics,
  workflowMetricEntries,
} from '@/lib/workflow-metrics'

describe('workflow metrics', () => {
  it('aggregates semantic status, evidence and nested operational counters', () => {
    const tests = [
      {
        node_id: 'scan',
        step_type: 'security.scan',
        step_version: 1,
        required: true,
        dependencies: [],
        status: 'succeeded',
        duration_ms: 40,
        metrics: {
          request_count: 2,
          finding_count: 5,
          poll: { poll_count: 1, wait_duration_ms: 3 },
        },
        assets: [{ id: 'report', artifact: { path: 'report.json' } }],
        hard_gates: [{ id: 'valid', passed: true, reason: 'ok' }],
        evaluations: [{ id: 'quality', outcome: 'passed', summary: 'ok' }],
        failures: [],
      },
      {
        node_id: 'reconcile',
        step_type: 'github.reconcile',
        step_version: 1,
        required: true,
        dependencies: ['scan'],
        status: 'failed',
        duration_ms: 10,
        metrics: { reconciliation_operations: 4 },
        assets: [],
        hard_gates: [{ id: 'available', passed: false, reason: 'offline' }],
        evaluations: [],
        failures: [{ phase: 'execute', message: 'offline' }],
      },
      {
        node_id: 'optional',
        step_type: 'optional.report',
        step_version: 1,
        required: false,
        dependencies: ['reconcile'],
        status: 'skipped',
        duration_ms: 0,
        assets: [],
        hard_gates: [],
        evaluations: [],
        failures: [],
      },
    ] as unknown as SemanticTestReport[]

    const metrics = aggregateWorkflowMetrics(tests)

    expect(metrics).toMatchObject({
      stepCount: 3,
      succeededSteps: 1,
      failedSteps: 1,
      skippedSteps: 1,
      durationMs: 50,
      assetCount: 1,
      hardGateCount: 2,
      passedHardGateCount: 1,
      evaluationCount: 1,
      failureCount: 1,
    })
    expect(metrics.numericMetrics).toEqual({
      finding_count: 5,
      'poll.poll_count': 1,
      'poll.wait_duration_ms': 3,
      reconciliation_operations: 4,
      request_count: 2,
    })
    expect(workflowMetricEntries(metrics).map(([key]) => key)).toEqual([
      'finding_count',
      'poll.poll_count',
      'poll.wait_duration_ms',
      'reconciliation_operations',
      'request_count',
    ])
  })
})
