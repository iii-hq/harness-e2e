import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { AssessmentRunView } from '@/lib/assessment-view'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import { buildExecutionPresentation } from '@/lib/execution-view'
import {
  EvidenceBundleUnavailable,
  executionOutcome,
  executionSuite,
  provenanceEntries,
  rerunParameters,
  stackVersions,
} from '@/pages/ExecutionPage'

const detail = {
  id: 'execution-1',
  label: 'Security review',
  status: 'passed',
  availability: 'full',
  subjects: [],
  reports: [],
  totals: {
    expected_reports: 1,
    received_reports: 1,
    scenario_pass_rate: 1,
    report_coverage: 1,
  },
  assessment_summary: {
    assessment_count: 46,
    evidence_reference_count: 17,
    assessment_outcomes: { passed: 39, partial: 7 },
    system_statuses: { passed: 1 },
  },
} as unknown as DashboardExecutionDetail

const run: AssessmentRunView = {
  key: 'security-review',
  subjectId: 'terra',
  scenarioId: 'security_review',
  behaviorSha256:
    'sha256:a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1',
  runId: 'run-1',
  attemptId: 'attempt-1',
  metrics: {
    totalTokens: null,
    inputTokens: null,
    outputTokens: null,
    cacheReadTokens: null,
    cacheWriteTokens: null,
    reasoningTokens: null,
    functionCalls: null,
    functionCallErrors: null,
    durationMs: 151_460,
    sessions: null,
    turns: null,
  },
  systemStatus: 'passed',
  score: 100,
  assessments: [],
  evidence: [],
}

describe('execution evidence', () => {
  it('reports an unavailable evidence bundle without losing the execution', () => {
    const retained = {
      ...detail,
      availability: 'unavailable',
      evidence_error: 'native bundle checksum mismatch',
      reports: [],
      totals: {
        received_reports: 3,
        total_tokens: 12_345,
        total_cost_usd: 0.42,
        wall_time_seconds: 125,
      },
    } as DashboardExecutionDetail
    const html = renderToStaticMarkup(
      <EvidenceBundleUnavailable detail={retained} />,
    )

    expect(html).toContain('Evidence bundle unavailable')
    expect(html).toContain('native bundle checksum mismatch')
    expect(html).toContain('Retained execution snapshot')
    expect(html).toContain('>3<')
    expect(html).toContain('12,345')
    expect(html).toContain('$0.4200')
    expect(html).toContain('2m 05s')
    expect(html).toContain('Full per-test metrics and evidence require')
    expect(html).not.toContain('0 tests')
    expect(html).not.toContain('no test results yet')
    expect(html).not.toContain('Execution not found')
  })
})

describe('execution layers', () => {
  // Audit ED-05: the system status is the only outcome the contract publishes.
  it('publishes the system status as the single execution outcome', () => {
    const presentation = buildExecutionPresentation(detail)
    expect(executionOutcome(presentation, [run])).toEqual({ value: 'passed' })
    expect(executionOutcome(presentation, [])).toEqual({ value: 'passed' })
  })

  it('includes later failures in the aggregate outcome instead of reporting only the first run', () => {
    expect(
      executionOutcome(buildExecutionPresentation(detail), [
        { ...run, systemStatus: 'passed' },
        { ...run, systemStatus: 'hard_gate_failed' },
      ]),
    ).toEqual({
      value: 'partial',
      label: '1 passed · 1 failed (legacy result)',
    })
  })

  it('lists provenance without null fields and with local timestamps', () => {
    const entries = provenanceEntries(
      {
        ...detail,
        run_id: 'run-1',
        attempt: 1,
        event: 'local',
        actor: 'layon',
        started_at: '2026-08-26T20:07:31Z',
        completed_at: '2026-08-26T20:11:31Z',
        source: {
          ref: null,
          repository: 'iii-hq/harness-e2e',
          sha: 'e34550995cE33809d0b9458f3689111faa8d3f0e'.toLowerCase(),
        },
        release: { registry_tag: null, stack_lock_digest: null },
      } as unknown as DashboardExecutionDetail,
      buildExecutionPresentation({
        ...detail,
        started_at: '2026-08-26T20:07:31Z',
        completed_at: '2026-08-26T20:11:31Z',
      } as unknown as DashboardExecutionDetail),
    )
    const byKey = Object.fromEntries(entries)
    expect(Object.keys(byKey)).not.toContain('release')
    expect(byKey.source).toBe(
      'repository iii-hq/harness-e2e · sha e34550995ce3',
    )
    expect(byKey.completed).toMatch(/· 4m 00s$/)
    expect(byKey.completed).not.toContain('2026-08-26T20:11:31Z')
    expect(byKey.actor).toBe('layon')
  })

  it('times an execution with a scenario run again by its current runs, not start to finish', () => {
    const reran = {
      ...detail,
      started_at: '2026-08-26T20:07:31Z',
      completed_at: '2026-08-28T09:00:00Z',
      totals: { ...detail.totals, wall_time_seconds: 150 },
      plan_execution: {
        slots: [{ previous_attempts: [{ execution_id: 'old', error: null }] }],
      },
    } as unknown as DashboardExecutionDetail
    const byKey = Object.fromEntries(
      provenanceEntries(reran, buildExecutionPresentation(reran)),
    )
    expect(byKey.completed).toMatch(
      /· after running a scenario again · 2m 30s of current runs$/,
    )
    expect(byKey.completed).not.toContain('36h')
  })
})

