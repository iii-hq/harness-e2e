import { describe, expect, it } from 'vitest'
import {
  comparisonOrigin,
  hashForComparison,
  hashForExecution,
  hashForStacks,
  hashForSuites,
  hashForTestHistory,
  hashForTrends,
  hashForVersionComparison,
  hashForWorkspace,
  hashFrom,
  routeFromHash,
  routeRenderIdentity,
  trendsOrigin,
} from '@/hooks/use-hash-route'

describe('a comparison opened from Trends', () => {
  it('goes back to the Trends view it came from, and to nothing else', () => {
    const trends = hashForTrends(new URLSearchParams({ stack: 'any' }))
    const compare = hashFrom(hashForComparison('plan-a', 'plan-b'), trends)
    expect(trendsOrigin(compare)).toBe(trends)
    expect(trendsOrigin(hashForComparison('plan-a', 'plan-b'))).toBeNull()
    for (const from of ['https://example.com/', hashForExecution('plan-a')])
      expect(
        trendsOrigin(hashFrom(hashForComparison('plan-a', 'plan-b'), from)),
      ).toBeNull()
  })
})

describe('a run page opened from a comparison', () => {
  const comparison = `${hashForComparison('plan-a', 'plan-b')}?exclude=minimal_path`
  const run = hashForExecution('plan-b', null, 'run-1', 'transcript')

  it('carries the comparison, its choice included, and gives it back', () => {
    const linked = hashFrom(run, comparison)
    expect(routeFromHash(linked)).toEqual(routeFromHash(run))
    expect(comparisonOrigin(linked)).toBe(comparison)
    expect(comparisonOrigin(run)).toBeNull()
  })

  it('accepts only a comparison of this dashboard, never any hash or URL', () => {
    for (const from of [
      'https://example.com/',
      'javascript:alert(1)',
      '#/compare/plan-a/plan-b',
      hashForExecution('plan-a'),
      hashForComparison('plan-a'),
      `${hashForComparison('plan-a', 'plan-b')}/../../stacks`,
    ])
      expect(comparisonOrigin(hashFrom(run, from))).toBeNull()
  })
})

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
      view: 'evidence',
    })
    const transcript = hashForExecution('exec-1', null, 'run/1', 'transcript')
    expect(transcript).toBe(
      '#/ext/harness-e2e/execution/exec-1/run/run%2F1/transcript',
    )
    expect(routeFromHash(transcript)).toMatchObject({
      runId: 'run/1',
      view: 'transcript',
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

  it('keeps suites, stacks and test metric history as independent routes', () => {
    expect(hashForTestHistory('direct/answer')).toBe(
      '#/ext/harness-e2e/tests/direct%2Fanswer',
    )
    expect(routeFromHash('#/ext/harness-e2e/tests/direct%2Fanswer')).toEqual({
      page: 'test-history',
      testId: 'direct/answer',
    })
    expect(hashForSuites()).toBe('#/ext/harness-e2e/suites')
    expect(routeFromHash(hashForSuites())).toEqual({ page: 'suites' })
    expect(hashForSuites('suite-1')).toBe(
      '#/ext/harness-e2e/suites?suite=suite-1',
    )
    expect(routeFromHash(hashForSuites('suite-1'))).toEqual({ page: 'suites' })
    expect(hashForStacks()).toBe('#/ext/harness-e2e/stacks')
    expect(routeFromHash(hashForStacks())).toEqual({ page: 'stacks' })
    expect(hashForTrends()).toBe('#/ext/harness-e2e/trends')
    expect(routeFromHash(hashForTrends())).toEqual({
      page: 'trends',
      request: {},
      period: { range: '30d' },
    })
    expect(
      routeFromHash(
        hashForTrends(
          new URLSearchParams({
            suite: 'regression',
            provider: 'deepseek',
            model: 'deepseek-flash',
            profile: '',
            stack: 'not_recorded',
            since: '2026-09-01',
            until: '2026-09-10',
          }),
        ),
      ),
    ).toEqual({
      page: 'trends',
      request: {
        suite: 'regression',
        provider: 'deepseek',
        model: 'deepseek-flash',
        profile: null,
        stack: 'not_recorded',
      },
      period: { since: '2026-09-01', until: '2026-09-10' },
    })
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
