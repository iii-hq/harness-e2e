import { describe, expect, it } from 'vitest'
import {
  applyDocumentTheme,
  preferredTheme,
  readDocumentTheme,
  storedTheme,
  THEME_STORAGE_KEY,
} from './theme'

describe('theme', () => {
  it('uses a valid stored theme and rejects unknown values', () => {
    expect(storedTheme({ getItem: () => 'dark' })).toBe('dark')
    expect(storedTheme({ getItem: () => 'sepia' })).toBeNull()
    expect(storedTheme({ getItem: () => null })).toBeNull()
  })

  it('falls back to the operating-system preference', () => {
    expect(preferredTheme(true)).toBe('dark')
    expect(preferredTheme(false)).toBe('light')
  })

  it('applies the theme to the document root', () => {
    const root = { dataset: {}, style: {} } as HTMLElement
    applyDocumentTheme(root, 'dark')
    expect(readDocumentTheme(root)).toBe('dark')
    expect(root.style.colorScheme).toBe('dark')
    expect(THEME_STORAGE_KEY).toBe('harness-e2e-theme')
  })
})
