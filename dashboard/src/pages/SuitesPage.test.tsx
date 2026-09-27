import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { hashForSuites, hashForTests } from '@/hooks/use-hash-route'
import type { Suite } from '@/lib/dashboard-data-source'
import {
  type SuiteListItem,
  suiteDraft,
  suiteListItem,
  tickDraft,
} from '@/lib/suites-view'
import type { TestCatalogRow } from '@/lib/test-catalog'
import { catalogRowView } from '@/lib/test-catalog-view'
import {
  SuiteDetail,
  type SuiteDetailProps,
  SuiteList,
  suitesHeaderActions,
} from '@/pages/SuitesPage'

function suite(id: string, facts: Partial<Suite> = {}): Suite {
  return {
    id,
    label: id,
    source: 'repository',
    purpose: '',
    scenarios: ['a', 'b'],
    repetitions: 1,
    technical_retries: 1,
    sha256: 'sha256:2463231e9958aaaa',
    updated_at: null,
    ...facts,
  }
}

const regression = suite('regression', {
  label: 'Regression',
  purpose: 'Detect regressions in core runtime behavior.',
})
const mine = suite('suite-1', {
  label: 'Regression · quick',
  source: 'local',
  sha256: null,
})

function items(suites: Suite[]) {
  return new Map<string, SuiteListItem>(
    suites.map((entry) => [entry.id, suiteListItem(entry, [], new Map())]),
  )
}

function detail(props: Partial<SuiteDetailProps> = {}) {
  return renderToStaticMarkup(
    <SuiteDetail
      suite={regression}
      runs={[]}
      views={new Map()}
      groups={[]}
      narrow={false}
      ready
      busy={false}
      error={null}
      onCopy={() => {}}
      onEdit={() => {}}
      onRun={() => {}}
      onDelete={() => {}}
      {...props}
    />,
  )
}

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

const views = new Map(
  [
    row('registry_implementation', {
      last_run: {
        at: '2026-09-24T10:00:00Z',
        score: 92,
        status: 'passed',
        completion: 'completed',
        definition: 'current',
      },
      recent_scores: [92, null, 100],
      runs_current: 5,
      runs_total: 8,
    }),
    row('registry_verification', {
      last_run: {
        at: '2026-09-08T10:00:00Z',
        score: 100,
        status: 'passed',
        completion: 'completed',
        definition: 'previous',
      },
      recent_scores: [100],
      runs_total: 1,
    }),
    row('minimal_path'),
  ].map((entry) => [entry.test_id, catalogRowView(entry, [], NOW)]),
)

describe('SuiteList', () => {
  it('keeps the repository apart from this Console, and says how to make one', () => {
    const html = renderToStaticMarkup(
      <SuiteList
        suites={[regression]}
        items={items([regression])}
        selectedId="regression"
      />,
    )
    expect(html).toContain('Repository')
    expect(html).toContain('read-only')
    expect(html).toContain('This Console')
    expect(html).toContain(`href="${hashForSuites('regression')}"`)
    expect(html).toContain('aria-current="true"')
    expect(html).toContain('not run in this Console')
    expect(html).toContain(`href="${hashForTests()}"`)
    expect(html).toContain('to make one you')
  })

  it('lists this Console’s suites as editable, without the note', () => {
    const html = renderToStaticMarkup(
      <SuiteList
        suites={[regression, mine]}
        items={items([regression, mine])}
        selectedId="regression"
      />,
    )
    expect(html).toContain('editable')
    expect(html).toContain('data-suite="suite-1"')
    expect(html).not.toContain('to make one you')
    expect(html.match(/aria-current/g)).toHaveLength(1)
  })
})

describe('SuiteDetail', () => {
  it('offers a repository suite to copy and run, not to delete', () => {
    const html = detail()
    expect(html).toContain('Regression')
    expect(html).toContain('repository · read-only')
    expect(html).toContain('2 tests · 1 run each · 1 retry')
    expect(html).toContain('2463231e9958')
    expect(html).toContain('Detect regressions in core runtime behavior.')
    expect(html).toContain('Copy to edit')
    expect(html).toContain('Run this suite')
    expect(html).not.toContain('Delete ')
    expect(html).not.toContain('st-back')
  })

  it('offers a suite of this Console to delete, and leads back in a narrow pane', () => {
    const html = detail({ suite: mine, narrow: true })
    expect(html).toContain('this Console')
    expect(html).toContain('aria-label="Delete Regression · quick"')
    expect(html).not.toContain('Copy to edit')
    expect(html).toContain(`href="${hashForSuites()}"`)
    // Not materialized by this runner: no digest.
    expect(html).toContain('Digest: none')
  })

  it('says why an action failed', () => {
    expect(detail({ error: 'The suite store is read-only.' })).toContain(
      'role="alert">The suite store is read-only.',
    )
  })
})

