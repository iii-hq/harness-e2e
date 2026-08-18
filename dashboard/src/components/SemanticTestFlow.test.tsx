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
                    assessment: {},
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
                        metrics: { function_calls: 3 },
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
    expect(html).toContain('Persisted workflow metrics')
    expect(html).toContain('Function Calls')
    expect(html).not.toContain('Security review execution')
  })
})
