import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { hashForRunComparison, routeFromHash } from '@/hooks/use-hash-route'
import {
  comparability,
  criteriaChanges,
  metricRows,
  pairFromHash,
  RunComparison,
  runsInvestigation,
  side,
  sideScore,
  workerCalls,
} from '@/pages/RunComparePage'
import { formFlowRow, run } from '@/test-fixtures/test-history'

// The canvas's pair: 9:13 with a Tech Lead (90) against 10:01 solo (100).
const a = side(run('9:13'))
const b = side(run('10:01'))

describe('two runs of a test', () => {
  it('routes to the A × B of a test by execution id', () => {
    const hash = hashForRunComparison('form_flow_build', 'exec-a', 'exec-b')
    expect(hash).toBe(
      '#/ext/harness-e2e/tests/form_flow_build/compare?a=exec-a&b=exec-b',
    )
    expect(routeFromHash(hash)).toEqual({
      page: 'test-history',
      testId: 'form_flow_build',
      compare: true,
    })
    expect(routeFromHash('#/ext/harness-e2e/tests/form_flow_build')).toEqual({
      page: 'test-history',
      testId: 'form_flow_build',
    })
  })

  it('investigates the two runs as a comparison of their executions, focused on them', () => {
    expect(runsInvestigation('form_flow_build', a, b)).toEqual({
      executionId: 'plan-f811eb1f31c9ab47bca5de86e3c895e5',
      comparisonExecutionId: 'plan-cf6ab5f943136bb54a954bc763a26eff',
      focus: {
        scenarioId: 'form_flow_build',
        runId: '32c6b9c4907846aaaea371cb86b00003',
        comparedRunId: '32c6b9c4907846aaaea371cb86b00004',
      },
      changes: [
        { what: 'profile', change: 'ade-worker-builder → ade-solo-builder' },
      ],
    })
  })

  it('says what the runs share and what changed', () => {
    const shared = comparability(a.observation, b.observation)
    expect(shared.same).toBe('Same definition 0c5c0902, model and system.')
    expect(shared.changed).toEqual([
      { label: 'profile', value: 'ade-worker-builder → ade-solo-builder' },
    ])
    expect(shared.note).toBe(
      'Differences below come from the profile and from run-to-run variation.',
    )
  })

  it('lists the metrics with B minus A and no verdict', () => {
    const rows = Object.fromEntries(
      metricRows(a, b).map((row) => [row.label, row]),
    )
    expect(rows.Score).toMatchObject({ a: 90, b: 100 })
    expect(rows['Criteria met'].text).toEqual({ a: '8/9', b: '9/9' })
    expect(rows.Sessions).toMatchObject({ a: 4, b: 1 })
    const html = renderToStaticMarkup(
      <RunComparison
        a={a}
        b={b}
        spec={formFlowRow.spec ?? null}
        onSwap={() => undefined}
      />,
    )
    expect(html).toContain('+10 pts')
    // Criteria met reads 8/9 → 9/9 and differs by a count.
    expect(html).toContain('>8/9</td>')
    expect(html).toContain('+1 · +13%')
    expect(html).toContain('−29m 19s · −78%')
    expect(html).not.toMatch(/Improved|Regressed|better|worse/)
  })

  it('reads every figure from the chosen runs, each over its own criteria', () => {
    // Two attempts: the observation's mean and median are not the run's.
    const twice = run('9:13')
    const last = structuredClone(twice.runs?.[0] ?? never())
    last.run_id = 'second'
    last.score = 70
    last.duration_seconds = 60
    last.details = {
      ...(last.details ?? never()),
      criteria: (last.details ?? never()).criteria.slice(0, 8),
    }
    twice.runs = [...(twice.runs ?? []), last]
    const rows = Object.fromEntries(
      metricRows(side(twice), b).map((row) => [row.label, row]),
    )
    expect(rows.Score).toMatchObject({ a: 70, b: 100 })
    expect(rows.Duration).toMatchObject({ a: 60_000 })
    expect(rows['Criteria met'].text).toEqual({ a: '7/8', b: '9/9' })
    expect(sideScore(side(twice))).toBe(70)
  })

  it('follows A and B in the hash', () => {
    expect(
      pairFromHash('#/ext/harness-e2e/tests/t/compare?a=x%3Ac1&b=y'),
    ).toEqual(['x:c1', 'y'])
    expect(pairFromHash('#/ext/harness-e2e/tests/t/compare')).toEqual(['', ''])
  })

  it('shows the criteria that changed with each run’s reason', () => {
    const criteria = criteriaChanges(a, b)
    expect(criteria.metOnBoth).toBe(8)
    expect(criteria.changes).toEqual([
      {
        id: 'runtime_contract',
        possible: 10,
        a: 0,
        b: 10,
        reasons: {
          a: 'compose_valid=true, worker_ready=true, function_surface=false',
          b: 'function_surface=true',
        },
      },
    ])
  })

  it('pairs the root session’s calls by worker and both session trees', () => {
    const calls = workerCalls(a, b)
    expect(calls.max).toBe(32)
    expect(calls.workers.slice(0, 3)).toEqual([
      { worker: 'browser', a: 32, b: 9 },
      { worker: 'coder', a: 9, b: 30 },
      { worker: 'form_flow', a: 7, b: 17 },
    ])
    const html = renderToStaticMarkup(
      <RunComparison a={a} b={b} spec={null} onSwap={() => undefined} />,
    )
    expect(html).toContain('root session only; A also ran 3 child sessions')
    expect(html).toContain('A · Sub-agents')
    expect(html).toContain('B · Sub-agents')
    expect(html).toContain('None. The run stayed in one session.')
    expect(html).toContain('aria-label="Swap A and B"')
    expect(html).toContain(
      `href="#/ext/harness-e2e/execution/${a.observation.plan_execution_id}"`,
    )
  })
})

function never(): never {
  throw new Error('fixture is missing a value')
}
