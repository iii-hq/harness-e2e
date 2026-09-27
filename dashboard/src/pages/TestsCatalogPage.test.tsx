import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  hashForTestHistory,
  hashForVersionComparison,
} from '@/hooks/use-hash-route'
import type { DashboardDataBridge } from '@/lib/dashboard-data-source'
import type { TestCatalogRow, TestsListInput } from '@/lib/test-catalog'
import { catalogRowView } from '@/lib/test-catalog-view'
import {
  CatalogTable,
  catalogHeaderActions,
  catalogSelection,
  listAllTests,
} from '@/pages/TestsCatalogPage'

const NOW = new Date('2026-09-27T12:00:00Z')

function row(id: string, facts: Partial<TestCatalogRow> = {}): TestCatalogRow {
  return {
    test_id: id,
    lifecycle: 'active',
    current_version: 'sha256:c3c3',
    available_versions: [],
    selected_version: null,
    result: null,
    last_run: null,
    recent_scores: [],
    runs_current: 0,
    runs_total: 0,
    ...facts,
  }
}

const views = [
  row('kanban_c1_foundation', {
    last_run: {
      at: '2026-09-24T10:00:00Z',
      score: 90,
      status: 'passed',
      completion: 'completed',
      definition: 'current',
    },
    recent_scores: [null, 100, 90],
    runs_current: 4,
    runs_total: 6,
  }),
  row('kanban_c2_persistence', {
    last_run: {
      at: '2026-09-08T10:00:00Z',
      score: null,
      status: 'infrastructure_error',
      completion: 'undetermined',
      definition: 'previous',
    },
    recent_scores: [null],
    runs_total: 1,
  }),
  row('minimal_path'),
].map((entry) =>
  catalogRowView(
    entry,
    entry.test_id === 'minimal_path' ? [{ id: 'pr', label: 'PR' }] : [],
    NOW,
  ),
)

const render = (selected: string[], narrow = false) =>
  renderToStaticMarkup(
    <CatalogTable
      all={views}
      shown={views}
      selected={selected}
      onSelect={() => undefined}
      narrow={narrow}
    />,
  )

/** Whether the box named `label` is ticked in the markup. */
const ticked = (html: string, label: string) =>
  new RegExp(`aria-label="${label}"[^>]*checked=""`).test(html)

describe('tests catalog table', () => {
  it('draws a block per family, Standalone last, with its note and count', () => {
    const html = render([])
    expect(html.indexOf('data-catalog-group="kanban"')).toBeLessThan(
      html.indexOf('data-catalog-group=":standalone"'),
    )
    expect(html).toContain('<tbody class="tc-group" aria-label="kanban"')
    expect(html).toContain('1 of 2 current')
    expect(html).toContain('>Standalone<')
    expect(html).toContain('none current')
    expect(html).toContain('aria-label="Tests, 3 of 3"')
  })

  it('writes each row: the result, score, date, runs, spark and suites', () => {
    const html = render([])
    expect(html).toContain('data-state="lost_points"')
    expect(html).toContain('>90<')
    expect(html).toContain('Sep 24')
    expect(html).toContain('4 on the current definition · 6 retained in all')
    expect(html).toContain('aria-label="Recent scores: none, 100, 90"')
    // An older definition's result is faded and says so.
    expect(html).toContain('data-older="true"')
    expect(html).toContain('Sep 8 · older definition')
    expect(html).toContain('data-state="infra_error"')
    // Never run: no score, no date.
    expect(html).toContain('data-state="never_run"')
    expect(html).toContain('No run retained')
    expect(html).toContain('title="PR">PR<')
    expect(html).toContain(`href="${hashForTestHistory('minimal_path')}"`)
    expect(html).toContain('aria-label="History of minimal_path"')
  })

  it('ticks a family whole, and every test shown', () => {
    const one = render(['kanban_c1_foundation'])
    expect(ticked(one, 'Select kanban_c1_foundation')).toBe(true)
    expect(ticked(one, 'Select every test in kanban')).toBe(false)
    expect(one).toContain('data-selected="true"')
    const family = render(['kanban_c1_foundation', 'kanban_c2_persistence'])
    expect(ticked(family, 'Select every test in kanban')).toBe(true)
    expect(ticked(family, 'Select every test shown')).toBe(false)
    const every = render(views.map((view) => view.id))
    expect(ticked(every, 'Select every test shown')).toBe(true)
  })

  it('keeps the test and its last result on a narrow pane', () => {
    const html = render([], true)
    expect(html).toContain('data-narrow="true"')
    expect(html).not.toContain('Recent scores')
    expect(html).not.toContain('>Suites<')
    expect(html).toContain('Last result')
  })
})

describe('tests catalog actions', () => {
  it('offers the version comparison, and Run tests once it can run', () => {
    expect(catalogHeaderActions().map((action) => action.label)).toEqual([
      'Compare system versions',
    ])
    const actions = catalogHeaderActions(() => undefined)
    expect(actions.map((action) => [action.label, action.primary])).toEqual([
      ['Compare system versions', undefined],
      ['Run tests', true],
    ])
    expect(actions[0].href).toBe(hashForVersionComparison())
  })

  it('names the selection and its run', () => {
    expect(catalogSelection(1)).toMatchObject({
      text: '1 test selected',
      run: 'Run 1 test',
    })
    expect(catalogSelection(3)).toMatchObject({
      text: '3 tests selected',
      run: 'Run 3 tests',
    })
  })

  it('lists every test through the pages of tests-list', async () => {
    const requests: TestsListInput[] = []
    const bridge = {
      listTests: async (input: TestsListInput = {}) => {
        requests.push(input)
        return {
          revision: 'r1',
          rows: [row(input.cursor ? 'b' : 'a')],
          total: 2,
          next_cursor: input.cursor ? null : 'r1:1',
        }
      },
    } as unknown as DashboardDataBridge
    const listed = await listAllTests(bridge)
    expect(listed.rows.map((entry) => entry.test_id)).toEqual(['a', 'b'])
    expect(listed.revision).toBe('r1')
    expect(requests).toEqual([{ limit: 100 }, { limit: 100, cursor: 'r1:1' }])
  })
})
