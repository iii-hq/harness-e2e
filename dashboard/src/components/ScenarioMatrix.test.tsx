import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { contractScent, ScenarioMatrix } from '@/components/ScenarioMatrix'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import { compareRuns } from '@/lib/execution-comparison'
import { buildExecutionMetrics } from '@/lib/execution-metrics'
import { buildPrimaryMetrics } from '@/lib/primary-metrics'
import { RESULT_CONTRACT_SHA256 } from '@/lib/result-contract.generated'
import { buildScenarioMatrix } from '@/lib/scenario-matrix'
import {
  executionMetricsFixture,
  metricRun,
} from '@/test-fixtures/execution-metrics'

const resultContract = {
  result_contract_sha256: RESULT_CONTRACT_SHA256,
  report_state: 'complete' as const,
  objective_outcome: 'passed' as const,
}

function aggregate(overrides: Record<string, unknown> = {}) {
  return {
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
    scored_runs: 1,
    mean_score: 100,
    total_tokens_consumed: 1200,
    tokens_completed_p50: 1200,
    failed_attempt_tokens: 0,
    tokens_per_completion: 1200,
    technical_failures: 0,
    ...overrides,
  }
}

const detail = {
  id: 'execution-1',
  status: 'failed',
  subjects: [{ id: 'terra', scenarios: [] }],
  reports: [
    {
      subject_id: 'terra',
      scenario_id: 'security_review',
      available: true,
      report: {
        ...resultContract,
        assessment_contract: { runs: [] },
        assessment_summary: {},
        scenarios: [
          {
            scenario_id: 'security_review',
            behavior_sha256:
              'sha256:a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1',
            passed: true,
            aggregate: aggregate(),
            runs: [
              {
                run_id: 'run-security',
                attempt_id: 'attempt-security',
                status: 'passed',
                completion: 'completed',
                technical: 'valid',
                evaluators: {
                  completion: 'available',
                },
                score: 100,
                wall_time_ms: 3_000,
                assessment: {
                  run_id: 'run-security',
                  attempt_id: 'attempt-security',
                  system_status: 'passed',
                  assessments: [],
                },
                semantic_tests: [
                  {
                    node_id: 'scan',
                    step_type: 'security.scan',
                    required: true,
                    dependencies: [],
                    status: 'succeeded',
                    duration_ms: 2_000,
                    cost_usd: 0.0123,
                    metrics: {
                      totals: {
                        input_tokens: 900,
                        output_tokens: 100,
                        function_calls: 3,
                        function_call_errors: 0,
                      },
                      finding_count: 5,
                    },
                    hard_gates: [],
                  },
                  {
                    node_id: 'report',
                    step_type: 'security.report',
                    required: true,
                    dependencies: ['scan'],
                    status: 'succeeded',
                    duration_ms: 1_000,
                    metrics: null,
                    hard_gates: [],
                  },
                ],
              },
            ],
          },
        ],
      },
    },
    {
      subject_id: 'terra',
      scenario_id: 'persistent_state',
      available: true,
      report: {
        ...resultContract,
        objective_outcome: 'passed',
        assessment_contract: { runs: [] },
        assessment_summary: {},
        scenarios: [
          {
            scenario_id: 'persistent_state',
            behavior_sha256:
              'sha256:b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2',
            passed: true,
            aggregate: aggregate({
              completed_runs: 0,
              task_incomplete_runs: 1,
              completion_rate: 0,
              mean_score: 65,
              total_tokens_consumed: null,
              tokens_completed_p50: null,
              failed_attempt_tokens: null,
              tokens_per_completion: null,
            }),
            runs: [
              {
                run_id: 'run-state',
                attempt_id: 'attempt-state',
                status: 'passed',
                completion: 'task_incomplete',
                technical: 'valid',
                evaluators: {
                  completion: 'available',
                },
                score: 65,
                assessment: {
                  system_status: 'passed',
                  assessments: [],
                },
              },
            ],
          },
        ],
      },
    },
    {
      subject_id: 'terra',
      scenario_id: 'research_pipeline',
      available: true,
      report: {
        ...resultContract,
        objective_outcome: 'inconclusive',
        assessment_contract: { runs: [] },
        assessment_summary: {},
        scenarios: [
          {
            scenario_id: 'research_pipeline',
            behavior_sha256:
              'sha256:b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2',
            passed: false,
            aggregate: aggregate({
              observed_runs: 0,
              deferred_runs: 1,
              completed_runs: 0,
              technical_valid_runs: 0,
              execution_reliability: 0,
              completion_evidence_coverage: 0,
              completion_rate: null,
              scored_runs: 0,
              mean_score: null,
              total_tokens_consumed: null,
              tokens_completed_p50: null,
              failed_attempt_tokens: null,
              tokens_per_completion: null,
            }),
            runs: [],
          },
        ],
      },
    },
    {
      subject_id: 'terra',
      scenario_id: 'missing_report',
      available: false,
    },
  ],
} as unknown as DashboardExecutionDetail

