import { type ReactNode, useEffect } from 'react'
import { useDashboardChrome } from '@/components/DashboardShell'

export type DashboardSection = 'tests' | 'executions' | 'plans'

function classes(...values: Array<string | false | null | undefined>) {
  return values.filter(Boolean).join(' ')
}

export function dashboardHeaderActionClassName({
  primary = false,
  className,
}: {
  primary?: boolean
  className?: string
} = {}) {
  return classes(
    'harness-e2e-header-action inline-flex min-h-7 shrink-0 items-center justify-center gap-1.5 rounded-[6px] border-0 bg-transparent px-2.5 font-mono text-[12px] font-medium lowercase leading-none text-ink-soft no-underline transition-colors duration-150 ease-in-out',
    primary
      ? 'harness-e2e-header-action-primary'
      : 'harness-e2e-header-action-secondary',
    className,
  )
}

export type DashboardPageActionsProps = {
  active: DashboardSection
  actions?: ReactNode
  actionsLabel?: string
  /** The open entity, shown as the console title's second line. */
  context?: string
}

export function DashboardPageActions({
  active,
  actions,
  actionsLabel = 'Page actions',
  context,
}: DashboardPageActionsProps) {
  const chrome = useDashboardChrome()
  const hasActions = Boolean(actions)
  const setHeader = chrome?.setHeader
  const clearHeader = chrome?.clearHeader

  useEffect(() => {
    if (!setHeader || !clearHeader) return
    setHeader({
      key: `${active}:${actionsLabel}:${hasActions}:${context ?? ''}`,
      actions,
      actionsLabel,
      context,
    })
    return clearHeader
  }, [
    actions,
    active,
    actionsLabel,
    clearHeader,
    context,
    hasActions,
    setHeader,
  ])

  // The shell owns the chrome: the actions render in the section bar inside
  // the page and the context goes to the console title. Nothing renders here.
  return null
}
