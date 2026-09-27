import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { hashForRunComparison, routeFromHash } from '@/hooks/use-hash-route'
import {
  comparability,
  criteriaChanges,
  metricRows,
  RunComparison,
  side,
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
    expect(rows['Criteria met'].format(8)).toBe('8/9')
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
