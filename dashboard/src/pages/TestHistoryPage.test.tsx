import { describe, expect, it } from 'vitest'
import type { TestCatalogRow } from '@/lib/test-catalog'
import {
  historyFacts,
  historyStateFromParams,
  historyStateToParams,
} from '@/pages/TestHistoryPage'
import { history } from '@/test-fixtures/test-history'

const A = `sha256:${'a1'.repeat(32)}`
const C = `sha256:${'c3'.repeat(32)}`

describe('test history page state', () => {
  it('round-trips filters and the A/B ticks through the hash', () => {
    const state = historyStateFromParams(
      new URLSearchParams(`definition=${A}&result=lost&profile=tech&a=x&b=y`),
    )
    expect(state.filters).toMatchObject({
      definition: A,
      result: 'lost',
      profile: 'tech',
      model: '',
    })
    expect(state.selected).toEqual(['x', 'y'])
    expect(historyStateToParams(state.filters, state.selected).toString()).toBe(
      `definition=${encodeURIComponent(A)}&profile=tech&result=lost&a=x&b=y`,
    )
  })

  it('shows every definition unless the link names one', () => {
    const state = historyStateFromParams(new URLSearchParams())
    expect(state.filters.definition).toBe('all')
    expect(historyStateToParams(state.filters, []).toString()).toBe('')
  })

  it('names the current definition, the runs and the contract in chips', () => {
    const row = {
      test_id: 'form_flow_build',
      current_version: C,
      spec: {
        prompt: '',
        criteria: [
          { id: 'one', weight: 60 },
          { id: 'two', weight: 40 },
        ],
      },
    } as unknown as TestCatalogRow
    expect(historyFacts(history({ current_version: C }), row)).toEqual([
      { label: 'Definition', value: 'c3c3c3c3 · current', full: C },
      { label: 'Runs', value: '6 in 3 definitions' },
      { label: 'Scored on', value: '2 criteria · 100 points' },
    ])
    expect(historyFacts(null, null)).toEqual([])
  })
})