describe('ScenarioMatrix', () => {
  it('sums scenario usage across runs and labels incomplete observations', () => {
    const complete = executionMetricsFixture([
      { runs: [metricRun('first', 100), metricRun('last', 200)] },
    ])
    const html = renderToStaticMarkup(
      <ScenarioMatrix detail={complete} onTranscript={() => {}} />,
    )
    expect(html).toMatch(
      /data-primary-metric="Total tokens"[\s\S]*?<strong[^>]*>300<\/strong>/,
    )
    expect(html).toMatch(
      /data-primary-metric="Reported cost"[\s\S]*?<strong[^>]*>\$0.2000<\/strong>/,
    )
    expect(html).toMatch(
      /data-primary-metric="Runtime"[\s\S]*?<strong[^>]*>2\.0 s<\/strong>/,
    )
    expect(html).toMatch(
      /data-primary-metric="Function calls"[\s\S]*?<strong[^>]*>20<\/strong>/,
    )
    expect(html).toContain('@[1000px]/harness:table')
    expect(html).not.toContain(' lg:')
    const partial = executionMetricsFixture([
      { runs: [metricRun('first', 100), metricRun('last', null)] },
    ])
    const partialHtml = renderToStaticMarkup(
      <ScenarioMatrix detail={partial} onTranscript={() => {}} />,
    )
    expect(partialHtml).toMatch(
      /data-primary-metric="Total tokens"[\s\S]*?<strong[^>]*>100<\/strong>/,
    )
    expect(partialHtml).toContain('Partial · 1/2 runs reported')
  })

  it('retains evidence and transcript access for every run across subjects', () => {
    const report = detail.reports[0].report
    if (!report) throw new Error('Expected retained report fixture')
    const scenario = report.scenarios[0]
    const originalRun = scenario.runs[0]
    const multiple = {
      ...detail,
      reports: ['terra', 'sol'].map((subjectId) => ({
        ...detail.reports[0],
        subject_id: subjectId,
        report: {
          ...report,
          scenarios: [
            {
              ...scenario,
              runs: ['first', 'last'].map((suffix) => ({
                ...originalRun,
                run_id: `${subjectId}-${suffix}`,
                attempt_id: `${subjectId}-${suffix}-attempt`,
                assessment: {
                  ...originalRun.assessment,
                  run_id: `${subjectId}-${suffix}`,
                  attempt_id: `${subjectId}-${suffix}-attempt`,
                },
                transcript: { messages: [] },
              })),
            },
          ],
        },
      })),
    } as DashboardExecutionDetail
    const html = renderToStaticMarkup(
      <ScenarioMatrix detail={multiple} onTranscript={() => {}} />,
    )
    expect(html).toContain('aria-label="Scenario results"')
    for (const runId of [
      'terra-first',
      'terra-last',
      'sol-first',
      'sol-last',
    ]) {
      expect(html).toContain(`/run/${runId}`)
    }
    expect(html.match(/>transcript<\/button>/g)).toHaveLength(4)
    expect(
      html.match(/aria-label="Evidence record for Security Review"/g),
    ).toHaveLength(2)
    expect(html.match(/aria-label="Retained runs"/g)).toHaveLength(2)
  })

  it('shows the sample size when only part of a scenario has a score', () => {
    const assessment = detail.reports[0].report?.scenarios[0].runs[0].assessment
    const partial = executionMetricsFixture([
      {
        runs: [
          metricRun('scored', 100, { score: 80, assessment }),
          metricRun('unscored', 100, { score: null, assessment }),
        ],
      },
    ])
    const html = renderToStaticMarkup(
      <ScenarioMatrix detail={partial} onTranscript={() => {}} />,
    )
    expect(html).toContain('80/100')
    expect(html).toContain('Mean · 1/2 planned runs scored')
  })

  it('keeps a small positive cost distinct from zero', () => {
    const assessment = detail.reports[0].report?.scenarios[0].runs[0].assessment
    const evidence = executionMetricsFixture([
      {
        runs: [
          metricRun('small-cost', 10, {
            assessment,
            cost: { total_usd: 0.00001 },
          }),
        ],
      },
    ])
    const html = renderToStaticMarkup(
      <ScenarioMatrix detail={evidence} onTranscript={() => {}} />,
    )
    expect(html).toContain('&lt;$0.0001')
    expect(html).not.toContain('$0.0000')
  })

  it('shows scenario results in a compact table with scores and evidence', () => {
    const html = renderToStaticMarkup(
      <ScenarioMatrix detail={detail} onTranscript={() => {}} />,
    )

    expect(html).toContain('4 scenarios')
    expect(html).toContain('report state')
    expect(html).toContain('objective outcome')
    // The report is identified by its results contract digest, shortened as
    // every other digest in the Console and never title cased (audit ED-30).
    expect(html).toContain('results contract')
    expect(html).toContain(
      RESULT_CONTRACT_SHA256.replace('sha256:', '').slice(0, 12),
    )
    expect(html).not.toContain('Sha256:')
    expect(html).not.toContain('Completion and evidence yield')
    expect(html).not.toContain('execution reliability')
    expect(html).toContain('Mean · 1/1 planned runs scored')
    expect(html).toContain('65/100')
    expect(html).not.toContain('completion rate')
    expect(html).not.toContain('data-scenario-aggregate')
    expect(html).not.toContain('quality')
    expect(html).not.toContain('Physical attempt outcomes')
    expect(html).not.toContain('Technical Invalid')
    expect(html).toContain('1 passed')
    expect(html).toContain('1 incomplete')
    expect(html).not.toContain('hard gate')
    expect(html).toContain('1 inconclusive')
    expect(html).toContain('1 unavailable')
    expect(html).toContain('Security Review · definition a1a1a1a1')
    expect(html).toContain('aria-label="Persistent State scenario result"')
    expect(html).toContain('aria-label="Missing Report scenario result"')
    expect(html).toContain(
      'The expected report for this scenario is unavailable',
    )
    expect(html).toMatch(
      /href="[^"]*execution\/execution-1\/run\/run-security"/,
    )
    expect(html).not.toContain('Advisory')
    expect(html).toContain('Workflow duration profile')
    expect(html).toContain('Tokens')
    expect(html).toContain('1,000')
    expect(html).toContain('Function calls')
    expect(html).toContain('Function errors')
    expect(html).toContain('Cost')
    expect(html).toContain('$0.0123')
    expect(html).toContain('data-primary-metric="Runtime"')
    expect(html).toContain('data-primary-metric="Total tokens"')
    expect(html).toContain('data-primary-metric="Function calls"')
    expect(html).toContain('data-primary-metric="Reported cost"')
    expect(html).not.toContain('data-primary-metric="Hard gates"')
    expect(html).not.toContain('data-step-metric="Findings"')
    expect(html).not.toContain('data-step-metric="Requests"')
    expect(html).not.toContain('data-step-metric="Polls"')
    expect(html).toContain('Not captured')
    expect(html).not.toContain('Inspect scenario evidence')
    expect(html).not.toContain('>Structure<')
    expect(html).not.toContain('logical run')
    expect(html).not.toContain('>avg<')
    expect(html).not.toContain('Recorded runs')
    expect(html).not.toContain('This scenario has no persisted workflow')
    expect(html).not.toMatch(/<button[^>]*data-scenario-row/)
  })

  it('marks a scenario that ran again and lists its previous attempts outside every figure', () => {
    const current = detail.reports[0]
    const scenario = current.report?.scenarios[0]
    const replaced = {
      ...current,
      native_execution_id: 'old-native',
      round: 1,
      report: {
        ...current.report,
        objective_outcome: 'failed',
        scenarios: [
          {
            ...scenario,
            passed: false,
            aggregate: aggregate({
              completed_runs: 0,
              task_incomplete_runs: 1,
            }),
            runs: [
              {
                ...scenario?.runs[0],
                run_id: 'run-old',
                attempt_id: 'attempt-old',
                status: 'hard_gate_failed',
                completion: 'task_incomplete',
                score: 20,
                failures: [
                  { phase: 'evaluation', message: 'scan never ended' },
                ],
              },
            ],
          },
        ],
      },
    }
    const crashed = {
      subject_id: 'terra',
      scenario_id: 'security_review',
      native_execution_id: 'crashed-native',
      round: 1,
      available: false,
      error: 'fixture repository unavailable',
    }
    const reran = {
      ...detail,
      reports: [{ ...current, round: 1 }, ...detail.reports.slice(1)],
      previous_reports: [replaced, crashed],
    } as unknown as DashboardExecutionDetail
    const html = renderToStaticMarkup(
      <ScenarioMatrix
        detail={reran}
        onTranscript={() => {}}
        onRerun={() => {}}
      />,
    )

    expect(html).toContain('data-reruns="2"')
    expect(html).toContain('rerun ×2')
    expect(html).toContain('previous attempts · not counted')
    expect(html).toContain('20/100')
    expect(html).toContain('scan never ended')
    expect(html).toMatch(/href="[^"]*execution\/old-native\/run\/run-old"/)
    expect(html).toContain('fixture repository unavailable')
    // Every row can run again; one that did not pass says so in words.
    expect(html).toContain('aria-label="Run Security Review again"')
    expect(html).toMatch(
      /data-rerun-scenario="persistent_state"[^>]*>.*?run again<\/button>/,
    )
    // The attempts it replaced reach no figure, summary or comparison run.
    const without = { ...reran, previous_reports: [] }
    expect(buildPrimaryMetrics(reran)).toEqual(buildPrimaryMetrics(without))
    expect(buildExecutionMetrics(reran)).toEqual(buildExecutionMetrics(without))
    expect(buildScenarioMatrix(reran).summary).toEqual(
      buildScenarioMatrix(without).summary,
    )
    expect(compareRuns(reran).map((run) => run.runId)).toEqual(
      compareRuns(without).map((run) => run.runId),
    )
    expect(
      renderToStaticMarkup(
        <ScenarioMatrix detail={without} onTranscript={() => {}} />,
      ),
    ).not.toContain('data-rerun')
  })

  it('shows a scenario running again as running, not unavailable', () => {
    const running = {
      ...detail,
      reports: [
        {
          subject_id: 'terra',
          scenario_id: 'security_review',
          available: false,
          state: 'running',
          error: null,
        },
      ],
    } as unknown as DashboardExecutionDetail
    const model = buildScenarioMatrix(running)
    expect(model.items[0].objective.status).toBe('running')
    expect(model.items[0].reason).toBeNull()
    expect(model.summary).toMatchObject({ running: 1, unavailable: 0 })
    const html = renderToStaticMarkup(
      <ScenarioMatrix detail={running} onTranscript={() => {}} />,
    )
    expect(html).not.toContain('The expected report for this scenario')
  })

  it('keeps incomplete outcomes and evidence access visible with secondary details collapsed', () => {
    const failedOnly = {
      ...detail,
      reports: detail.reports.filter(
        (report) => report.scenario_id === 'persistent_state',
      ),
    } as DashboardExecutionDetail
    const html = renderToStaticMarkup(
      <ScenarioMatrix detail={failedOnly} onTranscript={() => {}} />,
    )

    expect(html).not.toContain('>Structure<')
    expect(html).not.toContain('>Standard<')
    expect(html).not.toContain('Inspect scenario evidence')
    expect(html).toContain('aria-expanded="false"')
    const panelId = html.match(/aria-controls="([^"]+)"/)?.[1]
    expect(panelId).toBeTruthy()
    expect(html).toContain(`id="${panelId}" hidden=""`)
    expect(html).toContain('title="Persistent State · definition b2b2b2b2"')
    expect(html).toContain('data-status="incomplete"')
    expect(html).not.toContain('completion evaluator')
  })
})

it('shows every retained outcome in aggregate provenance', () => {
  const contracts = buildScenarioMatrix(detail).contracts
  expect(
    contractScent([
      { ...contracts[0], objectiveOutcome: 'passed' },
      { ...contracts[0], objectiveOutcome: 'failed' },
    ]),
  ).toContain('passed / failed')
})
