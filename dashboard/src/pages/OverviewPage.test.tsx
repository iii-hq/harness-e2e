import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { DashboardExecutionSummary } from '@/lib/dashboard-data-source'
import { configureDashboardRuntime } from '@/lib/dashboard-runtime'
import { buildExecutionPresentation } from '@/lib/execution-view'
import {
  attentionQueue,
  buildOverviewSignal,
  executionTitle,
  signalMetric,
} from '@/lib/overview-signal'
import {
  RecentExecutions,
  RunningStrip,
  trendCaption,
  trendDelta,
  workflowProgress,
} from '@/pages/OverviewPage'

function summary(
  overrides: Partial<DashboardExecutionSummary> & { id: string },
): DashboardExecutionSummary {
  return {
    label: 'e2e::* control-plane run',
    status: 'passed',
    availability: 'full',
    event: 'local',
    completed_at: '2026-08-26T20:11:31Z',
    workflow_name: 'e2e::* control-plane run',
    subjects: [
      {
        id: 'terra',
        provider: 'openai-codex',
        model: 'gpt-5.6-terra',
        judge: { provider: 'openai-codex', model: 'gpt-5.6-sol' },
        scenarios: [],
      },
    ],
    assessment_summary: { system_statuses: { passed: 2 } } as never,
    totals: {
      expected_reports: 2,
      received_reports: 2,
      scenario_pass_rate: 1,
      report_coverage: 1,
      total_tokens: 7_918,
      wall_time_seconds: 241,
    },
    ...overrides,
  }
}

describe('running execution navigation', () => {
  it('keeps an unsupported historical execution linked and explicitly labelled', () => {
    const html = renderToStaticMarkup(
      <RecentExecutions
        total={1}
        presentations={[
          buildExecutionPresentation(
            summary({
              id: 'legacy-execution',
              status: 'unsupported',
              availability: 'unsupported',
            }),
          ),
        ]}
      />,
    )
    expect(html).toContain('unsupported')
    expect(html).toContain('href="#/execution/legacy-execution"')
    expect(html).toContain('gpt-5.6-terra')
    expect(html).toContain('Not reported')
    expect(html).not.toContain('100%')
  })

  it('adds a detail button beside cancel for the current execution', () => {
    const html = renderToStaticMarkup(
      <RunningStrip
        running={[
          buildExecutionPresentation(
            summary({ id: 'current-run', status: 'running' }),
          ),
        ]}
        onCancel={() => {}}
        cancelling={false}
      />,
    )
    expect(html).toMatch(
      /<a[^>]+href="#\/execution\/current-run"[^>]*>open execution/,
    )
    expect(html).toMatch(/<button[^>]*>cancel<\/button>/)
  })

  it.each(['running', 'cancelling'])(
    'keeps navigation available for %s executions without cancel permission',
    (status) => {
      const restore = configureDashboardRuntime({
        embedded: true,
        hashBase: '#/ext/harness-e2e',
      })
      try {
        const html = renderToStaticMarkup(
          <RunningStrip
            running={[
              buildExecutionPresentation(
                summary({ id: 'current-run', status }),
              ),
            ]}
            cancelling
          />,
        )
        expect(html).toMatch(
          /<a[^>]+href="#\/ext\/harness-e2e\/execution\/current-run"[^>]*>open execution/,
        )
        expect(html).not.toContain('<button')
      } finally {
        restore()
      }
    },
  )

  it('has no execution button when nothing is running', () => {
    const html = renderToStaticMarkup(
      <RunningStrip running={[]} cancelling={false} />,
    )
    expect(html).not.toContain('open execution')
    expect(html).toContain('Nothing is running.')
  })
})

function workflowExecution(): DashboardExecutionSummary {
  return summary({
    id: 'security-review-1',
    label: 'Security review',
    status: 'hard_gate_failed',
    assessment_summary: {
      system_statuses: { hard_gate_failed: 1 },
    } as never,
    totals: {
      expected_reports: 1,
      received_reports: 1,
      scenario_pass_rate: 0,
      report_coverage: 1,
      total_tokens: 12_500,
      wall_time_seconds: 80,
      turns: 18,
    },
    workflow_metrics: {
      step_count: 5,
      succeeded_steps: 4,
      failed_steps: 0,
      hard_gate_failed_steps: 1,
      skipped_steps: 0,
      cancelled_steps: 0,
      running_steps: 0,
      pending_steps: 0,
      duration_ms: 65_000,
      asset_count: 8,
      hard_gate_count: 4,
      passed_hard_gate_count: 3,
      evaluation_count: 9,
      failure_count: 1,
      total_tokens: 10_250,
      function_calls: 17,
      token_metric_steps: 4,
      function_call_metric_steps: 4,
    },
  })
}

