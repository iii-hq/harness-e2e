import { StrictMode, useEffect } from 'react'
import { createRoot } from 'react-dom/client'
import { type DashboardRoute, useHashRoute } from '@/hooks/use-hash-route'
import { CoveragePage } from '@/pages/CoveragePage'
import { ExecutionPage } from '@/pages/ExecutionPage'
import { LocalPlanCreatePage, LocalPlanDetailPage } from '@/pages/LocalPlanPage'
import { OverviewPage } from '@/pages/OverviewPage'
import { PlansPage } from '@/pages/PlansPage'
import { TestHistoryPage } from '@/pages/TestHistoryPage'
import { TestsCatalogPage } from '@/pages/TestsCatalogPage'
import { TestsPage } from '@/pages/TestsPage'
import { WorkflowEditorPage } from '@/pages/WorkflowEditorPage'
import './index.css'

const root = document.querySelector<HTMLElement>('#root')
if (!root) throw new Error('missing #root container')

function RoutedPage({ route }: { route: DashboardRoute }) {
  switch (route.page) {
    case 'execution':
      return (
        <ExecutionPage executionId={route.executionId} anchor={route.anchor} />
      )
    case 'compare':
      return <TestsPage initialFrom={route.left} initialTo={route.right} />
    case 'test-history':
      // Reset filters, open detail, and comparison selection when navigating
      // between two independent test histories.
      return <TestHistoryPage key={route.testId} testId={route.testId} />
    case 'plans':
      return <PlansPage />
    case 'plan-create':
      return <LocalPlanCreatePage />
    case 'plan-detail':
      return <LocalPlanDetailPage planId={route.planId} />
    case 'coverage':
      return <CoveragePage />
    case 'workflows':
      return (
        <WorkflowEditorPage
          workflowId={route.workflowId}
          executionId={route.executionId}
        />
      )
    case 'overview':
      if (route.view === 'tests') {
        return <TestsCatalogPage />
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
        'test-history': 'Test metric history',
        plans: 'Local plans',
        'plan-create': 'Create local evidence plan',
        'plan-detail': 'Local evidence plan',
        coverage: 'Harness stack coverage',
        execution: 'Harness E2E execution detail',
        overview: 'Harness E2E executions',
        workflows: 'Workflow editor',
      }[route.page] ?? 'Harness E2E executions'
  }, [route.page])
  return <RoutedPage route={route} />
}

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
