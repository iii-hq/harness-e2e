import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  AssessmentDetailDialog,
  AssessmentPanel,
} from '@/components/AssessmentWorkspace'
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
      metrics: {
        totalTokens: 22668,
        inputTokens: 21296,
        outputTokens: 1372,
        cacheReadTokens: 161280,
        cacheWriteTokens: null,
        reasoningTokens: 706,
        functionCalls: 14,
        functionCallErrors: 0,
        durationMs: 62294,
        sessions: 1,
        turns: 16,
      },
      transcript: { messages: [] },
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
          diagnosis:
            'The durable-result hard gate failed because no value was persisted.',
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
      <AssessmentPanel
        model={model}
        filter="all"
        onTranscript={() => undefined}
      />,
    )
    const detailHtml = renderToStaticMarkup(
      <AssessmentDetailDialog
        run={model.runs[0]}
        onClose={() => undefined}
        onTranscript={() => undefined}
      />,
    )
    const rendered = `${html}${detailHtml}`
    expect(rendered).toContain('Objective system')
    expect(rendered).toContain('Advisory AI')
    expect(rendered).toContain('Effective harness')
    expect(rendered).toContain('run run-1')
    expect(rendered).not.toContain('attempt attempt-1')
    expect(rendered).toContain('data-primary-run-metrics')
    expect(rendered).toContain('Objective hard gates')
    expect(rendered).toContain('Assessment outcomes')
    expect(rendered).toContain('AI quality')
    expect(rendered).toContain('Tokens')
    expect(rendered).toContain('22,668')
    expect(rendered).toContain('Function calls')
    expect(rendered).toContain('14')
    expect(rendered).toContain('Duration')
    expect(rendered).toContain('1m 02s')
    expect(rendered).toContain('Function errors')
    expect(rendered).toContain('Runtime telemetry')
    expect(rendered).toContain('grid-flow-dense')
    expect(rendered).toContain('sm:grid-cols-2 lg:grid-cols-4')
    expect(rendered).toContain('Input tokens')
    expect(rendered).toContain('21,296')
    expect(rendered).toContain('Cache read')
    expect(rendered).toContain('161,280')
    expect(rendered).toContain('durable_result')
    expect(rendered).toContain('objective system outcome disagree')
    expect(rendered).not.toContain('Evidence register')
    expect(rendered).toContain('data-evidence-target="technical"')
    expect(rendered).not.toContain('href="#technical"')
    expect(rendered).toContain('90/100')
    expect(rendered).toContain('Diagnostic narrative')
    expect(rendered).toContain('Facts shown first')
    expect(rendered).toContain('role="tablist"')
    expect(rendered).toContain('aria-label="Diagnostic narrative sections"')
    expect(rendered).toContain('aria-label="AI-reported facts, 1 reported"')
    expect(rendered).toContain('title="1 reported"')
    expect(rendered).toContain(
      'border-2 border-brand bg-panel px-1.5 text-[0.7rem] font-bold leading-none tabular-nums text-ink',
    )
    expect(rendered).not.toContain('text-bg')
    expect(rendered.match(/role="tab"/g)).toHaveLength(4)
    expect(rendered).toContain('role="tabpanel"')
    expect(detailHtml).toContain('AI advisory')
    expect(detailHtml).toContain('Advisory guidance from the AI assessment')
    expect(detailHtml).toContain('What happened')
    expect(detailHtml).toContain(
      'The durable-result hard gate failed because no value was persisted.',
    )
    expect(detailHtml).toContain('Suggested correction or improvement')
    expect(detailHtml).toContain('Fix the objective gate before release.')
    expect(
      detailHtml.indexOf(
        'The durable-result hard gate failed because no value was persisted.',
      ),
    ).toBeLessThan(detailHtml.indexOf('Fix the objective gate before release.'))
    expect(detailHtml.indexOf('Objective hard gates')).toBeLessThan(
      detailHtml.indexOf('Outcome boundaries'),
    )
    expect(detailHtml.indexOf('Outcome boundaries')).toBeLessThan(
      detailHtml.indexOf('Advisory AI conclusion'),
    )
    expect(detailHtml.indexOf('Advisory AI conclusion')).toBeLessThan(
      detailHtml.indexOf('AI advisory'),
    )
    expect(rendered).toContain('Transcript')
    expect(rendered).toContain('data-transcript-action=')
    expect(html).toContain('Review evidence')
    expect(rendered.match(/<details[^>]*open/g) ?? []).toHaveLength(0)
    expect(html).toContain('Filter scenario runs by assessment signal')
    expect(rendered).toContain('Filter assessment matrix')
    expect(html).toContain('Open details for Direct Answer')
    expect(html).toContain('<details')
    expect(detailHtml).toContain('ds-dialog')
    expect(detailHtml).toContain('ds-dialog-lg')
    expect(detailHtml).not.toContain('data-transcript-action=')
    expect(detailHtml).not.toContain(
      'flex justify-end border-t border-line pt-3',
    )
    expect(detailHtml).toContain('Evidence record')
    expect(detailHtml).toContain('ds-dialog-header')
    expect(detailHtml).toContain('ds-dialog-actions')
  })

  it('surfaces security review capability metrics before evidence', () => {
    const originalResult = model.runs[0].finalAssessment.result
    if (!originalResult) throw new Error('expected final assessment fixture')
    const securityModel: AssessmentWorkspaceModel = {
      availability: 'available',
      runs: [
        {
          ...model.runs[0],
          key: 'security-review',
          scenarioId: 'security_review',
          systemStatus: 'passed',
          effectiveStatus: 'passed_with_concerns',
          hasAiDisagreement: false,
          assessments: [
            {
              ...model.runs[0].assessments[0],
              id: 'gate',
              criterionId: 'request_identity',
              outcome: 'passed',
              score: undefined,
            },
            {
              ...model.runs[0].assessments[0],
              id: 'detection',
              criterionId:
                'scan_commit_a.report.seeded_vulnerability_detection',
              policy: 'advisory',
              kind: 'signal',
              dimension: 'deliverable',
              outcome: 'partial',
              score: { awarded: 75, possible: 100 },
              summary: 'Detected 3 of 4 explicitly seeded vulnerable paths.',
            },
            {
              ...model.runs[0].assessments[0],
              id: 'patches',
              criterionId:
                'suggest_commit_a.report.suggested_patch_applicability',
              policy: 'advisory',
              kind: 'signal',
              dimension: 'deliverable',
              outcome: 'partial',
              score: { awarded: 0, possible: 100 },
              summary:
                '0 of 4 optional suggested patches passed git apply --check.',
            },
          ],
          finalAssessment: {
            availability: 'available',
            result: {
              ...originalResult,
              verdict: 'pass_with_concerns',
              quality_score: 75,
              confidence: 0.82,
            },
          },
        },
      ],
    }
    const html = renderToStaticMarkup(
      <AssessmentPanel model={securityModel} filter="all" />,
    )
    const detailHtml = renderToStaticMarkup(
      <AssessmentDetailDialog
        run={securityModel.runs[0]}
        onClose={() => undefined}
      />,
    )

    expect(html).toContain('Security Review')
    expect(html).toContain('Objective hard gates')
    expect(html).toContain('1/1')
    expect(html).toContain('Seeded detection')
    expect(html).toContain('3/4')
    expect(html).toContain('75% advisory coverage')
    expect(html).toContain('Optional patch checks')
    expect(html).toContain('0/4')
    expect(html).toContain('0% applied cleanly')
    expect(html).toContain('82% confidence')
    const effectiveIndex = detailHtml.indexOf('Effective harness')
    const effectiveBoundary = detailHtml.slice(
      effectiveIndex - 200,
      effectiveIndex + 500,
    )
    expect(effectiveBoundary).toContain('border-warning/30')
    expect(effectiveBoundary).not.toContain('border-danger/30')
  })

  // Audit AW-03 / AW-04: a run that retained no assessments gets neither a
  // filter bar over zero rows nor a "0/0 passed" outcome tile.
  it('drops the filter bar and reports unavailable outcomes for a run without assessments', () => {
    const emptyRun = {
      ...model.runs[0],
      key: 'judge-error',
      systemStatus: 'judge_error' as const,
      effectiveStatus: 'judge_error' as const,
      hasAiDisagreement: false,
      assessments: [],
      finalAssessment: { availability: 'unavailable' as const },
    }
    const html = renderToStaticMarkup(
      <AssessmentPanel
        model={{ availability: 'available', runs: [emptyRun] }}
        filter="all"
      />,
    )
    expect(html).not.toContain('Filter scenario runs by assessment signal')
    expect(html).toContain('Objective result')
    expect(html).toContain('Judge Error')
    expect(html).toContain('Assessment outcomes')
    expect(html).toContain('No assessments retained')
    expect(html).not.toContain('0/0')
    expect(html).not.toContain('bg-success/5')
    const detailHtml = renderToStaticMarkup(
      <AssessmentDetailDialog run={emptyRun} onClose={() => undefined} />,
    )
    expect(detailHtml).toContain('No assessments were retained for this run.')
    expect(detailHtml).not.toContain('border-t-[3px]')
    expect(detailHtml).toContain('tabindex="-1"')
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
