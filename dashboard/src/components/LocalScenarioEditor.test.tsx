import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  LOCAL_SCENARIO_TEMPLATE,
  LocalScenarioEditor,
} from '@/components/LocalScenarioEditor'
import type { DashboardDataBridge } from '@/lib/dashboard-data-source'

describe('local Markdown scenario editor', () => {
  it('explains local persistence and starts with a compiler-compatible template', () => {
    const html = renderToStaticMarkup(
      <LocalScenarioEditor
        bridge={{} as DashboardDataBridge}
        onClose={() => undefined}
        onCreated={() => undefined}
      />,
    )

    expect(html).toContain('Create a local test')
    expect(html).toContain('outside this repository')
    expect(html).toContain('does not start an execution')
    expect(html).toContain('Import .md')
    expect(html).toContain('Create test')
    expect(html).toContain('markdown_local_scenario')
    expect(html).toContain('100%')
    for (const section of [
      '## Plans',
      '## Version',
      '## Before Test',
      '## Prompt',
      '## Validations',
    ]) {
      expect(LOCAL_SCENARIO_TEMPLATE).toContain(section)
    }
    expect(LOCAL_SCENARIO_TEMPLATE).toContain('- local')
  })
})
