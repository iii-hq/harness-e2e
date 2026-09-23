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
  ScreenshotFigure,
  screenshotSource,
  screenshotsOf,
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
    for (const [detail, version] of [
      [a, '0.11.24'],
      [b, '0.11.27'],
    ] as const)
      for (const worker of detail.plan_execution?.stack ?? [])
        if (worker.name === 'harness-e2e') worker.observed = version
    const html = renderToStaticMarkup(
      <ComparisonView
        comparison={compareExecutions(a, b)}
        sides={{ a, b }}
        onToggleCounted={() => undefined}
        onRunAgain={() => undefined}
        onTranscript={() => undefined}
      />,
    )
    expect(html).toContain('A (base)')
    expect(html).toContain('GitHub run 35823421664 · RC 366030b3')
    expect(html).toContain(
      'Different runners: 0.11.24 → 0.11.27 — scenario definitions and scoring may differ.',
    )
    // One line for the stack at the top; its groups below the totals.
    expect(html).toContain(
      'stack · 2 workers from your code @a1b2c3d (uncommitted changes) · 1 only in B',
    )
    expect(html.indexOf('data-comparison-metrics')).toBeLessThan(
      html.indexOf('data-layer="comparison-stack"'),
    )
    expect(html).toContain('data-stack-group="only in B"')
    // Out of the totals, with the run's own reason, and its state where a
    // score would be.
    expect(html).toContain(
      'technical_invalid in A: infrastructure_error — scenario setup failed: database never became ready',
    )
    expect(html).toContain('>infrastructure_error</td>')
    expect(html).not.toMatch(/Not reported|Not comparable/)
    expect(html).toContain('1 (1 run out of the totals)')
    expect(html).toContain('data-metric-id="cache_read"')
    expect(html).not.toContain('Tokens (incl. cache)')
    expect(html.match(/data-scenario="/g)).toHaveLength(3)
    // Checkbox and name share one cell.
    expect(html).toMatch(
      /<td><span class="flex items-center gap-2"><input type="checkbox" aria-label="Select minimal_path to run again"/,
    )
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

  it('reads each declared screenshot from its native run and shows it', async () => {
    const b = local()
    const record = b.reports[1]
    record.native_execution_id = '0123456789abcdef0123456789abcdef'
    const run = record.report?.scenarios[0].runs[0]
    if (run)
      run.deliverables = [
        {
          id: 'board',
          artifact: { path: 'deliverables/r/a/board.json' },
          screenshots: [
            {
              pointer: '/attachments/board-desktop.png',
              caption: 'board, desktop',
              media_type: 'image/png',
            },
          ],
        },
      ]
    const [screenshot] = screenshotsOf(b, 'persistent_state')
    expect(screenshot).toMatchObject({
      executionId: '0123456789abcdef0123456789abcdef',
      path: 'deliverables/r/a/board.json',
      pointer: '/attachments/board-desktop.png',
      caption: 'board, desktop',
    })
    const requests: unknown[] = []
    const source = await screenshotSource(async (input) => {
      requests.push(input)
      return { media_type: 'image/png', base64: 'iVBORw0K' }
    }, screenshot)
    expect(requests).toEqual([
      {
        execution_id: '0123456789abcdef0123456789abcdef',
        path: 'deliverables/r/a/board.json',
        pointer: '/attachments/board-desktop.png',
      },
    ])
    expect(source).toBe('data:image/png;base64,iVBORw0K')

    const shown = renderToStaticMarkup(
      <ScreenshotFigure
        screenshot={screenshot}
        image={{ source }}
        evidenceHref="#run"
      />,
    )
    expect(shown).toContain('src="data:image/png;base64,iVBORw0K"')
    expect(shown).toContain('alt="board, desktop"')
    expect(
      renderToStaticMarkup(
        <ScreenshotFigure
          screenshot={screenshot}
          image={undefined}
          evidenceHref="#run"
        />,
      ),
    ).toContain('loading screenshot')
    expect(
      renderToStaticMarkup(
        <ScreenshotFigure
          screenshot={screenshot}
          image={{ error: 'Evidence is 11000000 bytes' }}
          evidenceHref="#run"
        />,
      ),
    ).toContain('Evidence is 11000000 bytes')
  })
})
