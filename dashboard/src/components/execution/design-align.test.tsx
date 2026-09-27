import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { matchesFilter, rowNote } from '@/components/ScenarioMatrix'
import type { MetricValue, PrimaryMetrics } from '@/lib/primary-metrics'
import type { ScenarioMatrixItem } from '@/lib/scenario-matrix'
import { ExecutionTotals } from './ExecutionTotals'
import { NeedsAttention } from './NeedsAttention'

const metric = (value: number | null): MetricValue => ({
  value,
  observed: value,
  samples: 1,
  expected: 1,
})

function item(partial: Partial<ScenarioMatrixItem>): ScenarioMatrixItem {
  return {
    reason: null,
    key: partial.scenarioId ?? 'k',
    scenarioId: 'kanban_c4',
    runCount: 1,
    runs: [],
    primaryRun: null,
    objective: { status: 'passed', label: 'Passed', raw: 'passed' },
    ...partial,
  } as ScenarioMatrixItem
}

const lost = item({
  scenarioId: 'kanban_c4',
  objective: { status: 'failed', label: 'Failed', raw: 'failed' },
  runs: [{ score: 80 }] as never,
  primaryRun: {
    criteria: [
      { id: 'a', awarded: 10, possible: 10, gate: false, reason: '' },
      { id: 'b', awarded: 0, possible: 15, gate: true, reason: 'missing' },
      { id: 'c', awarded: 5, possible: 10, gate: false, reason: 'half' },
    ],
  } as never,
})
const full = item({ scenarioId: 'full', runs: [{ score: 100 }] as never })
const notRun = item({
  scenarioId: 'kanban_c7',
  runCount: 0,
  objective: { status: 'unavailable', label: 'Not run', raw: 'unavailable' },
  reason: 'Didn’t start: compose::add failed',
})

describe('results by test (canvas)', () => {
  it('notes why a test lost points or did not run', () => {
    expect(rowNote(lost)).toBe('Failed a hard gate · 2 criteria lost')
    expect(rowNote(notRun)).toBe('Didn’t start')
    expect(rowNote(full)).toBe('')
  })

  it('filters by lost points, not run and full marks', () => {
    expect(matchesFilter(lost, 'lost')).toBe(true)
    expect(matchesFilter(full, 'passed')).toBe(true)
    expect(matchesFilter(full, 'lost')).toBe(false)
    expect(matchesFilter(notRun, 'notrun')).toBe(true)
    expect(matchesFilter(notRun, 'all')).toBe(true)
  })
})

describe('execution totals (canvas)', () => {
  it('shows six figures and the partial note', () => {
    const metrics = {
      tests: [
        { metrics: { score: metric(80) } },
        { metrics: { score: metric(100) } },
      ],
      metrics: {
        score: metric(90),
        totalTokens: metric(3_300_000),
        inputTokens: metric(2_100_000),
        outputTokens: metric(1_200_000),
        cacheRead: metric(82_100_000),
        cacheWrite: metric(null),
        turns: metric(10),
        functionCalls: metric(20),
        functionErrors: metric(1),
        durationMs: metric(8_940_000),
        costUsd: metric(2.566),
      },
    } as unknown as PrimaryMetrics
    const html = renderToStaticMarkup(
      <ExecutionTotals metrics={metrics} items={[lost, full, notRun]} />,
    )
    const text = html.replace(/<[^>]*>/g, ' ')
    for (const label of [
      'Score',
      'Tests passed',
      'Tokens',
      'Cache',
      'Duration',
      'Cost',
    ])
      expect(text).toContain(label)
    expect(text).toContain('1/3')
    expect(text).toContain('1 lost points · 1 not run')
    expect(text).toContain('3.30M')
    expect(text).toContain('2h 29m')
    expect(text).toContain('Partial: 2 of 3 tests reported')
    expect(html).toContain('aria-expanded="false"')
    expect(text).toContain('All metrics')
  })
})

describe('needs attention (canvas)', () => {
  it('counts items and offers Run again and Show error', () => {
    const html = renderToStaticMarkup(
      <NeedsAttention
        items={[
          {
            kind: 'test',
            key: 'k',
            scenarioId: 'kanban_c4',
            label: 'x',
            reason: 'short',
          },
          {
            kind: 'warning',
            key: 'w',
            text: `Execution error: ${'x'.repeat(300)}`,
          },
        ]}
        onShow={() => {}}
        onRerun={() => {}}
      />,
    )
    expect(html).toContain('data-attention-count="true">2<')
    expect(html).toContain('data-attention-rerun="kanban_c4"')
    expect(html).toContain('Show error')
  })
})

