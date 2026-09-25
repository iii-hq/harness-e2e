import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  buildLedgerRows,
  deleteConfirmation,
  deletedMessage,
  filterLedgerRows,
  groupLedgerRows,
  LEDGER_DEFAULT_FILTERS,
  type LedgerActions,
  type LedgerRow,
  LedgerTable,
  ledgerFiltersFromParams,
  ledgerFiltersToParams,
  ledgerSummary,
  resultSegments,
  rowMenuItems,
  selectionSummary,
  shownSelection,
  toggleSelection,
  toggleShown,
} from '@/pages/ExecutionsPage'
import {
  LEDGER_EXECUTIONS,
  LEDGER_NOW,
  LEDGER_TOTAL,
  ledgerExecution,
} from '@/test-fixtures/executions-ledger'

const rows = buildLedgerRows(LEDGER_EXECUTIONS, LEDGER_NOW)
const row = (id: string) => {
  const found = rows.find((entry) => entry.id.startsWith(id))
  if (!found) throw new Error(`no row ${id}`)
  return found
}
const ids = (list: LedgerRow[]) => list.map((entry) => entry.id.slice(0, 13))

const noop = () => undefined
const actions: LedgerActions = {
  open: noop,
  rename: noop,
  openOnGithub: noop,
  importAgain: noop,
  runAgain: noop,
  copyId: noop,
  cancel: noop,
  delete: noop,
}

describe('executions list filters', () => {
  // Audit E-04: filters round-trip through the hash.
  it('reads and writes only the non-default filters', () => {
    const filters = ledgerFiltersFromParams(
      new URLSearchParams('q=terra&status=failed&sort=tokens'),
    )
    expect(filters).toEqual({
      query: 'terra',
      status: 'failed',
      sort: 'tokens',
    })
    expect(ledgerFiltersToParams(filters).toString()).toBe(
      'q=terra&status=failed&sort=tokens',
    )
    expect(ledgerFiltersToParams(LEDGER_DEFAULT_FILTERS).toString()).toBe('')
    // What this list does not know falls back to the default.
    expect(
      ledgerFiltersFromParams(new URLSearchParams('status=cancelling&sort=x')),
    ).toEqual(LEDGER_DEFAULT_FILTERS)
  })

  it('searches the title, id, model, profile, origin and date', () => {
    const search = (query: string) =>
      ids(filterLedgerRows(rows, { ...LEDGER_DEFAULT_FILTERS, query }))
    expect(search('template only')).toEqual(['plan-81960bf0'])
    expect(search('plan-958b')).toEqual(['plan-958b1543'])
    expect(search('meu-profile')).toEqual(['plan-7706993f'])
    expect(search('docker')).toEqual(['plan-9c41d07b'])
    expect(search('github #35925167026')).toEqual(['plan-958b1543'])
    expect(search('sep 20')).toHaveLength(2)
    expect(search('deepseek/deepseek-flash')).toHaveLength(8)
  })

  it('also searches the tests, workflow, run id, commit and every model', () => {
    const native = ledgerExecution('c3cdb199')
    const paired = ledgerExecution('a1d33f69')
    const [first, second] = buildLedgerRows(
      [
        {
          ...native,
          run_id: 'run-7781',
          workflow_name: 'Harness E2E Local',
          source: { kind: 'local', sha: '88aee14d0c' },
          parameters: native.parameters && {
            ...native.parameters,
            scenarios: ['kanban_c2_persistence'],
          },
        },
        {
          ...paired,
          subjects: [
            ...paired.subjects,
            {
              id: 'fable',
              provider: 'claude-code',
              model: 'fable-5',
              scenarios: [],
            },
          ],
        },
      ],
      LEDGER_NOW,
    )
    const search = (query: string) =>
      filterLedgerRows([first, second], {
        ...LEDGER_DEFAULT_FILTERS,
        query,
      }).map((entry) => entry.id)
    for (const query of [
      'kanban_c2',
      'harness e2e local',
      'run-7781',
      '88aee14',
    ])
      expect(search(query)).toEqual([first.id])
    expect(search('claude-code/fable-5')).toEqual([second.id])
  })

  it('filters by result and sorts newest first, or as asked', () => {
    expect(
      ids(
        filterLedgerRows(rows, { ...LEDGER_DEFAULT_FILTERS, status: 'failed' }),
      ),
    ).toEqual([
      'plan-00ec9877',
      'plan-3ef1b6a7',
      'plan-1d320744',
      'plan-958b1543',
    ])
    expect(
      ids(
        filterLedgerRows(rows, {
          ...LEDGER_DEFAULT_FILTERS,
          status: 'running',
        }),
      ),
    ).toEqual(['plan-e5b0a2c4', 'plan-9c41d07b', 'plan-2b7e41c0'])
    const first = (sort: typeof LEDGER_DEFAULT_FILTERS.sort) =>
      filterLedgerRows(rows, { ...LEDGER_DEFAULT_FILTERS, sort })[0].id
    expect(first('newest')).toMatch(/^plan-e5b0/)
    expect(first('oldest')).toMatch(/^c92c4cd4/)
    expect(first('tokens')).toMatch(/^plan-958b/)
    expect(first('runtime')).toMatch(/^plan-958b/)
    expect(first('result')).toMatch(/^plan-00ec/)
  })

  it('offers each result present, with its count, after All', () => {
    expect(
      resultSegments(rows).map(({ value, label, count }) => [
        value,
        label,
        count,
      ]),
    ).toEqual([
      ['all', 'All', 16],
      ['passed', 'Passed', 8],
      ['failed', 'Failed', 4],
      ['incomplete', 'Incomplete', 1],
      ['running', 'Running', 3],
    ])
    expect(ledgerSummary(rows, LEDGER_TOTAL)).toBe(
      '58 retained · 16 loaded · 8 passed · 4 failed · 1 incomplete · 3 running',
    )
  })
})

