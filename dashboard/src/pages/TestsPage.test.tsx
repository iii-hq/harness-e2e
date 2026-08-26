import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { TestSideSummary } from '@/lib/test-catalog'
import { SideResult } from '@/pages/TestsPage'

describe('versioned test side presentation', () => {
  it('renders retained legacy summaries without inventing assessment results', () => {
    const legacy: TestSideSummary = {
      evaluated_version_id: 'legacy-version',
      execution_count: 1,
      total_runs: 1,
      scored_runs: 1,
      case_count: 1,
      median_score: 100,
      pass_rate: 1,
      median_cost_usd: null,
      median_tokens: null,
      median_duration_seconds: null,
      outcomes: {
        passed: 1,
        hard_gate_failed: 0,
        technical_failed: 0,
        infra_failed: 0,
      },
      samples: { score: 1, cost_usd: 0, tokens: 0, duration_seconds: 0 },
    }
    const html = renderToStaticMarkup(<SideResult summary={legacy} />)
    expect(html).toContain('System: Passed')
    expect(html).toContain('AI: Unavailable')
    expect(html).toContain('Effective: Unavailable')
    expect(html).toContain('0 evidence references')
  })
})
