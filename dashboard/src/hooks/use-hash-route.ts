import { useCallback, useEffect, useRef, useState } from 'react'

export type WorkspaceView = 'overview' | 'tests' | 'capability' | 'executions'

export type DashboardRoute =
  | { page: 'overview'; view: WorkspaceView }
  | { page: 'execution'; executionId: string; anchor: string | null }
  | { page: 'compare'; left: string | null; right: string | null }
  | { page: 'coverage' }

export type DashboardRoutes = {
  current: () => DashboardRoute
  workspace: (view?: WorkspaceView) => string
  execution: (executionId: string, anchor?: string | null) => string
  compare: (left?: string | null, right?: string | null) => string
  coverage: () => string
}

const workspaceViews = new Set<WorkspaceView>([
  'overview',
  'tests',
  'capability',
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

export function routeFromHash(rawHash: string): DashboardRoute | null {
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
  if (workspaceViews.has(head as WorkspaceView)) {
    return { page: 'overview', view: head as WorkspaceView }
  }
  if (head === 'execution') {
    return {
      page: 'execution',
      executionId: rest[0] ?? '',
      anchor: rest[1] ?? null,
    }
  }
  if (head === 'compare') {
    return {
      page: 'compare',
      left: rest[0] ?? null,
      right: rest[1] ?? null,
    }
  }
  if (head === 'coverage') return { page: 'coverage' }
  return null
}

export function currentDashboardRoute(): DashboardRoute {
  if (typeof window === 'undefined') return defaultRoute
  return routeFromHash(window.location.hash) ?? defaultRoute
}

export function hashForWorkspace(view: WorkspaceView = 'overview'): string {
  return `#/${view}`
}

export function hashForExecution(
  executionId: string,
  anchor: string | null = null,
): string {
  const route = `#/execution${executionId ? `/${encodeSegment(executionId)}` : ''}`
  return anchor ? `${route}/${encodeSegment(anchor)}` : route
}

export function hashForComparison(
  left: string | null = null,
  right: string | null = null,
): string {
  if (!left) return '#/compare'
  const route = `#/compare/${encodeSegment(left)}`
  return right ? `${route}/${encodeSegment(right)}` : route
}

export function hashForCoverage(): string {
  return '#/coverage'
}

export const dashboardRoutes: DashboardRoutes = {
  current: currentDashboardRoute,
  workspace: hashForWorkspace,
  execution: hashForExecution,
  compare: hashForComparison,
  coverage: hashForCoverage,
}

export function routeRenderIdentity(route: DashboardRoute): string {
  if (route.page === 'execution') return `${route.page}:${route.executionId}`
  if (route.page === 'compare') {
    return `${route.page}:${route.left ?? ''}:${route.right ?? ''}`
  }
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
      window.location.hash === '' ||
      window.location.hash === '#' ||
      window.location.hash === '#/'
    ) {
      replaceHash(hashForWorkspace())
    }

    const handle = () => {
      const next = routeFromHash(window.location.hash)
      if (!next) return

      // Each coarse page still has an isolated legacy renderer. A document
      // reload keeps those renderers single-instanced while the hash router
      // owns the public URL and overview/anchor navigation stays client-side.
      if (routeRenderIdentity(next) !== routeRenderIdentity(routeRef.current)) {
        window.location.reload()
        return
      }
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