describe('executions list rows', () => {
  it('writes each cell as the canvas does', () => {
    expect(row('plan-cf6ab5f9')).toMatchObject({
      title: 'Opus 5.5 · ade-worker-builder · wake fix',
      meta: 'This harness · 10:10 AM · plan-cf6ab5f9',
      result: { state: 'passed' },
      issue: null,
      model: 'claude-code/claude-opus-5-5',
      profile: 'profile ade-solo-builder',
      tests: '2/2',
      score: '100',
      passRate: '100%',
      runtime: '17m 29s',
      tokens: '85.4K',
    })
    expect(row('plan-958b1543')).toMatchObject({
      meta: 'GitHub #35925167026 · 8:00 PM · plan-958b1543',
      result: { state: 'failed' },
      issue: '1 infrastructure event',
      model: 'deepseek/deepseek-flash',
      tests: '13/13',
      score: '—',
      passRate: '61.5%',
      runtime: '3h 22m',
      tokens: '11.9M',
      github: {
        runId: 35925167026,
        url: 'https://github.com/iii-hq/harness-e2e/actions/runs/35925167026',
      },
    })
    expect(row('plan-7706993f')).toMatchObject({
      result: { state: 'incomplete' },
      issue: '2 inconclusive events',
      profile: 'profile meu-profile',
      tests: '0/2',
      runtime: '—',
    })
    expect(row('c3cdb199')).toMatchObject({
      meta: 'This harness · 12:18 AM · c3cdb199cf8ec',
      profile: 'no profile',
      runtime: '8.8s',
      tokens: '4.2K',
    })
  })

  it('titles an untitled execution by its model and when it was created', () => {
    expect(row('plan-3ef1b6a7').title).toBe(
      'claude-code/claude-opus-5-5 · Sep 24, 2026, 6:43 AM',
    )
  })

  it('reads what runs: Docker by its groups, an import, this harness by its tests', () => {
    expect(row('plan-9c41d07b')).toMatchObject({
      meta: 'Docker · 8:12 PM · plan-9c41d07b',
      result: { state: 'running' },
      live: true,
      issue: '3 of 9 groups finished · 2 running · 4 waiting',
      tests: '3/9',
    })
    expect(row('plan-e5b0a2c4')).toMatchObject({
      meta: 'GitHub #36073359724 · 8:33 PM · plan-e5b0a2c4',
      result: { state: 'running', label: 'Importing' },
      live: true,
      issue: '6 of 14 group jobs finished',
    })
    expect(row('plan-2b7e41c0')).toMatchObject({
      result: { state: 'running' },
      issue: '1 of 2 runs reported',
    })
  })

  // Audit E-12: what runs comes first, then one group per day.
  it('groups what runs first, then by day', () => {
    const groups = groupLedgerRows(
      filterLedgerRows(rows, LEDGER_DEFAULT_FILTERS),
      LEDGER_NOW,
    )
    expect(groups.map((group) => [group.label, group.rows.length])).toEqual([
      ['Running', 3],
      ['Today · Sep 24', 8],
      ['Yesterday · Sep 23', 1],
      ['Sep 21', 2],
      ['Sep 20', 2],
    ])
  })
})

