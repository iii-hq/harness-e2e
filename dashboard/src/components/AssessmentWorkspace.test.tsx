import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { AssessmentPanel } from '@/components/AssessmentWorkspace'
import type { AssessmentWorkspaceModel } from '@/lib/assessment-view'

const model: AssessmentWorkspaceModel = {
  availability: 'available',
  runs: [
    {
      key: 'subject:scenario:run:attempt',
      subjectId: 'codex/terra',
      scenarioId: 'direct_answer',
      scenarioVersion: 4,
      runId: 'run-1',
      attemptId: 'attempt-1',
      systemStatus: 'hard_gate_failed',
      effectiveStatus: 'hard_gate_failed',
      hasAiDisagreement: true,
      assessments: [
        {
          id: 'assessment:durable_result',
          criterionId: 'durable_result',
          targetId: 'durable_result',
          kind: 'required_check',
          policy: 'hard_gate',
          dimension: 'structural_integrity',
          source: 'deterministic',
          outcome: 'failed',
          score: { awarded: 0, possible: 70 },
          summary: 'The durable result was not observed.',
          evidence: [
            {
              artifact_id: 'transcript',
              artifact_sha256: `sha256:${'a'.repeat(64)}`,
              locator: '/messages/4',
            },
          ],
        },
      ],
      finalAssessment: {
        availability: 'available',
        result: {
          verdict: 'pass',
          quality_score: 90,
          confidence: 0.8,
          summary: 'The response reads well.',
          facts: ['A response was produced.'],
          strengths: ['Clear wording'],
          concerns: ['The required durable result is missing.'],
          recommendation: 'Fix the objective gate before release.',
          limitations: ['One sample'],
          evidence: [
            {
              artifact_id: 'transcript',
              artifact_sha256: `sha256:${'a'.repeat(64)}`,
              locator: '/messages/4',
            },
          ],
        },
      },
      evidence: [
        {
          artifact_id: 'transcript',
          artifact_sha256: `sha256:${'a'.repeat(64)}`,
          locator: '/messages/4',
        },
      ],
    },
  ],
}

describe('assessment workspace component', () => {
  it('renders separated outcome boundaries, rubric identity, disagreement, and evidence links', () => {
    const html = renderToStaticMarkup(
      <AssessmentPanel model={model} filter="all" />,
    )
    expect(html).toContain('Objective system')
    expect(html).toContain('Advisory AI')
    expect(html).toContain('Effective harness')
    expect(html).toContain('durable_result')
    expect(html).toContain('objective system outcome disagree')
    expect(html).toContain('Evidence register')
    expect(html).toContain('href="#evidence-')
    expect(html).toContain('Filter assessment matrix')
  })

  it('renders explicit legacy and unavailable states without a default verdict', () => {
    const legacy = renderToStaticMarkup(
      <AssessmentPanel
        model={{ availability: 'legacy', runs: [] }}
        filter="all"
      />,
    )
    const unavailable = renderToStaticMarkup(
      <AssessmentPanel
        model={{ availability: 'unavailable', runs: [] }}
        filter="all"
      />,
    )
    expect(legacy).toContain('has no assessment contract')
    expect(unavailable).toContain('Assessment data is unavailable')
    expect(unavailable).toContain(
      'No status or AI conclusion has been inferred',
    )
  })
})
