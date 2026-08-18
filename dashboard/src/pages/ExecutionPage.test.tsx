import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { AssessmentRunView } from '@/lib/assessment-view'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import { buildExecutionPresentation } from '@/lib/execution-view'
import { DecisionSection } from '@/pages/ExecutionPage'

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

describe('execution decision hierarchy', () => {
  it('keeps the objective outcome authoritative while surfacing AI guidance', () => {
    const html = renderToStaticMarkup(
      <DecisionSection
        detail={detail}
        presentation={buildExecutionPresentation(detail)}
        primaryRun={run}
      />,
    )

    expect(html).toContain('Passed objectively; advisory review found gaps')
    expect(html).toContain('System: Passed')
    expect(html).toContain('AI: Pass With Concerns')
    const effectiveBoundary = html.slice(
      html.indexOf('Effective harness'),
      html.indexOf('Effective harness') + 600,
    )
    expect(effectiveBoundary).toContain('ds-status-recommendation')
    expect(effectiveBoundary).not.toContain('ds-status-failed')
    expect(html).toContain(
      'Emit a complete per-path detection manifest and rerun.',
    )
    expect(html).toContain('Only three of four seeded paths were detected.')
    expect(html).toContain('46')
    expect(html).toContain('39')
    expect(html).toContain('7')
    expect(html).toContain('17')
  })
})
