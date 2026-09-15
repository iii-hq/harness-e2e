import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { AssessmentRunView } from '@/lib/assessment-view'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import { buildExecutionPresentation } from '@/lib/execution-view'
import {
  EvidenceBundleUnavailable,
  executionOutcome,
  provenanceEntries,
} from '@/pages/ExecutionPage'

const detail = {
  id: 'execution-1',
  label: 'Security review',
  status: 'passed',
  availability: 'full',
  subjects: [],
  reports: [],
  totals: {
    expected_reports: 1,
    received_reports: 1,
    scenario_pass_rate: 1,
    report_coverage: 1,
  },
  assessment_summary: {
    assessment_count: 46,
    evidence_reference_count: 17,
    assessment_outcomes: { passed: 39, partial: 7 },
    system_statuses: { passed: 1 },
  },
} as unknown as DashboardExecutionDetail

const run: AssessmentRunView = {
  key: 'security-review',
  subjectId: 'terra',
  scenarioId: 'security_review',
  behaviorSha256:
    'sha256:a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1',
  runId: 'run-1',
  attemptId: 'attempt-1',
  metrics: {
    totalTokens: null,
    inputTokens: null,
    outputTokens: null,
    cacheReadTokens: null,
    cacheWriteTokens: null,
    reasoningTokens: null,
    functionCalls: null,
    functionCallErrors: null,
    durationMs: 151_460,
    sessions: null,
    turns: null,
  },
  systemStatus: 'passed',
  score: 100,
  assessments: [],
  evidence: [],
}

describe('execution evidence', () => {
  it('reports an unavailable evidence bundle without losing the execution', () => {
    const retained = {
      ...detail,
      availability: 'unavailable',
      evidence_error: 'native bundle checksum mismatch',
      reports: [],
      totals: {
        received_reports: 3,
        total_tokens: 12_345,
        total_cost_usd: 0.42,
        wall_time_seconds: 125,
      },
    } as DashboardExecutionDetail
    const html = renderToStaticMarkup(
      <EvidenceBundleUnavailable detail={retained} />,
    )

    expect(html).toContain('Evidence bundle unavailable')
    expect(html).toContain('native bundle checksum mismatch')
    expect(html).toContain('Retained execution snapshot')
    expect(html).toContain('>3<')
    expect(html).toContain('12,345')
    expect(html).toContain('$0.4200')
    expect(html).toContain('2m 05s')
    expect(html).toContain('Full per-test metrics and evidence require')
    expect(html).not.toContain('0 tests')
    expect(html).not.toContain('no test results yet')
    expect(html).not.toContain('Execution not found')
  })
})

describe('execution layers', () => {
  // Audit ED-05: the system status is the only outcome the contract publishes.
  it('publishes the system status as the single execution outcome', () => {
    const presentation = buildExecutionPresentation(detail)
    expect(executionOutcome(presentation, [run])).toEqual({ value: 'passed' })
    expect(executionOutcome(presentation, [])).toEqual({ value: 'passed' })
  })

  it('includes later failures in the aggregate outcome instead of reporting only the first run', () => {
    expect(
      executionOutcome(buildExecutionPresentation(detail), [
        { ...run, systemStatus: 'passed' },
        { ...run, systemStatus: 'hard_gate_failed' },
      ]),
    ).toEqual({
      value: 'partial',
      label: '1 passed · 1 failed (legacy result)',
    })
  })

  it('lists provenance without null fields and with local timestamps', () => {
    const entries = provenanceEntries(
      {
        ...detail,
        run_id: 'run-1',
        attempt: 1,
        event: 'local',
        actor: 'layon',
        started_at: '2026-08-26T20:07:31Z',
        completed_at: '2026-08-26T20:11:31Z',
        source: {
          ref: null,
          repository: 'iii-hq/harness-e2e',
          sha: 'e34550995cE33809d0b9458f3689111faa8d3f0e'.toLowerCase(),
        },
        release: { registry_tag: null, stack_lock_digest: null },
      } as unknown as DashboardExecutionDetail,
      buildExecutionPresentation({
        ...detail,
        started_at: '2026-08-26T20:07:31Z',
        completed_at: '2026-08-26T20:11:31Z',
      } as unknown as DashboardExecutionDetail),
    )
    const byKey = Object.fromEntries(entries)
    expect(Object.keys(byKey)).not.toContain('release')
    expect(byKey.source).toBe(
      'repository iii-hq/harness-e2e · sha e34550995ce3',
    )
    expect(byKey.completed).toMatch(/· 4m 00s$/)
    expect(byKey.completed).not.toContain('2026-08-26T20:11:31Z')
    expect(byKey.actor).toBe('layon')
  })
})
