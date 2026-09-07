import { describe, expect, it } from 'vitest'
import type {
  AssessmentResult,
  EvidenceReference,
  RunAssessmentContract,
} from '@/lib/assessment-contract'
import {
  aggregateAssessmentMetrics,
  assessmentFilterCounts,
  buildAssessmentWorkspace,
  buildHarnessRecommendation,
  matchesAssessmentFilter,
} from '@/lib/assessment-view'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import {
  RESULT_CONTRACT_SHA256,
  RESULTS_SCHEMA_VERSION,
  SCORING_PROFILE_SHA256,
} from '@/lib/result-contract.generated'

const transcriptEvidence: EvidenceReference[] = [
  {
    artifact_id: 'transcript',
    artifact_sha256: `sha256:${'a'.repeat(64)}`,
    locator: '/messages/2',
  },
]

function result(overrides: Partial<AssessmentResult> = {}): AssessmentResult {
  return {
    criterion_id: 'answer_quality',
    target: { kind: 'criterion', id: 'answer_quality' },
    kind: 'signal',
    policy: 'advisory',
    dimension: 'deliverable',
    outcome: 'passed',
    score: { awarded: 30, possible: 30 },
    summary: 'The answer meets the rubric criterion.',
    evidence: transcriptEvidence,
    ...overrides,
  }
}

function contract(
  overrides: Partial<RunAssessmentContract> = {},
): RunAssessmentContract {
  return {
    run_id: 'run-1',
    attempt_id: 'attempt-1',
    system_status: 'passed',
    assessments: [result()],
    assets: [
      {
        asset_id: 'answer',
        outcome: 'valid',
        summary: 'The captured asset is structurally valid.',
        evidence: [
          {
            artifact_id: 'answer',
            artifact_sha256: `sha256:${'c'.repeat(64)}`,
          },
        ],
      },
    ],
    ...overrides,
  }
}

function detail(run: RunAssessmentContract): DashboardExecutionDetail {
  return {
    id: 'execution-1',
    status: 'passed',
    assessment_summary: {} as never,
    subjects: [],
    reports: [
      {
        subject_id: 'codex/terra',
        scenario_id: 'direct_answer',
        available: true,
        report: {
          schema_version: RESULTS_SCHEMA_VERSION,
          result_contract_sha256: RESULT_CONTRACT_SHA256,
          scoring_profile_sha256: SCORING_PROFILE_SHA256,
          report_state: 'complete',
          objective_outcome: 'passed',
          assessment_availability: 'available',
          assessment_contract: { runs: [run] },
          assessment_summary: {} as never,
          scenarios: [
            {
              scenario_id: 'direct_answer',
              scenario_version: 4,
              assessment_summary: {} as never,
              aggregate: {
                planned_runs: 1,
                observed_runs: 1,
                deferred_runs: 0,
                completed_runs: 1,
                task_incomplete_runs: 0,
                undetermined_runs: 0,
                technical_valid_runs: 1,
                technical_invalid_runs: 0,
                execution_reliability: 1,
                completion_evidence_coverage: 1,
                completion_rate: 1,
                objective_scored_runs: 1,
                objective_median_score: 100,
                objective_score_coverage: 1,
                quality_scored_completed_runs: 1,
                quality_score_completed: 100,
                quality_coverage: 1,
                total_tokens_consumed: 1200,
                tokens_completed_p50: 1200,
                failed_attempt_tokens: 0,
                tokens_per_completion: 1200,
                hard_gate_failures: 0,
                technical_failures: 0,
              },
              runs: [
                {
                  run_id: run.run_id,
                  attempt_id: run.attempt_id,
                  status: 'passed',
                  completion: 'completed',
                  technical: 'valid',
                  evaluators: {
                    completion: 'available',
                    quality: 'available',
                  },
                  objective_score: 100,
                  quality_score_completed: 100,
                  assessment: run,
                },
              ],
            },
          ],
        },
      },
    ],
  }
}

