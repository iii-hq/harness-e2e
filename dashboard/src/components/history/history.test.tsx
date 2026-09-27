import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  executionText,
  groupByDefinition,
  RunsTable,
} from '@/components/history/RunsTable'
import { SessionTree } from '@/components/history/SessionTree'
import { budgetLine, TestContract } from '@/components/history/TestContract'
import { runKey } from '@/lib/test-history'
import { formFlow, formFlowRow, run } from '@/test-fixtures/test-history'

const table = (open: string[] = [], narrow = false, selected: string[] = []) =>
  renderToStaticMarkup(
    <RunsTable
      observations={formFlow.observations}
      current={formFlow.current_version ?? null}
      grouped
      selected={selected}
      onToggleSelected={() => undefined}
      open={new Set(open)}
      onToggleOpen={() => undefined}
      narrow={narrow}
    />,
  )

describe('the runs list', () => {
  it('heads each definition and marks the sub-agents a run spawned', () => {
    const html = table()
    expect(html).toContain('data-history-table')
    expect((html.match(/data-run-key=/g) ?? []).length).toBe(6)
    expect(html).toContain('Definition 1768da69')
    expect(html).toContain('Definition 0c5c0902')
    expect(html).toContain('3 sub-agents')
    expect(html).toContain('runtime_contract −10')
    expect(html).toContain('profile ade-solo-builder')
    expect(html).toContain('no profile')
    expect(html).toContain('full marks')
    expect(html).toContain('no points')
    // Closed rows render no detail.
    expect(html).not.toContain('data-sessions=')
    expect(
      groupByDefinition(formFlow.observations, null).map(
        (group) => group.rows.length,
      ),
    ).toEqual([1, 4, 1])
  })

  it('opens a run on its tiles, session tree, lost criteria and links', () => {
    const tree = run('9:13')
    const html = table([runKey(tree)])
    expect(html).toContain('data-sessions="3"')
    expect(html).toContain(
      '1 root session + 3 sub-agents, 2 levels deep · 196 turns in all',
    )
    expect(html).toContain('tech-lead · spawned by the root')
    expect(html).toContain('spawned by form-flow-lead-8c2e')
    expect(html).toContain('root session · the subject')
    expect(html).toContain('>Cache read</dt><dd>17.1M</dd>')
    expect(html).toContain(
      'title="compose_valid=true, worker_ready=true, function_surface=false"',
    )
    expect(html).toContain(
      `href="#/ext/harness-e2e/execution/${tree.plan_execution_id}"`,
    )
    expect(html).toContain('Evidence record')
  })

  it('says a run stayed in one session', () => {
    const solo = run('10:01')
    const html = renderToStaticMarkup(
      <SessionTree details={solo.runs?.[0].details ?? never()} turns={53} />,
    )
    expect(html).toContain('None. The run stayed in one session.')
    expect((html.match(/data-session=/g) ?? []).length).toBe(1)
  })

  it('keeps run, score and actions on a narrow pane and letters A and B', () => {
    const [b, a] = [run('9:13'), run('10:01')]
    const html = table([], true, [runKey(a), runKey(b)])
    expect(html).not.toContain('Model · profile')
    expect(html).not.toContain('Criteria lost')
    expect(html).toContain('title="Compared as A"')
    expect(html).toContain('title="Compared as B"')
    // A third run cannot be ticked while two are.
    expect(html).toContain('disabled=""')
  })

  it('keeps apart two cases of one execution', () => {
    const first = run('9:13')
    const second = { ...run('9:13'), case_id: 'form_flow_build:seed-2' }
    const html = renderToStaticMarkup(
      <RunsTable
        observations={[first, second]}
        current={null}
        grouped={false}
        selected={[runKey(second)]}
        onToggleSelected={() => undefined}
        open={new Set([runKey(second)])}
        onToggleOpen={() => undefined}
        narrow={false}
      />,
    )
    expect(html).toContain(`data-run-key="${runKey(first)}"`)
    expect(html).toContain(`data-run-key="${runKey(second)}"`)
    // Only the second case is ticked and open.
    expect((html.match(/title="Compared as A"/g) ?? []).length).toBe(1)
    expect((html.match(/data-sessions=/g) ?? []).length).toBe(1)
    expect(html).toContain(`id="th-detail-${runKey(second)}"`)
  })

  it('names an untitled execution by its id', () => {
    expect(executionText(run('3:29'))).toBe('execution bc0664bb')
    expect(executionText(run('9:13'))).toBe(
      'Opus 5.5 · ade-worker-builder · wake fix',
    )
  })
})

describe('the contract', () => {
  it('shows the prompt, the criteria and the budget', () => {
    const spec = formFlowRow.spec ?? never()
    expect(budgetLine(spec.execution)).toBe(
      'Budget per run: 256 turns · 65,536 output tokens · 6,000,000 tokens in total · stops after 30 min without progress.',
    )
    const html = renderToStaticMarkup(<TestContract spec={spec} />)
    expect(html).toContain('9 criteria · 100 points')
    expect(html).toContain('buildWorkerUi')
  })
})

function never(): never {
  throw new Error('fixture is missing a value')
}
