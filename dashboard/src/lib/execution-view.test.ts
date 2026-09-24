import { describe, expect, it } from 'vitest'
import type { DashboardExecutionSummary } from '@/lib/dashboard-data-source'
import {
  attentionState,
  buildExecutionPresentation,
  executionProgress,
  executionTitle,
  failureBreakdown,
  formatDuration,
  primaryIssue,
  providerModel,
  workerVersion,
} from '@/lib/execution-view'

function execution(
  overrides: Partial<DashboardExecutionSummary> = {},
): DashboardExecutionSummary {
  return {
    id: 'execution-1',
    label: 'Ten case audit',
    status: 'technical_failed',
    subjects: [
      {
        id: 'terra',
        model: 'gpt-5.6-terra',
        provider: 'openai-codex',
        scenarios: [],
      },
    ],
    assessment_summary: {
      run_count: 10,
      assessment_count: 0,
      asset_count: 0,
      evidence_reference_count: 0,
      system_statuses: {
        passed: 5,
        hard_gate_failed: 3,
        infrastructure_error: 1,
        resource_limit: 1,
        subject_error: 0,
        unavailable: 0,
      },
      assessment_outcomes: {} as never,
      asset_validation_outcomes: {} as never,
    },
    totals: {
      expected_reports: 10,
      received_reports: 10,
      passed_scenarios: 5,
      scenario_pass_rate: 0.5,
      report_coverage: 1,
      wall_time_seconds: 1501,
    },
    ...overrides,
  }
}

describe('execution presentation view model', () => {
  it('ignores legacy gate outcomes when classifying operational failures', () => {
    const value = failureBreakdown(execution())
    expect(value).toMatchObject({
      passed: 5,
      infrastructure: 1,
      resource_limit: 1,
      total: 7,
      issues: 2,
    })
    expect(primaryIssue(value)).toEqual({
      category: 'infrastructure',
      count: 1,
    })
    expect(attentionState(execution(), value)).toBe('needs_attention')
  })

  it('uses friendly identity and model provenance in the presentation', () => {
    const presentation = buildExecutionPresentation(execution())
    expect(presentation.label).toBe('Ten case audit')
    expect(presentation.subjects[0]).toEqual({
      provider: 'openai-codex',
      model: 'gpt-5.6-terra',
    })
  })

  it('treats an execution with only passed scenarios as healthy', () => {
    const passed = execution({
      status: 'passed',
      assessment_summary: {
        ...execution().assessment_summary,
        system_statuses: { passed: 10 } as never,
      } as never,
    })
    expect(buildExecutionPresentation(passed).attention).toBe('passed')
  })
})

describe('execution identity', () => {
  it('titles an unlabelled execution by its subject and date', () => {
    const labelled = buildExecutionPresentation(
      execution({
        id: 'a',
        label: 'e2e::* control-plane run',
        workflow_name: 'e2e::* control-plane run',
      }),
    )
    expect(executionTitle(labelled)).toEqual({
      title: 'e2e::* control-plane run',
      detail: 'e2e::* control-plane run',
    })
    const unlabelled = buildExecutionPresentation(
      execution({
        id: 'b',
        label: undefined,
        workflow_name: 'e2e::* control-plane run',
      }),
    )
    expect(executionTitle(unlabelled).title).toMatch(/^gpt-5\.6-terra · /)
    expect(executionTitle(unlabelled).detail).toBe('e2e::* control-plane run')
  })
})

