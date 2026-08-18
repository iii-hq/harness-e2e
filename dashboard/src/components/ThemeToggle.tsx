import { Moon, Sun } from 'lucide-react'
import { useTheme } from '@/hooks/useTheme'
import type { Theme } from '@/lib/theme'

export function ThemeToggle({
  theme: controlledTheme,
  onChange,
}: {
  theme?: Theme
  onChange?: (next: Theme) => void
} = {}) {
  const [localTheme, setLocalTheme] = useTheme({
    syncDocument: controlledTheme === undefined,
  })
  const theme = controlledTheme ?? localTheme
  const setTheme = onChange ?? setLocalTheme
  const nextTheme = theme === 'dark' ? 'light' : 'dark'
  const label = nextTheme === 'dark' ? 'Dark' : 'Light'
  const Icon = nextTheme === 'dark' ? Moon : Sun

  return (
    <button
      className="harness-e2e-header-action harness-e2e-theme-toggle"
      type="button"
      data-theme-toggle
      aria-label={`Use ${nextTheme} theme`}
      title={`Use ${nextTheme} theme`}
      onClick={() => setTheme(nextTheme)}
    >
      <Icon size={15} strokeWidth={1.8} data-theme-icon aria-hidden="true" />
      <span className="max-sm:sr-only">{label}</span>
    </button>
  )
}