describe('executions list selection', () => {
  it('ticks one at a time or every row shown, as a tri-state box', () => {
    const shown = ['a', 'b', 'c']
    expect(shownSelection([], shown)).toBe('none')
    expect(shownSelection(['b'], shown)).toBe('some')
    expect(toggleShown(['b', 'z'], shown)).toEqual(['b', 'z', 'a', 'c'])
    expect(shownSelection(['b', 'z', 'a', 'c'], shown)).toBe('all')
    // Rows filtered out keep their tick.
    expect(toggleShown(['b', 'z', 'a', 'c'], shown)).toEqual(['z'])
    expect(toggleSelection(['a', 'b'], 'a')).toEqual(['b'])
    expect(toggleSelection(['b'], 'a')).toEqual(['b', 'a'])
  })

  it('compares exactly two, A first, and keeps what runs out of a delete', () => {
    const one = selectionSummary([row('plan-cf6ab5f9')])
    expect(one).toMatchObject({
      text: '1 selected',
      hint: 'Tick one more to compare.',
      compare: null,
      deleteLabel: 'Delete',
    })
    const two = selectionSummary([row('plan-cf6ab5f9'), row('plan-81960bf0')])
    expect(two.hint).toBe('A is the first you ticked.')
    expect(two.compare).toEqual([
      row('plan-cf6ab5f9').id,
      row('plan-81960bf0').id,
    ])
    expect(two.deleteLabel).toBe('Delete 2')
    const three = selectionSummary([
      row('plan-2b7e41c0'),
      row('plan-00ec9877'),
      row('plan-3ef1b6a7'),
    ])
    expect(three).toMatchObject({
      text: '3 selected',
      hint: '1 running will be kept.',
      compare: null,
      deleteLabel: 'Delete 2',
    })
    expect(three.deletable).toEqual([
      row('plan-00ec9877').id,
      row('plan-3ef1b6a7').id,
    ])
    expect(selectionSummary([row('plan-2b7e41c0')]).deletable).toEqual([])
  })
})

describe('the row menu', () => {
  const menu = (id: string) =>
    rowMenuItems(row(id), actions).map((item) =>
      [
        item.separator ? '—' : '',
        item.label,
        item.hint ? `(${item.hint})` : '',
        item.disabledReason ? `[${item.disabledReason}]` : '',
      ]
        .join('')
        .trim(),
    )

  it('runs again what ran here, and imports again what came from GitHub', () => {
    expect(menu('plan-cf6ab5f9')).toEqual([
      'Open',
      'Rename',
      'Run again',
      'Copy execution id',
      '—Delete…',
    ])
    expect(menu('plan-1d320744')).toEqual([
      'Open',
      'Rename',
      'Open on GitHub',
      'Import again(Replaces its evidence with the run’s)',
      'Copy execution id',
      '—Delete…',
    ])
    // A native run has no name of its own to change.
    expect(menu('c3cdb199')).toEqual([
      'Open',
      'Run again',
      'Copy execution id',
      '—Delete…',
    ])
  })

  it('cancels what runs, and says why it cannot be deleted yet', () => {
    expect(menu('plan-2b7e41c0')).toEqual([
      'Open',
      'Rename',
      'Copy execution id',
      '—Cancel execution',
      'Delete…[Finish or cancel it first]',
    ])
    expect(menu('plan-9c41d07b')).toContain('—Cancel execution')
    expect(menu('plan-e5b0a2c4')).toEqual([
      'Open',
      'Rename',
      'Open on GitHub',
      'Copy execution id',
      '—Delete…[Wait for the import to finish]',
    ])
  })
})

