import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { compareExecutions } from '@/lib/execution-comparison'
import {
  ComparisonPlaceholder,
  ComparisonView,
  choiceFromParams,
  choiceToParams,
  ExecutionComparePage,
  loadExecutionPair,
  toggleCounted,
} from '@/pages/ExecutionComparePage'
import { imported, local } from '@/test-fixtures/execution-comparison'

describe('execution comparison page', () => {
  it('keeps the reader’s choices in the hash', () => {
    const choice = choiceFromParams(
      new URLSearchParams('include=shell_coder_sandbox&exclude=a,b'),
    )
    expect(choice).toEqual({
      include: ['shell_coder_sandbox'],
      exclude: ['a', 'b'],
    })
    expect(choiceToParams(choice).toString()).toBe(
      'include=shell_coder_sandbox&exclude=a%2Cb',
    )
    expect(choiceToParams({ include: [], exclude: [] }).toString()).toBe('')
  })

  it('toggles a scenario in and out of the totals, automatic or not', () => {
    const a = imported()
    const b = local()
    let choice = { include: [] as string[], exclude: [] as string[] }
    const scenario = (id: string) => {
      const found = compareExecutions(a, b, choice).scenarios.find(
        (entry) => entry.id === id,
      )
      if (!found) throw new Error(id)
      return found
    }
    // An automatic exclusion comes back, then leaves again.
    choice = toggleCounted(choice, scenario('shell_coder_sandbox'))
    expect(choice).toEqual({ include: ['shell_coder_sandbox'], exclude: [] })
    expect(scenario('shell_coder_sandbox').counted).toBe(true)
    choice = toggleCounted(choice, scenario('shell_coder_sandbox'))
    expect(scenario('shell_coder_sandbox').counted).toBe(false)
    // A counted scenario leaves by the reader's hand and comes back.
    choice = toggleCounted(choice, scenario('minimal_path'))
    expect(choice).toEqual({ include: [], exclude: ['minimal_path'] })
    expect(scenario('minimal_path')).toMatchObject({
      counted: false,
      leftOut: true,
    })
    choice = toggleCounted(choice, scenario('minimal_path'))
    expect(scenario('minimal_path').counted).toBe(true)
  })

  it('shows what changed, the totals and every scenario, without a verdict', () => {
    const a = imported()
    const b = local()
    const html = renderToStaticMarkup(
      <ComparisonView
        comparison={compareExecutions(a, b)}
        sides={{ a, b }}
        onToggleCounted={() => undefined}
        onTranscript={() => undefined}
      />,
    )
    expect(html).toContain('A (base)')
    expect(html).toContain('GitHub run 35823421664 · RC 366030b3')
    expect(html).toContain('data-change="llm-router"')
    expect(html).toContain('Out of the totals')
    expect(html).toContain('shell_coder_sandbox (technical_invalid in A)')
    expect(html).toContain('data-metric-id="tokens_per_completion"')
    expect(html.match(/data-scenario="/g)).toHaveLength(3)
    expect(html).toContain('+ answer cites the source')
    expect(html).toContain('rerun selected')
    expect(html).not.toMatch(/better|worse|improv|regress|winner/i)
  })

  it('loads both executions and names the side that failed', async () => {
    const a = imported()
    const b = local()
    const get = (id: string) =>
      id === a.id
        ? Promise.resolve(a)
        : id === b.id
          ? Promise.resolve(b)
          : Promise.reject(new Error('Execution not found'))
    await expect(loadExecutionPair(get, a.id, b.id)).resolves.toEqual({ a, b })
    await expect(loadExecutionPair(get, a.id, 'gone')).rejects.toThrow(
      'B (gone) could not be loaded: Execution not found',
    )
    await expect(loadExecutionPair(get, 'gone', 'lost')).rejects.toThrow(
      'A (gone) could not be loaded: Execution not found · B (lost) could not be loaded: Execution not found',
    )
  })

  it('asks for two executions, shows loading, then the error', () => {
    expect(
      renderToStaticMarkup(<ExecutionComparePage left="a" right={null} />),
    ).toContain('Choose two executions')
    const loading = renderToStaticMarkup(
      <ExecutionComparePage left="a" right="b" />,
    )
    expect(loading).toContain('aria-busy="true"')
    expect(loading).toContain('Loading both executions')
    const failed = renderToStaticMarkup(
      <ComparisonPlaceholder
        missing={false}
        error="B (gone) could not be loaded: Execution not found"
      />,
    )
    expect(failed).toContain('The comparison could not be loaded')
    expect(failed).toContain('B (gone) could not be loaded')
  })
})
