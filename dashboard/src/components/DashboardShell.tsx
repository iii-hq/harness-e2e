import { PageBody, PageHeader, PageMain, PageShell } from '@iii-dev/console-ui'
import {
  FlaskConical,
  LayoutGrid,
  ListChecks,
  PieChart,
  Route,
} from 'lucide-react'
import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useState,
} from 'react'
import { ThemeToggle } from '@/components/ThemeToggle'
import { useContainerNarrow } from '@/hooks/use-container-narrow'
import {
  type DashboardRoute,
  hashForCoverage,
  hashForPlans,
  hashForWorkspace,
  routeRenderIdentity,
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
  /** The open entity (execution label, test id, plan label) for the console title. */
  context?: string
}

export const MAIN_ID = 'harness-e2e-main'

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

const sectionIcons: Record<DashboardSection, ReactNode> = {
  overview: <LayoutGrid size={15} aria-hidden="true" />,
  tests: <FlaskConical size={15} aria-hidden="true" />,
  executions: <ListChecks size={15} aria-hidden="true" />,
  plans: <Route size={15} aria-hidden="true" />,
  coverage: <PieChart size={15} aria-hidden="true" />,
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

export type PageActionsBarProps = {
  actions?: ReactNode
  label?: string
}

// Audit S-04 / S-05 / S-07: a section's actions live in the page, in the
// same bar as the section links, so the console header keeps only context,
// theme and close. The bar wraps in narrow containers instead of hiding.
export function PageActionsBar({ actions, label }: PageActionsBarProps) {
  if (!actions) return null
  return (
    <section
      className="harness-e2e-page-actions flex min-w-0 flex-wrap items-center justify-end gap-2"
      aria-label={label ?? 'Page actions'}
    >
      {actions}
    </section>
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

  // Audit S-07: a route change starts at the top, unless the route names an
  // anchor the page scrolls to itself. Both scrollers are reset because the
  // console scrolls its main pane and the standalone app scrolls the window.
  const routeIdentity = routeRenderIdentity(route)
  const routeAnchor = route.page === 'execution' ? route.anchor : null
  // biome-ignore lint/correctness/useExhaustiveDependencies: the route identity is the trigger, not a value the effect reads
  useEffect(() => {
    if (routeAnchor) return
    window.scrollTo(0, 0)
    document.getElementById(MAIN_ID)?.scrollTo(0, 0)
  }, [routeIdentity, routeAnchor])

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
          description={header.context ?? sectionLabel ?? 'Overview'}
          actions={
            !embedded ? (
              <ThemeToggle
                theme={standaloneTheme}
                onChange={setStandaloneTheme}
              />
            ) : undefined
          }
          onClose={embedded ? onRequestClose : undefined}
        />
        <PageBody side={panelSide}>
          {/* The console owns the hash router, so the skip link moves focus
              instead of changing the route (audit A11Y-06). */}
          <a
            className="skip-link"
            href={`#${MAIN_ID}`}
            onClick={(click) => {
              click.preventDefault()
              document.getElementById(MAIN_ID)?.focus()
            }}
          >
            Skip to content
          </a>
          {/* No overflow on the standalone main: the document scrolls, and an
              overflow: auto here without a fixed height would make every
              sticky element (section nav, sheet footers) stick to nothing. */}
          <PageMain
            id={MAIN_ID}
            tabIndex={-1}
            className="harness-e2e-console-main min-h-0 min-w-0 p-0 outline-none"
          >
            <div
              ref={mainRef}
              className="harness-e2e-dashboard min-h-full min-w-0 bg-panel text-ink"
              data-harness-e2e-dashboard
              data-theme={theme}
            >
              {/* Audit S-04 / A11Y-05: sections are links with aria-current,
                  so the browser, assistive technology and the hash router all
                  agree on what navigation is. Page actions share the bar. */}
              <nav
                className="harness-e2e-navigation sticky top-0 z-10 flex min-w-0 flex-wrap items-center justify-between gap-2 bg-panel px-4 py-1.5"
                data-section={section}
                aria-label="Harness E2E sections"
              >
                <ul className="harness-e2e-navigation-wide m-0 min-w-0 list-none items-center gap-1 overflow-x-auto p-0 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
                  {navigation.map((item) => (
                    <li key={item.value}>
                      <a
                        className="harness-e2e-nav-link"
                        href={hashForSection(item.value)}
                        aria-current={
                          item.value === section ? 'page' : undefined
                        }
                      >
                        {sectionIcons[item.value]}
                        <span>{item.label}</span>
                      </a>
                    </li>
                  ))}
                </ul>
                {/* Visibility of the wide links and the narrow select lives in
                    dashboard-shell.css, keyed on data-narrow: a Tailwind
                    `hidden` here would win over that CSS (Tailwind is
                    imported with `important`; audit S-01). */}
                <div className="harness-e2e-navigation-narrow min-w-0 flex-1">
                  <select
                    className="harness-e2e-nav-select min-h-9 w-full rounded-[6px] border-0 bg-panel-soft px-2.5 font-mono text-[12px] font-medium lowercase leading-none text-ink"
                    value={section}
                    onChange={(event) =>
                      navigate(event.target.value as DashboardSection)
                    }
                    aria-label="Harness E2E section"
                  >
                    {navigation.map((item) => (
                      <option key={item.value} value={item.value}>
                        {item.label}
                      </option>
                    ))}
                    {section === 'coverage' ? (
                      <option value="coverage">Coverage</option>
                    ) : null}
                  </select>
                </div>
                <PageActionsBar
                  actions={header.actions}
                  label={header.actionsLabel}
                />
              </nav>
              <div className="harness-e2e-content min-w-0">{children}</div>
            </div>
          </PageMain>
        </PageBody>
      </PageShell>
    </DashboardChromeContext.Provider>
  )
}
