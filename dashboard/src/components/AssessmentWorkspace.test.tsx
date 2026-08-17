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
    expect(rendered).toContain('data-run-metrics')
    expect(rendered).toContain('Tokens')
    expect(rendered).toContain('22,668')
    expect(rendered).toContain('Functions')
    expect(rendered).toContain('14')
    expect(rendered).toContain('Duration')
    expect(rendered).toContain('1m 02s')
    expect(rendered).toContain('Function errors')
    expect(rendered).toContain('Runtime metrics')
    expect(rendered).toContain(
      'min-h-10 min-w-0 grid-cols-[minmax(0,1fr)_auto] items-center gap-1 overflow-hidden',
    )
    expect(rendered).toContain('shrink-0 whitespace-nowrap text-xl')
    expect(rendered).toContain('sm:w-56')
    expect(rendered).toContain('Input tokens')
    expect(rendered).toContain('21,296')
    expect(rendered).toContain('Cache read')
    expect(rendered).toContain('161,280')
    expect(rendered).toContain('durable_result')
    expect(rendered).toContain('objective system outcome disagree')
    expect(rendered).not.toContain('Evidence register')
    expect(rendered).toContain('href="#technical"')
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
    expect(detailHtml).toContain('AI recommended next steps')
    expect(detailHtml).toContain('Advisory guidance from the AI assessment')
    expect(detailHtml).toContain('Fix the objective gate before release.')
    expect(detailHtml.indexOf('Advisory AI conclusion')).toBeLessThan(
      detailHtml.indexOf('AI recommended next steps'),
    )
    expect(detailHtml.indexOf('AI recommended next steps')).toBeLessThan(
      detailHtml.indexOf('Outcome boundaries'),
    )
    expect(rendered).toContain('Chat')
    expect(rendered).toContain('data-transcript-action=')
    expect(html).toContain('pointer-events-none absolute right-4 bottom-3')
    expect(html).not.toContain(
      'flex justify-end border-t border-line px-4 py-2',
    )
    expect(rendered.match(/<details[^>]*open/g) ?? []).toHaveLength(0)
    expect(rendered).toContain('Filter assessment matrix')
    expect(html).toContain('Open details for Direct Answer')
    expect(html).not.toContain('<details')
    expect(detailHtml).toContain('assessment-detail-dialog')
    expect(detailHtml).toContain('m-auto')
    expect(detailHtml).not.toContain('data-transcript-action=')
    expect(detailHtml).not.toContain(
      'flex justify-end border-t border-line pt-3',
    )
    expect(detailHtml).toContain('Scenario detail')
    expect(detailHtml).toContain('assessment-detail-header')
    expect(detailHtml).toContain('assessment-detail-actions')
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
