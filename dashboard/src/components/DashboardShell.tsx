import {
  PageBody,
  PageHeader,
  PageMain,
  PageShell,
  Select,
  Tabs,
  TabsList,
  TabsTrigger,
} from '@iii-dev/console-ui'
import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useState,
} from 'react'
import { ThemeToggle } from '@/components/ThemeToggle'
import { useContainerNarrow } from '@/hooks/use-container-narrow'
import {
  type DashboardRoute,
  hashForCoverage,
  hashForPlans,
  hashForWorkspace,
  type WorkspaceView,
} from '@/hooks/use-hash-route'
import { useTheme } from '@/hooks/useTheme'
import './dashboard-shell.css'

export type DashboardSection =
  | 'overview'
  | 'tests'
  | 'executions'
  | 'plans'
  | 'coverage'

export type DashboardHeaderState = {
  key: string
  actions?: ReactNode
  actionsLabel?: string
}

export type DashboardChromeContextValue = {
  embedded: boolean
  tabId: string
  panelSide?: 'left' | 'right'
  narrow: boolean
  setHeader: (next: DashboardHeaderState) => void
  clearHeader: () => void
}

export const DashboardChromeContext =
  createContext<DashboardChromeContextValue | null>(null)

export function useDashboardChrome() {
  const value = useContext(DashboardChromeContext)
  return value
}

export function sectionForRoute(route: DashboardRoute): DashboardSection {
  if (route.page === 'coverage') return 'coverage'
  if (
    route.page === 'plans' ||
    route.page === 'plan-create' ||
    route.page === 'plan-detail'
  ) {
    return 'plans'
  }
  if (
    route.page === 'execution' ||
    (route.page === 'overview' && route.view === 'executions')
  ) {
    return 'executions'
  }
  if (
    route.page === 'compare' ||
    route.page === 'test-history' ||
    (route.page === 'overview' && route.view === 'tests')
  ) {
    return 'tests'
  }
  return 'overview'
}

function hashForSection(section: DashboardSection): string {
  if (section === 'plans') return hashForPlans()
  if (section === 'coverage') return hashForCoverage()
  return hashForWorkspace(section as WorkspaceView)
}

const navigation: Array<{ value: DashboardSection; label: string }> = [
  { value: 'overview', label: 'Overview' },
  { value: 'tests', label: 'Tests' },
  { value: 'executions', label: 'Executions' },
  { value: 'plans', label: 'Plans' },
  { value: 'coverage', label: 'Coverage' },
]

function HarnessE2eIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <path
        d="M2.5 4.25h11M2.5 8h7.25M2.5 11.75h11"
        stroke="currentColor"
        strokeWidth="1.35"
        strokeLinecap="round"
      />
      <circle cx="11.75" cy="8" r="1.5" fill="currentColor" />
    </svg>
  )
}

export type DashboardShellProps = {
  children: ReactNode
  route: DashboardRoute
  embedded: boolean
  tabId?: string
  panelSide?: 'left' | 'right'
  theme?: 'light' | 'dark'
  onRequestClose?: () => void
}

export function DashboardShell({
  children,
  route,
  embedded,
  tabId = 'standalone',
  panelSide,
  theme: embeddedTheme,
  onRequestClose,
}: DashboardShellProps) {
  const [standaloneTheme, setStandaloneTheme] = useTheme({
    syncDocument: !embedded,
  })
  const theme = embeddedTheme ?? standaloneTheme
  const [mainRef, narrow] = useContainerNarrow(720)
  const [header, setHeaderState] = useState<DashboardHeaderState>({ key: '' })
  const setHeader = useCallback((next: DashboardHeaderState) => {
    setHeaderState((current) => (current.key === next.key ? current : next))
  }, [])
  const clearHeader = useCallback(() => setHeaderState({ key: '' }), [])
  const section = sectionForRoute(route)
  const sectionLabel = navigation.find((item) => item.value === section)?.label
  const contextValue: DashboardChromeContextValue = {
    embedded,
    tabId,
    panelSide,
    narrow,
    setHeader,
    clearHeader,
  }

  const navigate = (next: DashboardSection) => {
    window.location.hash = hashForSection(next)
  }

  return (
    <DashboardChromeContext.Provider value={contextValue}>
      <PageShell
        className="harness-e2e-shell"
        data-mode={embedded ? 'embedded' : 'standalone'}
        data-theme={theme}
        data-narrow={narrow ? 'true' : 'false'}
      >
        <PageHeader
          icon={<HarnessE2eIcon />}
          title="harness e2e"
          description={`${sectionLabel ?? 'Overview'} · evidence, plans and live evaluation control`}
          actions={
            <div className="harness-e2e-header-actions">
              {!embedded ? (
                <ThemeToggle
                  theme={standaloneTheme}
                  onChange={setStandaloneTheme}
                />
              ) : null}
              {header.actions}
            </div>
          }
          onClose={embedded ? onRequestClose : undefined}
        />
        <PageBody side={panelSide}>
          <PageMain className="harness-e2e-console-main">
            <div
              ref={mainRef}
              className="harness-e2e-dashboard"
              data-harness-e2e-dashboard
              data-theme={theme}
            >
              <div className="harness-e2e-navigation" data-section={section}>
                <div className="harness-e2e-navigation-wide">
                  <Tabs
                    value={section}
                    onValueChange={(next) => navigate(next as DashboardSection)}
                    aria-label="Harness E2E sections"
                  >
                    <TabsList>
                      {navigation.map((item) => (
                        <TabsTrigger key={item.value} value={item.value}>
                          {item.label}
                        </TabsTrigger>
                      ))}
                    </TabsList>
                  </Tabs>
                </div>
                <div className="harness-e2e-navigation-narrow">
                  <Select
                    value={section}
                    options={navigation}
                    onChange={(next) => navigate(next as DashboardSection)}
                    aria-label="Harness E2E section"
                  />
                </div>
              </div>
              <div className="harness-e2e-content">{children}</div>
            </div>
          </PageMain>
        </PageBody>
      </PageShell>
    </DashboardChromeContext.Provider>
  )
}
