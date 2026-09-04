import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { AssessmentRunView } from '@/lib/assessment-view'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import { executionVerdict } from '@/lib/execution-verdict'
import { buildExecutionPresentation } from '@/lib/execution-view'
import type { ScenarioMatrixSummary } from '@/lib/scenario-matrix'
import {
  buildSummaryExecutionMetrics,
  DecisionSection,
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
  scenarioVersion: 2,
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
  effectiveStatus: 'passed_with_concerns',
  assessments: [],
  evidence: [],
  hasAiDisagreement: false,
  finalAssessment: {
    availability: 'available',
    result: {
      verdict: 'pass_with_concerns',
      quality_score: 75,
      confidence: 0.82,
      summary: 'Objective checks passed, but detection evidence is incomplete.',
      facts: [],
      concerns: ['Only three of four seeded paths were detected.'],
      recommendation: 'Emit a complete per-path detection manifest and rerun.',
    },
  },
}

function scenarioSummary(overrides: Partial<ScenarioMatrixSummary> = {}) {
  return {
    total: 2,
    passed: 1,
    failed: 1,
    hardGate: 0,
    inconclusive: 0,
    unavailable: 0,
    running: 0,
    incomplete: 0,
    ...overrides,
  }
}

describe('execution verdict', () => {
  // Audit ED-03: one aggregated verdict, never a per-scenario headline
  // contradicting the objective one.
  it('aggregates the scenario outcomes into one sentence', () => {
    const verdict = executionVerdict(
      buildExecutionPresentation(detail),
      scenarioSummary({ hardGate: 1, failed: 1, passed: 3, total: 5 }),
      [],
      run,
    )
    expect(verdict.headline).toBe('1 failure · 1 hard gate failed · 3 passed')
    expect(verdict.nextStep).toBe(
      'Emit a complete per-path detection manifest and rerun.',
    )
  })

  it('says plainly when no report was retained', () => {
    const cancelled = buildExecutionPresentation({
      ...detail,
      status: 'cancelled',
      availability: 'unavailable',
      assessment_summary: undefined,
      totals: undefined,
    } as unknown as DashboardExecutionDetail)
    const verdict = executionVerdict(cancelled, null, [], null)
    expect(verdict.headline).toBe('cancelled · no scenario report retained')
    expect(verdict.nextStep).toBe('Re-run the same scope to obtain a report.')
    expect(verdict.diagnosis).toBeNull()
  })

  it('has nothing to act on when every scenario passed', () => {
    const verdict = executionVerdict(
      buildExecutionPresentation(detail),
      scenarioSummary({ total: 2, passed: 2, failed: 0 }),
      [],
      null,
    )
    expect(verdict.headline).toBe('2 passed')
    expect(verdict.nextStep).toBe('Nothing to act on: every scenario passed.')
  })
})

describe('execution decision section', () => {
  // Audit ED-05: the status is stated once; "effective" only when it differs.
  it('states the verdict once and keeps the boundaries as fields', () => {
    const html = renderToStaticMarkup(
      <DecisionSection
        detail={detail}
        presentation={buildExecutionPresentation(detail)}
        verdict={executionVerdict(
          buildExecutionPresentation(detail),
          scenarioSummary(),
          [],
          run,
        )}
        primaryRun={run}
        metrics={null}
        scenarioSummary={scenarioSummary()}
      />,
    )
    expect(html).toContain('>decision<')
    expect(html).toContain('1 failure · 1 passed')
    // The three outcomes read as one derivation: two inputs and what the
    // contract publishes, each named by its role.
    expect(html).toContain('data-outcome-derivation')
    expect(html).toContain('system · deterministic gates')
    expect(html).toContain('advisory · separate qualitative conclusion')
    expect(html).toContain('effective · the status the result contract')
    expect(html).toContain(
      'Emit a complete per-path detection manifest and rerun.',
    )
    expect(html).not.toContain('System: Passed')
    expect(html).not.toContain('AI: Pass With Concerns')
    expect(html).toContain('46')
    expect(html).toContain('17 evidence references')
    expect(html).toContain('not captured for this run')
  })

  it('hides the effective boundary when it repeats the objective one', () => {
    const presentation = buildExecutionPresentation(detail)
    const html = renderToStaticMarkup(
      <DecisionSection
        detail={detail}
        presentation={presentation}
        verdict={executionVerdict(presentation, scenarioSummary(), [], run)}
        primaryRun={{
          ...run,
          effectiveStatus: 'passed',
          systemStatus: 'passed',
        }}
        metrics={null}
        scenarioSummary={scenarioSummary()}
      />,
    )
    expect(html).toContain('system · deterministic gates')
    expect(html).not.toContain('effective · the status')
  })
})

describe('execution provenance', () => {
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

  it('prefers consolidated execution totals for primary usage metrics', () => {
    const metrics = buildSummaryExecutionMetrics(
      buildExecutionPresentation(detail),
      {
        totalTokens: 100,
        inputTokens: null,
        outputTokens: null,
        cacheReadTokens: null,
        cacheWriteTokens: null,
        reasoningTokens: null,
        functionCalls: 2,
        functionCallErrors: 1,
        durationMs: 1_000,
        sessions: null,
        turns: null,
      },
      1,
      null,
      {
        total_tokens: 445_900,
        function_calls: 58,
        function_call_errors: 3,
        total_cost_usd: 1.2345,
        turns: 24,
      },
    )

    expect(metrics).toMatchObject({
      totalTokens: 445_900,
      functionCalls: 58,
      functionCallErrors: 3,
      totalCostUsd: 1.2345,
      turns: 24,
    })
  })

  it('prefers the sum of retained test metrics over partial execution totals', () => {
    const metrics = buildSummaryExecutionMetrics(
      buildExecutionPresentation(detail),
      {
        totalTokens: 4_509,
        inputTokens: null,
        outputTokens: null,
        cacheReadTokens: null,
        cacheWriteTokens: null,
        reasoningTokens: null,
        functionCalls: 3,
        functionCallErrors: 0,
        durationMs: 1_000,
        sessions: null,
        turns: 7,
      },
      3,
      null,
      {
        total_tokens: 4_509,
        function_calls: 3,
        function_call_errors: 0,
      },
      {
        totalTokens: 9_145,
        functionCalls: 16,
        functionCallErrors: 0,
        costUsd: null,
        backfilled: true,
      },
    )

    expect(metrics).toMatchObject({
      totalTokens: 9_145,
      functionCalls: 16,
      functionCallErrors: 0,
      totalCostUsd: null,
      turns: 7,
    })
  })
})
