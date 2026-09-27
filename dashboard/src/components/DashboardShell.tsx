import { PageBody, PageHeader, PageMain, PageShell } from '@iii-dev/console-ui'
import { FlaskConical } from 'lucide-react'
import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from 'react'
import {
  type HeaderAction,
  HeaderActions,
  PinnedPrimary,
} from '@/components/shell/HeaderActions'
import { SectionNav } from '@/components/shell/SectionNav'
import { useContainerNarrow } from '@/hooks/use-container-narrow'
import {
  type DashboardRoute,
  hashForStacks,
  hashForSuites,
  hashForWorkspace,
  routeRenderIdentity,
  type WorkspaceView,
} from '@/hooks/use-hash-route'
import { useViewportPhone } from '@/hooks/use-viewport-phone'
import '@/design-system/styles.css'
import './dashboard-shell.css'

export type DashboardSection = 'tests' | 'executions' | 'suites' | 'stacks'

export type DashboardHeaderState = {
  key: string
  /** The section's actions; the shell places them by pane width. */
  actions?: HeaderAction[]
  actionsLabel?: string
  /** The open entity (execution label, test id) for the console title. */
  context?: string
}

export const MAIN_ID = 'harness-e2e-main'

export type DashboardChromeContextValue = {
  tabId: string
  panelSide?: 'left' | 'right'
  narrow: boolean
  /** Viewport under 640 px. */
  phone: boolean
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

/** Pages about one record: capped width, back button, one title. */
function detailPage(route: DashboardRoute) {
  return (
    route.page === 'execution' ||
    route.page === 'compare' ||
    route.page === 'test-history' ||
    route.page === 'versions'
  )
}

const sectionLinks = navigation.map((item) => ({
  ...item,
  href: hashForSection(item.value),
}))

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
  const phone = useViewportPhone()
  const [header, setHeaderState] = useState<DashboardHeaderState>({ key: '' })
  const setHeader = useCallback((next: DashboardHeaderState) => {
    setHeaderState((current) => nextHeader(current, next))
  }, [])
  const clearHeader = useCallback(() => setHeaderState({ key: '' }), [])
  const section = sectionForRoute(route)
  // Stable across header updates, so a page that reads the chrome does not
  // re-render (and re-send its actions) because its own actions changed.
  const contextValue = useMemo<DashboardChromeContextValue>(
    () => ({ tabId, panelSide, narrow, phone, setHeader, clearHeader }),
    [tabId, panelSide, narrow, phone, setHeader, clearHeader],
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
          icon={<FlaskConical aria-hidden="true" />}
          title={phone ? undefined : 'Harness E2E'}
          actions={
            <HeaderActions
              actions={header.actions ?? []}
              label={header.actionsLabel}
              narrow={narrow}
              phone={phone}
            />
          }
          onClose={onRequestClose}
        >
          <SectionNav
            sections={sectionLinks}
            current={section}
            narrow={narrow}
            phone={phone}
            onNavigate={navigate}
          />
        </PageHeader>
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
              data-section={section}
            >
              <div
                className="harness-e2e-content min-w-0"
                data-detail={detailPage(route) ? 'true' : undefined}
              >
                {children}
              </div>
              <PinnedPrimary actions={header.actions ?? []} phone={phone} />
            </div>
          </PageMain>
        </PageBody>
      </PageShell>
    </DashboardChromeContext.Provider>
  )
}
