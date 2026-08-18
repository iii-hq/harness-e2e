import { Moon, Sun } from 'lucide-react'
import { useTheme } from '@/hooks/useTheme'

export function ThemeToggle() {
  const [theme, setTheme] = useTheme()
  const nextTheme = theme === 'dark' ? 'light' : 'dark'
  const label = nextTheme === 'dark' ? 'Dark' : 'Light'
  const Icon = nextTheme === 'dark' ? Moon : Sun

  return (
    <button
      className="inline-flex min-h-9 min-w-9 shrink-0 items-center justify-center gap-2 rounded-lg border border-[var(--ds-color-line-strong)] bg-[var(--ds-color-surface-raised)] px-2.5 text-xs font-semibold text-[var(--ds-color-ink)] transition-colors motion-reduce:transition-none hover:border-[var(--ds-color-ink-muted)] hover:bg-[var(--ds-color-surface-strong)] sm:min-w-[5.125rem] sm:px-3"
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
