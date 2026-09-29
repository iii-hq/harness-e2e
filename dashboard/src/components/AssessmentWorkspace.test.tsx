import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { AssessmentDetailContent } from '@/components/AssessmentWorkspace'
import type {
  AssessmentRunView,
  AssessmentWorkspaceModel,
} from '@/lib/assessment-view'

const model: AssessmentWorkspaceModel = {
  availability: 'available',
  runs: [
    {
      key: 'subject:scenario:run:attempt',
      subjectId: 'codex/terra',
      scenarioId: 'direct_answer',
      behaviorSha256:
        'sha256:a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1',
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
      score: 35,
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

const render = (run: AssessmentRunView) =>
  renderToStaticMarkup(
    <AssessmentDetailContent run={run} entries={run.assessments} />,
  )

// What the Evidence record shows for a run without criteria.
describe('assessment detail', () => {
  it('renders the system outcome, the run metrics and the assessment matrix with its evidence', () => {
    const html = render(model.runs[0])
    expect(html).toContain('data-system-outcome')
    expect(html).toContain('data-primary-run-metrics')
    expect(html).toContain('35/100')
    expect(html).toContain('22,668')
    expect(html).toContain('aria-label="Run metrics"')
    expect(html).toContain('Cache written')
    expect(html).toContain('21,296')
    expect(html).toContain('161,280')
    expect(html).toContain('1m 02s')
    expect(html).toContain('durable_result')
    expect(html).toContain('data-evidence-target="technical"')
    expect(html).not.toContain('Evidence register')
    expect(html).not.toContain('hard gate')
    expect(html.indexOf('data-run-metrics-detail')).toBeLessThan(
      html.indexOf('durable_result'),
    )
  })

  it('surfaces security review capability metrics', () => {
    const html = render({
      ...model.runs[0],
      key: 'security-review',
      scenarioId: 'security_review',
      score: 38,
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
          criterionId: 'scan_commit_a.report.seeded_vulnerability_detection',
          dimension: 'deliverable',
          outcome: 'partial',
          score: { awarded: 75, possible: 100 },
          summary: 'Detected 3 of 4 explicitly seeded vulnerable paths.',
        },
        {
          ...model.runs[0].assessments[0],
          id: 'patches',
          criterionId: 'suggest_commit_a.report.suggested_patch_applicability',
          dimension: 'deliverable',
          outcome: 'partial',
          score: { awarded: 0, possible: 100 },
          summary:
            '0 of 4 optional suggested patches passed git apply --check.',
        },
      ],
    })
    expect(html).toContain('38/100')
    expect(html).not.toContain('75/200')
    expect(html).toContain('Seeded detection')
    expect(html).toContain('3/4')
    expect(html).toContain('75% of the possible score')
    expect(html).toContain('Optional patch checks')
    expect(html).toContain('0/4')
    expect(html).toContain('0% applied cleanly')
    // A passing run reads as passed, with no second, softer verdict beside it.
    const at = html.indexOf('system · completion, execution and infrastructure')
    const outcome = html.slice(at - 400, at + 100)
    expect(outcome).toContain('ds-status-passed')
    expect(outcome).not.toContain('ds-status-failed')
  })

  // Audit ED-25: passing on infrastructure alone is not the same as passing.
  it('says a run without assessments was only checked on execution and infrastructure', () => {
    const html = render({
      ...model.runs[0],
      systemStatus: 'subject_error',
      score: null,
      assessments: [],
    })
    expect(html).toContain('Only execution and infrastructure were checked')
    expect(html).toContain('nothing about the deliverable')
    expect(html).toContain('Not reported')
    expect(html).toContain('No assessments retained')
    expect(html).toContain('No assessments were retained for this run.')
    expect(html).not.toContain('0/0')
  })

  it('does not blame the subject for criteria an infrastructure failure never reached', () => {
    // The assessments exist, but the run died before any of them ran.
    const html = render({
      ...model.runs[0],
      systemStatus: 'infrastructure_error',
      score: null,
      metrics: { ...model.runs[0].metrics, durationMs: 100 },
      assessments: model.runs[0].assessments.map((entry) => ({
        ...entry,
        outcome: 'not_evaluated' as const,
        score: undefined,
      })),
    })
    expect(html).toContain('No score retained')
    expect(html).toContain('1 not evaluated')
    expect(html).not.toContain('1 failed')
    expect(html).not.toContain('1 need review')
    // The tiles measured nothing, so they are not painted as a failure.
    expect(html).not.toContain('[&_[data-metric-value]]:text-danger')
  })
})