describe('evidence record (canvas)', () => {
  it('groups audit flags, criteria by dimension, the deliverable and the run facts', async () => {
    const { EvidenceRecordPage } = await import('./EvidenceRecord')
    const projection = {
      run_id: 'run-1',
      attempt_id: 'att-1',
      attempt_number: 1,
      completion: 'completed',
      audit: {
        flags: [1, 2, 3].map(() => ({
          kind: 'out_of_scope_session_access',
          summary: 'The subject used browser session b64.',
          evidence: [{ function_id: 'browser::act', detail: 'session b64' }],
        })),
      },
      criteria: [
        {
          id: 'runtime_contract',
          description: 'Worker ready.',
          awarded: 10,
          possible: 10,
          gate: true,
          reason: 'Worker ready. compose_valid=true',
        },
        {
          id: 'domain_primary',
          description: 'Primary path.',
          awarded: 5,
          possible: 20,
          gate: false,
          reason: 'mismatch',
        },
      ],
      deliverables: [
        {
          id: 'form_flow_worker_evidence',
          kind: 'visual_worker_audit',
          content_format: 'json',
          schema_valid: true,
          provenance_valid: true,
          invariants: [{ passed: true }, { passed: true }],
          screenshots: [{ pointer: '/a' }],
          artifact: {
            path: 'deliverables/r/a/evidence.json',
            sha256: 'sha256:2b58805cffff',
            size_bytes: 423_321,
          },
        },
      ],
      evidence: [
        {
          path: 'evidence/r/a/transcript.json',
          size_bytes: 2048,
          sha256: 'sha256:2a4c32ffaaaa',
        },
      ],
    }
    const detail = {
      id: 'e',
      reports: [
        {
          subject_id: 'opus',
          scenario_id: 'form_flow_build',
          report: {
            scenarios: [{ scenario_id: 'form_flow_build', runs: [projection] }],
          },
        },
      ],
    } as never
    const run = {
      key: 'k',
      subjectId: 'opus',
      scenarioId: 'form_flow_build',
      behaviorSha256: null,
      runId: 'run-1',
      attemptId: 'att-1',
      metrics: {
        totalTokens: null,
        inputTokens: 6044,
        outputTokens: null,
        cacheReadTokens: null,
        cacheWriteTokens: null,
        reasoningTokens: null,
        functionCalls: 75,
        functionCallErrors: 16,
        durationMs: 509_000,
        sessions: 1,
        turns: 53,
      },
      systemStatus: 'passed',
      score: 15,
      assessments: [
        { criterionId: 'runtime_contract', dimension: 'deliverable' },
        { criterionId: 'domain_primary', dimension: 'structural_integrity' },
      ],
      evidence: [],
    } as never
    const html = renderToStaticMarkup(
      <EvidenceRecordPage
        run={run}
        detail={detail}
        backHref="#"
        onOpenFile={async () => {}}
      />,
    )
    const text = html.replace(/<[^>]*>/g, ' ').replace(/\s+/g, ' ')
    expect(text).toContain('3 audit warnings, one cause')
    expect(text).toContain('browser::act ×3')
    expect(text).toContain('Show flags')
    expect(text).toContain('1 of 2 criteria met')
    expect(text).toContain('Deliverable what the Worker delivered 10/10')
    expect(text).toContain(
      'Structural integrity checked against the Harness oracle 5/20',
    )
    // The reason drops the description it repeats.
    expect(text).toContain('compose_valid=true')
    expect(text).not.toContain('Worker ready. Worker ready.')
    expect(text).toContain('JSON · 413.4 KiB · sha 2b58805c')
    expect(text).toContain('Open JSON')
    expect(text).toContain('Copy path')
    expect(text).toContain('Schema valid')
    expect(text).toContain('2 of 2 invariants hold')
    expect(text).toContain('Runtime 8m 29s')
    expect(text).toContain('Evidence files')
    expect(text).toContain('transcript.json')
  })
})

describe('stack panel (canvas)', () => {
  it('reads the stack as written and splits declared from dependencies', async () => {
    const { parseStackYaml, StackPanel } = await import('./StackPanel')
    const yaml = [
      '# The default stack',
      'iii: latest',
      '',
      'containers:',
      '  # The application under test.',
      '  harness:',
      '    worker: package://harness',
      '    version: latest',
      '  fp:',
      '    worker: package://fp',
      '    version: "0.3.1" # pinned',
    ].join('\n')
    const parsed = parseStackYaml(yaml)
    expect(parsed.iii).toBe('latest')
    expect(parsed.containers).toEqual([
      {
        name: 'harness',
        worker: 'package://harness',
        version: 'latest',
        commit: null,
      },
      { name: 'fp', worker: 'package://fp', version: '0.3.1', commit: null },
    ])
    const worker = (
      name: string,
      observed: string | null,
      groups?: string[],
    ) => ({
      name,
      source: 'package' as const,
      requested: null,
      observed,
      commit: null,
      dirty: null,
      groups,
    })
    const execution = {
      parameters: { stack: { name: 'default', yaml } },
      source: { kind: 'github' },
      stack: [
        worker('harness', '1.8.8'),
        worker('fp', '0.3.1'),
        worker('state', null, ['g1']),
        worker('browser', '0.9.0'),
      ],
      slots: [{ group_id: 'g1' }, { group_id: 'g2' }],
    } as never
    const html = renderToStaticMarkup(<StackPanel execution={execution} />)
    const text = html.replace(/<[^>]*>/g, ' ').replace(/\s+/g, ' ')
    expect(text).toContain(
      'Stack default iii latest · 4 workers · one worker-compose.lock for every group',
    )
    expect(text).toContain('Open in Stacks')
    expect(text).toContain('Workers 4 as Compose resolved them')
    expect(text).toContain('Declared 2 containers in the stack')
    expect(text).toContain('From dependencies 2 resolved from the packages')
    expect(text).toContain('Groups 2 each starts from the same lock')
    expect(text).toContain('package://harness · the application under test')
    expect(html).toContain('data-stack-dependency="state"')
    expect(text).toContain('differs in 1 group')
    expect(text).toContain('from the lock')
    expect(html).not.toContain('data-stack-dependency="fp"')
  })
})
