export type Theme = 'light' | 'dark'

export const THEME_STORAGE_KEY = 'harness-e2e-theme'

export function preferredTheme(prefersDark: boolean): Theme {
  return prefersDark ? 'dark' : 'light'
}

export function storedTheme(storage: Pick<Storage, 'getItem'>): Theme | null {
  try {
    const value = storage.getItem(THEME_STORAGE_KEY)
    return value === 'light' || value === 'dark' ? value : null
  } catch {
    return null
  }
}

export function readDocumentTheme(root: HTMLElement): Theme {
  return root.dataset.theme === 'dark' ? 'dark' : 'light'
}

export function applyDocumentTheme(root: HTMLElement, theme: Theme) {
  root.dataset.theme = theme
  root.style.colorScheme = theme
}
