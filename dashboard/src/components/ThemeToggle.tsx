import { Moon, Sun } from 'lucide-react'
import { useTheme } from '@/hooks/useTheme'

export function ThemeToggle() {
  const [theme, setTheme] = useTheme()
  const nextTheme = theme === 'dark' ? 'light' : 'dark'
  const label = nextTheme === 'dark' ? 'Dark' : 'Light'
  const Icon = nextTheme === 'dark' ? Moon : Sun

  return (
    <button
      className="button theme-toggle"
      type="button"
      data-theme-toggle
      aria-label={`Use ${nextTheme} theme`}
      title={`Use ${nextTheme} theme`}
      onClick={() => setTheme(nextTheme)}
    >
      <Icon size={15} strokeWidth={1.8} data-theme-icon aria-hidden="true" />
      <span>{label}</span>
    </button>
  )
}
