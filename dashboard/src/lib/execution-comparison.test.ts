import { describe, expect, it } from 'vitest'
import {
  automaticExclusions,
  type CompareRun,
  compareExecutions,
  compareRuns,
  comparisonMarkdown,
} from '@/lib/execution-comparison'
import {
  execution,
  imported,
  local,
} from '@/test-fixtures/execution-comparison'
import ledger from '@/test-fixtures/rc-ledger-runs.json'

function total(
  comparison: ReturnType<typeof compareExecutions>,
  id: string,
  side: 'baseline' | 'candidate' = 'candidate',
) {
  return comparison.totals.find((metric) => metric.id === id)?.[side]
}

// The `fair comparison` cases of Release Control's
// `api/tests/version-compare.test.ts`, run through this projection.
describe('fair comparison', () => {
  it('takes out every gap the subject did not cause, from both sides', () => {
    const a = execution('a', [
      {},
      {
        technical: 'technical_invalid',
        completion: 'undetermined',
        score: null,
      },
      {},
      {},
      {},
      { definition: 'changed' },
    ])
    const b = execution('b', [
      {},
      {},
      { completion: 'undetermined', score: null },
      { score: null },
      {},
      {},
    ])
    b.reports = b.reports.filter((record) => record.scenario_id !== 'test_4')

    expect([
      ...automaticExclusions(compareRuns(a), compareRuns(b)).values(),
    ]).toEqual([
      { scenario_id: 'test_1', reason: 'technical_invalid', sides: ['a'] },
      { scenario_id: 'test_2', reason: 'undetermined', sides: ['b'] },
      { scenario_id: 'test_3', reason: 'no_score', sides: ['b'] },
      { scenario_id: 'test_4', reason: 'missing', sides: ['b'] },
      { scenario_id: 'test_5', reason: 'redefined', sides: ['a', 'b'] },
    ])
  })

  it('keeps what the subject did: a zero, a low score and an incomplete task stay in', () => {
    const a = execution('a', [
      { score: 0 },
      { completion: 'task_incomplete', score: 35 },
      { score: 12 },
    ])
    const b = execution('b', [{ score: 90 }, { score: 90 }, { score: 90 }])
    expect(automaticExclusions(compareRuns(a), compareRuns(b)).size).toBe(0)
    const comparison = compareExecutions(a, b)
    expect(comparison.exclusions).toEqual([])
    expect(comparison.scenarios.every((scenario) => scenario.counted)).toBe(
      true,
    )
  })

  it('restores the score an infrastructure failure used to veto', () => {
    const a = execution('a', [{ score: 60 }, { score: 80 }, { score: 100 }])
    const b = execution('b', [
      { score: 70 },
      { technical: 'technical_invalid', score: null },
      { score: 90 },
    ])
    // Every test in: one unmeasured run hides the whole side.
    expect(
      total(compareExecutions(a, b, { include: ['test_1'] }), 'score'),
    ).toBeNull()

    const fair = compareExecutions(a, b)
    expect(fair.exclusions).toEqual([
      {
        scenario_id: 'test_1',
        reason: 'technical_invalid',
        sides: ['b'],
        applied: true,
      },
    ])
    // Out of both sides, so A is not scored over work B did not do.
    expect(total(fair, 'score', 'baseline')).toBe(80)
    expect(total(fair, 'score')).toBe(80)
  })

  it('lets the reader bring an automatic exclusion back and take more out', () => {
    const a = execution('a', [{}, {}, {}])
    const b = execution('b', [
      {},
      { technical: 'technical_invalid', score: null },
      {},
    ])
    const back = compareExecutions(a, b, { include: ['test_1'] })
    expect(back.exclusions).toEqual([
      {
        scenario_id: 'test_1',
        reason: 'technical_invalid',
        sides: ['b'],
        applied: false,
      },
    ])
    expect(back.scenarios.every((scenario) => scenario.counted)).toBe(true)
    expect(total(back, 'score')).toBeNull()

    const narrower = compareExecutions(a, b, { exclude: ['test_2'] })
    expect(
      narrower.scenarios
        .filter((scenario) => scenario.counted)
        .map((scenario) => scenario.id),
    ).toEqual(['test_0'])
  })

  it('names the gaps in the recorded executions', () => {
    const runs = ledger as Record<string, CompareRun[]>
    const reasons = (left: CompareRun[], right: CompareRun[]) =>
      [...automaticExclusions(left, right).values()].map(({ reason }) => reason)
    // The fixtures' two moved definitions (4 → 5), and the invalid and unplanned runs of 5 → 6.
    expect(reasons(runs['execution-4'], runs['execution-5'])).toContain(
      'redefined',
    )
    expect(reasons(runs['execution-5'], runs['execution-6'])).toEqual(
      expect.arrayContaining(['missing', 'undetermined']),
    )
  })
})

