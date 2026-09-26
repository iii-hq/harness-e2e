import { describe, expect, it } from 'vitest'
import type {
  DashboardExecutionDetail,
  ExecutionParameters,
} from '@/lib/dashboard-data-source'
import {
  automaticExclusions,
  comparedValue,
  compareExecutions,
  compareRuns,
  comparisonMarkdown,
  exclusionPhrase,
  rerunPhrase,
  runnerWarning,
  scenarioScore,
  stackSummary,
  yourCodeWorkers,
} from '@/lib/execution-comparison'
import { buildExecutionMetrics } from '@/lib/execution-metrics'
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
    // Cost is the reported total the execution page sums, not Release
    // Control's subject cost; this ledger carries only the latter.
    expect(fourFive.cost).toEqual([null, null])
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
        baseline: 200,
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
      'GitHub run 35823421664 · RC 366030b3 · runner 0.9.3',
    )
    expect(comparison.b.origin).toBe(
      'local · runner 0.9.3 · llm-router, session-manager @ a1b2c3d (uncommitted changes)',
    )
    expect(comparison.parameters).toEqual([])
    // A stack that pinned harness to a commit ran that commit, whatever
    // version its Cargo manifest reports.
    const pinned = imported()
    const workers = pinned.plan_execution?.stack ?? []
    workers.push({
      name: 'harness',
      source: 'package',
      requested: null,
      observed: '1.8.37-rc.1',
      commit: '3f2a9c1dddddddddddddddddddddddddddddddd',
      dirty: null,
    })
    expect(compareExecutions(pinned, local()).a.origin).toBe(
      'GitHub run 35823421664 · RC 366030b3 · harness @3f2a9c1 · runner 0.9.3',
    )
    // Workers from a checkout are one line per commit; the requested version
    // is never a difference.
    expect(comparison.stack).toEqual({
      recorded: { a: true, b: true },
      yourCode: [
        {
          side: 'b',
          commit: 'a1b2c3d',
          dirty: true,
          workers: ['llm-router', 'session-manager'],
          onlyHere: ['session-manager'],
        },
      ],
      versions: [],
      // session-manager is your code: one line, marked, not listed again.
      onlyA: [],
      onlyB: [],
    })
    expect(yourCodeWorkers(comparison.stack.yourCode[0])).toBe(
      'llm-router, session-manager (only in B)',
    )
    expect(stackSummary(comparison.stack)).toBe(
      '2 workers from your code @a1b2c3d (uncommitted changes) · 1 only in B',
    )
    expect(runnerWarning(comparison.runner)).toBeNull()
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
    ).toMatchObject({
      label: 'Total tokens',
      baseline: 2100,
      candidate: 1900,
      delta: -200,
    })
    expect(
      comparison.totals.find((metric) => metric.id === 'cache_read'),
    ).toMatchObject({ label: 'Cache read', baseline: 210, candidate: 190 })
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
    expect(backward.stack.yourCode).toEqual(
      forward.stack.yourCode.map((group) => ({ ...group, side: 'a' })),
    )
    expect(backward.stack.onlyA).toEqual(forward.stack.onlyB)
  })

  it('names parameter changes and a side whose stack was not recorded', () => {
    const a = imported()
    a.parameters = {
      ...(a.parameters as ExecutionParameters),
      suite: {
        id: 'regression',
        label: 'Regression',
        sha256: 'sha256:0123456789abcdef0123',
      },
    }
    const b = local()
    b.parameters = {
      suite: { label: '', sha256: 'sha256:fedcba9876543210fedc' },
      scenarios: ['minimal_path', 'timer_wake'],
      runs: 3,
      technical_retries: 1,
      // A model id that carries its provider is not prefixed again.
      model: 'deepseek/pro',
      provider: 'deepseek',
      agent: 'coder',
    }
    delete b.plan_execution
    b.stack = { mode: 'source', versions: null, lock_digest: null }
    const comparison = compareExecutions(a, b)
    expect(comparison.parameters).toEqual([
      // The suite by name and digest; scenarios ticked by hand are unnamed.
      {
        field: 'suite',
        a: 'Regression · 0123456789ab',
        b: 'unnamed suite · fedcba987654',
      },
      {
        field: 'scenarios',
        a: '3 scenarios · only here: persistent_state, shell_coder_sandbox',
        b: '2 scenarios · only here: timer_wake',
      },
      { field: 'runs', a: '1', b: '3' },
      { field: 'model', a: 'flash', b: 'deepseek/pro' },
      { field: 'profile', a: 'no profile', b: 'coder' },
    ])
    expect(comparison.b.subject).toBe('deepseek/pro')
    expect(comparison.stack.recorded).toEqual({ a: true, b: false })
    expect(stackSummary(comparison.stack)).toBe('no stack recorded for B')
  })

  it('says why a scenario is out of the totals, in the runs’ own words', () => {
    const b = local()
    b.reports = b.reports.filter(
      (record) => record.scenario_id !== 'persistent_state',
    )
    const comparison = compareExecutions(imported(), b)
    const byId = Object.fromEntries(
      comparison.scenarios.map((scenario) => [scenario.id, scenario]),
    )
    const sandbox = byId.shell_coder_sandbox
    expect(exclusionPhrase(sandbox)).toBe(
      'technical_invalid in A: infrastructure_error — scenario setup failed: database never became ready',
    )
    // The run exists: its state stands where a score would.
    expect(scenarioScore(sandbox, 'a')).toBe('infrastructure_error')
    expect(scenarioScore(sandbox, 'b')).toBe('40')
    expect(exclusionPhrase(byId.persistent_state)).toBe('no run in B')
    expect(scenarioScore(byId.persistent_state, 'b')).toBe('no run')
  })

  it('counts invalid runs and coverage over every run and says how many are out', () => {
    const comparison = compareExecutions(imported(), local())
    const invalid = comparison.totals.find(
      (metric) => metric.id === 'technical_failures',
    )
    expect(invalid).toMatchObject({
      baseline: 1,
      candidate: 0,
      delta: -1,
      outside: { baseline: 1, candidate: 0 },
    })
    if (!invalid) throw new Error('technical_failures')
    expect(comparedValue(invalid, 'baseline')).toBe(
      '1 (1 run out of the totals)',
    )
    expect(
      comparison.totals.find((metric) => metric.id === 'coverage'),
    ).toMatchObject({
      baseline: 100,
      candidate: 100,
      outside: { baseline: 1, candidate: 1 },
    })
  })

  it('counts tokens and cost as the execution page does, per scenario too', () => {
    const b = local()
    const run = b.reports[0].report?.scenarios[0].runs[0]
    if (run) {
      run.efficiency = { ...run.efficiency, total_tokens: 22_928 }
      run.metrics = {
        complete: true,
        totals: { cache_read_tokens: 181_000, cache_write_tokens: 400 },
      } as never
    }
    const comparison = compareExecutions(imported(), b)
    const minimal = comparison.scenarios.find(
      (scenario) => scenario.id === 'minimal_path',
    )
    const figure = (id: string) =>
      minimal?.metrics.find((metric) => metric.id === id)?.candidate
    const page = buildExecutionMetrics({
      ...b,
      reports: b.reports.filter(
        (record) => record.scenario_id === 'minimal_path',
      ),
    })
    expect(figure('tokens')).toBe(22_928)
    expect(figure('tokens')).toBe(page.subjectTokens.total)
    expect(figure('cache_read')).toBe(181_000)
    expect(figure('cache_write')).toBe(400)
    // Reported cost: the run's total, not its subject share.
    expect(figure('cost')).toBe(0.012)
    expect(figure('cost')).toBe(page.cost.total)
    expect(minimal?.metrics.find((metric) => metric.id === 'cost')?.label).toBe(
      'Reported cost',
    )
    expect(comparison.totals.map((metric) => metric.label)).not.toContain(
      'Tokens (incl. cache)',
    )
  })

  it('warns when the runners differ and names the definitions that moved', () => {
    const a = imported()
    const b = local()
    const runnerOf = (detail: typeof a, version: string) => {
      for (const stack of [detail.stack, detail.plan_execution?.stack])
        for (const worker of Array.isArray(stack) ? stack : [])
          if (worker.name === 'harness-e2e') worker.observed = version
    }
    runnerOf(a, '0.11.24')
    runnerOf(b, '0.11.27')
    const scenario = b.reports[1].report?.scenarios[0]
    if (scenario) scenario.behavior_sha256 = 'sha256:rescored'
    const comparison = compareExecutions(a, b)
    expect(comparison.runner).toEqual({
      a: '0.11.24',
      b: '0.11.27',
      differs: true,
      definitionsChanged: ['persistent_state'],
    })
    // Same inputs: the definition moved with the runner, not the case.
    expect(comparison.exclusions.map((entry) => entry.reason)).not.toContain(
      'redefined',
    )
    expect(runnerWarning(comparison.runner)).toBe(
      'Different runners: 0.11.24 → 0.11.27 — scenario definitions and scoring may differ. Definitions changed: persistent_state.',
    )
    // The runner is not listed again as a version difference.
    expect(comparison.stack.versions).toEqual([])
  })

  it('groups version differences and workers on one side only', () => {
    const a = imported()
    const b = local()
    for (const detail of [a, b])
      detail.plan_execution?.stack.push(
        {
          name: 'state',
          source: 'package',
          requested: null,
          observed: detail === a ? '0.22.3' : '0.22.17',
          commit: null,
          dirty: null,
        },
        {
          name: 'queue',
          source: 'package',
          requested: null,
          observed: '1.0.0',
          commit: null,
          dirty: null,
        },
      )
    a.plan_execution?.stack.push({
      name: 'legacy',
      source: 'package',
      requested: null,
      observed: '0.1.0',
      commit: null,
      dirty: null,
    })
    const { stack } = compareExecutions(a, b)
    expect(stack.versions).toEqual([
      { field: 'state', a: '0.22.3', b: '0.22.17' },
    ])
    expect(stack.onlyA).toEqual(['legacy'])
    expect(stack.onlyB).toEqual([])
    expect(stackSummary(stack)).toBe(
      '2 workers from your code @a1b2c3d (uncommitted changes) · 1 version difference · 1 only in A · 1 only in B',
    )
  })

  it('compares a worker built from a commit by the commit, not its Cargo version', () => {
    const harness = (detail: DashboardExecutionDetail, commit: string | null) =>
      detail.plan_execution?.stack.push({
        name: 'harness',
        source: 'package',
        requested: null,
        observed: '1.8.8-rc.3',
        commit,
        dirty: null,
      })
    const a = imported()
    const b = local()
    harness(a, '3f2a9c1dddddddddddddddddddddddddddddddd')
    harness(b, null)
    expect(compareExecutions(a, b).stack.versions).toEqual([
      { field: 'harness', a: '@3f2a9c1', b: '1.8.8-rc.3' },
    ])
    const same = local()
    harness(same, '3f2a9c1dddddddddddddddddddddddddddddddd')
    expect(compareExecutions(a, same).stack.versions).toEqual([])
  })
})

