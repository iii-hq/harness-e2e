import { describe, expect, it } from 'vitest'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import { comparePrimaryMetrics } from '@/lib/primary-metrics'
import {
  getImportedReference,
  listImportedExecutions,
  localReferencePrimaryMetrics,
  localScenarioObservations,
  type RcReference,
  referencePrimaryMetrics,
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
        result_contract_sha256: 'result-contract',
        scoring_profile_sha256: 'scoring-profile',
        scenarios: [
          {
            scenario_id: row.scenario,
            scenario_version: 2,
            case_id:
              row.caseId ??
              `${row.scenario}:v2:seed-${BigInt(row.seed).toString(16).padStart(16, '0')}`,
            case: {
              seed: row.seed,
              inputs_sha256: `definition-${row.scenario}`,
            },
            aggregate: {
              planned_runs: 1,
              observed_runs: 1,
              deferred_runs: 0,
            },
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
                metrics: {
                  complete: true,
                  totals: {
                    input_tokens: 7,
                    output_tokens: 3,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    turns: 1,
                    function_calls: 1,
                    function_call_errors: 0,
                  },
                },
                cost: { subject_usd: 0.01, total_usd: 0.02 },
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
  const repetitions = rows.filter(
    (row) => row.scenario === rows[0]?.scenario,
  ).length
  return {
    ...reference,
    execution: {
      ...reference.execution,
      plan: {
        subject: { provider: 'provider', model: 'model' },
      },
    },
    runs: rows.map((row, index) => ({
      ...reference.runs[0],
      id: `rc-${index}`,
      scenarioId: row.scenario,
      scenarioVersion: 2,
      seed: row.seed,
      repetition: row.repetition ?? 0,
      objectiveScore: row.score,
      caseId: `${row.scenario}:v2:seed-${BigInt(row.seed).toString(16).padStart(16, '0')}`,
      identity: {
        definitionSha256: `definition-${row.scenario}`,
        resultContractSha256: 'result-contract',
        scoringProfileSha256: 'scoring-profile',
      },
    })),
    aggregate: {
      planned_runs: rows.length,
      observed_runs: rows.length,
      completion_rate: 1,
      execution_reliability: 1,
    },
    materialized: { profile: { repetitions } },
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

  it('requires the local Harness bridge for imported history', async () => {
    await expect(listImportedExecutions()).rejects.toThrow('initialized')
  })
})

describe('shared primary metrics projection', () => {
  it('weights every RC test equally after averaging its repetitions', () => {
    const remote = comparisonReference([
      { scenario: 'alpha', seed: '1', score: 100, repetition: 0 },
      { scenario: 'alpha', seed: '1', score: 0, repetition: 1 },
      { scenario: 'beta', seed: '2', score: 100, repetition: 0 },
      { scenario: 'beta', seed: '2', score: 100, repetition: 1 },
    ])

    const metrics = referencePrimaryMetrics(remote)

    expect(metrics.tests.map((test) => test.metrics.score.value)).toEqual([
      50, 100,
    ])
    expect(metrics.metrics.score.value).toBe(75)
  })

  it('keeps the RC token total and leaves unavailable breakdowns and total spend absent', () => {
    const metrics = referencePrimaryMetrics(
      comparisonReference([{ scenario: 'alpha', seed: '1', score: 80 }]),
    )

    expect(metrics.metrics.totalTokens.value).toBe(30)
    expect(metrics.metrics.inputTokens.value).toBeNull()
    expect(metrics.metrics.outputTokens.value).toBeNull()
    expect(metrics.metrics.cacheRead.value).toBeNull()
    expect(metrics.metrics.costUsd.value).toBe(0.01)
  })

  it('allows matched case and contracts but blocks incompatible contracts or repetitions', () => {
    const remote = referencePrimaryMetrics(
      comparisonReference([{ scenario: 'alpha', seed: '7', score: 80 }]),
    )
    const localDetail = comparisonCandidate([
      { scenario: 'alpha', seed: 7, score: 90 },
    ])
    const local = localReferencePrimaryMetrics(localDetail, true)

    expect(comparePrimaryMetrics(remote, local, false).deltas.score).toBe(10)
    expect(
      comparePrimaryMetrics(
        remote,
        localReferencePrimaryMetrics(localDetail, false),
        false,
      ).deltas.score,
    ).toBeNull()

    const changedCase = structuredClone(localDetail)
    const changedScenario = changedCase.reports[0]?.report?.scenarios[0]
    if (!changedScenario) throw new Error('fixture must contain a scenario')
    changedScenario.case_id = 'another-case'
    expect(
      comparePrimaryMetrics(
        remote,
        localReferencePrimaryMetrics(changedCase, true),
        false,
      ).deltas.score,
    ).toBeNull()

    const changedVersion = structuredClone(localDetail)
    const versionedScenario = changedVersion.reports[0]?.report?.scenarios[0]
    if (!versionedScenario) throw new Error('fixture must contain a scenario')
    versionedScenario.scenario_version = 3
    expect(
      comparePrimaryMetrics(
        remote,
        localReferencePrimaryMetrics(changedVersion, true),
        false,
      ),
    ).toMatchObject({ totalTests: 2, deltas: { score: null } })

    const report = localDetail.reports[0]?.report
    if (!report) throw new Error('fixture must contain a report')
    report.result_contract_sha256 = 'changed-contract'
    expect(
      comparePrimaryMetrics(
        remote,
        localReferencePrimaryMetrics(localDetail, true),
        false,
      ).deltas.score,
    ).toBeNull()

    const repeated = comparisonCandidate([
      { scenario: 'alpha', seed: 7, score: 90, repetition: 0 },
      { scenario: 'alpha', seed: 7, score: 95, repetition: 1 },
    ])
    expect(
      comparePrimaryMetrics(
        remote,
        localReferencePrimaryMetrics(repeated, true),
        false,
      ).deltas.score,
    ).toBeNull()
  })

  it('blocks shifted repetition slots and changed model cohorts before aggregating', () => {
    const remoteReference = comparisonReference([
      { scenario: 'alpha', seed: '7', score: 80, repetition: 0 },
      { scenario: 'alpha', seed: '7', score: 90, repetition: 1 },
    ])
    const shifted = comparisonCandidate([
      { scenario: 'alpha', seed: 7, score: 90, repetition: 1 },
      { scenario: 'alpha', seed: 7, score: 95, repetition: 2 },
    ])

    expect(
      comparePrimaryMetrics(
        referencePrimaryMetrics(remoteReference),
        localReferencePrimaryMetrics(shifted, true),
        false,
      ).deltas.score,
    ).toBeNull()

    const changedCohort = structuredClone(remoteReference)
    const first = changedCohort.runs[0]
    if (!first?.identity) throw new Error('fixture must contain run identity')
    first.identity.subjectModel = 'other-model'
    expect(
      comparePrimaryMetrics(
        referencePrimaryMetrics(changedCohort),
        localReferencePrimaryMetrics(
          comparisonCandidate([
            { scenario: 'alpha', seed: 7, score: 90, repetition: 0 },
            { scenario: 'alpha', seed: 7, score: 95, repetition: 1 },
          ]),
          true,
        ),
        false,
      ).deltas.score,
    ).toBeNull()
  })

  it('keeps every RC aggregate unavailable when the global planned scope is incomplete', () => {
    const remote = comparisonReference([
      { scenario: 'alpha', seed: '7', score: 80, repetition: 0 },
    ])
    remote.aggregate.planned_runs = 2

    const metrics = referencePrimaryMetrics(remote)

    expect(metrics.tests[0]?.scopeKnown).toBe(false)
    expect(metrics.metrics.score.value).toBeNull()
    expect(metrics.metrics.totalTokens.value).toBeNull()
  })

  it('rejects a globally complete count distributed into the wrong test slots', () => {
    const remote = comparisonReference([
      { scenario: 'alpha', seed: '7', score: 80, repetition: 0 },
      { scenario: 'alpha', seed: '7', score: 90, repetition: 1 },
    ])
    remote.materialized = { profile: { repetitions: 1 } }

    const metrics = referencePrimaryMetrics(remote)

    expect(metrics.tests[0]?.scopeKnown).toBe(false)
    expect(metrics.metrics.score.value).toBeNull()
  })

  it('uses the shared symmetric zero and missing filter for RC/local rows', () => {
    const remote = referencePrimaryMetrics(
      comparisonReference([
        { scenario: 'kept', seed: '1', score: 80 },
        { scenario: 'zero', seed: '2', score: 70 },
        { scenario: 'missing', seed: '3', score: null },
      ]),
    )
    const local = localReferencePrimaryMetrics(
      comparisonCandidate([
        { scenario: 'kept', seed: 1, score: 90 },
        { scenario: 'zero', seed: 2, score: 0 },
        { scenario: 'missing', seed: 3, score: 90 },
      ]),
      true,
    )

    const filtered = comparePrimaryMetrics(remote, local, true)

    expect(filtered).toMatchObject({ excluded: 2, totalTests: 3 })
    expect(filtered.tests.map((test) => test.label)).toEqual(['kept'])
    expect(filtered.baseline.metrics.totalTokens.value).toBe(30)
    expect(filtered.candidate.metrics.totalTokens.value).toBe(10)
  })
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
    execution_id: 'e-1',
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

it('does not query Release Control directly for imported history', async () => {
  await expect(getImportedReference('remote-execution-one')).rejects.toThrow(
    'initialized',
  )
})