describe('assessment presentation model', () => {
  it('publishes the system status as the only run outcome', () => {
    const model = buildAssessmentWorkspace(detail(contract()))
    expect(model.availability).toBe('available')
    expect(model.runs[0]).toMatchObject({ systemStatus: 'passed' })
    expect(model.runs[0]).not.toHaveProperty('effectiveStatus')
    expect(model.runs[0]).not.toHaveProperty('finalAssessment')
  })

  it('projects run-level token, function, and duration metrics', () => {
    const input = detail(contract())
    const report = input.reports[0].report
    if (!report) throw new Error('test report is missing')
    report.scenarios[0].runs[0] = {
      ...report.scenarios[0].runs[0],
      wall_time_ms: 62294,
      metrics: {
        totals: {
          input_tokens: 21296,
          output_tokens: 1372,
          cache_read_tokens: 161280,
          reasoning_tokens: 706,
          function_calls: 14,
          function_call_errors: 0,
          sessions: 1,
          turns: 16,
        },
      },
    }
    expect(buildAssessmentWorkspace(input).runs[0].metrics).toEqual({
      totalTokens: 22668,
      inputTokens: 21296,
      outputTokens: 1372,
      cacheReadTokens: 161280,
      cacheWriteTokens: null,
      reasoningTokens: 706,
      functionCalls: 14,
      functionCallErrors: 0,
      durationMs: 62294,
      sessions: 1,
      turns: 16,
    })
  })

  it('aggregates run metrics for the execution summary', () => {
    const model = buildAssessmentWorkspace(detail(contract()))
    const first = {
      ...model.runs[0],
      metrics: {
        ...model.runs[0].metrics,
        totalTokens: 120,
        functionCalls: 7,
        functionCallErrors: 1,
        durationMs: 1500,
      },
    }
    const second = {
      ...first,
      key: 'second-run',
      metrics: {
        ...first.metrics,
        totalTokens: 80,
        functionCalls: 3,
        functionCallErrors: 2,
        durationMs: 2500,
      },
    }
    expect(aggregateAssessmentMetrics([first, second])).toMatchObject({
      totalTokens: 200,
      functionCalls: 10,
      functionCallErrors: 3,
      durationMs: 4000,
    })
  })

  it('orders a hard-gate failure ahead of a passing run', () => {
    const failing = contract({
      run_id: 'run-2',
      system_status: 'hard_gate_failed',
    })
    const input = detail(contract())
    const report = input.reports[0].report
    if (!report) throw new Error('test report is missing')
    report.scenarios[0].runs.push({
      ...report.scenarios[0].runs[0],
      run_id: failing.run_id,
      assessment: failing,
    })
    const model = buildAssessmentWorkspace(input)
    expect(model.runs.map((run) => run.systemStatus)).toEqual([
      'hard_gate_failed',
      'passed',
    ])
  })

  it('derives next-run guidance from authoritative harness status', () => {
    const infrastructure = buildAssessmentWorkspace(
      detail(contract({ system_status: 'infrastructure_error' })),
    ).runs[0]
    expect(buildHarnessRecommendation(infrastructure)).toMatch(
      /collection or serialization path/i,
    )

    const resource = buildAssessmentWorkspace(
      detail(contract({ system_status: 'resource_limit' })),
    ).runs[0]
    expect(buildHarnessRecommendation(resource)).toMatch(/resource footprint/i)

    const passing = buildAssessmentWorkspace(detail(contract())).runs[0]
    expect(buildHarnessRecommendation(passing)).toMatch(/comparable scenario/i)
  })

  it('preserves technical failures without translating them into quality failures', () => {
    const model = buildAssessmentWorkspace(
      detail(contract({ system_status: 'subject_error' })),
    )
    expect(model.runs[0].systemStatus).toBe('subject_error')
    expect(buildHarnessRecommendation(model.runs[0])).toMatch(
      /subject execution or transport path/i,
    )
  })

  it('filters failed, unavailable and asset assessments', () => {
    const run = contract({
      assessments: [
        result({ outcome: 'failed' }),
        result({ criterion_id: 'scored' }),
        result({ criterion_id: 'missing', outcome: 'unavailable' }),
      ],
    })
    const entries = buildAssessmentWorkspace(detail(run)).runs[0].assessments
    expect(
      entries.some((entry) => matchesAssessmentFilter(entry, 'failed')),
    ).toBe(true)
    expect(
      entries.some((entry) => matchesAssessmentFilter(entry, 'unavailable')),
    ).toBe(true)
    expect(
      entries.filter((entry) => matchesAssessmentFilter(entry, 'asset')),
    ).toHaveLength(1)
    expect(
      assessmentFilterCounts(buildAssessmentWorkspace(detail(run)).runs),
    ).toEqual({ all: 4, failed: 1, unavailable: 1, asset: 1 })
  })

  it('projects an asset validation as a deterministic objective entry', () => {
    const entries = buildAssessmentWorkspace(detail(contract())).runs[0]
      .assessments
    expect(entries.at(-1)).toMatchObject({
      criterionId: 'asset:answer',
      kind: 'asset_validation',
      policy: 'objective',
      outcome: 'passed',
      validationOutcome: 'valid',
    })
  })

  it('keeps missing assessment contracts unavailable', () => {
    expect(buildAssessmentWorkspace(undefined).availability).toBe('unavailable')
    const unavailable = detail(contract())
    unavailable.reports[0].report = {
      ...unavailable.reports[0].report,
      assessment_availability: 'unavailable',
      scenarios: [],
    } as never
    expect(buildAssessmentWorkspace(unavailable).availability).toBe(
      'unavailable',
    )
  })
})
