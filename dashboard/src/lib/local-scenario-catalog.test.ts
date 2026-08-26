import { describe, expect, it } from 'vitest'
import { localScenariosFromCatalog } from '@/lib/local-scenario-catalog'

describe('local scenario catalog', () => {
  it('keeps valid local definitions sorted and ignores malformed entries', () => {
    const scenarios = localScenariosFromCatalog({
      local_scenarios: [
        {
          id: 'markdown_zulu',
          title: 'Zulu',
          version: 1,
          source_path: 'local-scenarios/zulu.md',
          source_sha256: 'sha256:zulu',
        },
        { id: 'missing-fields' },
        {
          id: 'markdown_alpha',
          title: 'Alpha',
          version: 2,
          source_path: 'local-scenarios/alpha.md',
          source_sha256: 'sha256:alpha',
        },
      ],
    })

    expect(scenarios.map((scenario) => scenario.id)).toEqual([
      'markdown_alpha',
      'markdown_zulu',
    ])
  })
})
