import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { ScenarioChatAction } from '@/components/ScenarioChatAction'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import { scenarioChatTargets } from '@/lib/scenario-chat'
import { ScenarioChatProvider } from '@/lib/scenario-chat-context'

function detail(): DashboardExecutionDetail {
  return {
    id: 'execution-1',
    reports: [
      {
        subject_id: 'openai/codex',
        scenario_id: 'direct_answer',
        available: true,
        report: {
          assessment_contract: {} as never,
          assessment_summary: {} as never,
          scenarios: [
            {
              scenario_id: 'direct_answer',
              scenario_version: 2,
              runs: [
                {
                  run_id: 'run-1',
                  attempt_id: 'attempt-2',
                  attempt_number: 2,
                  session_id: 'session-current',
                  status: 'passed',
                  assessment: {} as never,
                  retry_attempts: [
                    {
                      run_id: 'run-1',
                      attempt_id: 'attempt-1',
                      attempt_number: 1,
                      session_id: 'session-retry',
                      status: 'subject_error',
                    },
                  ],
                },
              ],
            },
          ],
        },
      },
    ],
  } as unknown as DashboardExecutionDetail
}

describe('scenario chat targets', () => {
  it('keeps the current attempt first and preserves retry sessions', () => {
    const targets = scenarioChatTargets(detail(), 'direct_answer')
    expect(
      targets.map((target) => ({
        sessionId: target.sessionId,
        attempt: target.attemptNumber,
        current: target.current,
      })),
    ).toEqual([
      { sessionId: 'session-current', attempt: 2, current: true },
      { sessionId: 'session-retry', attempt: 1, current: false },
    ])
  })

  it('filters a specific subject and logical run', () => {
    expect(
      scenarioChatTargets(detail(), 'direct_answer', 'openai/codex', 'run-1'),
    ).toHaveLength(2)
    expect(
      scenarioChatTargets(detail(), 'direct_answer', null, 'missing-run'),
    ).toEqual([])
  })
})

describe('scenario chat action', () => {
  function render(targetsDetail: DashboardExecutionDetail, enabled = true) {
    return renderToStaticMarkup(
      <ScenarioChatProvider openChat={enabled ? () => undefined : undefined}>
        <ScenarioChatAction detail={targetsDetail} scenarioId="direct_answer" />
      </ScenarioChatProvider>,
    )
  }

  it('shows a session count when a run retains retries', () => {
    const html = render(detail())
    expect(html).toContain('Chats · 2')
    expect(html).toContain('aria-haspopup="menu"')
  })

  it('uses a direct action for one session and hides without host support', () => {
    const value = detail()
    const run = value.reports[0].report?.scenarios[0].runs[0]
    if (!run) throw new Error('missing run fixture')
    run.retry_attempts = []
    expect(render(value)).toContain('Open chat')
    expect(render(value, false)).toBe('')
  })
})
