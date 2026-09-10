import { describe, expect, it, vi } from 'vitest'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import { installDashboardIiiClient } from '@/lib/iii-client'
import {
  comparisonSummary,
  filterReferenceComparison,
  getReleaseControlReference,
  listReleaseControlExecutions,
  localScenarioObservations,
  objectiveScore,
  type RcReference,
  referenceScenarioObservations,
  referenceSummary,
} from '@/lib/release-control-reference'

function comparisonCandidate(
  rows: Array<{
    scenario: string
    seed: string | number
    score: number | null
    repetition?: number
    caseId?: string
  }>,
) {
  return {
    id: 'local',
    status: 'passed',
    subjects: [
      {
        id: 'subject',
        model: 'model',
        provider: 'provider',
        scenarios: rows.map((row) => ({ id: row.scenario })),
      },
    ],
    scenario_metrics: [{ scenario_id: 'legacy' }],
    totals: {
      expected_reports: 99,
      received_reports: 99,
      missing_reports: 8,
      total_tokens: 999,
      wall_time_seconds: 999,
      total_cost_usd: 999,
      function_calls: 999,
      turns: 999,
    },
    reports: rows.map((row, index) => ({
      subject_id: 'subject',
      scenario_id: row.scenario,
      available: true,
      ...(row.repetition === undefined ? {} : { round: row.repetition + 1 }),
      report: {
        scenarios: [
          {
            scenario_id: row.scenario,
            scenario_version: 7,
            case_id:
              row.caseId ??
              `${row.scenario}:v7:seed-${BigInt(row.seed).toString(16).padStart(16, '0')}`,
            case: { seed: row.seed },
            aggregate: { planned_runs: 1 },
            runs: [
              {
                run_id: `run-${index}`,
                status: row.score === 0 ? 'failed' : 'passed',
                completion: row.score === null ? 'undetermined' : 'completed',
                technical: 'valid',
                objective_score: row.score,
                wall_time_ms: 1000,
                efficiency: {
                  total_tokens: 10,
                  function_calls: 1,
                  root_turns: 1,
                },
                metrics: { complete: true, totals: {} },
                cost: { subject_usd: 0.01 },
              },
            ],
          },
        ],
      },
    })),
  } as unknown as DashboardExecutionDetail
}

function comparisonReference(
  rows: Array<{
    scenario: string
    seed: string
    score: number | null
    repetition?: number
  }>,
): RcReference {
  return {
    ...reference,
    execution: { ...reference.execution },
    runs: rows.map((row, index) => ({
      ...reference.runs[0],
      id: `rc-${index}`,
      scenarioId: row.scenario,
      scenarioVersion: 2,
      seed: row.seed,
      repetition: row.repetition ?? 0,
      objectiveScore: row.score,
    })),
  }
}

const reference: RcReference = {
  execution: {
    id: 'e-1',
    campaignId: 'campaign-1',
    planKey: 'regression',
    attempt: 2,
    trigger: 'manual',
    requestedBy: 'layon',
    label: 'Regression',
    phase: 'complete',
    terminal: true,
    resultState: 'complete',
    requestedAt: '2026-09-08T10:00:00Z',
    completedAt: '2026-09-08T10:02:00Z',
    error: null,
    runCount: 2,
    reportCount: 2,
    plan: {},
    request: {},
  },
  aggregate: {
    planned_runs: 3,
    observed_runs: 2,
    completion_rate: 0.5,
    execution_reliability: 2 / 3,
  },
  runs: [
    {
      attemptsComplete: true,
      scenarioId: 'alpha',
      scenarioVersion: 2,
      status: 'passed',
      completion: 'completed',
      technical: 'valid',
      objectiveScore: 100,
      wallTimeMs: 1200,
      totalTokens: 30,
      costSubjectUsd: 0.01,
      turns: 2,
      functionCalls: 1,
    },
    {
      attemptsComplete: true,
      scenarioId: 'alpha',
      scenarioVersion: 2,
      status: 'passed',
      completion: 'completed',
      technical: 'valid',
      objectiveScore: 100,
      wallTimeMs: 800,
      totalTokens: 20,
      costSubjectUsd: 0.02,
      turns: 1,
      functionCalls: 2,
    },
  ],
  materialized: { profile: { id: 'regression' } },
  shards: [{ runs: [] }],
}

