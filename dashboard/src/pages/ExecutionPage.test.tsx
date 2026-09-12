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
  CountsSection,
  countsScent,
  EvidenceBundleUnavailable,
  executionOutcome,
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
  objectiveScore: 100,
  assessments: [],
  evidence: [],
}

function scenarioSummary(overrides: Partial<ScenarioMatrixSummary> = {}) {
  return {
    total: 2,
    passed: 1,
    failed: 1,
    inconclusive: 0,
    unavailable: 0,
    running: 0,
    incomplete: 0,
    ...overrides,
  }
}

describe('execution verdict', () => {
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

  // Audit ED-03: one aggregated verdict, never a per-scenario headline
  // contradicting the objective one.
  it('aggregates the scenario outcomes into one sentence', () => {
    const verdict = executionVerdict(
      buildExecutionPresentation(detail),
      scenarioSummary({ failed: 2, passed: 3, total: 5 }),
      [],
    )
    expect(verdict.headline).toBe('2 failures · 3 passed')
    expect(verdict.nextStep).toBe(
      'Inspect the retained evidence of the failing scenario before deciding whether to re-run.',
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
    const verdict = executionVerdict(cancelled, null, [])
    expect(verdict.headline).toBe('cancelled · no scenario report retained')
    expect(verdict.nextStep).toBe('Re-run the same scope to obtain a report.')
    expect(verdict).not.toHaveProperty('diagnosis')
  })

  it('has nothing to act on when every scenario passed', () => {
    const verdict = executionVerdict(
      buildExecutionPresentation(detail),
      scenarioSummary({ total: 2, passed: 2, failed: 0 }),
      [],
    )
    expect(verdict.headline).toBe('2 passed')
    expect(verdict.nextStep).toBe('Nothing to act on: every scenario passed.')
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

  // Audit ED-26: the words live in a layer; its closed row says what they say.
  it('tells what to do next, and scents the closed row with it', () => {
    const presentation = buildExecutionPresentation(detail)
    const verdict = executionVerdict(presentation, scenarioSummary(), [])
    const html = renderToStaticMarkup(<NarrativeSection verdict={verdict} />)
    expect(html).toContain('next step')
    expect(html).toContain('Inspect the retained evidence')
    expect(html).not.toContain('what happened')
    expect(html).not.toContain('System: Passed')
    expect(html).not.toContain('AI: Pass With Concerns')
    expect(narrativeScent(verdict)).toBe(
      'Inspect the retained evidence of the failing scenario before deciding whether to re-run',
    )
  })

  it('keeps report coverage and retained assessments in the counts layer, headless', () => {
    const html = renderToStaticMarkup(
      <CountsSection
        detail={detail}
        presentation={buildExecutionPresentation(detail)}
        scenarioSummary={scenarioSummary()}
      />,
    )
    expect(html).toContain('1/2')
    expect(html).toContain('46')
    expect(html).toContain('17 evidence references')
    expect(html).not.toContain('reported cost')
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
        durationMs: 167_000,
      },
      {
        scenarioId: 'persistent_state',
        scenarioVersion: 1,
        objective: { label: 'Passed', status: 'passed', raw: 'passed' },
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
        durationMs: null,
      },
    ] as unknown as ScenarioMatrixItem[]
    // Scenarios are separated by a wider, non-collapsing gap than the facts
    // inside each one.
    expect(resultsScent(items)).toBe(
      [
        'minimal path v2 passed · 2m 47s',
        'persistent state v1 passed · 2m 08s',
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
})
