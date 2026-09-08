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
      systemStatus: 'passed',
      objectiveScore: 35,
      assessments: [
        {
          id: 'assessment:durable_result',
          criterionId: 'durable_result',
          targetId: 'durable_result',
          kind: 'signal',
          policy: 'advisory',
          dimension: 'structural_integrity',
          outcome: 'failed',
          score: { awarded: 35, possible: 70 },
          summary: 'The durable result was partially observed.',
          evidence: [
            {
              artifact_id: 'transcript',
              artifact_sha256: `sha256:${'a'.repeat(64)}`,
              locator: '/messages/4',
            },
          ],
        },
      ],
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
  it('renders the system outcome, the assessment matrix and its evidence links', () => {
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
    // One status, in the same shape the execution page uses.
    expect(rendered).toContain('data-system-outcome')
    expect(rendered).toContain(
      'system · completion, execution and infrastructure',
    )
    expect(rendered).not.toContain('advisory ·')
    expect(rendered).not.toContain('effective ·')
    expect(rendered).toContain('run run-1')
    expect(rendered).not.toContain('attempt attempt-1')
    expect(rendered).toContain('data-primary-run-metrics')
    expect(rendered).toContain('Objective score')
    expect(rendered).toContain('35/100')
    expect(rendered).toContain('Assessment outcomes')
    expect(rendered).toContain('Subject tokens')
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
    expect(rendered).not.toContain('Evidence register')
    expect(rendered).toContain('data-evidence-target="technical"')
    expect(rendered).not.toContain('href="#technical"')
    // Nothing is left of the advisory AI conclusion or its narrative.
    expect(rendered).not.toContain('Advisory AI conclusion')
    expect(rendered).not.toContain('Diagnostic narrative')
    expect(rendered).not.toContain('AI-reported facts')
    expect(rendered).not.toContain('role="tablist"')
    expect(rendered).not.toContain('Analyzer provenance')
    expect(rendered).not.toContain('confidence')
    expect(detailHtml).toContain('Suggested next step')
    expect(detailHtml).not.toContain('hard gate')
    expect(detailHtml.indexOf('Objective score')).toBeLessThan(
      detailHtml.indexOf('System outcome'),
    )
    expect(detailHtml.indexOf('System outcome')).toBeLessThan(
      detailHtml.indexOf('Suggested next step'),
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
    const securityModel: AssessmentWorkspaceModel = {
      availability: 'available',
      runs: [
        {
          ...model.runs[0],
          key: 'security-review',
          scenarioId: 'security_review',
          systemStatus: 'passed',
          objectiveScore: 38,
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
    expect(html).toContain('Objective score')
    expect(html).toContain('38/100')
    expect(html).not.toContain('75/200')
    expect(html).toContain('Seeded detection')
    expect(html).toContain('3/4')
    expect(html).toContain('75% of the possible score')
    expect(html).toContain('Optional patch checks')
    expect(html).toContain('0/4')
    expect(html).toContain('0% applied cleanly')
    // A passing run reads as passed, with no second, softer verdict beside it.
    const outcomeIndex = detailHtml.indexOf(
      'system · completion, execution and infrastructure',
    )
    const outcome = detailHtml.slice(outcomeIndex - 400, outcomeIndex + 100)
    expect(outcome).toContain('ds-status-passed')
    expect(outcome).not.toContain('ds-status-failed')
  })

  // Audit AW-03 / AW-04: a run that retained no assessments gets neither a
  // filter bar over zero rows nor a "0/0 passed" outcome tile.
  it('drops the filter bar and reports unavailable outcomes for a run without assessments', () => {
    const emptyRun = {
      ...model.runs[0],
      key: 'judge-error',
      systemStatus: 'judge_error' as const,
      objectiveScore: null,
      assessments: [],
    }
    const html = renderToStaticMarkup(
      <AssessmentPanel
        model={{ availability: 'available', runs: [emptyRun] }}
        filter="all"
      />,
    )
    expect(html).not.toContain('Filter scenario runs by assessment signal')
    expect(html).toContain('Objective score')
    expect(html).toContain('Not reported')
    expect(html).toContain('Assessment outcomes')
    expect(html).toContain('No assessments retained')
    expect(html).not.toContain('0/0')
    expect(html).not.toContain('bg-success/5')
    const detailHtml = renderToStaticMarkup(
      <AssessmentDetailDialog run={emptyRun} onClose={() => undefined} />,
    )
    expect(detailHtml).toContain('No assessments were retained for this run.')
    // Audit ED-25: passing on infrastructure alone is not the same as passing.
    expect(detailHtml).toContain(
      'only execution and infrastructure were checked',
    )
    expect(detailHtml).toContain('nothing about the deliverable')
    expect(detailHtml).not.toContain('border-t-[3px]')
    expect(detailHtml).toContain('tabindex="-1"')
  })

  it('does not blame the subject for criteria an infrastructure failure never reached', () => {
    // The real shape of a technical failure: the assessments exist, but the run
    // died before any of them ran.
    const abortedRun = {
      ...model.runs[0],
      key: 'infrastructure-error',
      systemStatus: 'infrastructure_error' as const,
      objectiveScore: null,
      metrics: { ...model.runs[0].metrics, durationMs: 100 },
      assessments: model.runs[0].assessments.map((entry) => ({
        ...entry,
        outcome: 'not_evaluated' as const,
        score: undefined,
      })),
    }
    const html = renderToStaticMarkup(
      <AssessmentPanel
        model={{ availability: 'available', runs: [abortedRun] }}
        filter="all"
      />,
    )
    expect(html).toContain('No objective score retained')
    expect(html).toContain('1 not evaluated')
    // The old projection counted not_evaluated as a failure on the subject.
    expect(html).not.toContain('1 failed')
    expect(html).not.toContain('1 need review')
    // The system status is genuinely an error and keeps its red badge; the
    // metric tiles must not be, since they measured nothing.
    expect(html).toContain('System: Infrastructure Error')
    expect(html).not.toContain('[&_[data-metric-value]]:text-danger')
  })

  it('reads each criterion of the contract against what the run did with it', () => {
    const spec = {
      prompt: 'Store the durable result.',
      criteria: [
        {
          id: 'durable_result',
          weight: 70,
          description: 'The durable result must be observable after the run.',
          kind: 'required_check' as const,
          policy: 'hard_gate' as const,
          dimension: 'structural_integrity' as const,
        },
        {
          id: 'never_reported',
          weight: 30,
          description: 'A criterion this run never reported on.',
          kind: 'signal' as const,
          policy: 'advisory' as const,
          dimension: 'efficiency' as const,
        },
      ],
      execution: { max_turns: 12, stuck_timeout_seconds: 300 },
      denied_functions: [],
    }
    const html = renderToStaticMarkup(
      <AssessmentPanel
        model={model}
        filter="all"
        spec={spec}
        onTranscript={() => undefined}
      />,
    )
    expect(html).toContain('what this test required')
    // The requirement comes from the contract...
    expect(html).toContain('must be observable after the run')
    // ...and the verdict from the run.
    expect(html).toContain('failed')
    // A criterion the run never reported still shows what it demanded.
    expect(html).toContain('never_reported')
    expect(html).toContain('not evaluated')
  })

  it('renders unavailable assessments without a default verdict', () => {
    const unavailable = renderToStaticMarkup(
      <AssessmentPanel
        model={{ availability: 'unavailable', runs: [] }}
        filter="all"
      />,
    )
    expect(unavailable).toContain('Assessment data is unavailable')
    expect(unavailable).toContain('No status has been inferred')
  })
})
