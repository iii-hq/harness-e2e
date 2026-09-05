import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { AssessmentRunView } from '@/lib/assessment-view'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import { executionVerdict } from '@/lib/execution-verdict'
import { buildExecutionPresentation } from '@/lib/execution-view'
import type {
  ScenarioMatrixItem,
  ScenarioMatrixSummary,
} from '@/lib/scenario-matrix'
import {
  buildSummaryExecutionMetrics,
  CountsSection,
  countsScent,
  executionBoundaries,
  NarrativeSection,
  narrativeScent,
  provenanceEntries,
  resultsScent,
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
  it('explains retained but incompatible history without suggesting an automatic re-run', () => {
    const unsupported = buildExecutionPresentation({
      ...detail,
      status: 'unsupported',
      availability: 'unsupported',
      first_failure: {
        kind: 'unsupported_results_schema',
        message: 'Results schema v3 cannot be compared with v4.',
      },
    })
    const verdict = executionVerdict(unsupported, null)
    expect(verdict.headline).toBe(
      'unsupported result contract · historical evidence retained',
    )
    expect(verdict.diagnosis).toBe(
      'Results schema v3 cannot be compared with v4.',
    )
    expect(verdict.nextStep).toContain('cannot be used as a baseline')
    expect(verdict.nextStep).not.toContain('Re-run')
  })

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

describe('execution layers', () => {
  // Audit ED-05: two inputs and, only when it differs, the published status.
  it('derives the outcome once and adds the effective status only when it differs', () => {
    const presentation = buildExecutionPresentation(detail)
    expect(executionBoundaries(presentation, run)).toEqual([
      { role: 'system', value: 'passed' },
      { role: 'advisory', value: 'pass_with_concerns' },
      { role: 'effective', value: 'passed_with_concerns' },
    ])
    expect(
      executionBoundaries(presentation, {
        ...run,
        effectiveStatus: 'passed',
        systemStatus: 'passed',
      }).map((row) => row.role),
    ).toEqual(['system', 'advisory'])
  })

  // Audit ED-26: the words live in a layer; its closed row says what they say.
  it('tells what happened and what to do, and scents the closed row with both', () => {
    const presentation = buildExecutionPresentation(detail)
    const verdict = executionVerdict(presentation, scenarioSummary(), [], run)
    const html = renderToStaticMarkup(<NarrativeSection verdict={verdict} />)
    expect(html).toContain('next step')
    expect(html).toContain(
      'Emit a complete per-path detection manifest and rerun.',
    )
    expect(html).not.toContain('System: Passed')
    expect(html).not.toContain('AI: Pass With Concerns')
    expect(narrativeScent(verdict)).toBe(
      'Emit a complete per-path detection manifest and rerun',
    )
    expect(
      narrativeScent({
        ...verdict,
        diagnosis: 'Only three of four seeded paths were detected. More text.',
      }),
    ).toBe(
      'Only three of four seeded paths were detected · Emit a complete per-path detection manifest and rerun',
    )
  })

  it('keeps report coverage and retained assessments in the counts layer, headless', () => {
    const html = renderToStaticMarkup(
      <CountsSection
        detail={detail}
        presentation={buildExecutionPresentation(detail)}
        metrics={null}
        scenarioSummary={scenarioSummary()}
      />,
    )
    expect(html).toContain('1/2')
    expect(html).toContain('46')
    expect(html).toContain('17 evidence references')
    expect(html).toContain('not captured for this run')
    // The layer row is the heading: no second title, no second anchor.
    expect(html).not.toContain('execution summary')
    expect(html).not.toContain('id="metrics"')
    expect(html).toContain('No compatible run evidence')
    expect(countsScent(detail)).toBe(
      'no compatible run evidence to consolidate · assessments 46, 17 evidence references',
    )
  })

  it('scents the results row with each scenario verdict and runtime', () => {
    const items = [
      {
        scenarioId: 'minimal_path',
        scenarioVersion: 2,
        objective: { label: 'Passed', status: 'passed', raw: 'passed' },
        advisory: { label: 'AI passed', status: 'passed' },
        durationMs: 167_000,
      },
      {
        scenarioId: 'persistent_state',
        scenarioVersion: 1,
        objective: { label: 'Passed', status: 'passed', raw: 'passed' },
        advisory: { label: 'AI concerns', status: 'recommendation' },
        durationMs: 128_000,
      },
      {
        scenarioId: 'research_pipeline',
        scenarioVersion: null,
        objective: {
          label: 'Unavailable',
          status: 'unavailable',
          raw: 'unavailable',
        },
        advisory: { label: 'No report', status: 'unavailable' },
        durationMs: null,
      },
    ] as unknown as ScenarioMatrixItem[]
    // Scenarios are separated by a wider, non-collapsing gap than the facts
    // inside each one.
    expect(resultsScent(items)).toBe(
      [
        'minimal path v2 passed · 2m 47s',
        'persistent state v1 passed, ai concerns · 2m 08s',
        'research pipeline unavailable',
      ].join(' \u00a0·\u00a0 '),
    )
    expect(resultsScent([])).toBe('no scenario report retained')
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