describe('comparing two executions', () => {
  it('compares two executions that share no plan and names what differs', () => {
    const comparison = compareExecutions(imported(), local())

    expect(comparison.a.origin).toBe(
      'GitHub run 35823421664 · RC 366030b3 · harness 0.9.3',
    )
    expect(comparison.b.origin).toBe(
      'local · harness 0.9.3 · llm-router, session-manager @ a1b2c3d (uncommitted changes)',
    )
    expect(comparison.parameters).toEqual([])
    // The requested version is not a difference; source, observed version,
    // commit and dirty state are.
    expect(comparison.stack).toEqual([
      {
        field: 'llm-router',
        a: 'package 1.2.0',
        b: 'path 1.3.0-dev @ a1b2c3d (uncommitted changes)',
      },
      {
        field: 'session-manager',
        a: 'absent',
        b: 'path 0.4.0 @ a1b2c3d (uncommitted changes)',
      },
    ])
    expect(comparison.exclusions).toEqual([
      {
        scenario_id: 'shell_coder_sandbox',
        reason: 'technical_invalid',
        sides: ['a'],
        applied: true,
      },
    ])
    // The excluded scenario keeps its row and its values.
    const sandbox = comparison.scenarios.find(
      (scenario) => scenario.id === 'shell_coder_sandbox',
    )
    expect(sandbox).toMatchObject({ counted: false })
    expect(
      sandbox?.metrics.find((metric) => metric.id === 'score'),
    ).toMatchObject({ baseline: null, candidate: 40 })
    // Totals over the two scenarios both sides measured.
    expect(
      comparison.totals.find((metric) => metric.id === 'score'),
    ).toMatchObject({ baseline: 91, candidate: 78, delta: -13 })
    expect(
      comparison.totals.find((metric) => metric.id === 'tokens'),
    ).toMatchObject({ baseline: 2100, candidate: 1900, delta: -200 })
    expect(
      comparison.totals.find((metric) => metric.id === 'completed'),
    ).toMatchObject({ baseline: 2, candidate: 2, delta: 0 })
    expect(comparison).not.toHaveProperty('verdict')
  })

  it('reports only the criteria whose points moved, with each side’s reason', () => {
    const comparison = compareExecutions(imported(), local())
    const byId = Object.fromEntries(
      comparison.scenarios.map((scenario) => [scenario.id, scenario.criteria]),
    )
    expect(byId.minimal_path).toEqual([
      {
        key: 'cites_source:20',
        label: 'answer cites the source',
        possible: 20,
        a: 8,
        b: 20,
        delta: 12,
        reasons: { a: ['no source named'], b: ['names the source'] },
      },
    ])
    expect(byId.persistent_state).toMatchObject([
      {
        key: 'state_after_restart:50',
        delta: -38,
        reasons: { a: ['read back'], b: ['state lost on restart'] },
      },
    ])
  })

  it('inverts the base without changing what was observed', () => {
    const forward = compareExecutions(imported(), local())
    const backward = compareExecutions(local(), imported())
    expect(backward.a.id).toBe('local-b')
    expect(
      backward.totals.find((metric) => metric.id === 'score'),
    ).toMatchObject({ baseline: 78, candidate: 91, delta: 13 })
    expect(backward.exclusions[0].sides).toEqual(['b'])
    expect(backward.stack[0]).toEqual({
      field: 'llm-router',
      a: forward.stack[0].b,
      b: forward.stack[0].a,
    })
  })

  it('names parameter changes and a side whose stack was not recorded', () => {
    const b = local()
    b.parameters = {
      scenarios: ['minimal_path', 'timer_wake'],
      runs: 3,
      technical_retries: 1,
      seed: null,
      model: 'pro',
      provider: 'deepseek',
      agent: 'coder',
    }
    delete b.plan_execution
    b.stack = { mode: 'source', versions: null, lock_digest: null }
    const comparison = compareExecutions(imported(), b)
    expect(comparison.parameters).toEqual([
      {
        field: 'scenarios',
        a: '3 scenarios · only here: persistent_state, shell_coder_sandbox',
        b: '2 scenarios · only here: timer_wake',
      },
      { field: 'runs', a: '1', b: '3' },
      { field: 'model', a: 'flash', b: 'pro' },
      { field: 'profile', a: 'no profile', b: 'coder' },
    ])
    expect(comparison.stackRecorded).toEqual({ a: true, b: false })
    expect(comparison.stack).toEqual([])
  })
})

