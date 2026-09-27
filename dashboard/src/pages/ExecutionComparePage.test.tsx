import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  byScoreChange,
  ComparisonView,
  deltaText,
  pairByCaption,
  RowDetail,
  ScreenshotFigure,
} from '@/components/compare/ComparisonView'
import { compareExecutions } from '@/lib/execution-comparison'
import { type ScreenshotEntry, screenshotsOf } from '@/lib/screenshots'
import {
  ComparisonPlaceholder,
  choiceCounting,
  choiceFromParams,
  choiceToParams,
  ExecutionComparePage,
  loadExecutionPair,
} from '@/pages/ExecutionComparePage'
import { imported, local } from '@/test-fixtures/execution-comparison'

function withRunners(a = imported(), b = local()) {
  for (const [detail, version] of [
    [a, '0.11.24'],
    [b, '0.11.27'],
  ] as const)
    for (const worker of detail.plan_execution?.stack ?? [])
      if (worker.name === 'harness-e2e') worker.observed = version
  return { a, b }
}

const view = (a = imported(), b = local(), choice = {}) =>
  renderToStaticMarkup(
    <ComparisonView
      comparison={compareExecutions(a, b, choice)}
      sides={{ a, b }}
      swap="#swap"
      onCount={() => undefined}
      onRunTest={() => undefined}
    />,
  )

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

  it('counts exactly the tests the reader keeps, automatic exclusions included', () => {
    const { scenarios } = compareExecutions(imported(), local())
    // The rule took shell_coder_sandbox out: keeping it brings it back.
    expect(
      choiceCounting(scenarios, [
        'minimal_path',
        'persistent_state',
        'shell_coder_sandbox',
      ]),
    ).toEqual({ include: ['shell_coder_sandbox'], exclude: [] })
    // Leaving a counted test out is the reader's exclusion.
    const choice = choiceCounting(scenarios, ['persistent_state'])
    expect(choice).toEqual({ include: [], exclude: ['minimal_path'] })
    const after = compareExecutions(imported(), local(), choice)
    expect(
      after.scenarios.map((scenario) => [scenario.id, scenario.counted]),
    ).toEqual([
      ['minimal_path', false],
      ['persistent_state', true],
      ['shell_coder_sandbox', false],
    ])
    expect(
      after.scenarios.find((scenario) => scenario.id === 'minimal_path'),
    ).toMatchObject({ leftOut: true })
  })

  it('shows both sides, what changed, the tests counted, highlights, totals and every test, without a verdict', () => {
    const { a, b } = withRunners()
    const html = view(a, b)
    // Each side: its role, title, model, profile, suite and where it ran.
    expect(html).toContain('aria-label="A · Reference"')
    expect(html).toContain('deepseek/flash · no profile · suite not recorded')
    expect(html).toContain('GitHub run 35823421664 · RC 366030b3')
    expect(html).toContain('href="#swap"')
    expect(html).toContain('aria-label="Swap A and B"')
    // What changed: the runner and the stack, and the warning.
    expect(html).toContain('data-change="runner"')
    expect(html).toContain('0.11.24 → 0.11.27')
    expect(html).toContain('2 workers changed · 1 only in B')
    expect(html).toContain('Same tests, model and profile.')
    expect(html).toContain(
      'Different runners: 0.11.24 → 0.11.27 — scenario definitions and scoring may differ.',
    )
    // The rule took a test out by itself, in its runs' words.
    expect(html).toContain('2 of 3 counted · totals recomputed from them')
    expect(html).toContain(
      'Out by itself · technical_invalid in A: infrastructure_error — scenario setup failed: database never became ready',
    )
    expect(html).toContain('1 test did here.')
    expect(html).toContain('>Automatic<')
    // Highlights and totals.
    expect(html).toContain('B scored 13 points lower')
    expect(html).toContain('lost 38 points in B: state_after_restart')
    expect(html).toContain('data-kpi="score"')
    // Every test, largest score change first; the one out says why, with
    // its state where a score would be.
    expect(
      [...html.matchAll(/data-scenario="([^"]+)"/g)].map((match) => match[1]),
    ).toEqual(['persistent_state', 'minimal_path', 'shell_coder_sandbox'])
    expect(html).toContain('technically invalid</span>')
    expect(html).toContain('infrastructure error → </span>40')
    expect(html).toContain('data-cell="tokens"')
    expect(html).toContain('data-layer="comparison-stack"')
    expect(html).not.toMatch(/better|worse|improv|regress|winner/i)
  })

  it('writes a difference as B minus A, never a judgement', () => {
    const { scenarios } = compareExecutions(imported(), local())
    const [persistent] = byScoreChange(scenarios)
    const metric = (id: string) => {
      const found = persistent.metrics.find((entry) => entry.id === id)
      if (!found) throw new Error(id)
      return found
    }
    expect(deltaText(metric('score'))).toBe('−38 pts')
    expect(deltaText(metric('turns'))).toBe('no change')
    const sandbox = scenarios.find(
      (scenario) => scenario.id === 'shell_coder_sandbox',
    )
    const score = sandbox?.metrics.find((entry) => entry.id === 'score')
    expect(score && deltaText(score)).toBe('not comparable')
  })

  it('opens a test on the criteria that moved, those lost on both sides, its metrics and runs', () => {
    const a = imported()
    const b = local()
    const minimal = compareExecutions(a, b).scenarios.find(
      (scenario) => scenario.id === 'minimal_path',
    )
    if (!minimal) throw new Error('minimal_path')
    const html = renderToStaticMarkup(
      <RowDetail
        scenario={minimal}
        sides={{ a, b }}
        bridge={null}
        onRunTest={() => undefined}
      />,
    )
    expect(html).toContain('1 criterion changed · B gained 12 points')
    expect(html).toContain('Run this test again')
    expect(html).toContain('data-criterion="cites_source:20"')
    expect(html).toContain('answer cites the source')
    expect(html).toContain('no source named')
    expect(html).toContain('names the source')
    expect(html).toContain('Lost points on both sides')
    expect(html).toContain('A 74/80 · B 74/80')
    expect(html).toContain('data-metric-id="cache_read"')
    // A run of each side, its transcript and evidence record.
    expect(html).toContain('data-run-side="a"')
    expect(html).toContain(
      'href="#/ext/harness-e2e/execution/local-b/run/local-b-0/transcript"',
    )
    expect(html).toContain(
      'href="#/ext/harness-e2e/execution/import-a/run/import-a-0"',
    )
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
    const empty = renderToStaticMarkup(
      <ExecutionComparePage left="a" right={null} />,
    )
    expect(empty).toContain('Choose two executions')
    expect(empty).toContain('Back to Executions')
    const loading = renderToStaticMarkup(
      <ExecutionComparePage left="a" right="b" />,
    )
    expect(loading).toContain('aria-busy="true"')
    expect(loading).toContain('Loading both executions')
    const failed = renderToStaticMarkup(
      <ComparisonPlaceholder
        missing={false}
        error="B (gone) could not be loaded: Execution not found"
        onRetry={() => undefined}
      />,
    )
    expect(failed).toContain('The comparison could not be loaded')
    expect(failed).toContain('B (gone) could not be loaded')
    expect(failed).toContain('Retry')
  })

  it('says a side is still running and its figures are partial', () => {
    const b = local()
    b.status = 'running'
    const html = view(local(), b)
    expect(html).toContain('data-comparison-live')
    expect(html).toContain('B is still running')
    expect(html).toContain('(partial)')
  })

  it('pairs both sides’ screenshots by caption and shows each', () => {
    const shot = (runId: string, caption: string): ScreenshotEntry => ({
      key: `${runId}:${caption}`,
      executionId: 'x',
      runId,
      path: 'deliverables/board.json',
      pointer: `/attachments/${caption}.png`,
      caption,
    })
    const pairs = pairByCaption(
      [shot('a1', 'board'), shot('a1', 'only in A')],
      [shot('b1', 'list'), shot('b1', 'board')],
    )
    expect(
      pairs.map((pair) => [pair.caption, pair.a?.runId, pair.b?.runId]),
    ).toEqual([
      ['board', 'a1', 'b1'],
      ['only in A', 'a1', undefined],
      ['list', undefined, 'b1'],
    ])

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
    const figure = (image: Parameters<typeof ScreenshotFigure>[0]['image']) =>
      renderToStaticMarkup(
        <ScreenshotFigure
          which="b"
          screenshot={screenshot}
          image={image}
          evidenceHref="#run"
        />,
      )
    const shown = figure({ source: 'data:image/png;base64,iVBORw0K' })
    expect(shown).toContain('src="data:image/png;base64,iVBORw0K"')
    expect(shown).toContain('alt="B · board, desktop"')
    expect(shown).toContain('aria-label="Open B · board, desktop full size"')
    expect(shown).toContain('Evidence record')
    expect(figure(undefined)).toContain('Loading screenshot')
    expect(figure({ error: 'Evidence is 11000000 bytes' })).toContain(
      'Evidence is 11000000 bytes',
    )
    expect(
      renderToStaticMarkup(
        <ScreenshotFigure
          which="a"
          screenshot={null}
          image={undefined}
          evidenceHref={null}
        />,
      ),
    ).toContain('No screenshot with this caption in A')
  })
})
