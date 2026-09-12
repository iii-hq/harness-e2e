import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it, vi } from 'vitest'
import { ScenarioChatAction } from '@/components/ScenarioChatAction'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import { getDashboardDataBridge } from '@/lib/dashboard-data-source'
import {
  loadScenarioChatTargets,
  scenarioChatTargets,
} from '@/lib/scenario-chat'

vi.mock('@/lib/dashboard-data-source', () => ({
  getDashboardDataBridge: vi.fn(),
}))

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
                  transcript: {
                    messages: [{ role: 'assistant', content: 'done' }],
                  },
                  status: 'passed',
                  assessment: {} as never,
                  retry_attempts: [
                    {
                      run_id: 'run-1',
                      attempt_id: 'attempt-1',
                      attempt_number: 1,
                      session_id: 'session-retry',
                      transcript: {
                        messages: [{ role: 'assistant', content: 'retry' }],
                      },
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
        messages: target.messages,
      })),
    ).toEqual([
      {
        sessionId: 'session-current',
        attempt: 2,
        current: true,
        messages: [{ role: 'assistant', content: 'done' }],
      },
      {
        sessionId: 'session-retry',
        attempt: 1,
        current: false,
        messages: [{ role: 'assistant', content: 'retry' }],
      },
    ])
  })

  it('keeps transcript inspection within the selected test version', () => {
    const value = detail()
    const other = structuredClone(value.reports[0])
    const scenario = other.report!.scenarios[0]
    scenario.scenario_version = 3
    scenario.runs[0].session_id = 'version-3-session'
    scenario.runs[0].retry_attempts = []
    value.reports.push(other)
    expect(
      scenarioChatTargets(value, 'direct_answer', null, null, 2).map(
        (target) => target.sessionId,
      ),
    ).toEqual(['session-current', 'session-retry'])
    expect(
      scenarioChatTargets(value, 'direct_answer', null, null, 3).map(
        (target) => target.sessionId,
      ),
    ).toEqual(['version-3-session'])
    expect(
      scenarioChatTargets(value, 'direct_answer', null, null, null),
    ).toEqual([])
  })

  it('reloads retained attempts after an active execution gains evidence', async () => {
    const empty = { ...detail(), status: 'running', reports: [] }
    const current = detail()
    const getExecution = vi
      .fn()
      .mockResolvedValueOnce(empty)
      .mockResolvedValueOnce(current)
    vi.mocked(getDashboardDataBridge).mockResolvedValue({
      getExecution,
    } as never)
    const source = { executionId: 'execution-1', scenarioId: 'direct_answer' }
    expect(await loadScenarioChatTargets(source)).toEqual([])
    expect(
      (await loadScenarioChatTargets(source)).map((target) => target.sessionId),
    ).toEqual(['session-current', 'session-retry'])
    expect(getExecution).toHaveBeenCalledTimes(2)
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
  function render(targetsDetail: DashboardExecutionDetail) {
    return renderToStaticMarkup(
      <ScenarioChatAction detail={targetsDetail} scenarioId="direct_answer" />,
    )
  }

  it('shows a session count when a run retains retries', () => {
    const html = render(detail())
    expect(html).toContain('Transcripts · 2')
    expect(html).toContain('aria-haspopup="menu"')
  })

  it('offers a retained transcript without host chat integration', () => {
    const value = detail()
    const run = value.reports[0].report?.scenarios[0].runs[0]
    if (!run) throw new Error('missing run fixture')
    run.retry_attempts = []
    expect(render(value)).toContain('View transcript')
  })
})
