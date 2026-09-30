import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  byScoreChange,
  ComparisonView,
  deltaText,
  metricTone,
  outsideText,
  pairByCaption,
  RowDetail,
  ScreenshotFigure,
} from '@/components/compare/ComparisonView'
import { compareExecutions, comparisonGroup } from '@/lib/execution-comparison'
import { type ScreenshotEntry, screenshotsOf } from '@/lib/screenshots'
import {
  CandidateList,
  ComparisonPlaceholder,
  choiceCounting,
  choiceFromParams,
  choiceToParams,
  comparisonInvestigation,
  ExecutionComparePage,
  loadCandidates,
  loadExecutions,
  loadOlderCandidates,
  pickSides,
  selectionOf,
  viewParams,
} from '@/pages/ExecutionComparePage'
import {
  execution,
  imported,
  local,
} from '@/test-fixtures/execution-comparison'
import { LEDGER_EXECUTIONS } from '@/test-fixtures/executions-ledger'

function withRunners(a = imported(), b = local()) {
  for (const [detail, version] of [
    [a, '0.11.24'],
    [b, '0.11.27'],
  ] as const)
    for (const worker of detail.plan_execution?.stack ?? [])
      if (worker.name === 'harness-e2e') {
        worker.observed = version
        if (worker.resolved) worker.resolved = version
      }
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
  it('hands the chat the counted tests, the ones left out and why, and what changed', () => {
    expect(
      comparisonInvestigation(
        compareExecutions(imported(), local(), { exclude: ['minimal_path'] }),
      ),
    ).toEqual({
      executionId: 'import-a',
      comparisonExecutionId: 'local-b',
      visibleScenarioIds: ['persistent_state'],
      excludedScenarios: [
        { scenario_id: 'minimal_path', reason: 'left out by the reader' },
        {
          scenario_id: 'shell_coder_sandbox',
          reason:
            'technical_invalid in A: infrastructure_error — scenario setup failed: database never became ready',
        },
      ],
      unavailableDeltas: [],
      changes: [{ what: 'llm-router', change: '1.2.0 → @a1b2c3d + changes' }],
    })
  })

  it('tells the chat a side recorded no stack instead of listing nothing', () => {
    const a = imported()
    if (a.plan_execution) a.plan_execution.stack = []
    const { changes } = comparisonInvestigation(compareExecutions(a, local()))
    expect(changes?.[0]).toMatchObject({ what: 'stack' })
    expect(changes?.[0].change).toContain('no stack recorded for A')
  })

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
    // Opened from Trends: the way back stays in the hash.
    expect(
      viewParams(choice, '#/ext/harness-e2e/trends?stack=any').toString(),
    ).toBe(
      `include=shell_coder_sandbox&exclude=a%2Cb&from=${encodeURIComponent('#/ext/harness-e2e/trends?stack=any')}`,
    )
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
    // No verdict in words; the colour of a difference is an attribute.
    expect(html.replace(/<[^>]*>/g, ' ')).not.toMatch(
      /better|worse|improv|regress|winner/i,
    )
  })

  it('never calls uncommitted builds the same stack', () => {
    // The same executions twice: the only workers from a checkout carry
    // uncommitted changes, so nothing vouches that they match.
    const html = view(local(), local())
    expect(html).toContain('2 workers not comparable')
    expect(html).not.toContain('Nothing recorded differs')
    expect(html).not.toContain('Every worker on both sides ran the same build')
    expect(html).toContain('data-stack-unverified="llm-router"')
    expect(html).toContain('uncommitted changes')
  })

  it('says each side’s result as the executions list does', () => {
    const a = imported()
    const b = local()
    a.status = 'failed'
    a.started_at = '2026-09-29T12:00:00Z'
    const html = view(a, b)
    const card = (which: string) =>
      html.slice(
        html.indexOf(`data-comparison-side="${which}"`),
        html.indexOf(
          '</article>',
          html.indexOf(`data-comparison-side="${which}"`),
        ),
      )
    expect(card('a')).toContain('data-state="failed"')
    expect(card('a')).toContain('<span>Failed</span>')
    expect(html).not.toContain('Finished')
    // Where and when it ran; versions belong to the stack section.
    expect(card('a')).toMatch(
      /GitHub run 35823421664 · RC 366030b3 · \w{3} \d+/,
    )
    expect(card('a')).not.toContain('runner 0.9.3')
    b.status = 'running'
    expect(view(imported(), b)).toContain('<span>Running</span>')
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
    // Invalid runs count over every run; the ones out of the totals are said.
    const invalid = compareExecutions(imported(), local()).totals.find(
      (entry) => entry.id === 'technical_failures',
    )
    expect(invalid && outsideText(invalid)).toBe('1 run in A out of the totals')
  })

  it('colours a difference green or red (Compare.dc.html), the sign kept', () => {
    const { scenarios } = compareExecutions(imported(), local())
    const [persistent] = byScoreChange(scenarios)
    const metric = (id: string) => {
      const found = persistent.metrics.find((entry) => entry.id === id)
      if (!found) throw new Error(id)
      return found
    }
    // Each measure's own direction (the design system's deltaTone): more
    // score is better, fewer tokens and function calls are, cache figures
    // are neither; no change is neutral; nothing to compare has no colour.
    expect(metricTone(metric('score'))).toBe('negative')
    expect(metricTone({ ...metric('tokens'), delta: -10 })).toBe('positive')
    expect(metricTone({ ...metric('tokens'), delta: 10 })).toBe('negative')
    expect(metricTone({ ...metric('completed'), delta: 1 })).toBe('positive')
    expect(metricTone({ ...metric('function_calls'), delta: -2 })).toBe(
      'positive',
    )
    expect(metricTone({ ...metric('cache_read'), delta: 500 })).toBe('neutral')
    expect(metricTone(metric('turns'))).toBe('neutral')
    expect(metricTone({ ...metric('score'), delta: null })).toBe('unavailable')
    // A score or percent difference that rounds to 0 reads, and colours, as
    // no change.
    const tiny = { ...metric('score'), delta: 0.04 }
    expect(deltaText(tiny)).toBe('no change')
    expect(metricTone(tiny)).toBe('neutral')
    expect(
      metricTone({
        ...metric('score'),
        format: 'percent_points',
        delta: -0.02,
      }),
    ).toBe('neutral')
    expect(view()).toMatch(/data-tone="negative"[^>]*>−38 pts</)
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
    // A run of each side, its transcript and evidence record, which come
    // back to this comparison.
    const back = `?from=${encodeURIComponent('#/ext/harness-e2e/compare/import-a/local-b')}`
    expect(html).toContain('data-run-side="a"')
    expect(html).toContain(
      `href="#/ext/harness-e2e/execution/local-b/run/local-b-0/transcript${back}"`,
    )
    expect(html).toContain(
      `href="#/ext/harness-e2e/execution/import-a/run/import-a-0${back}"`,
    )
  })

  it('loads every execution in order and names each one that failed', async () => {
    const a = imported()
    const b = local()
    const get = (id: string) =>
      id === a.id
        ? Promise.resolve(a)
        : id === b.id
          ? Promise.resolve(b)
          : Promise.reject(new Error('Execution not found'))
    await expect(loadExecutions(get, [a.id, b.id])).resolves.toEqual([a, b])
    await expect(loadExecutions(get, [a.id, 'gone'])).rejects.toThrow(
      'gone could not be loaded: Execution not found',
    )
    await expect(loadExecutions(get, ['gone', b.id, 'lost'])).rejects.toThrow(
      'gone could not be loaded: Execution not found · lost could not be loaded: Execution not found',
    )
  })

  it('reads the reference and the one in detail from the hash, and refuses a pick outside the group', () => {
    const ids = ['a', 'b', 'c']
    expect(pickSides(ids, null, null)).toEqual({ a: 'a', b: 'b', error: null })
    // The one in detail made the reference: the next one takes its place.
    expect(pickSides(ids, 'b', null)).toEqual({ a: 'b', b: 'a', error: null })
    expect(pickSides(ids, null, 'c')).toEqual({ a: 'a', b: 'c', error: null })
    expect(pickSides(ids, 'x', null).error).toBe(
      'The reference x is not one of the executions compared.',
    )
    expect(pickSides(ids, 'a', 'y').error).toBe(
      'y is not one of the executions compared.',
    )
    expect(pickSides(ids, 'b', 'b').error).toBe(
      'The execution read in detail cannot be the reference.',
    )
    // Only a pick that is not the default goes in the hash.
    expect(selectionOf(ids, 'a', 'b')).toEqual({
      reference: null,
      compared: null,
    })
    expect(selectionOf(ids, 'b', 'a')).toEqual({
      reference: 'b',
      compared: null,
    })
    expect(selectionOf(ids, 'a', 'c')).toEqual({
      reference: null,
      compared: 'c',
    })
    expect(
      viewParams(
        { include: [], exclude: ['t'] },
        null,
        selectionOf(ids, 'b', 'c'),
      ).toString(),
    ).toBe('exclude=t&reference=b&compared=c')
  })

  it('asks for executions, shows loading, then the error', () => {
    const empty = renderToStaticMarkup(
      <ExecutionComparePage executionIds={[]} />,
    )
    expect(empty).toContain('Choose executions to compare')
    expect(empty).toContain('Back to Executions')
    const loading = renderToStaticMarkup(
      <ExecutionComparePage executionIds={['a', 'b', 'c']} />,
    )
    expect(loading).toContain('aria-busy="true"')
    expect(loading).toContain('Loading the executions')
    // A pick outside the group is said, never guessed.
    const outside = renderToStaticMarkup(
      <ExecutionComparePage executionIds={['a', 'b']} reference="z" />,
    )
    expect(outside).toContain(
      'The reference z is not one of the executions compared.',
    )
    const failed = renderToStaticMarkup(
      <ComparisonPlaceholder
        missing={false}
        error="gone could not be loaded: Execution not found"
        onRetry={() => undefined}
      />,
    )
    expect(failed).toContain('The comparison could not be loaded')
    expect(failed).toContain('gone could not be loaded')
    expect(failed).toContain('Retry')
  })

  it('Compare with… lists the latest executions but A, each opening A × B, several ticked together', async () => {
    const [a, ...others] = LEDGER_EXECUTIONS
    const asked: unknown[] = []
    const candidates = await loadCandidates(
      {
        listExecutions: async (input = {}) => {
          asked.push(input)
          return {
            executions: input.ids ? [a] : LEDGER_EXECUTIONS,
            next_cursor: input.ids ? null : 'older',
          }
        },
      },
      a.id,
    )
    expect(asked).toEqual([{ limit: 50 }, { ids: [a.id], limit: 1 }])
    expect(candidates.a?.id).toBe(a.id)
    expect(candidates.cursor).toBe('older')
    expect(candidates.rows.map((row) => row.id)).toEqual(
      others.map((other) => other.id),
    )
    const html = renderToStaticMarkup(
      <CandidateList left={a.id} {...candidates} onMore={() => undefined} />,
    )
    expect(html).toContain('Compare with…')
    expect(html).toContain(candidates.a?.title)
    expect(html).toContain(
      `href="#/ext/harness-e2e/compare/${a.id}/${others[0].id}"`,
    )
    expect(html).not.toContain(`data-candidate="${a.id}"`)
    // Several can be ticked; nothing ticked, nothing to compare yet.
    expect(html).toContain(`aria-label="Select ${candidates.rows[0].title}"`)
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*data-compare-picked/)
    expect(html).toContain('data-candidates-more')
    // An older page adds what is not listed yet, A and the rows kept apart.
    const older = await loadOlderCandidates(
      {
        listExecutions: async (input = {}) => {
          asked.push(input)
          return {
            executions: [a, others[0], { ...others[0], id: 'older-1' }],
            next_cursor: null,
          }
        },
      },
      a.id,
      candidates,
    )
    expect(asked.at(-1)).toEqual({ limit: 50, cursor: 'older' })
    expect(older.rows.map((row) => row.id)).toEqual([
      ...others.map((other) => other.id),
      'older-1',
    ])
    expect(older.cursor).toBeNull()
    expect(
      renderToStaticMarkup(
        <CandidateList left={a.id} {...older} onMore={() => undefined} />,
      ),
    ).not.toContain('data-candidates-more')
    expect(
      renderToStaticMarkup(<CandidateList left={a.id} a={null} rows={[]} />),
    ).toContain('No other execution to compare with')
  })

  it('shows a group side by side, each against the reference, over the same tests', () => {
    const a = execution('a', [{ score: 60 }, { score: 80 }, { score: 100 }])
    const b = execution('b', [{ score: 70 }, { score: 90 }, { score: 90 }])
    const c = execution('c', [
      { score: 50 },
      { technical: 'technical_invalid', score: null },
      { score: 100 },
    ])
    const group = comparisonGroup([a, b, c])
    const pairs = [b, c].map((one) => compareExecutions(a, one, { group }))
    const html = renderToStaticMarkup(
      <ComparisonView
        comparison={pairs[0]}
        sides={{ a, b }}
        group={{
          executions: [a, b, c],
          pairs,
          compareHref: (id) => `#compare-${id}`,
          referenceHref: (id) => `#reference-${id}`,
        }}
        swap="#swap"
        onCount={() => undefined}
      />,
    )
    expect(html).toContain('3 executions side by side')
    expect(
      [...html.matchAll(/data-matrix-execution="([^"]+)"/g)].map(
        (match) => match[1],
      ),
    ).toEqual(['a', 'b', 'c'])
    expect(html).toContain('data-role="reference"')
    expect(html).toContain('href="#compare-c"')
    expect(html).not.toContain('href="#compare-b"')
    expect(html).toContain('href="#reference-b"')
    expect(html).toContain('data-matrix-metric="score"')
    // C's invalid run takes test_1 out of every total, B's included.
    expect(html).toContain('data-matrix-scenario="test_1" data-counted="false"')
    expect(html).toContain('data-matrix-cell="c"')
    expect(html).toContain('In detail: the reference and')
    // Two executions keep the page as it was.
    expect(view()).not.toContain('data-comparison-group')
  })

  it('says a side is still running and its figures are partial', () => {
    const b = local()
    b.status = 'running'
    const html = view(local(), b)
    expect(html).toContain('data-comparison-live')
    expect(html).toContain('B is still running')
    expect(html).toContain('B partial')
    // A refresh that failed is said above the comparison, which stays.
    const failed = renderToStaticMarkup(
      <ComparisonView
        comparison={compareExecutions(local(), b)}
        sides={{ a: local(), b }}
        swap="#swap"
        refreshError="engine unavailable"
        onCount={() => undefined}
      />,
    )
    expect(failed).toContain('Refresh failed.')
    expect(failed).toContain('engine unavailable')
    expect(failed).toContain('data-comparison-scenarios')
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
