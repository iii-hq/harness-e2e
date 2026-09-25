import { PageBody, PageHeader, PageMain, PageShell } from '@iii-dev/console-ui'
import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from 'react'
import { useContainerNarrow } from '@/hooks/use-container-narrow'
import {
  type DashboardRoute,
  hashForStacks,
  hashForSuites,
  hashForWorkspace,
  routeRenderIdentity,
  type WorkspaceView,
} from '@/hooks/use-hash-route'
import '@/design-system/styles.css'
import './dashboard-shell.css'

export type DashboardSection = 'tests' | 'executions' | 'suites' | 'stacks'

export type DashboardHeaderState = {
  key: string
  actions?: ReactNode
  actionsLabel?: string
  /** The open entity (execution label, test id) for the console title. */
  context?: string
}

export const MAIN_ID = 'harness-e2e-main'

export type DashboardChromeContextValue = {
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
  if (route.page === 'suites') return 'suites'
  if (route.page === 'stacks') return 'stacks'
  if (
    route.page === 'execution' ||
    route.page === 'compare' ||
    (route.page === 'workspace' && route.view === 'executions')
  ) {
    return 'executions'
  }
  if (
    route.page === 'versions' ||
    route.page === 'test-history' ||
    (route.page === 'workspace' && route.view === 'tests')
  ) {
    return 'tests'
  }
  return 'executions'
}

function hashForSection(section: DashboardSection): string {
  if (section === 'suites') return hashForSuites()
  if (section === 'stacks') return hashForStacks()
  return hashForWorkspace(section as WorkspaceView)
}

const navigation: Array<{ value: DashboardSection; label: string }> = [
  { value: 'tests', label: 'Tests' },
  { value: 'executions', label: 'Executions' },
  { value: 'suites', label: 'Suites' },
  { value: 'stacks', label: 'Stacks' },
]

const sectionLabels: Record<DashboardSection, string> = {
  tests: 'Tests',
  executions: 'Executions',
  suites: 'Suites',
  stacks: 'Stacks',
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

/** The header a page asks for. The key says which page and labels; the
 *  actions are compared too, since a page re-renders them as its state
 *  changes (a button that enables, a label that flips) under the same key,
 *  and the shell should not rely on the page clearing its header first. */
export function nextHeader(
  current: DashboardHeaderState,
  next: DashboardHeaderState,
): DashboardHeaderState {
  return current.key === next.key && current.actions === next.actions
    ? current
    : next
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
      className="harness-e2e-page-actions"
      aria-label={label ?? 'Page actions'}
    >
      {actions}
    </section>
  )
}

export type DashboardShellProps = {
  children: ReactNode
  route: DashboardRoute
  tabId?: string
  panelSide?: 'left' | 'right'
  theme?: 'light' | 'dark'
  onRequestClose?: () => void
}

export function DashboardShell({
  children,
  route,
  tabId = 'harness-e2e',
  panelSide,
  theme,
  onRequestClose,
}: DashboardShellProps) {
  const [mainRef, narrow] = useContainerNarrow(720)
  const [header, setHeaderState] = useState<DashboardHeaderState>({ key: '' })
  const setHeader = useCallback((next: DashboardHeaderState) => {
    setHeaderState((current) => nextHeader(current, next))
  }, [])
  const clearHeader = useCallback(() => setHeaderState({ key: '' }), [])
  const section = sectionForRoute(route)
  const sectionLabel = sectionLabels[section]
  // Stable across header updates, so a page that reads the chrome does not
  // re-render (and re-send its actions) because its own actions changed.
  const contextValue = useMemo<DashboardChromeContextValue>(
    () => ({ tabId, panelSide, narrow, setHeader, clearHeader }),
    [tabId, panelSide, narrow, setHeader, clearHeader],
  )

  const navigate = (next: DashboardSection) => {
    window.location.hash = hashForSection(next)
  }

  const routeIdentity = routeRenderIdentity(route)
  const routeAnchor = route.page === 'execution' ? route.anchor : null
  // biome-ignore lint/correctness/useExhaustiveDependencies: the route identity is the trigger, not a value the effect reads
  useEffect(() => {
    if (routeAnchor) return
    document.getElementById(MAIN_ID)?.scrollTo(0, 0)
  }, [routeIdentity, routeAnchor])

  return (
    <DashboardChromeContext.Provider value={contextValue}>
      <PageShell
        className="harness-e2e-shell"
        data-theme={theme}
        data-narrow={narrow ? 'true' : 'false'}
      >
        <PageHeader
          icon={<HarnessE2eIcon />}
          title="harness e2e"
          description={header.context ?? sectionLabel}
          onClose={onRequestClose}
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
                  agree on what navigation is. Page actions share the bar.
                  The bar's look lives in dashboard-shell.css: the extension's
                  layers outrank Tailwind utilities inside the Console. */}
              <nav
                className="harness-e2e-navigation"
                data-section={section}
                aria-label="Harness E2E sections"
              >
                <ul className="harness-e2e-navigation-wide">
                  {navigation.map((item) => (
                    <li key={item.value}>
                      <a
                        className="harness-e2e-nav-link"
                        href={hashForSection(item.value)}
                        aria-current={
                          item.value === section ? 'page' : undefined
                        }
                      >
                        {item.label}
                      </a>
                    </li>
                  ))}
                </ul>
                {/* Visibility of the wide links and the narrow select lives in
                    dashboard-shell.css, keyed on data-narrow: a Tailwind
                    `hidden` here would win over that CSS (audit S-01). */}
                <div className="harness-e2e-navigation-narrow">
                  <select
                    className="harness-e2e-nav-select"
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
