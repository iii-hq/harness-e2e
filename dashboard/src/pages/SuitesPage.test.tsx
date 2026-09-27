import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { hashForSuites, hashForTests } from '@/hooks/use-hash-route'
import type { Suite } from '@/lib/dashboard-data-source'
import { type SuiteListItem, suiteListItem } from '@/lib/suites-view'
import {
  SuiteDetail,
  type SuiteDetailProps,
  SuiteList,
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
      narrow={false}
      ready
      busy={false}
      error={null}
      onCopy={() => {}}
      onRun={() => {}}
      onDelete={() => {}}
      {...props}
    />,
  )
}

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
