import { type ReactNode, useEffect } from 'react'
import { useDashboardChrome } from '@/components/DashboardShell'

export type AppHeaderSection =
  | 'overview'
  | 'tests'
  | 'executions'
  | 'plans'
  | 'coverage'

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
    'harness-e2e-header-action inline-flex min-h-9 shrink-0 items-center justify-center rounded-[6px] border border-line bg-panel-raised px-3 text-xs font-semibold leading-none text-ink no-underline transition-colors duration-150 ease-in-out',
    primary
      ? 'harness-e2e-header-action-primary'
      : 'harness-e2e-header-action-secondary',
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
}: AppHeaderProps) {
  const chrome = useDashboardChrome()
  const hasActions = Boolean(actions)
  const setHeader = chrome?.setHeader
  const clearHeader = chrome?.clearHeader

  useEffect(() => {
    if (!setHeader || !clearHeader) return
    setHeader({
      key: `${active}:${actionsLabel}:${hasActions}`,
      actions,
      actionsLabel,
    })
    return clearHeader
  }, [actions, active, actionsLabel, clearHeader, hasActions, setHeader])

  // The Console owns the page chrome. This bridge keeps route-level actions
  // available to the single PageHeader without rendering a second header.
  return null
}
