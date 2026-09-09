import { describe, expect, it, vi } from 'vitest'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import { installDashboardIiiClient } from '@/lib/iii-client'
import {
  comparisonSummary,
  listReleaseControlExecutions,
  objectiveScore,
  type RcReference,
  referenceDetail,
  referenceSummary,
} from '@/lib/release-control-reference'

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

  it('exposes no invented native report for a remote reference', () => {
    expect(referenceDetail(reference).reports).toEqual([])
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
