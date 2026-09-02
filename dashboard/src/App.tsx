import { useEffect } from 'react'
import { DashboardShell } from '@/components/DashboardShell'
import { type DashboardRoute, useHashRoute } from '@/hooks/use-hash-route'
import { ScenarioChatProvider } from '@/lib/scenario-chat-context'
import { ExecutionPage } from '@/pages/ExecutionPage'
import { ExecutionsPage } from '@/pages/ExecutionsPage'
import { LocalPlanCreatePage, LocalPlanDetailPage } from '@/pages/LocalPlanPage'
import { OverviewPage } from '@/pages/OverviewPage'
import { PlansPage } from '@/pages/PlansPage'
import { TestHistoryPage } from '@/pages/TestHistoryPage'
import { TestsCatalogPage } from '@/pages/TestsCatalogPage'
import { TestsPage } from '@/pages/TestsPage'

function RoutedPage({ route }: { route: DashboardRoute }) {
  switch (route.page) {
    case 'execution':
      return (
        <ExecutionPage executionId={route.executionId} anchor={route.anchor} />
      )
    case 'compare':
      return <TestsPage initialFrom={route.left} initialTo={route.right} />
    case 'test-history':
      return <TestHistoryPage key={route.testId} testId={route.testId} />
    case 'plans':
      return <PlansPage />
    case 'plan-create':
      return <LocalPlanCreatePage />
    case 'plan-detail':
      return <LocalPlanDetailPage planId={route.planId} />
    case 'overview':
      if (route.view === 'tests') return <TestsCatalogPage />
      if (route.view === 'executions') return <ExecutionsPage />
      return <OverviewPage />
  }
}

export function App({
  embedded = false,
  tabId,
  panelSide,
  theme,
  onRequestClose,
  openChat,
  manageDocumentTitle = true,
}: {
  embedded?: boolean
  tabId?: string
  panelSide?: 'left' | 'right'
  theme?: 'light' | 'dark'
  onRequestClose?: () => void
  openChat?: (sessionId: string) => void
  manageDocumentTitle?: boolean
}) {
  const [route] = useHashRoute()
  useEffect(() => {
    if (!manageDocumentTitle) return
    document.title =
      {
        compare: 'Compare Harness E2E system versions',
        'test-history': 'Test metric history',
        plans: 'Local plans',
        'plan-create': 'Create local evidence plan',
        'plan-detail': 'Local evidence plan',
        execution: 'Harness E2E execution detail',
        overview: 'Harness E2E executions',
      }[route.page] ?? 'Harness E2E executions'
  }, [manageDocumentTitle, route.page])
  return (
    <DashboardShell
      route={route}
      embedded={embedded}
      tabId={tabId}
      panelSide={panelSide}
      theme={theme}
      onRequestClose={onRequestClose}
    >
      <ScenarioChatProvider openChat={openChat}>
        <RoutedPage route={route} />
      </ScenarioChatProvider>
    </DashboardShell>
  )
}
