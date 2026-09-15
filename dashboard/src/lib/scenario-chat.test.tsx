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
              behavior_sha256:
                'sha256:a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1',
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
  it('loads imported transcripts and retries with the same identity filters', async () => {
    const local = detail()
    const scenario = local.reports[0].report?.scenarios[0]
    if (!scenario) throw new Error('missing scenario fixture')
    const imported = {
      ...local,
      id: 'remote-execution-1',
      origin: 'remote',
      reports: [],
      remote_reference: {
        runs: [
          {
            scenarioId: scenario.scenario_id,
            behaviorSha256: scenario.behavior_sha256,
            identity: { subjectModel: 'openai/codex' },
            record: scenario.runs[0],
          },
        ],
      },
    } as DashboardExecutionDetail
    const getExecution = vi.fn().mockResolvedValue(imported)
    vi.mocked(getDashboardDataBridge).mockResolvedValue({
      getExecution,
    } as never)
    const targets = await loadScenarioChatTargets({
      executionId: imported.id,
      scenarioId: 'direct_answer',
      subjectId: 'openai/codex',
      runId: 'run-1',
      behaviorSha256: scenario.behavior_sha256,
    })
    expect(targets).toEqual(
      scenarioChatTargets(local, 'direct_answer').map((target) => ({
        ...target,
        executionId: imported.id,
      })),
    )
    expect(getExecution).toHaveBeenCalledWith(imported.id)
    expect(scenarioChatTargets(imported, 'other_test')).toEqual([])
    expect(
      scenarioChatTargets(imported, 'direct_answer', 'other-model'),
    ).toEqual([])
    expect(
      scenarioChatTargets(imported, 'direct_answer', null, 'other-run'),
    ).toEqual([])
    expect(
      scenarioChatTargets(imported, 'direct_answer', null, null, null),
    ).toEqual([])
    imported.remote_reference = { runs: [{ scenarioId: 'direct_answer' }] }
    expect(scenarioChatTargets(imported, 'direct_answer')).toEqual([])
  })

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

  it('keeps transcript inspection within the selected definition', () => {
    const value = detail()
    const other = structuredClone(value.reports[0])
    const scenario = other.report?.scenarios[0]
    if (!scenario) throw new Error('fixture requires a scenario')
    scenario.behavior_sha256 =
      'sha256:c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3'
    scenario.runs[0].session_id = 'other-definition-session'
    scenario.runs[0].retry_attempts = []
    value.reports.push(other)
    expect(
      scenarioChatTargets(
        value,
        'direct_answer',
        null,
        null,
        'sha256:a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1',
      ).map((target) => target.sessionId),
    ).toEqual(['session-current', 'session-retry'])
    expect(
      scenarioChatTargets(
        value,
        'direct_answer',
        null,
        null,
        'sha256:c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3',
      ).map((target) => target.sessionId),
    ).toEqual(['other-definition-session'])
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
