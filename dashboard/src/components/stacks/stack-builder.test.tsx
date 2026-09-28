import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { DashboardDataBridge } from '@/lib/dashboard-data-source'
import { StackBuilder } from './StackBuilder'

describe('stack builder', () => {
  it('opens on the templates, holding Create back with the reason', () => {
    const html = renderToStaticMarkup(
      <StackBuilder
        stacks={[]}
        narrow={false}
        bridge={{} as DashboardDataBridge}
        onCreated={() => {}}
        onEditYaml={() => {}}
        onClose={() => {}}
      />,
    )
    for (const text of [
      'New stack',
      'Start from',
      'An iii-hq/templates project',
      'Reading iii-hq/templates…',
      'Added when it runs',
      'harness-e2e',
      'containers: {}',
      'Pick a template.',
    ])
      expect(html).toContain(text)
    expect(html).toMatch(
      /<button[^>]*aria-disabled="true"[^>]*aria-describedby="sb-status"[^>]*>Create stack<\/button>/,
    )
    expect(html).toContain('<span id="sb-status">Pick a template.</span>')
  })
})
