import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { DashboardDataBridge, Stack } from '@/lib/dashboard-data-source'
import { StackSheet } from './StackSheet'

const repository: Stack = {
  id: 'default',
  label: 'default',
  source: 'repository',
  yaml: '# The default stack.\niii: latest\n\ncontainers:\n  harness:\n    worker: package://harness\n    version: latest\n',
  iii: 'latest',
  template: null,
  containers: [
    {
      name: 'harness',
      worker: 'package://harness',
      version: 'latest',
      commit: null,
    },
  ],
  warnings: [],
  updated_at: null,
}

describe('stack sheet', () => {
  it('opens a new stack on the Form, holding Create back with the reason', () => {
    const html = renderToStaticMarkup(
      <StackSheet
        mode="new"
        stack={null}
        stacks={[]}
        narrow={false}
        bridge={{} as DashboardDataBridge}
        onClose={() => {}}
      />,
    )
    for (const text of [
      'New stack',
      'Start from',
      'An iii-hq/templates project',
      'Reading iii-hq/templates…',
      'iii release',
      'Added when it runs',
      'harness-e2e',
      'containers: {}',
      'Pick a template.',
    ])
      expect(html).toContain(text)
    expect(html).toMatch(/role="tab"[^>]*aria-selected="true"[^>]*>.*Form/)
    expect(html).toMatch(
      /<button[^>]*aria-disabled="true"[^>]*aria-describedby="sb-status"[^>]*>Create stack<\/button>/,
    )
    expect(html).toContain('<span id="sb-status">Pick a template.</span>')
  })

  it('opens a repository stack read-only, with Copy to edit', () => {
    const html = renderToStaticMarkup(
      <StackSheet
        mode="view"
        stack={repository}
        stacks={[repository]}
        narrow={false}
        bridge={{} as DashboardDataBridge}
        onClose={() => {}}
      />,
    )
    expect(html).toContain('Repository · read-only')
    expect(html).toMatch(/<fieldset class="sb-form" disabled="">/)
    expect(html).toContain('Copy to edit')
    expect(html).toContain(
      'Read-only. Copies you make appear under This Console.',
    )
    expect(html).not.toContain('Add worker')
    expect(html).not.toContain('Save stack')
  })
})