describe('deleting executions', () => {
  it('says what one imported execution takes with it and what stays', () => {
    const confirmation = deleteConfirmation(
      [row('plan-1d320744')],
      [],
      LEDGER_NOW,
    )
    expect(confirmation).toMatchObject({
      title: 'Delete “Software engineering”?',
      body: 'This can’t be undone.',
      items: [
        {
          title: 'Software engineering',
          meta: 'GitHub #35965100994 · Sep 24, 4:25 AM · 13/15 tests · 3.34M tokens',
        },
      ],
      more: null,
      action: 'Delete execution',
    })
    expect(confirmation.facts).toEqual([
      {
        tone: 'gone',
        text: '13 test runs with their transcripts, reports and screenshots leave this Console.',
      },
      {
        tone: 'gone',
        text: 'Links to it, comparisons included, stop working.',
      },
      {
        tone: 'kept',
        text: 'The run on GitHub is not touched. You can import #35965100994 again.',
      },
    ])
  })

  it('lists five, sums up the rest and names what keeps running', () => {
    const finished = rows.filter((entry) => !entry.live).slice(2, 9)
    const confirmation = deleteConfirmation(
      finished,
      [row('plan-2b7e41c0')],
      LEDGER_NOW,
    )
    expect(confirmation.title).toBe('Delete 7 executions?')
    expect(confirmation.items).toHaveLength(5)
    expect(confirmation.more).toBe('and 2 more')
    expect(confirmation.action).toBe('Delete 7 executions')
    expect(confirmation.facts.map((fact) => fact.text)).toEqual([
      '34 test runs with their transcripts, reports and screenshots leave this Console.',
      'Links to them, comparisons included, stop working.',
      'The runs on GitHub are not touched. You can import them again.',
      '“Opus 5.5 · ade-worker-builder · retry” is still running and stays. Cancel it first to delete it.',
    ])
    expect(deletedMessage(['no profile'])).toBe(
      'Deleted “no profile” with its runs and evidence.',
    )
    expect(deletedMessage(['a', 'b'])).toBe(
      'Deleted 2 executions with their runs and evidence.',
    )
  })
})

describe('the executions table', () => {
  const render = (selected: string[]) =>
    renderToStaticMarkup(
      <LedgerTable
        groups={groupLedgerRows(rows, LEDGER_NOW)}
        selected={selected}
        onSelect={noop}
        actions={actions}
      />,
    )

  it('renders each group under its heading with every column named', () => {
    const html = render([])
    for (const header of [
      'Execution',
      'Result',
      'Model',
      'Tests',
      'Score',
      'Pass rate',
      'Runtime',
      'Tokens',
      'Actions',
    ])
      expect(html).toContain(`>${header}<`)
    expect(html).toContain('aria-label="Select every execution shown"')
    expect(html.match(/data-execution-id=/g)).toHaveLength(16)
    expect(html).toContain('aria-label="Select no profile"')
    expect(html).toContain('aria-label="Actions for no profile"')
    expect(html).toContain('>Today · Sep 24<')
    expect(html).toContain('title="3,339,305 tokens"')
    expect(html).toContain('3 of 9 groups finished · 2 running · 4 waiting')
    expect(html).toContain('>Importing<')
    expect(html).not.toContain('Compared as')
  })

  it('keeps execution, result, tests and the menu in a narrow pane', () => {
    const html = renderToStaticMarkup(
      <LedgerTable
        narrow
        groups={groupLedgerRows(rows, LEDGER_NOW)}
        selected={[]}
        onSelect={noop}
        actions={actions}
      />,
    )
    for (const header of ['Execution', 'Result', 'Tests', 'Actions'])
      expect(html).toContain(`>${header}<`)
    for (const header of ['Model', 'Score', 'Pass rate', 'Runtime', 'Tokens'])
      expect(html).not.toContain(`>${header}<`)
    expect(html).toContain('colSpan="5"')
  })

  it('marks A and B when exactly two are ticked', () => {
    const a = ledgerExecution('plan-81960bf0').id
    const b = ledgerExecution('plan-cf6ab5f9').id
    const html = render([a, b])
    expect(html.match(/title="Compared as (A|B)"/g)).toEqual([
      'title="Compared as B"',
      'title="Compared as A"',
    ])
    expect(html.match(/checked=""/g)).toHaveLength(2)
    expect(render([a, b, ledgerExecution('c3cdb199').id])).not.toContain(
      'Compared as',
    )
  })
})