describe('Release Control reference adapter', () => {
  it('preserves RC identity and projects only evidence-backed totals', () => {
    const summary = referenceSummary(reference)
    expect(summary).toMatchObject({
      id: 'rc:e-1',
      status: 'complete',
      source: { provenance: 'release-control' },
      totals: {
        expected_reports: 3,
        received_reports: 2,
        total_tokens: 50,
        wall_time_seconds: 2,
        total_cost_usd: 0.03,
      },
    })
    expect(summary.scenario_metrics?.[0]).toMatchObject({
      scenario_id: 'alpha',
      run_count: 2,
      averages: { tokens: 25, duration_seconds: 1 },
    })
  })

  it('keeps incomplete optional metrics unavailable', () => {
    const summary = referenceSummary({
      ...reference,
      runs: [{ ...reference.runs[0], totalTokens: null }],
    })
    expect(summary.totals?.total_tokens).toBeNull()
  })

  it('reads recent executions from the plans wrapper without inventing coverage', async () => {
    const trigger = vi.fn().mockResolvedValue({
      plans: [{ recentExecutions: [reference.execution] }],
    })
    installDashboardIiiClient({
      browserId: 'browser',
      trigger,
      on: () => () => undefined,
      registerTrigger: () => () => undefined,
    })
    const [summary] = await listReleaseControlExecutions()
    expect(summary.id).toBe('rc:e-1')
    expect(summary.totals?.expected_reports).toBeNull()
    expect(trigger).toHaveBeenCalledWith(
      'release-control::test-plans::list',
      {},
      { namespace: 'default' },
    )
  })

  it('averages only finite objective scores', () => {
    expect(
      objectiveScore({
        ...reference,
        runs: [
          { ...reference.runs[0], objectiveScore: 100 },
          { ...reference.runs[1], objectiveScore: null },
        ],
      }),
    ).toBe(100)
  })
})

describe('incomplete comparison filtering', () => {
  it('keeps only paired non-zero slots from a complete RC plan and a partial local run', () => {
    const remote = comparisonReference(
      Array.from({ length: 9 }, (_, index) => ({
        scenario: `scenario-${index}`,
        seed: '42',
        score: 80,
      })),
    )
    const local = comparisonCandidate([
      { scenario: 'scenario-0', seed: 42, score: 90 },
    ])

    const filtered = filterReferenceComparison(remote, local)

    expect(filtered.reference.runs.map((run) => run.scenarioId)).toEqual([
      'scenario-0',
    ])
    expect(filtered.candidate.reports).toHaveLength(1)
    expect(referenceSummary(filtered.reference).totals).toMatchObject({
      expected_reports: 1,
      received_reports: 1,
      total_tokens: 30,
    })
    expect(comparisonSummary(filtered.candidate).totals).toMatchObject({
      expected_reports: 1,
      received_reports: 1,
      total_tokens: 10,
    })
  })

  it.each([
    ['remote zero', 0, 80],
    ['remote missing', null, 80],
    ['local zero', 80, 0],
    ['local missing', 80, null],
  ])('removes a slot with %s from both sides', (_, remoteScore, localScore) => {
    const filtered = filterReferenceComparison(
      comparisonReference([
        { scenario: 'alpha', seed: '7', score: remoteScore },
      ]),
      comparisonCandidate([{ scenario: 'alpha', seed: 7, score: localScore }]),
    )
    expect(filtered.reference.runs).toEqual([])
    expect(filtered.candidate.reports).toEqual([])
  })

  it('removes the whole slot when any observation in it has no positive score', () => {
    const filtered = filterReferenceComparison(
      comparisonReference([
        { scenario: 'alpha', seed: '7', score: 80 },
        { scenario: 'alpha', seed: '7', score: 0 },
      ]),
      comparisonCandidate([{ scenario: 'alpha', seed: 7, score: 90 }]),
    )
    expect(filtered.reference.runs).toEqual([])
    expect(filtered.candidate.reports).toEqual([])
  })

  it('matches repetitions and full-width seeds without comparing scenario versions', () => {
    const seed = '18446744073709551615'
    const remote = comparisonReference([
      { scenario: 'alpha', seed, score: 70, repetition: 0 },
      { scenario: 'alpha', seed, score: 75, repetition: 1 },
    ])
    const local = comparisonCandidate([
      {
        scenario: 'alpha',
        seed,
        score: 80,
        repetition: 0,
        caseId: 'alpha:v7:seed-ffffffffffffffff',
      },
      {
        scenario: 'alpha',
        seed,
        score: 85,
        repetition: 1,
        caseId: 'alpha:v7:seed-ffffffffffffffff',
      },
    ])

    const filtered = filterReferenceComparison(remote, local)

    expect(filtered.reference.runs).toHaveLength(2)
    expect(filtered.candidate.reports).toHaveLength(2)
    expect(objectiveScore(filtered.reference)).toBe(72.5)
    expect(objectiveScore(filtered.candidate)).toBe(82.5)
  })

  it('removes slots missing on either side', () => {
    const filtered = filterReferenceComparison(
      comparisonReference([{ scenario: 'only-remote', seed: '1', score: 80 }]),
      comparisonCandidate([{ scenario: 'only-local', seed: 1, score: 90 }]),
    )
    expect(filtered.reference.runs).toEqual([])
    expect(filtered.candidate.reports).toEqual([])
  })

  it('clears legacy metrics when every slot is removed and leaves its inputs intact', () => {
    const remote = comparisonReference([
      { scenario: 'alpha', seed: '1', score: 0 },
    ])
    const local = comparisonCandidate([
      { scenario: 'alpha', seed: 1, score: 90 },
    ])
    const beforeRemote = structuredClone(remote)
    const beforeLocal = structuredClone(local)

    const filtered = filterReferenceComparison(remote, local)

    expect(referenceSummary(filtered.reference).totals).toMatchObject({
      expected_reports: 0,
      received_reports: 0,
      total_tokens: null,
      wall_time_seconds: null,
    })
    expect(comparisonSummary(filtered.candidate)).toMatchObject({
      scenario_metrics: [],
      totals: {
        expected_reports: 0,
        received_reports: 0,
        missing_reports: 0,
        total_tokens: null,
        wall_time_seconds: null,
        total_cost_usd: null,
        function_calls: null,
        turns: null,
      },
    })
    expect(remote).toEqual(beforeRemote)
    expect(local).toEqual(beforeLocal)
  })
})

