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

function TabGlyph({ children }: { children: ReactNode }) {
  return (
    <svg
      viewBox="0 0 16 16"
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.35"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {children}
    </svg>
  )
}

const sectionIcons: Record<DashboardSection, ReactNode> = {
  overview: (
    <TabGlyph>
      <rect x="2.5" y="2.5" width="4.8" height="7" rx="1" />
      <rect x="2.5" y="11.5" width="4.8" height="2" rx="1" />
      <rect x="8.7" y="2.5" width="4.8" height="2" rx="1" />
      <rect x="8.7" y="6.5" width="4.8" height="7" rx="1" />
    </TabGlyph>
  ),
  tests: (
    <TabGlyph>
      <path d="M6.4 2.2h3.2M8 2.2v3.4l3.9 6.3a1.4 1.4 0 0 1-1.19 2.1H5.29a1.4 1.4 0 0 1-1.19-2.1L8 5.6" />
      <path d="M5.7 10.2h4.6" />
    </TabGlyph>
  ),
  executions: (
    <TabGlyph>
      <circle cx="8" cy="8.2" r="5.3" />
      <path d="M8 5.4v2.8l1.9 1.4" />
    </TabGlyph>
  ),
  plans: (
    <TabGlyph>
      <circle cx="4.6" cy="4.6" r="1.9" />
      <circle cx="11.4" cy="11.4" r="1.9" />
      <path d="M4.6 6.5v2.4a2.3 2.3 0 0 0 2.3 2.3h1.8" />
      <path d="M11.4 9.5V7.1a2.3 2.3 0 0 0-2.3-2.3H7.3" />
    </TabGlyph>
  ),
  coverage: (
    <TabGlyph>
      <circle cx="8" cy="8" r="5.3" />
      <path d="M8 2.7v5.3l3.7 3.7" />
    </TabGlyph>
  ),
}

// Coverage stays reachable by URL but is deliberately absent from the menu.
const navigation: Array<{ value: DashboardSection; label: string }> = [
  { value: 'overview', label: 'Overview' },
  { value: 'tests', label: 'Tests' },
  { value: 'executions', label: 'Executions' },
  { value: 'plans', label: 'Plans' },
]

const sectionLabels: Record<DashboardSection, string> = {
  overview: 'Overview',
  tests: 'Tests',
  executions: 'Executions',
  plans: 'Plans',
  coverage: 'Coverage',
}

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
  const sectionLabel = sectionLabels[section]
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
            <div className="harness-e2e-header-actions flex min-w-0 items-center justify-end gap-2">
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
          <PageMain className="harness-e2e-console-main min-h-0 min-w-0 overflow-auto p-0">
            <div
              ref={mainRef}
              className="harness-e2e-dashboard min-h-full min-w-0 overflow-x-hidden bg-panel text-ink"
              data-harness-e2e-dashboard
              data-theme={theme}
            >
              <div
                className="harness-e2e-navigation sticky top-0 z-10 min-w-0 border-b border-line bg-panel"
                data-section={section}
              >
                <div className="harness-e2e-navigation-wide min-w-0 px-4">
                  <Tabs
                    value={section}
                    onValueChange={(next) => navigate(next as DashboardSection)}
                    aria-label="Harness E2E sections"
                  >
                    <TabsList className="flex items-center gap-1 overflow-x-auto py-1.5 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
                      {navigation.map((item) => (
                        // Embedded: no className override, so the Console
                        // host's canonical content-navigation tab (line
                        // variant + semantic icon) fully governs the look.
                        // Standalone: the local stub is unstyled, so it gets
                        // the pill vocabulary explicitly.
                        <TabsTrigger
                          key={item.value}
                          value={item.value}
                          icon={sectionIcons[item.value]}
                          className={
                            embedded
                              ? undefined
                              : 'whitespace-nowrap rounded-[6px] px-2.5 py-1.5 text-[13px] font-medium leading-none text-ink-soft transition-colors hover:bg-panel-soft hover:text-ink aria-[selected=true]:bg-[var(--surface-selected)] aria-[selected=true]:font-semibold aria-[selected=true]:text-ink'
                          }
                        >
                          {item.label}
                        </TabsTrigger>
                      ))}
                    </TabsList>
                  </Tabs>
                </div>
                <div className="harness-e2e-navigation-narrow hidden min-w-0 px-4 py-2">
                  <Select
                    value={section}
                    options={navigation}
                    onChange={(next) => navigate(next as DashboardSection)}
                    className="min-h-9 w-full rounded-[6px] border-0 bg-panel-soft px-2.5 font-mono text-[12px] font-medium lowercase leading-none text-ink"
                    aria-label="Harness E2E section"
                  />
                </div>
              </div>
              <div className="harness-e2e-content min-w-0">{children}</div>
            </div>
          </PageMain>
        </PageBody>
      </PageShell>
    </DashboardChromeContext.Provider>
  )
}