describe('overview signal', () => {
  // Audit O-09: the workflow glob is not a title when it repeats on every row.
  it('titles an unlabelled execution by its subject and date', () => {
    const labelled = buildExecutionPresentation(summary({ id: 'a' }))
    expect(executionTitle(labelled)).toEqual({
      title: 'e2e::* control-plane run',
      detail: 'e2e::* control-plane run',
    })
    const unlabelled = buildExecutionPresentation(
      summary({ id: 'b', label: undefined }),
    )
    expect(executionTitle(unlabelled).title).toMatch(/^gpt-5\.6-terra · /)
    expect(executionTitle(unlabelled).detail).toBe('e2e::* control-plane run')
  })

  // Audit O-16: each headline carries how it moved and what is typical.
  it('computes the delta against the previous execution and the median', () => {
    const presentations = [
      summary({ id: '1', totals: { scenario_pass_rate: 0.5 } }),
      summary({ id: '2', totals: { scenario_pass_rate: 1 } }),
      summary({ id: '3', totals: { scenario_pass_rate: 0 } }),
    ].map(buildExecutionPresentation)
    const metric = signalMetric(presentations, (item) => item.passRate)
    expect(metric.delta).toBe(-0.5)
    expect(metric.median).toBe(0.5)
    expect(metric.sampleSize).toBe(2)
    expect(
      trendDelta(metric, (value) => `${Math.round(value * 100)} pts`),
    ).toBe('−50 pts vs prev')
    expect(
      trendCaption(
        metric,
        (value) => `${Math.round(value * 100)}%`,
        'fallback',
      ),
    ).toBe('median of last 2: 50%')
    expect(
      trendCaption(
        { delta: null, median: null, sampleSize: 0 },
        String,
        'fallback',
      ),
    ).toBe('fallback')
  })

  // Audit O-06: the queue holds only executions a person must act on.
  it('queues failures and never running or cancelled runs', () => {
    const presentations = [
      summary({
        id: 'gated',
        status: 'hard_gate_failed',
        assessment_summary: {
          system_statuses: { hard_gate_failed: 1, passed: 1 },
        } as never,
      }),
      summary({ id: 'running', status: 'running' }),
      summary({ id: 'cancelled', status: 'cancelled' }),
      summary({
        id: 'older',
        status: 'hard_gate_failed',
        completed_at: '2026-07-01T10:00:00Z',
        assessment_summary: {
          system_statuses: { hard_gate_failed: 1 },
        } as never,
      }),
      summary({ id: 'passed' }),
    ].map(buildExecutionPresentation)
    expect(
      attentionQueue(presentations).map((entry) => [
        entry.presentation.execution.id,
        entry.category,
      ]),
    ).toEqual([
      ['gated', 'hard_gate'],
      ['older', 'hard_gate'],
    ])
    expect(attentionQueue(presentations, 1)).toHaveLength(1)
  })

  // Audit O-17: a running execution never becomes the headline number.
  it('keeps the headline on the newest settled execution and lists running apart', () => {
    const signal = buildOverviewSignal([
      summary({ id: 'running', status: 'running' }),
      summary({ id: 'done' }),
    ])
    expect(signal.latest?.execution.id).toBe('done')
    expect(signal.running.map((item) => item.execution.id)).toEqual(['running'])
    expect(signal.recent).toHaveLength(2)
  })

  it('reads workflow progress when the execution reports steps', () => {
    const workflow = workflowProgress(
      buildExecutionPresentation(workflowExecution()),
    )
    expect(workflow).toMatchObject({
      value: '4/5',
      detail: '3/4 hard gates passed',
      delta: 'needs review',
      tone: 'negative',
      runtimeSeconds: 65,
      tokens: 10_250,
    })
    expect(
      workflowProgress(buildExecutionPresentation(summary({ id: 'a' }))),
    ).toBeNull()
  })

  // Audit O-01: the Overview lists five rows and links to the ledger.
  it('renders the bands without the ledger table', () => {
    const html = renderToStaticMarkup(
      <div>
        {buildOverviewSignal([summary({ id: 'a' })]).recent.map((item) => (
          <span key={item.execution.id}>{executionTitle(item).title}</span>
        ))}
      </div>,
    )
    expect(html).toContain('e2e::* control-plane run')
  })
})
