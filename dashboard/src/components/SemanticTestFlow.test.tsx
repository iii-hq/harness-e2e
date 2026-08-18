import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { SemanticTestFlow } from '@/components/SemanticTestFlow'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'

describe('SemanticTestFlow', () => {
  it('renders semantic evidence for any composite scenario in the common detail', () => {
    const detail = {
      id: 'execution-1',
      reports: [
        {
          subject_id: 'codex',
          scenario_id: 'future_asset_refinement',
          available: true,
          report: {
            assessment_contract: { runs: [] },
            assessment_summary: {},
            scenarios: [
              {
                scenario_id: 'future_asset_refinement',
                scenario_version: 1,
                runs: [
                  {
                    run_id: 'run-1',
                    attempt_id: 'attempt-1',
                    assessment: {
                      ai_final_assessment: {
                        analyzer_usage: {
                          input_tokens: 3344,
                          output_tokens: 970,
                        },
                      },
                    },
                    scenario_flow: {
                      definition_sha256: `sha256:${'a'.repeat(64)}`,
                      snapshot: { executable: false },
                      checkpoint: { path: 'checkpoints/final.json' },
                      cleanup: { status: 'succeeded', duration_ms: 8 },
                    },
                    semantic_tests: [
                      {
                        node_id: 'evaluate_asset',
                        step_type: 'asset.evaluate',
                        step_version: 1,
                        required: true,
                        dependencies: ['produce_asset'],
                        status: 'hard_gate_failed',
                        duration_ms: 1200,
                        metrics: {
                          totals: {
                            input_tokens: 3200,
                            output_tokens: 1000,
                            function_calls: 3,
                            function_call_errors: 0,
                          },
                        },
                        assets: [
                          {
                            id: 'evaluation',
                            artifact: { path: 'deliverables/evaluation.json' },
                          },
                        ],
                        hard_gates: [
                          {
                            id: 'valid',
                            passed: false,
                            reason: 'Asset is incomplete',
                          },
                        ],
                        evaluations: [],
                        failures: [],
                      },
                      {
                        node_id: 'scan_source',
                        step_type: 'security.scan',
                        step_version: 1,
                        required: true,
                        dependencies: [],
                        status: 'succeeded',
                        duration_ms: 800,
                        metrics: {
                          totals: {
                            input_tokens: 100,
                            output_tokens: 50,
                            function_calls: 1,
                            function_call_errors: 0,
                          },
                          request_count: 2,
                          poll: { poll_count: 78 },
                        },
                        assets: [],
                        hard_gates: [],
                        evaluations: [],
                        failures: [],
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

    const html = renderToStaticMarkup(<SemanticTestFlow detail={detail} />)
    expect(html).toContain('Future Asset Refinement')
    expect(html).toContain('Evaluate Asset')
    expect(html).toContain('Asset is incomplete')
    expect(html).toContain('deliverables/evaluation.json')
    expect(html).toContain('Workflow metrics')
    expect(html).toContain('Function calls')
    expect(html).toContain('Workflow tokens')
    expect(html).toContain('4,350')
    expect(html).toContain('all 2 steps reported')
    expect(html).toContain('4,200')
    expect(html).toContain('150')
    expect(html).not.toContain('Assessment tokens')
    expect(html).not.toContain('4,314')
    expect(html).toContain('1 hard gate failed')
    expect(html).toContain('Decision evidence')
    expect(html).toContain('Technical evidence')
    expect(html).toContain('Additional runtime counters')
    expect(html).not.toContain('<table')
    expect(html).not.toContain('Security review execution')
  })
})
