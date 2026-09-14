import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { AssessmentRunView } from '@/lib/assessment-view'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import {
  executionMetricCards,
  executionOutcome,
  executionSummarySentence,
  liveStateCopy,
  provenanceEntries,
  resultFilterCounts,
  snapshotMetricCards,
  verdictVariant,
} from '@/lib/execution-detail'
import { executionVerdict } from '@/lib/execution-verdict'
import { buildExecutionPresentation } from '@/lib/execution-view'
import type {
  ScenarioMatrixItem,
  ScenarioMatrixSummary,
} from '@/lib/scenario-matrix'
import { ResultsTable } from '@/pages/ExecutionPage'

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
    totalTokens: 4_182,
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

const items = [
  {
    key: 'terra:security_review',
    subjectId: 'terra',
    scenarioId: 'security_review',
    behaviorSha256:
      'sha256:a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1',
    objective: { label: 'Passed', status: 'passed', raw: 'passed' },
    reason: null,
    durationMs: 151_460,
    durationKind: 'single',
    runCount: 1,
    runs: [],
    aggregate: { mean_score: 100, total_tokens_consumed: 4_182 },
  },
  {
    key: 'terra:research_pipeline',
    subjectId: 'terra',
    scenarioId: 'research_pipeline',
    behaviorSha256: null,
    objective: { label: 'Failed', status: 'failed', raw: 'failed' },
    reason: '1 subject model event',
    durationMs: null,
    durationKind: null,
    runCount: 0,
    runs: [],
    aggregate: null,
  },
] as unknown as ScenarioMatrixItem[]

describe('execution verdict', () => {
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
  })

  it('colours the verdict panel by the aggregate outcome', () => {
    expect(
      verdictVariant(scenarioSummary({ total: 2, passed: 2, failed: 0 })),
    ).toBe('success')
    expect(verdictVariant(scenarioSummary())).toBe('alert')
    expect(
      verdictVariant(
        scenarioSummary({ total: 2, passed: 1, failed: 0, inconclusive: 1 }),
      ),
    ).toBe('warn')
    expect(verdictVariant(null)).toBe('warn')
  })

  it('publishes the system status as the single execution outcome', () => {
    const presentation = buildExecutionPresentation(detail)
    expect(executionOutcome(presentation, [run])).toEqual({ value: 'passed' })
    expect(
      executionOutcome(presentation, [
        { ...run, systemStatus: 'passed' },
        { ...run, systemStatus: 'hard_gate_failed' },
      ]),
    ).toEqual({
      value: 'partial',
      label: '1 passed · 1 failed (legacy result)',
    })
  })
})

describe('execution numbers', () => {
  it('states one sentence under the title', () => {
    expect(
      executionSummarySentence({
        detail,
        live: false,
        scenarioSummary: scenarioSummary(),
        runCount: 3,
      }),
    ).toBe('2 tests · 3 runs')
    expect(
      executionSummarySentence({
        detail,
        live: true,
        scenarioSummary: null,
        runCount: 0,
      }),
    ).toBe('Execution in progress · results are provisional')
  })

  it('describes the five metric cards from the retained detail', () => {
    const cards = executionMetricCards(detail, scenarioSummary())
    expect(cards.map((card) => card.label)).toEqual([
      'tests',
      'score',
      'completion',
      'runtime',
      'tokens',
    ])
    expect(cards[0]).toMatchObject({
      value: '1/2',
      detail: '1 failed',
      tone: 'negative',
    })
    expect(cards[1]).toMatchObject({ value: '—', tone: 'unavailable' })
  })

  it('keeps the retained totals when the evidence bundle is unavailable', () => {
    const cards = snapshotMetricCards({
      ...detail,
      totals: {
        received_reports: 3,
        total_tokens: 12_345,
        total_cost_usd: 0.42,
        wall_time_seconds: 125,
      },
    } as unknown as DashboardExecutionDetail)
    expect(cards.map((card) => card.value)).toEqual([
      '3',
      '12,345',
      '$0.4200',
      '2m 05s',
    ])
    expect(cards.every((card) => card.tone === 'neutral')).toBe(true)
  })

  it('says how far a live execution got and for how long', () => {
    const presentation = buildExecutionPresentation({
      ...detail,
      status: 'running',
      started_at: '2026-08-26T20:07:31Z',
      totals: { expected_reports: 4, received_reports: 1 },
    } as unknown as DashboardExecutionDetail)
    const copy = liveStateCopy(
      presentation,
      false,
      Date.parse('2026-08-26T20:09:31Z'),
    )
    expect(copy.headline).toBe('running · 1 of 4 tests · 2m 00s elapsed')
    expect(copy.note).toContain('follows recorded progress')
  })

  it('counts the result filters in severity order', () => {
    expect(resultFilterCounts(items)).toEqual([
      ['failed', 1],
      ['passed', 1],
    ])
  })
})

describe('execution results table', () => {
  it('renders one row per test on the Console table, opening onto its runs', () => {
    const html = renderToStaticMarkup(
      <ResultsTable
        items={items}
        runs={[run]}
        detail={detail}
        expanded={new Set(['terra:security_review'])}
        onToggle={() => {}}
        onTranscript={() => {}}
      />,
    )
    expect(html).toContain('class="iii-ui-table" data-density="compact"')
    expect(html).toContain('data-scenario-id="security_review"')
    expect(html).toMatch(/data-badge-variant="ok"[^>]*>passed</)
    expect(html).toMatch(/data-badge-variant="alert"[^>]*>failed</)
    expect(html).toContain('1 subject model event')
    expect(html).toContain('definition a1a1a1a1')
    // The expanded test lists its runs with the evidence route.
    expect(html).toContain('data-run-id="run-1"')
    expect(html).toContain('execution/execution-1/run/run-1')
    expect(html).toContain('4,182')
    expect(html).toContain('2m 31s')
    // The collapsed test does not.
    expect(html).not.toContain('data-scenario-detail="research_pipeline"')
    expect(html).toContain('aria-expanded="true"')
    expect(html).toContain('aria-expanded="false"')
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
