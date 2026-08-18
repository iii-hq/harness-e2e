import type { ReactNode } from 'react'
import { ThemeToggle } from '@/components/ThemeToggle'
import {
  hashForCoverage,
  hashForPlans,
  hashForWorkspace,
} from '@/hooks/use-hash-route'
import { isEmbeddedDashboard } from '@/lib/dashboard-runtime'
import '@/design-system/styles.css'

export type AppHeaderSection =
  | 'overview'
  | 'tests'
  | 'executions'
  | 'plans'
  | 'coverage'

const navigation: Array<{
  id: AppHeaderSection
  label: string
  href: () => string
}> = [
  { id: 'overview', label: 'Overview', href: () => hashForWorkspace() },
  { id: 'tests', label: 'Tests', href: () => hashForWorkspace('tests') },
  {
    id: 'executions',
    label: 'Executions',
    href: () => hashForWorkspace('executions'),
  },
  { id: 'plans', label: 'Plans', href: () => hashForPlans() },
  { id: 'coverage', label: 'Coverage', href: () => hashForCoverage() },
]

function classes(...values: Array<string | false | null | undefined>) {
  return values.filter(Boolean).join(' ')
}

export function appHeaderActionClassName({
  primary = false,
  className,
}: {
  primary?: boolean
  className?: string
} = {}) {
  return classes(
    'inline-flex min-h-9 shrink-0 items-center justify-center rounded-lg border px-3 text-xs font-semibold no-underline transition-colors motion-reduce:transition-none',
    primary
      ? 'border-[var(--ds-color-brand)] bg-[var(--ds-color-brand)] text-[var(--ds-color-brand-ink)] hover:border-[var(--ds-color-focus)] hover:bg-[var(--ds-color-focus)]'
      : 'border-[var(--ds-color-line-strong)] bg-[var(--ds-color-surface-raised)] text-[var(--ds-color-ink)] hover:border-[var(--ds-color-ink-muted)] hover:bg-[var(--ds-color-surface-strong)]',
    className,
  )
}

export type AppHeaderProps = {
  active: AppHeaderSection
  actions?: ReactNode
  actionsLabel?: string
  showThemeToggle?: boolean
}

export function AppHeader({
  active,
  actions,
  actionsLabel = 'Page actions',
  showThemeToggle = true,
}: AppHeaderProps) {
  return (
    <header
      className="ds-root sticky top-0 z-50 mx-auto grid min-h-16 w-[calc(100%_-_1.5rem)] max-w-[1440px] grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 border-b border-[var(--ds-color-line)] bg-[color:rgba(var(--ds-color-canvas-rgb),0.92)] backdrop-blur-xl md:w-[calc(100%_-_3rem)] lg:grid-cols-[auto_minmax(0,1fr)_auto]"
      data-app-header
    >
      <a
        className="inline-flex min-h-11 min-w-0 items-center gap-2 self-center text-[var(--ds-color-ink)] no-underline"
        href={hashForWorkspace()}
        aria-label="Harness E2E dashboard"
      >
        <strong className="shrink-0 text-sm font-semibold tracking-[-0.045em]">
          iii
        </strong>
        <span className="truncate font-mono text-[0.65rem] text-[var(--ds-color-ink-muted)] max-[430px]:hidden">
          Harness benchmarks
        </span>
      </a>

      <nav
        className="order-3 col-span-2 flex min-w-0 snap-x overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden lg:order-none lg:col-span-1 lg:h-16 lg:justify-self-center lg:overflow-visible"
        aria-label="Dashboard"
      >
        {navigation.map((item) => {
          const current = item.id === active
          return (
            <a
              className="inline-flex min-h-11 shrink-0 snap-start items-center justify-center border-b-2 border-transparent px-3 text-xs font-medium text-[var(--ds-color-ink-soft)] no-underline transition-colors motion-reduce:transition-none hover:bg-[var(--ds-color-surface-raised)] hover:text-[var(--ds-color-ink)] aria-[current=page]:border-[var(--ds-color-brand)] aria-[current=page]:text-[var(--ds-color-ink)] lg:min-h-16 lg:px-4 lg:hover:bg-transparent"
              href={item.href()}
              aria-current={current ? 'page' : undefined}
              key={item.id}
            >
              {item.label}
            </a>
          )
        })}
      </nav>

      <div className="flex min-w-0 items-center justify-self-end gap-2">
        {actions ? (
          <nav
            className="flex min-w-0 items-center justify-end gap-2"
            aria-label={actionsLabel}
          >
            {actions}
          </nav>
        ) : null}
        {showThemeToggle && !isEmbeddedDashboard() ? <ThemeToggle /> : null}
      </div>
    </header>
  )
}
