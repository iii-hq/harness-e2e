import { StrictMode, useEffect } from 'react'
import { createRoot } from 'react-dom/client'
import {
  type DashboardRoute,
  dashboardRoutes,
  useHashRoute,
} from '@/hooks/use-hash-route'
import { CoveragePage } from '@/pages/CoveragePage'
import { ExecutionPage } from '@/pages/ExecutionPage'
import { OverviewPage } from '@/pages/OverviewPage'
import { TestsPage } from '@/pages/TestsPage'
import './index.css'

const root = document.querySelector<HTMLElement>('#root')
if (!root) throw new Error('missing #root container')

window.HarnessDashboardRoutes = dashboardRoutes

function RoutedPage({ route }: { route: DashboardRoute }) {
  switch (route.page) {
    case 'execution':
      return <ExecutionPage executionId={route.executionId} />
    case 'compare':
      return <TestsPage initialFrom={route.left} initialTo={route.right} />
    case 'coverage':
      return <CoveragePage />
    case 'overview':
      if (route.view === 'tests') {
        return <TestsPage />
      }
      return <OverviewPage activeView={route.view} />
  }
}

function App() {
  const [route] = useHashRoute()
  useEffect(() => {
    document.title =
      {
        compare: 'Compare Harness E2E system versions',
        coverage: 'Harness stack coverage',
        execution: 'Harness E2E execution detail',
        overview: 'Harness E2E executions',
      }[route.page] ?? 'Harness E2E executions'
  }, [route.page])
  return <RoutedPage route={route} />
}

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
