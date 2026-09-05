import { useCallback, useEffect, useRef, useState } from 'react'
import {
  dashboardHash,
  dashboardRouteHash,
  isEmbeddedDashboard,
} from '@/lib/dashboard-runtime'

export type WorkspaceView = 'overview' | 'tests' | 'executions'

export type DashboardRoute =
  | { page: 'overview'; view: WorkspaceView }
  | {
      page: 'execution'
      executionId: string
      anchor: string | null
      /** Evidence record open on top of the execution (audit AW-09). */
      runId: string | null
    }
  | { page: 'compare'; left: string | null; right: string | null }
  | { page: 'test-history'; testId: string }
  | { page: 'plans' }
  | {
      page: 'plan-create'
      profileId?: string
      duplicateId?: string
      editId?: string
      manual?: boolean
    }
  | { page: 'plan-detail'; planId: string }

export type DashboardRoutes = {
  current: () => DashboardRoute
  workspace: (view?: WorkspaceView) => string
  execution: (
    executionId: string,
    anchor?: string | null,
    runId?: string | null,
  ) => string
  compare: (left?: string | null, right?: string | null) => string
  testHistory: (testId: string) => string
  plans: () => string
  newPlan: () => string
  plan: (planId: string) => string
}

const workspaceViews = new Set<WorkspaceView>([
  'overview',
  'tests',
  'executions',
])
const defaultRoute: DashboardRoute = { page: 'overview', view: 'overview' }

function decodeSegment(segment: string): string {
  try {
    return decodeURIComponent(segment)
  } catch {
    return segment
  }
}

function encodeSegment(segment: string): string {
  return encodeURIComponent(segment)
}

/** The `?key=value` part of a dashboard hash, if any (audit T-08 / TH-19). */
export function routeParams(rawHash: string): URLSearchParams {
  const index = rawHash.indexOf('?')
  return new URLSearchParams(index === -1 ? '' : rawHash.slice(index + 1))
}

export function hashWithParams(hash: string, params: URLSearchParams): string {
  const base = hash.split('?')[0]
  const query = params.toString()
  return query ? `${base}?${query}` : base
}

/** Rewrites the whole hash without a navigation (no hashchange, no reload). */
export function replaceDashboardHash(targetHash: string) {
  if (targetHash === window.location.hash) return
  window.history.replaceState(
    window.history.state,
    '',
    `${window.location.pathname}${window.location.search}${targetHash}`,
  )
}

/** Rewrites the current hash's params without a navigation or a scroll reset. */
export function replaceRouteParams(params: URLSearchParams) {
  const target = hashWithParams(window.location.hash, params)
  if (target === window.location.hash) return
  window.history.replaceState(
    window.history.state,
    '',
    `${window.location.pathname}${window.location.search}${target}`,
  )
}

export function routeFromHash(rawHash: string): DashboardRoute | null {
  const routedHash = dashboardRouteHash(rawHash.split('?')[0])
  if (routedHash === null) return null
  rawHash = routedHash
  if (rawHash === '' || rawHash === '#' || rawHash === '#/') {
    return defaultRoute
  }

  if (!rawHash.startsWith('#/')) return null

  const segments = rawHash
    .slice(2)
    .split('/')
    .filter(Boolean)
    .map(decodeSegment)
  const [head, ...rest] = segments

  if (head === 'scenarios') {
    return { page: 'overview', view: 'tests' }
  }
  if (head === 'tests' && rest[0]) {
    return { page: 'test-history', testId: rest[0] }
  }
  if (workspaceViews.has(head as WorkspaceView)) {
    return { page: 'overview', view: head as WorkspaceView }
  }
  if (head === 'execution') {
    // #/execution/<id>/run/<runId> opens the evidence record as a route, so
    // the browser's back button returns to the execution (audit AW-09).
    if (rest[1] === 'run') {
      return {
        page: 'execution',
        executionId: rest[0] ?? '',
        anchor: null,
        runId: rest[2] ?? null,
      }
    }
    return {
      page: 'execution',
      executionId: rest[0] ?? '',
      anchor: rest[1] ?? null,
      runId: null,
    }
  }
  if (head === 'compare') {
    return {
      page: 'compare',
      left: rest[0] ?? null,
      right: rest[1] ?? null,
    }
  }
  if (head === 'plans') {
    if (!rest[0]) return { page: 'plans' }
    if (rest[0] === 'new') {
      if (rest[1] === 'profile' && rest[2])
        return { page: 'plan-create', profileId: rest[2] }
      if (rest[1] === 'duplicate' && rest[2])
        return { page: 'plan-create', duplicateId: rest[2] }
      if (rest[1] === 'edit' && rest[2])
        return { page: 'plan-create', editId: rest[2] }
      if (rest[1] === 'manual') return { page: 'plan-create', manual: true }
      return { page: 'plan-create' }
    }
    return { page: 'plan-detail', planId: rest[0] }
  }
  return null
}