describe('comparison summary', () => {
  it('writes a pull-request-ready markdown summary without a verdict', () => {
    const a = imported()
    const b = local()
    // A scenario that moved nowhere lands on the "no difference" line.
    for (const detail of [a, b]) {
      detail.reports.push(
        ...execution(detail.id, [{ scenario: 'timer_wake', score: 70 }])
          .reports,
      )
      const record = detail.reports.at(-1)
      const run = record?.report?.scenarios[0].runs[0]
      if (run) run.run_id = `${detail.id}-timer`
    }
    expect(comparisonMarkdown(compareExecutions(a, b))).toBe(
      [
        '### smoke · deepseek/flash · no profile',
        '',
        'A (base): smoke · GitHub run 35823421664 · RC 366030b3 · harness 0.9.3',
        'B: smoke · local · harness 0.9.3 · llm-router, session-manager @ a1b2c3d (uncommitted changes)',
        '',
        'What changed:',
        '- llm-router: package 1.2.0 → path 1.3.0-dev @ a1b2c3d (uncommitted changes)',
        '- session-manager: absent → path 0.4.0 @ a1b2c3d (uncommitted changes)',
        '',
        '| Metric | A | B | Difference |',
        '| --- | --- | --- | --- |',
        '| Mean score | 84 | 75.3 | -8.7 pts |',
        '| Completed | 3 | 3 | No change |',
        '| Coverage | 100% | 100% | No change |',
        '| Technical failures | 0 | 0 | No change |',
        '| Tokens | 2.2K | 2K | -200 (-9.1%) |',
        '| Tokens per completion | 733 | 667 | -66.7 (-9.1%) |',
        '| Cost | $0.0300 | $0.0300 | No change |',
        '| Duration | 6.0s | 6.0s | No change |',
        '| Turns | 6 | 6 | No change |',
        '| Function calls | 9 | 9 | No change |',
        '| Function errors | 0 | 0 | No change |',
        '',
        '| Scenario | Score A → B | Criteria that changed |',
        '| --- | --- | --- |',
        '| minimal_path | 82 → 94 | + answer cites the source |',
        '| persistent_state | 100 → 62 | − state read after restart |',
        '| shell_coder_sandbox | — → 40 | — |',
        '',
        'No difference: timer_wake.',
        '',
        'Out of the totals: shell_coder_sandbox (technical_invalid in A).',
        '',
      ].join('\n'),
    )
  })
})
