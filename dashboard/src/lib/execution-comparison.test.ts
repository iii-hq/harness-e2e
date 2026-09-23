import { describe, expect, it } from 'vitest'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import {
  automaticExclusions,
  compareExecutions,
  compareRuns,
  comparisonMarkdown,
} from '@/lib/execution-comparison'
import {
  execution,
  imported,
  local,
  reportRun,
} from '@/test-fixtures/execution-comparison'
import ledger from '@/test-fixtures/rc-ledger-runs.json'

type LedgerRow = (typeof ledger)['execution-4'][number]

/** A recorded Release Control execution as reports: one per run, the run's
 *  ledger columns written back into the fields the ledger reads them from. */
function ledgerExecution(id: keyof typeof ledger): DashboardExecutionDetail {
  return {
    id,
    status: 'passed',
    subjects: [],
    reports: (ledger[id] as LedgerRow[]).map((row) => ({
      subject_id: 'subject',
      scenario_id: row.scenario_id,
      available: true,
      report: {
        scenarios: [
          {
            scenario_id: row.scenario_id,
            case: { seed: row.seed, inputs_sha256: row.inputs_sha256 },
            runs: [
              {
                run_id: row.run_id,
                technical: row.technical,
                completion: row.completion,
                score: row.score,
                wall_time_ms: row.wall_time_ms,
                efficiency: {
                  total_tokens: row.total_tokens,
                  root_turns: row.turns,
                  function_calls: row.function_calls,
                  technical_attempts: row.attempts_complete ? 1 : 2,
                },
                metrics: {
                  complete: true,
                  totals: { cache_read_tokens: 0, cache_write_tokens: 0 },
                },
                cost: { subject_usd: row.cost_subject_usd },
              },
            ],
          },
        ],
      },
    })),
  } as unknown as DashboardExecutionDetail
}

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

  it('names the gaps in the recorded executions and consolidates them as Release Control does', () => {
    // Release Control's own output for these ledgers: `automaticExclusions`
    // and `compareFairly(...).consolidated` run on api/tests/fixtures/ledger.
    const four = ledgerExecution('execution-4')
    const five = ledgerExecution('execution-5')
    const six = ledgerExecution('execution-6')
    const reasons = (
      left: DashboardExecutionDetail,
      right: DashboardExecutionDetail,
    ) => [
      ...automaticExclusions(compareRuns(left), compareRuns(right)).values(),
    ]
    expect(reasons(four, five)).toEqual([
      { scenario_id: 'minimal_path', reason: 'redefined', sides: ['a', 'b'] },
      {
        scenario_id: 'persistent_state',
        reason: 'redefined',
        sides: ['a', 'b'],
      },
      {
        scenario_id: 'swe_cache_invalidation',
        reason: 'technical_invalid',
        sides: ['a', 'b'],
      },
      {
        scenario_id: 'swe_config_isolation',
        reason: 'technical_invalid',
        sides: ['a', 'b'],
      },
      { scenario_id: 'swe_replay_recovery', reason: 'missing', sides: ['b'] },
      {
        scenario_id: 'database_migration_recovery',
        reason: 'missing',
        sides: ['a'],
      },
    ])
    expect(reasons(five, six)).toEqual([
      { scenario_id: 'persistent_state', reason: 'undetermined', sides: ['b'] },
      {
        scenario_id: 'swe_cache_invalidation',
        reason: 'missing',
        sides: ['b'],
      },
      { scenario_id: 'swe_config_isolation', reason: 'missing', sides: ['b'] },
      {
        scenario_id: 'tool_contract_recovery',
        reason: 'technical_invalid',
        sides: ['b'],
      },
    ])
    const figures = (comparison: ReturnType<typeof compareExecutions>) =>
      Object.fromEntries(
        comparison.totals.map((metric) => [
          metric.id,
          [metric.baseline, metric.candidate],
        ]),
      )
    const fourFive = figures(compareExecutions(four, five))
    expect(fourFive.completed).toEqual([6, 5])
    expect(fourFive.score[0]).toBeCloseTo(46.666666666666664)
    expect(fourFive.score[1]).toBe(30)
    expect(fourFive.tokens).toEqual([181135, 181974])
    expect(fourFive.turns).toEqual([105, 110])
    expect(fourFive.duration).toEqual([609.092, 594.593])
    expect(fourFive.cost[0]).toBeCloseTo(0.0408912672)
    expect(fourFive.cost[1]).toBeCloseTo(0.0414897784)
    expect(fourFive.function_calls).toEqual([131, 125])
    const fiveSix = figures(compareExecutions(five, six))
    expect(fiveSix.completed).toEqual([7, 7])
    expect(fiveSix.score[0]).toBeCloseTo(42.142857142857146)
    expect(fiveSix.score[1]).toBeCloseTo(63.57142857142857)
    expect(fiveSix.tokens).toEqual([178204, 189979])
    expect(fiveSix.turns).toEqual([103, 105])
    expect(fiveSix.duration).toEqual([678.723, 764.022])
    expect(fiveSix.function_calls).toEqual([120, 124])
  })
})