describe('comparison summary', () => {
  it('writes a pull-request-ready markdown summary without a verdict', () => {
    const a = imported()
    const b = local()
    a.label = 'smoke\r\nnightly'
    for (const [detail, version] of [
      [a, '0.11.24'],
      [b, '0.11.27'],
    ] as const)
      for (const worker of detail.plan_execution?.stack ?? [])
        if (worker.name === 'harness-e2e') worker.observed = version
    const persistent = b.reports[1].report?.scenarios[0]
    if (persistent) persistent.behavior_sha256 = 'sha256:rescored'
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
        'A (base): smoke nightly · GitHub run 35823421664 · RC 366030b3 · runner 0.11.24',
        'B: smoke · local · runner 0.11.27 · llm-router, session-manager @ a1b2c3d (uncommitted changes)',
        '',
        '> **Different runners: 0.11.24 → 0.11.27 — scenario definitions and scoring may differ. Definitions changed: persistent_state.**',
        '',
        'Stack: 2 workers from your code @a1b2c3d (uncommitted changes) · 1 only in B',
        '- Your code in B @a1b2c3d (uncommitted changes): llm-router, session-manager (only in B)',
        '',
        '| Metric | A | B | Difference |',
        '| --- | --- | --- | --- |',
        '| Score | 84 | 75.3 | -8.7 pts |',
        '| Completed tasks | 3 | 3 | No change |',
        '| Observed / planned runs | 100% (2 runs out of the totals) | 100% (2 runs out of the totals) | No change |',
        '| Technically invalid runs | 1 (1 run out of the totals) | 0 | -1 (-100.0%) |',
        '| Total tokens | 2.2K | 2K | -200 (-9.1%) |',
        '| Cache read | 220 | 200 | -20 (-9.1%) |',
        '| Cache written | 0 | 0 | No change |',
        '| Tokens per completed task | 733 | 667 | -66.7 (-9.1%) |',
        '| Reported cost | $0.0360 | $0.0360 | No change |',
        '| Total run duration | 6.0s | 6.0s | No change |',
        '| Total turns | 9 | 9 | No change |',
        '| Function calls | 9 | 9 | No change |',
        '| Function call errors | 0 | 0 | No change |',
        '',
        '| Scenario | Score A → B | Criteria that changed |',
        '| --- | --- | --- |',
        '| minimal_path | 82 → 94 | + answer cites the source |',
        '| persistent_state | 100 → 62 | − state read after restart |',
        '| shell_coder_sandbox | infrastructure_error → 40 | — |',
        '',
        'No difference: timer_wake.',
        '',
        'Out of the totals:',
        '- quiet_path: left out by the reader',
        '- shell_coder_sandbox: technical_invalid in A: infrastructure_error — scenario setup failed: database never became ready',
        '',
      ].join('\n'),
    )
  })

  it('marks a scenario that ran again and compares only its last attempt', () => {
    const a = imported()
    const b = local()
    const before = compareExecutions(a, b)
    // B ran minimal_path twice more: its earlier attempts scored far lower.
    const slot = (previous: number) => ({
      round: 1,
      scenario_id: 'minimal_path',
      execution_id: 'current',
      previous_attempts: Array.from({ length: previous }, (_, index) => ({
        execution_id: `attempt-${index + 1}`,
        error: null,
      })),
    })
    Object.assign(b.plan_execution ?? {}, {
      slots: [slot(2), { ...slot(0), scenario_id: 'persistent_state' }],
    })
    b.previous_reports = execution('attempt-1', [
      { scenario: 'minimal_path', score: 5, tokens: 9000 },
    ]).reports
    const comparison = compareExecutions(a, b)
    const scenario = comparison.scenarios.find(
      (entry) => entry.id === 'minimal_path',
    )
    expect(scenario?.sides.b.reruns).toBe(2)
    expect(scenario && rerunPhrase(scenario)).toBe('rerun ×2 in B')
    expect(scenarioScore(scenario ?? before.scenarios[0], 'b')).toBe('94')
    expect(comparison.totals).toEqual(before.totals)
    expect(
      comparison.scenarios
        .filter((entry) => entry.id !== 'minimal_path')
        .map(rerunPhrase),
    ).toEqual([null, null])
    expect(comparisonMarkdown(comparison)).toContain(
      '\n\nRun again (only the last attempt is compared):\n- minimal_path: rerun ×2 in B\n',
    )
    expect(comparisonMarkdown(before)).not.toContain('Run again')
    // Swapped, the mark follows the execution.
    const swapped = compareExecutions(b, a).scenarios.find(
      (entry) => entry.id === 'minimal_path',
    )
    expect(swapped && rerunPhrase(swapped)).toBe('rerun ×2 in A')
  })

  it('states no profile or suite difference when a side did not record its parameters', () => {
    const a = imported()
    a.parameters = {
      ...(a.parameters as ExecutionParameters),
      suite: { id: 'regression', label: 'Regression', sha256: 'sha256:0a' },
    }
    const b = local()
    b.parameters = null
    delete b.plan_execution
    const comparison = compareExecutions(a, b)
    expect(comparison.b.profile).toBeNull()
    for (const field of ['profile', 'suite'])
      expect(comparison.parameters.map((change) => change.field)).not.toContain(
        field,
      )
    expect(comparisonMarkdown(comparison).split('\n')[0]).toBe(
      '### smoke · deepseek/flash',
    )
  })
})
