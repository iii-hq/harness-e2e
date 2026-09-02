import { describe, expect, it } from 'vitest'
import {
  hashForComparison,
  hashForExecution,
  hashForNewPlan,
  hashForPlan,
  hashForPlans,
  hashForTestHistory,
  hashForWorkspace,
  routeFromHash,
  routeRenderIdentity,
} from '@/hooks/use-hash-route'
import { configureDashboardRuntime } from '@/lib/dashboard-runtime'

describe('dashboard hash routes', () => {
  it('routes the evidence workspace without an html page name', () => {
    expect(routeFromHash('')).toEqual({ page: 'overview', view: 'overview' })
    expect(routeFromHash('#/scenarios')).toEqual({
      page: 'overview',
      view: 'tests',
    })
    expect(routeFromHash('#/tests')).toEqual({
      page: 'overview',
      view: 'tests',
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

  it('routes comparisons from the single entry point', () => {
    const comparison = hashForComparison('version/a', 'version b')
    expect(routeFromHash(comparison)).toEqual({
      page: 'compare',
      left: 'version/a',
      right: 'version b',
    })
    expect(routeFromHash('#main')).toBeNull()
  })

  it('keeps local plans and test metric history as independent routes', () => {
    expect(hashForTestHistory('direct/answer')).toBe('#/tests/direct%2Fanswer')
    expect(routeFromHash('#/tests/direct%2Fanswer')).toEqual({
      page: 'test-history',
      testId: 'direct/answer',
    })
    expect(hashForPlans()).toBe('#/plans')
    expect(routeFromHash(hashForPlans())).toEqual({ page: 'plans' })
    expect(hashForNewPlan()).toBe('#/plans/new')
    expect(routeFromHash(hashForNewPlan())).toEqual({ page: 'plan-create' })
    expect(routeFromHash(hashForPlan('plan/one'))).toEqual({
      page: 'plan-detail',
      planId: 'plan/one',
    })
  })

  it('keeps every route inside the injectable Console page', () => {
    const restore = configureDashboardRuntime({
      embedded: true,
      hashBase: '#/ext/harness-e2e',
    })
    try {
      expect(hashForWorkspace()).toBe('#/ext/harness-e2e/overview')
      expect(hashForPlan('plan/one')).toBe('#/ext/harness-e2e/plans/plan%2Fone')
      expect(routeFromHash('#/ext/harness-e2e/execution/run%2Fone')).toEqual({
        page: 'execution',
        executionId: 'run/one',
        anchor: null,
      })
      expect(routeFromHash('#/workers')).toBeNull()
    } finally {
      restore()
    }
  })

  it('reloads only when navigation swaps the legacy workspace renderer', () => {
    expect(routeRenderIdentity({ page: 'overview', view: 'overview' })).toBe(
      'overview:workspace',
    )
    expect(routeRenderIdentity({ page: 'overview', view: 'executions' })).toBe(
      'overview:workspace',
    )
    expect(routeRenderIdentity({ page: 'overview', view: 'tests' })).toBe(
      'overview:tests',
    )
  })
})
