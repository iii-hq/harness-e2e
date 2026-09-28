import { DashboardShell } from '@/components/DashboardShell'
import { type DashboardRoute, useHashRoute } from '@/hooks/use-hash-route'
import { ExecutionComparePage } from '@/pages/ExecutionComparePage'
import { ExecutionPage } from '@/pages/ExecutionPage'
import { ExecutionsPage } from '@/pages/ExecutionsPage'
import { RunComparePage } from '@/pages/RunComparePage'
import { StacksPage } from '@/pages/StacksPage'
import { SuitesPage } from '@/pages/SuitesPage'
import { TestHistoryPage } from '@/pages/TestHistoryPage'
import { TestsCatalogPage } from '@/pages/TestsCatalogPage'
import { TestsPage } from '@/pages/TestsPage'
import { TrendsPage } from '@/pages/TrendsPage'

function RoutedPage({ route }: { route: DashboardRoute }) {
  switch (route.page) {
    case 'execution':
      return (
        <ExecutionPage
          executionId={route.executionId}
          anchor={route.anchor}
          runId={route.runId}
          view={route.view ?? null}
        />
      )
    case 'compare':
      return (
        <ExecutionComparePage
          key={`${route.left}:${route.right}`}
          left={route.left}
          right={route.right}
        />
      )
    case 'versions':
      return <TestsPage initialFrom={route.left} initialTo={route.right} />
    case 'test-history':
      return route.compare ? (
        <RunComparePage key={route.testId} testId={route.testId} />
      ) : (
        <TestHistoryPage key={route.testId} testId={route.testId} />
      )
    case 'suites':
      return <SuitesPage />
    case 'stacks':
      return <StacksPage />
    case 'trends':
      return <TrendsPage request={route.request} period={route.period} />
    case 'workspace':
      if (route.view === 'tests') return <TestsCatalogPage />
      return <ExecutionsPage />
  }
}

export function App({
  tabId,
  panelSide,
  theme,
  onRequestClose,
}: {
  tabId?: string
  panelSide?: 'left' | 'right'
  theme?: 'light' | 'dark'
  onRequestClose?: () => void
}) {
  const [route] = useHashRoute()
  return (
    <DashboardShell
      route={route}
      tabId={tabId}
      panelSide={panelSide}
      theme={theme}
      onRequestClose={onRequestClose}
    >
      <RoutedPage route={route} />
    </DashboardShell>
  )
}
