import { describe, expect, it } from 'vitest'
import {
  hashForComparison,
  hashForCoverage,
  hashForExecution,
  hashForWorkspace,
  routeFromHash,
} from '@/hooks/use-hash-route'

describe('dashboard hash routes', () => {
  it('routes the evidence workspace without an html page name', () => {
    expect(routeFromHash('')).toEqual({ page: 'overview', view: 'overview' })
    expect(routeFromHash('#/scenarios')).toEqual({
      page: 'overview',
      view: 'scenarios',
    })
    expect(hashForWorkspace('executions')).toBe('#/executions')
  })

  it('round-trips execution ids and diagnostic anchors', () => {
    const hash = hashForExecution(
      'run/id with spaces',
      'scenario-direct_answer',
    )
    expect(hash).toBe(
      '#/execution/run%2Fid%20with%20spaces/scenario-direct_answer',
    )
    expect(routeFromHash(hash)).toEqual({
      page: 'execution',
      executionId: 'run/id with spaces',
      anchor: 'scenario-direct_answer',
    })
  })

  it('routes comparisons and coverage from the single entry point', () => {
    const comparison = hashForComparison('baseline/a', 'candidate b')
    expect(routeFromHash(comparison)).toEqual({
      page: 'compare',
      left: 'baseline/a',
      right: 'candidate b',
    })
    expect(hashForCoverage()).toBe('#/coverage')
    expect(routeFromHash('#main')).toBeNull()
  })
})