// Local totals use the same inclusive token and subject-only cost definitions as RC.
it('compares cache and retry consumption without mixing judge cost or report medians', () => {
  const retryMetrics = { complete: true, totals: { cache_write_tokens: 5 } }
  const detail = {
    id: 'local',
    status: 'passed',
    subjects: [],
    totals: { total_tokens: 999, total_cost_usd: 9 },
    reports: [
      {
        scenario_id: 'alpha',
        report: {
          scenarios: [
            {
              runs: [
                {
                  technical: 'valid',
                  objective_score: 90,
                  wall_time_ms: 2000,
                  efficiency: {
                    total_tokens: 100,
                    function_calls: 3,
                    root_turns: 2,
                    child_turns: 1,
                  },
                  metrics: {
                    complete: true,
                    totals: { cache_read_tokens: 20 },
                  },
                  cost: { subject_usd: 0.2, total_usd: 0.7 },
                  retry_attempts: [
                    {
                      metrics: retryMetrics,
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
  expect(comparisonSummary(detail).totals).toMatchObject({
    total_tokens: 125,
    total_cost_usd: 0.2,
    wall_time_seconds: 2,
    turns: 3,
  })
  expect(objectiveScore(detail)).toBe(90)
  retryMetrics.complete = false
  expect(comparisonSummary(detail).totals?.total_tokens).toBeNull()
})

it('retains zero observed coverage for an empty but materialized reference', () => {
  expect(
    referenceSummary({
      ...reference,
      runs: [],
      aggregate: { ...reference.aggregate, observed_runs: 0 },
    }).totals,
  ).toMatchObject({
    expected_reports: 3,
    received_reports: 0,
    report_coverage: 0,
  })
})

it('does not compare consumption whose technical attempts are incomplete', () => {
  expect(
    referenceSummary({
      ...reference,
      runs: [{ ...reference.runs[0], attemptsComplete: false }],
    }).totals,
  ).toMatchObject({ total_tokens: null, total_cost_usd: null })
})

it('groups RC repetitions by frozen case identity and uses true medians', () => {
  const run = {
    ...reference.runs[0],
    caseId: 'case-a',
    seed: '7',
    cohortSha256: 'cohort-a',
  }
  const observations = referenceScenarioObservations(
    {
      ...reference,
      execution: {
        ...reference.execution,
        plan: {
          subject: { provider: 'openai', model: 'subject' },
          judge: { provider: 'openai', model: 'judge' },
        },
      },
      runs: [
        { ...run, id: 'a-1', objectiveScore: 10, totalTokens: 10 },
        { ...run, id: 'a-2', objectiveScore: 100, totalTokens: 30 },
        { ...run, id: 'a-3', objectiveScore: 30, totalTokens: 20 },
        { ...run, id: 'b-1', caseId: 'case-b', seed: '8' },
        { ...run, id: 'unknown-1', caseId: null, seed: null },
        { ...run, id: 'unknown-2', caseId: null, seed: null },
        { ...run, id: 'large-seed', seed: '18446744073709551615' },
      ],
    },
    'alpha',
  )

  expect(observations).toHaveLength(5)
  expect(observations[0]).toMatchObject({
    execution_id: 'rc:e-1',
    source: 'release-control',
    case_id: 'case-a',
    scenario_version: 2,
    seed: 7,
    run_count: 3,
    median_score: 30,
    median_tokens: 20,
    subject_provider: 'openai',
    subject_model: 'subject',
  })
  expect(observations.slice(2, 4).map((item) => item.run_count)).toEqual([1, 1])
  expect(new Set(observations.map((item) => item.observation_id)).size).toBe(5)
  expect(observations[4]?.seed).toBeNull()
})

it('normalizes local objective, cache-inclusive tokens and subject cost once', () => {
  const scenario = {
    scenario_id: 'alpha',
    scenario_version: 2,
    case_id: 'case-a',
    case: { seed: 7 },
    runs: [
      {
        run_id: 'run-1',
        status: 'passed',
        completion: 'completed',
        technical: 'valid',
        objective_score: 90,
        wall_time_ms: 2000,
        efficiency: { total_tokens: 100, function_calls: 3, root_turns: 2 },
        metrics: { complete: true, totals: { cache_read_tokens: 20 } },
        cost: { subject_usd: 0.2, total_usd: 0.7 },
      },
      {
        run_id: 'run-2',
        status: 'passed',
        completion: 'completed',
        technical: 'valid',
        objective_score: 70,
        wall_time_ms: 1000,
        efficiency: { total_tokens: 50, function_calls: 1, root_turns: 1 },
        metrics: { complete: true, totals: { cache_read_tokens: 10 } },
        cost: { subject_usd: 0.4, total_usd: 1.1 },
      },
    ],
  }
  const report = {
    scenario_id: 'alpha',
    native_execution_id: 'child-1',
    available: true,
    report: { scenarios: [scenario] },
  }
  const detail = {
    id: 'local-parent',
    status: 'passed',
    completed_at: '2026-09-08T12:00:00Z',
    subjects: [{ provider: 'openai', model: 'subject' }],
    scenario_metrics: [
      { scenario_id: 'alpha', contract_fingerprint: 'contract-a' },
    ],
    totals: {},
    reports: [report, report],
  } as unknown as DashboardExecutionDetail

  expect(localScenarioObservations(detail, 'alpha')).toEqual([
    expect.objectContaining({
      execution_id: 'local-parent',
      source: 'local',
      run_count: 2,
      scored_runs: 2,
      median_score: 80,
      median_tokens: 90,
      median_cost_usd: 0.30000000000000004,
      contract_sha256: 'contract-a',
    }),
  ])

  const withoutChildIds = {
    ...detail,
    reports: [
      { ...report, native_execution_id: undefined },
      { ...report, native_execution_id: undefined },
    ],
  } as unknown as DashboardExecutionDetail
  expect(localScenarioObservations(withoutChildIds, 'alpha')).toEqual([
    expect.objectContaining({ run_count: 4 }),
  ])
})

it('explains an absent RC bridge and preserves structured RPC error messages', async () => {
  const trigger = vi.fn().mockRejectedValue({
    code: 'function_not_found',
    message: 'Missing function',
  })
  installDashboardIiiClient({
    browserId: 'test',
    trigger,
    on: () => () => {},
    registerTrigger: () => () => {},
  })
  await expect(listReleaseControlExecutions()).rejects.toThrow('same Engine')
  trigger.mockRejectedValue({
    code: 'unauthorized',
    message: 'Session expired',
  })
  await expect(getReleaseControlReference('rc:one')).rejects.toThrow(
    'Session expired',
  )
  expect(trigger).toHaveBeenLastCalledWith(
    'release-control::test-executions::reference',
    { executionId: 'one' },
    { namespace: 'default' },
  )
})
