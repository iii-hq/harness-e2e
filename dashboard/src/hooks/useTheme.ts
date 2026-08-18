import { useCallback, useEffect, useState } from 'react'
import {
  applyDocumentTheme,
  readDocumentTheme,
  storedTheme,
  THEME_STORAGE_KEY,
  type Theme,
} from '@/lib/theme'

export function useTheme({
  syncDocument = true,
}: {
  syncDocument?: boolean
} = {}): [Theme, (next: Theme) => void] {
  const [theme, setThemeState] = useState<Theme>(() =>
    readDocumentTheme(document.documentElement),
  )

  useEffect(() => {
    if (!syncDocument) return
    applyDocumentTheme(document.documentElement, theme)
    try {
      localStorage.setItem(THEME_STORAGE_KEY, theme)
    } catch {
      // Storage can be unavailable in private or embedded contexts.
    }
  }, [syncDocument, theme])

  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const syncSystemTheme = (event: MediaQueryListEvent) => {
      if (!storedTheme(localStorage)) {
        setThemeState(event.matches ? 'dark' : 'light')
      }
    }
    media.addEventListener('change', syncSystemTheme)
    return () => media.removeEventListener('change', syncSystemTheme)
  }, [])

  const setTheme = useCallback((next: Theme) => setThemeState(next), [])
  return [theme, setTheme]
}