describe('pairing', () => {
  it('pairs by scenario, case seed and the runner repetition, never by case id', () => {
    const a = execution('a', [{ scenario: 'minimal_path', seed: 7 }])
    const b = execution('b', [{ scenario: 'minimal_path', seed: 7 }])
    // Before 0.11 the case id carried the scenario version.
    const scenario = a.reports[0].report?.scenarios[0]
    if (scenario) scenario.case_id = 'minimal_path:v2:seed-0000000000000007'
    expect(compareRuns(a)[0].slotId).toBe(compareRuns(b)[0].slotId)
    expect(compareExecutions(a, b).exclusions).toEqual([])
    const reseeded = execution('b', [{ scenario: 'minimal_path', seed: 8 }])
    expect(compareRuns(reseeded)[0].slotId).not.toBe(compareRuns(a)[0].slotId)
  })

  it('reads the definition from case.inputs_sha256, not the behavior digest', () => {
    const a = execution('a', [{ scenario: 'minimal_path' }])
    const b = execution('b', [{ scenario: 'minimal_path' }])
    const scenario = b.reports[0].report?.scenarios[0]
    if (scenario) scenario.behavior_sha256 = 'sha256:another-behavior'
    expect(compareExecutions(a, b).exclusions).toEqual([])
    const redefined = execution('b', [
      { scenario: 'minimal_path', definition: 'other-inputs' },
    ])
    expect(compareExecutions(a, redefined).exclusions).toEqual([
      {
        scenario_id: 'minimal_path',
        reason: 'redefined',
        sides: ['a', 'b'],
        applied: true,
      },
    ])
  })

  it('keeps plan rounds on the runner slot and repetitions apart', () => {
    // A plan runs each round as its own native run, repetition 0, like
    // Release Control's campaigns; a plain run numbers its repetitions.
    const rounds = execution('rounds', [
      { scenario: 'minimal_path', round: 1 },
      { scenario: 'minimal_path', round: 2 },
    ])
    const plain = execution('plain', [{ scenario: 'minimal_path' }])
    const report = plain.reports[0].report
    if (report)
      report.scenarios[0].runs.push(
        reportRun('plain-1', { scenario: 'minimal_path' }) as never,
      )
    const [first, second] = compareRuns(rounds)
    const [zero, one] = compareRuns(plain)
    expect(first.slotId).toBe(second.slotId)
    expect(first.slotId).toBe(zero.slotId)
    expect(one.slotId).not.toBe(zero.slotId)
    // A round measuring another definition redefines the shared slot.
    const moved = execution('rounds', [
      { scenario: 'minimal_path', round: 1 },
      { scenario: 'minimal_path', round: 2, definition: 'other-inputs' },
    ])
    expect([
      ...automaticExclusions(compareRuns(moved), compareRuns(plain)).values(),
    ]).toEqual([
      { scenario_id: 'minimal_path', reason: 'redefined', sides: ['a', 'b'] },
    ])
  })

  it('keeps an unavailable report out of the runs and narrows the plan once a test is out', () => {
    const a = execution('a', [
      { scenario: 'minimal_path', round: 1 },
      { scenario: 'persistent_state', round: 1 },
    ])
    a.reports.push({
      subject_id: 'subject',
      scenario_id: 'minimal_path',
      round: 2,
      available: false,
      report: undefined,
    } as never)
    const b = execution('b', [
      { scenario: 'minimal_path', round: 1 },
      { scenario: 'persistent_state', round: 1 },
    ])
    expect(compareRuns(a)).toHaveLength(2)
    // Nothing out: A planned three runs and observed two.
    const whole = compareExecutions(a, b)
    expect(whole.exclusions).toEqual([])
    expect(whole.totals.find((metric) => metric.id === 'score')).toMatchObject({
      baseline: null,
      candidate: 80,
      delta: null,
    })
    expect(whole.totals.find((metric) => metric.id === 'tokens')).toMatchObject(
      {
        baseline: 220,
        partial: { baseline: true, candidate: false },
        delta: null,
      },
    )
    // A test out of the totals narrows the plan to the runs kept.
    const narrowed = compareExecutions(a, b, { exclude: ['persistent_state'] })
    expect(
      narrowed.totals.find((metric) => metric.id === 'score'),
    ).toMatchObject({ baseline: 80, candidate: 80, delta: 0 })
  })

  it('takes a scenario neither side observed out as missing on both', () => {
    const a = execution('a', [{ scenario: 'minimal_path' }])
    const b = execution('b', [{ scenario: 'minimal_path' }])
    for (const detail of [a, b])
      detail.reports.push({
        subject_id: 'subject',
        scenario_id: 'timer_wake',
        available: false,
      } as never)
    const comparison = compareExecutions(a, b)
    expect(comparison.exclusions).toEqual([
      {
        scenario_id: 'timer_wake',
        reason: 'missing',
        sides: ['a', 'b'],
        applied: true,
      },
    ])
    expect(
      comparison.totals.find((metric) => metric.id === 'score'),
    ).toMatchObject({ baseline: 80, candidate: 80 })
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
    ).toMatchObject({ baseline: 2310, candidate: 2090, delta: -220 })
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
    a.label = 'smoke\r\nnightly'
    // Scenarios that moved nowhere: one counted, one the reader left out.
    for (const detail of [a, b])
      for (const scenario of ['timer_wake', 'quiet_path']) {
        detail.reports.push(
          ...execution(detail.id, [{ scenario, score: 70 }]).reports,
        )
        const run = detail.reports.at(-1)?.report?.scenarios[0].runs[0]
        if (run) run.run_id = `${detail.id}-${scenario}`
      }
    expect(
      comparisonMarkdown(compareExecutions(a, b, { exclude: ['quiet_path'] })),
    ).toBe(
      [
        '### deepseek/flash · no profile',
        '',
        'A (base): smoke nightly · GitHub run 35823421664 · RC 366030b3 · harness 0.9.3',
        'B: smoke · local · harness 0.9.3 · llm-router, session-manager @ a1b2c3d (uncommitted changes)',
        '',
        'What changed:',
        '- llm-router: package 1.2.0 → path 1.3.0-dev @ a1b2c3d (uncommitted changes)',
        '- session-manager: absent → path 0.4.0 @ a1b2c3d (uncommitted changes)',
        '',
        '| Metric | A | B | Difference |',
        '| --- | --- | --- | --- |',
        '| Score | 84 | 75.3 | -8.7 pts |',
        '| Completed tasks | 3 | 3 | No change |',
        '| Observed / planned runs | 100% | 100% | No change |',
        '| Technically invalid runs | 0 | 0 | No change |',
        '| Tokens (incl. cache) | 2.42K | 2.2K | -220 (-9.1%) |',
        '| Tokens per completed task | 807 | 733 | -73.3 (-9.1%) |',
        '| Subject cost | $0.0300 | $0.0300 | No change |',
        '| Total run duration | 6.0s | 6.0s | No change |',
        '| Total turns | 9 | 9 | No change |',
        '| Function calls | 9 | 9 | No change |',
        '| Function call errors | 0 | 0 | No change |',
        '',
        '| Scenario | Score A → B | Criteria that changed |',
        '| --- | --- | --- |',
        '| minimal_path | 82 → 94 | + answer cites the source |',
        '| persistent_state | 100 → 62 | − state read after restart |',
        '| shell_coder_sandbox | — → 40 | — |',
        '',
        'No difference: timer_wake.',
        '',
        'Out of the totals: quiet_path (left out by the reader), shell_coder_sandbox (technical_invalid in A).',
        '',
      ].join('\n'),
    )
  })

  it('states no profile difference when a side did not record its parameters', () => {
    const b = local()
    b.parameters = null
    delete b.plan_execution
    const comparison = compareExecutions(imported(), b)
    expect(comparison.b.profile).toBeNull()
    expect(comparison.parameters.map((change) => change.field)).not.toContain(
      'profile',
    )
    expect(comparisonMarkdown(comparison).split('\n')[0]).toBe(
      '### smoke · deepseek/flash',
    )
  })
})