describe('the open suite’s tests and executions', () => {
  const software = suite('software-engineering', {
    label: 'Software engineering',
    scenarios: [
      'minimal_path',
      'registry_implementation',
      'registry_verification',
    ],
  })

  it('lists each test with its last result, its scores and its step', () => {
    const html = detail({
      suite: software,
      views,
      groups: [['registry_implementation', 'registry_verification']],
    })
    expect(html).toContain('data-test-id="registry_implementation"')
    expect(html).toContain('1 of 2 · in order')
    expect(html).toContain('2 of 2 · in order')
    expect(html).toContain(
      'registry_implementation and registry_verification run whole, in order.',
    )
    expect(html).toContain('Lost points')
    expect(html).toContain('Sep 8 · older definition')
    expect(html).toContain('No run retained')
    expect(html).toContain('Recent scores: 92, none, 100')
    // The one test that changed since it ran, said once above the table.
    expect(html).toContain('1 of these 3 tests changed definition')
    expect(html).toContain(
      'Not run in this Console yet. Executions started from this suite',
    )
  })

  it('keeps the test and its last result in a narrow pane', () => {
    const html = detail({ suite: software, views, narrow: true })
    expect(html).not.toContain('Recent scores')
    expect(html).toContain('Lost points')
  })

  it('links the executions that ran it', () => {
    const html = detail({
      suite: software,
      runs: [
        {
          id: 'plan-1',
          title: 'Software engineering',
          meta: 'GitHub #35965100994 · Sep 24, 4:25 AM',
          result: { state: 'failed' },
          outcome: '11 of 15 passed · score 93.2',
          day: 'Sep 24',
        },
      ],
    })
    expect(html).toContain('href="#/ext/harness-e2e/execution/plan-1"')
    expect(html).toContain('Failed')
    expect(html).toContain('· 11 of 15 passed · score 93.2')
    expect(html).not.toContain('Not run in this Console yet')
  })
})

describe('editing a suite of this Console', () => {
  const groups = [['registry_implementation', 'registry_verification']]
  const quick = suite('suite-1', {
    label: 'Regression · quick',
    source: 'local',
    scenarios: [
      'minimal_path',
      'registry_implementation',
      'registry_verification',
    ],
  })
  const editing = (draft = suiteDraft(quick)) => ({
    draft,
    catalog: [...views.values()],
    saving: false,
    onChange: () => {},
    onSave: () => {},
    onDiscard: () => {},
  })

  it('offers Edit, Delete and Run while it is not being edited', () => {
    const html = detail({ suite: quick })
    expect(html).toContain('id="st-edit"')
    expect(html).toContain('aria-label="Delete Regression · quick"')
    expect(html).toContain('Run this suite')
    expect(html).not.toContain('Save suite')
  })

  it('edits the name, runs and retries in place, with Discard and Save', () => {
    const html = detail({ suite: quick, views, groups, editing: editing() })
    expect(html).toContain('id="st-name"')
    expect(html).toContain('value="Regression · quick"')
    expect(html).toContain('Runs of each test')
    expect(html).toContain('Retries on crash')
    expect(html).toContain('aria-label="More runs"')
    expect(html).toContain('Discard')
    expect(html).toContain('Save suite')
    expect(html).not.toContain('Run this suite')
    expect(html).not.toContain('id="st-edit"')
    // Nothing changed yet.
    expect(html).not.toContain('Unsaved changes')
    expect(html).toContain('Untick to take a test out')
    expect(html).toContain('aria-label="Keep minimal_path in the suite"')
    expect(html).toContain('Add tests')
  })

  it('holds every control still while it saves', () => {
    const html = detail({
      suite: quick,
      views,
      groups,
      editing: { ...editing(), saving: true },
    })
    expect(html).toMatch(/id="st-name"[^>]*disabled=""/)
    expect(html).toMatch(
      /aria-label="Keep minimal_path in the suite"[^>]*disabled=""/,
    )
    expect(html).toMatch(/id="st-add"[^>]*disabled=""/)
    expect(html).toMatch(/aria-label="More runs"[^>]*disabled=""/)
    expect(html).toContain('Saving…')
  })

  it('keeps an unticked test listed, faded, and says the changes are unsaved', () => {
    const draft = tickDraft(suiteDraft(quick), 'minimal_path', false, groups)
    const html = detail({
      suite: quick,
      views,
      groups,
      editing: editing(draft),
    })
    expect(html).toContain('Unsaved changes')
    expect(html).toMatch(/data-test-id="minimal_path" data-off="true"/)
    expect(html).toContain('2 tests · 1 run each · 1 retry')
  })
})

describe('the Suites header', () => {
  it('leads New suite to the catalog, and adds Run tests once the worker answers', () => {
    expect(suitesHeaderActions().map((action) => action.id)).toEqual(['new'])
    const [create, run] = suitesHeaderActions(() => {})
    expect(create).toMatchObject({
      label: 'New suite',
      href: hashForTests(),
      title: 'Tick tests in the catalog, then save them as a suite',
    })
    expect(run).toMatchObject({ label: 'Run tests', primary: true })
  })

  it('asks before New suite leaves unsaved changes', () => {
    const ask = () => {}
    const [create] = suitesHeaderActions(undefined, ask)
    expect(create.href).toBeUndefined()
    expect(create.onSelect).toBe(ask)
  })
})
