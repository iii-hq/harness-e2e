import { describe, expect, it } from 'vitest'
import {
  definitionTitle,
  shortDefinition,
  UNMATERIALIZED_DEFINITION,
} from '@/lib/definition-digest'

const digest = `sha256:${'ab12cd34'.repeat(8)}`

describe('scenario definition digests', () => {
  it('shortens a digest to its first eight hex characters', () => {
    expect(shortDefinition(digest)).toBe('ab12cd34')
    expect(definitionTitle(digest)).toBe(digest)
  })

  it('names the unmaterialized marker the server sends', () => {
    expect(shortDefinition(UNMATERIALIZED_DEFINITION)).toBe('not materialized')
    expect(definitionTitle(UNMATERIALIZED_DEFINITION)).toContain(
      'never materialized',
    )
  })

  it('reports an absent digest instead of inventing one', () => {
    expect(shortDefinition(undefined)).toBeNull()
    expect(shortDefinition('')).toBeNull()
    expect(definitionTitle(null)).toBeUndefined()
  })
})