describe('an execution as it runs', () => {
  it('keeps one title from start to end, dated by its creation', () => {
    const titles = [
      {
        status: 'running',
        completed_at: '',
        generated_at: '2026-09-23T07:52:00Z',
      },
      {
        status: 'running',
        completed_at: '',
        generated_at: '2026-09-23T08:04:00Z',
      },
      { status: 'passed', completed_at: '2026-09-23T09:30:00Z' },
    ].map(
      (moment) =>
        executionTitle(
          buildExecutionPresentation(
            execution({
              label: '',
              started_at: '2026-09-23T07:51:00Z',
              ...moment,
            }),
          ),
        ).title,
    )
    expect(new Set(titles).size).toBe(1)
    expect(titles[0]).toMatch(/^gpt-5\.6-terra · Sep 23, 2026/)
  })

  it('reads its progress as slots done of those planned', () => {
    expect(
      executionProgress(
        execution({
          status: 'running',
          plan_execution: { planned: 9, finished: 1 },
        } as Partial<DashboardExecutionSummary>),
      ),
    ).toBe('1 of 9 done')
    expect(
      executionProgress(
        execution({
          status: 'running',
          live_progress: { runs_committed: 2, planned_slots: 4 },
        } as Partial<DashboardExecutionSummary>),
      ),
    ).toBe('2 of 4 done')
    // Finished, or with nothing planned yet (an import), it says nothing.
    expect(
      executionProgress(
        execution({
          plan_execution: { planned: 9, finished: 9 },
        } as Partial<DashboardExecutionSummary>),
      ),
    ).toBeNull()
    expect(
      executionProgress(
        execution({
          status: 'running',
          plan_execution: { planned: 0, finished: 0 },
        } as Partial<DashboardExecutionSummary>),
      ),
    ).toBeNull()
  })

  it('names the version each stack worker ran', () => {
    const stack = [
      {
        name: 'harness',
        source: 'package' as const,
        requested: '1.8.31',
        observed: '1.8.8',
        commit: null,
        dirty: null,
        groups: ['a'],
      },
      {
        name: 'harness',
        source: 'package' as const,
        requested: '1.8.31',
        observed: '1.8.9',
        commit: null,
        dirty: null,
        groups: ['b'],
      },
      {
        name: 'harness-e2e',
        source: 'path' as const,
        requested: null,
        observed: '0.11.28',
        commit: '0123456789abcdef0123',
        dirty: true,
      },
    ]
    expect(workerVersion(stack, 'harness')).toBe('1.8.8, 1.8.9')
    expect(workerVersion(stack, 'harness-e2e')).toBe(
      'path @0123456789ab (dirty)',
    )
    expect(workerVersion(stack, 'state')).toBeNull()
    expect(workerVersion(undefined, 'harness')).toBeNull()
  })
})

describe('list details', () => {
  it('names a model once when its id already carries the provider', () => {
    expect(
      providerModel({
        provider: 'claude-code',
        model: 'claude-code/claude-fable-5',
      }),
    ).toBe('claude-code/claude-fable-5')
    expect(
      providerModel({ provider: 'deepseek', model: 'deepseek-v4-flash' }),
    ).toBe('deepseek/deepseek-v4-flash')
    expect(providerModel({ provider: '', model: 'gpt-5.6-terra' })).toBe(
      'gpt-5.6-terra',
    )
  })

  it('rounds a runtime before splitting minutes from seconds', () => {
    expect(formatDuration(119.6)).toBe('2m 00s')
    expect(formatDuration(59.7)).toBe('1m 00s')
    expect(formatDuration(75.2)).toBe('1m 15s')
    expect(formatDuration(8.25)).toBe('8.3s')
  })

  it('shows a cancelled execution as cancelled, not as an infrastructure event', () => {
    const cancelled = execution({
      status: 'cancelled',
      assessment_summary: undefined,
      totals: {
        expected_reports: 9,
        received_reports: 1,
        technical_failures: 1,
      },
      plan_execution: { planned: 9, finished: 1 },
    } as Partial<DashboardExecutionSummary>)
    const presentation = buildExecutionPresentation(cancelled)
    expect(presentation.attention).toBe('cancelled')
    expect(presentation.primaryIssue).toBeNull()
    // How far it got before it was stopped.
    expect(executionProgress(cancelled)).toBe('1 of 9 done')
  })
})
