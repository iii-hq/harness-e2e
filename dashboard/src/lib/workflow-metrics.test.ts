import { describe, expect, it } from 'vitest'
import type {
  DashboardExecutionDetail,
  SemanticTestReport,
} from '@/lib/dashboard-data-source'
import {
  aggregateWorkflowMetrics,
  summedGeneralRunMetricsFromDetail,
  workflowMetricEntries,
  workflowStepUsage,
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

  it('consolidates canonical step usage and exposes reporting coverage', () => {
    const tests = [
      {
        node_id: 'produce',
        status: 'succeeded',
        duration_ms: 10,
        metrics: {
          totals: {
            input_tokens: 1_200,
            output_tokens: 300,
            function_calls: 4,
            function_call_errors: 1,
          },
        },
      },
      {
        node_id: 'validate',
        status: 'succeeded',
        duration_ms: 20,
        metrics: {
          totals: {
            input_tokens: 700,
            output_tokens: 100,
            function_calls: 2,
            function_call_errors: 0,
          },
        },
      },
    ] as unknown as SemanticTestReport[]

    const metrics = aggregateWorkflowMetrics(tests)

    expect(metrics).toMatchObject({
      inputTokens: 1900,
      outputTokens: 400,
      totalTokens: 2300,
      functionCalls: 6,
      functionCallErrors: 1,
      inputTokenMetricSteps: 2,
      outputTokenMetricSteps: 2,
      tokenMetricSteps: 2,
      functionCallMetricSteps: 2,
      functionCallErrorMetricSteps: 2,
    })
    expect(workflowMetricEntries(metrics)).toEqual([])
  })

  it('keeps partial usage explicit and reads legacy direct keys', () => {
    const tests = [
      {
        node_id: 'legacy',
        status: 'succeeded',
        duration_ms: 10,
        metrics: { total_tokens: 900, function_calls: 3 },
      },
      {
        node_id: 'operational-only',
        status: 'succeeded',
        duration_ms: 20,
        metrics: { request_count: 2 },
      },
    ] as unknown as SemanticTestReport[]

    const metrics = aggregateWorkflowMetrics(tests)

    expect(metrics).toMatchObject({
      totalTokens: 900,
      functionCalls: 3,
      tokenMetricSteps: 1,
      functionCallMetricSteps: 1,
    })
    expect(workflowMetricEntries(metrics)).toEqual([['request_count', 2]])
    expect(workflowStepUsage({ totals: { input_tokens: 10 } })).toMatchObject({
      inputTokens: 10,
      outputTokens: null,
      totalTokens: null,
    })
  })

  it('sums the general metrics shown by every retained test', () => {
    const run = (
      totalTokens: number,
      functionCalls: number,
      functionCallErrors: number,
    ) => ({
      metrics: {
        totals: {
          total_tokens: totalTokens,
          function_calls: functionCalls,
          function_call_errors: functionCallErrors,
        },
      },
      semantic_tests: [],
    })
    const detail = {
      reports: [
        {
          report: {
            scenarios: [{ runs: [run(367, 0, 0)] }],
          },
        },
        {
          report: {
            scenarios: [{ runs: [run(4_142, 3, 0)] }],
          },
        },
        {
          report: {
            scenarios: [
              {
                runs: [
                  {
                    judge_usage: { input_tokens: 3_325, output_tokens: 1_311 },
                    semantic_tests: [
                      {
                        node_id: 'scan_commit_a',
                        status: 'succeeded',
                        duration_ms: 0,
                        metrics: {
                          request_count: 2,
                          poll: { poll_count: 1 },
                        },
                        failures: [],
                      },
                      {
                        node_id: 'suggest_commit_a',
                        status: 'succeeded',
                        duration_ms: 0,
                        metrics: {
                          request_count: 1,
                          poll: { poll_count: 1 },
                        },
                        failures: [],
                      },
                      {
                        node_id: 'scan_commit_b',
                        status: 'succeeded',
                        duration_ms: 0,
                        metrics: {
                          request_count: 1,
                          poll: { poll_count: 1 },
                        },
                        failures: [],
                      },
                      {
                        node_id: 'github_reconciliation',
                        status: 'succeeded',
                        duration_ms: 0,
                        metrics: { reconciliation_operations: 4 },
                        failures: [],
                      },
                      {
                        node_id: 'list_run_history',
                        status: 'succeeded',
                        duration_ms: 0,
                        failures: [],
                      },
                    ],
                  },
                ],
              },
            ],
          },
        },
      ],
    } as unknown as DashboardExecutionDetail

    expect(summedGeneralRunMetricsFromDetail(detail)).toMatchObject({
      totalTokens: 9_145,
      functionCalls: 16,
      functionCallErrors: 0,
      costUsd: null,
      backfilled: true,
    })
  })
})