describe('run again', () => {
  const parameters = {
    scenarios: ['minimal_path'],
    runs: 3,
    technical_retries: 2,
    model: 'gpt-5.6-terra',
    provider: 'openai-codex',
    agent: 'tech-lead',
  }
  const subject = { provider: 'deepseek', model: 'deepseek-v4-flash' }

  it("starts from the execution's recorded parameters", () => {
    const execution = {
      ...detail,
      plan_execution: { parameters },
      parameters: { ...parameters, runs: 1 },
    } as unknown as DashboardExecutionDetail
    expect(rerunParameters(execution, ['other'], subject)).toBe(parameters)
  })

  it('starts an older native run from its own request', () => {
    const native = { ...detail, parameters } as DashboardExecutionDetail
    expect(rerunParameters(native, ['other'], subject)).toBe(parameters)
  })

  it('falls back to what the report shows when nothing was recorded', () => {
    expect(rerunParameters(detail, ['a', 'b', 'a'], subject)).toEqual({
      scenarios: ['a', 'b'],
      runs: 1,
      technical_retries: 1,
      model: 'deepseek-v4-flash',
      provider: 'deepseek',
      agent: null,
    })
  })
})

describe('versions in the header', () => {
  it('shows the Harness and the runner the stack recorded, and nothing else', () => {
    const withStack = {
      ...detail,
      plan_execution: {
        stack: [
          {
            name: 'harness',
            source: 'package',
            requested: null,
            observed: '1.8.8',
            commit: null,
            dirty: null,
          },
          {
            name: 'harness-e2e',
            source: 'path',
            requested: null,
            observed: '0.11.28',
            commit: 'abcdef0123456789',
            dirty: false,
          },
          {
            name: 'state',
            source: 'package',
            requested: null,
            observed: '0.22.3',
            commit: null,
            dirty: null,
          },
        ],
      },
    } as unknown as DashboardExecutionDetail
    expect(stackVersions(withStack)).toEqual([
      ['harness', '1.8.8'],
      ['runner', 'path @abcdef012345'],
    ])
    expect(stackVersions(detail)).toEqual([])
  })

  it('names the suite with its digest, and the stack an imported contract names', () => {
    const imported = {
      ...detail,
      plan_execution: {
        parameters: {
          suite: {
            id: 'regression',
            label: 'Regression',
            sha256: 'sha256:0123456789abcdef0123',
          },
        },
        source: {
          kind: 'github',
          run_id: 42,
          url: 'https://github.com/o/r/actions/runs/42',
          stack: 'default',
        },
        stack: [],
      },
    } as unknown as DashboardExecutionDetail
    expect(executionSuite(imported)).toBe('Regression · 0123456789ab')
    expect(stackVersions(imported)).toEqual([['stack', 'default']])
    // Ticked by hand, and from before suites.
    const unnamed = {
      ...detail,
      parameters: { suite: { label: '', sha256: 'sha256:fedcba9876543210' } },
    } as unknown as DashboardExecutionDetail
    expect(executionSuite(unnamed)).toBe('unnamed suite · fedcba987654')
    expect(executionSuite(detail)).toBe('not recorded')
    // An older import knew only the suite's id: named, digest unknown.
    const byId = {
      ...detail,
      parameters: { suite: { id: 'pr', label: 'pr', sha256: '' } },
    } as unknown as DashboardExecutionDetail
    expect(executionSuite(byId)).toBe('pr')
  })
})