export function currentDashboardRoute(): DashboardRoute {
  if (typeof window === 'undefined') return defaultRoute
  return routeFromHash(window.location.hash) ?? defaultRoute
}

export function hashForTests(params?: URLSearchParams): string {
  const hash = hashForWorkspace('tests')
  return params ? hashWithParams(hash, params) : hash
}

export function hashForWorkspace(view: WorkspaceView = 'overview'): string {
  return dashboardHash(view)
}

export function hashForExecution(
  executionId: string,
  anchor: string | null = null,
  runId: string | null = null,
): string {
  const route = dashboardHash(
    `execution${executionId ? `/${encodeSegment(executionId)}` : ''}`,
  )
  if (runId) return `${route}/run/${encodeSegment(runId)}`
  return anchor ? `${route}/${encodeSegment(anchor)}` : route
}

export function hashForComparison(
  left: string | null = null,
  right: string | null = null,
): string {
  if (!left) return dashboardHash('compare')
  const route = dashboardHash(`compare/${encodeSegment(left)}`)
  return right ? `${route}/${encodeSegment(right)}` : route
}

export function hashForTestHistory(testId: string): string {
  return dashboardHash(`tests/${encodeSegment(testId)}`)
}

export function hashForNewPlan(): string {
  return dashboardHash('plans/new')
}

export function hashForPlans(): string {
  return dashboardHash('plans')
}

export function hashForPlan(planId: string): string {
  return dashboardHash(`plans/${encodeSegment(planId)}`)
}

export const dashboardRoutes: DashboardRoutes = {
  current: currentDashboardRoute,
  workspace: hashForWorkspace,
  execution: hashForExecution,
  compare: hashForComparison,
  testHistory: hashForTestHistory,
  plans: hashForPlans,
  newPlan: hashForNewPlan,
  plan: hashForPlan,
}

export function routeRenderIdentity(route: DashboardRoute): string {
  if (route.page === 'execution') return `${route.page}:${route.executionId}`
  if (route.page === 'compare') {
    return `${route.page}:${route.left ?? ''}:${route.right ?? ''}`
  }
  if (route.page === 'test-history') return `${route.page}:${route.testId}`
  if (route.page === 'plan-detail') return `${route.page}:${route.planId}`
  if (route.page === 'plan-create') return route.page
  if (route.page === 'overview') {
    return route.view === 'tests' ? 'overview:tests' : 'overview:workspace'
  }
  return route.page
}

function replaceHash(targetHash: string) {
  window.history.replaceState(
    window.history.state,
    '',
    `${window.location.pathname}${window.location.search}${targetHash}`,
  )
}

export function useHashRoute(): [DashboardRoute, (targetHash: string) => void] {
  const [route, setRoute] = useState<DashboardRoute>(currentDashboardRoute)
  const routeRef = useRef(route)
  routeRef.current = route

  useEffect(() => {
    if (
      !isEmbeddedDashboard() &&
      (window.location.hash === '' ||
        window.location.hash === '#' ||
        window.location.hash === '#/')
    ) {
      replaceHash(hashForWorkspace())
    }

    const handle = () => {
      const next = routeFromHash(window.location.hash)
      if (!next) return

      // Audit S-07: every page is a React component now, so navigation
      // stays client-side in the standalone build too. The reload used to
      // flash the html background and drop filters, scroll and dialogs.
      setRoute(next)
    }
    window.addEventListener('hashchange', handle)
    return () => window.removeEventListener('hashchange', handle)
  }, [])

  const navigate = useCallback((targetHash: string) => {
    if (window.location.hash !== targetHash) window.location.hash = targetHash
    else {
      const next = routeFromHash(targetHash)
      if (next) setRoute(next)
    }
  }, [])

  return [route, navigate]
}
