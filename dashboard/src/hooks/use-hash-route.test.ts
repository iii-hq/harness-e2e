import { describe, expect, it } from 'vitest'
import {
  hashForComparison,
  hashForExecution,
  hashForSuites,
  hashForTestHistory,
  hashForVersionComparison,
  hashForWorkspace,
  routeFromHash,
  routeRenderIdentity,
} from '@/hooks/use-hash-route'

describe('dashboard hash routes', () => {
  it('opens executions in the Console and rejects standalone routes', () => {
    expect(routeFromHash('#/ext/harness-e2e')).toEqual({
      page: 'workspace',
      view: 'executions',
    })
    expect(routeFromHash('#/tests')).toBeNull()
    expect(routeFromHash('#/ext/harness-e2e/tests')).toEqual({
      page: 'workspace',
      view: 'tests',
    })
    expect(hashForWorkspace()).toBe('#/ext/harness-e2e/executions')
  })

  it('round-trips execution ids and diagnostic anchors', () => {
    const hash = hashForExecution(
      'run/id with spaces',
      'scenario-direct_answer',
    )
    expect(hash).toBe(
      '#/ext/harness-e2e/execution/run%2Fid%20with%20spaces/scenario-direct_answer',
    )
    expect(routeFromHash(hash)).toEqual({
      page: 'execution',
      executionId: 'run/id with spaces',
      anchor: 'scenario-direct_answer',
      runId: null,
    })
  })

  // Audit AW-09: the evidence record has its own route under the execution.
  it('routes an evidence record under its execution', () => {
    const hash = hashForExecution('exec-1', null, 'run/1')
    expect(hash).toBe('#/ext/harness-e2e/execution/exec-1/run/run%2F1')
    expect(routeFromHash(hash)).toEqual({
      page: 'execution',
      executionId: 'exec-1',
      anchor: null,
      runId: 'run/1',
    })
  })

  it('routes execution and version comparisons apart', () => {
    const comparison = hashForComparison('execution/a', 'execution b')
    expect(comparison).toBe(
      '#/ext/harness-e2e/compare/execution%2Fa/execution%20b',
    )
    expect(routeFromHash(comparison)).toEqual({
      page: 'compare',
      left: 'execution/a',
      right: 'execution b',
    })
    expect(
      routeFromHash(hashForVersionComparison('version/a', 'version b')),
    ).toEqual({ page: 'versions', left: 'version/a', right: 'version b' })
    expect(routeFromHash('#main')).toBeNull()
  })

  it('keeps suites and test metric history as independent routes', () => {
    expect(hashForTestHistory('direct/answer')).toBe(
      '#/ext/harness-e2e/tests/direct%2Fanswer',
    )
    expect(routeFromHash('#/ext/harness-e2e/tests/direct%2Fanswer')).toEqual({
      page: 'test-history',
      testId: 'direct/answer',
    })
    expect(hashForSuites()).toBe('#/ext/harness-e2e/suites')
    expect(routeFromHash(hashForSuites())).toEqual({ page: 'suites' })
    // The retired plan pages are no route of this page any more.
    expect(routeFromHash('#/ext/harness-e2e/plans')).toBeNull()
    expect(routeFromHash('#/ext/harness-e2e/plans/new')).toBeNull()
  })

  it('does not claim routes belonging to another Console page', () => {
    expect(routeFromHash('#/workers')).toBeNull()
    expect(routeFromHash('#/ext/other/execution/run')).toBeNull()
    expect(routeRenderIdentity({ page: 'workspace', view: 'executions' })).toBe(
      'workspace:executions',
    )
    expect(routeRenderIdentity({ page: 'workspace', view: 'tests' })).toBe(
      'workspace:tests',
    )
  })
})
