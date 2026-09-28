import { useCallback, useEffect, useState } from 'react'
import { dashboardHash, dashboardRouteHash } from '@/lib/dashboard-runtime'

export type WorkspaceView = 'tests' | 'executions'

export type DashboardRoute =
  | { page: 'workspace'; view: WorkspaceView }
  | {
      page: 'execution'
      executionId: string
      anchor: string | null
      /** A run of the execution shown as its own page (audit AW-09). */
      runId: string | null
      /** Which page of the run: its evidence record (default) or its
       *  transcript. */
      view?: 'evidence' | 'transcript' | null
    }
  /** Two executions, A (base) and B. */
  | { page: 'compare'; left: string | null; right: string | null }
  /** Two evaluated system versions of the test catalog. */
  | { page: 'versions'; left: string | null; right: string | null }
  /** A test's runs; `compare` is two of them, A and B, in the hash's
   *  `a` and `b` params. */
  | { page: 'test-history'; testId: string; compare?: boolean }
  | { page: 'suites' }
  | { page: 'stacks' }

const workspaceViews = new Set<WorkspaceView>(['tests', 'executions'])
const defaultRoute: DashboardRoute = { page: 'workspace', view: 'executions' }

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

  if (head === 'tests' && rest[0]) {
    return rest[1] === 'compare'
      ? { page: 'test-history', testId: rest[0], compare: true }
      : { page: 'test-history', testId: rest[0] }
  }
  if (workspaceViews.has(head as WorkspaceView)) {
    return { page: 'workspace', view: head as WorkspaceView }
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
        view: rest[3] === 'transcript' ? 'transcript' : 'evidence',
      }
    }
    return {
      page: 'execution',
      executionId: rest[0] ?? '',
      anchor: rest[1] ?? null,
      runId: null,
    }
  }
  if (head === 'compare' || head === 'versions') {
    return {
      page: head,
      left: rest[0] ?? null,
      right: rest[1] ?? null,
    }
  }
  if (head === 'suites' && !rest[0]) return { page: 'suites' }
  if (head === 'stacks' && !rest[0]) return { page: 'stacks' }
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

export function hashForWorkspace(view: WorkspaceView = 'executions'): string {
  return dashboardHash(view)
}

export function hashForExecution(
  executionId: string,
  anchor: string | null = null,
  runId: string | null = null,
  view: 'evidence' | 'transcript' = 'evidence',
): string {
  const route = dashboardHash(
    `execution${executionId ? `/${encodeSegment(executionId)}` : ''}`,
  )
  if (runId)
    return `${route}/run/${encodeSegment(runId)}${view === 'transcript' ? '/transcript' : ''}`
  return anchor ? `${route}/${encodeSegment(anchor)}` : route
}

function pairHash(head: string, left: string | null, right: string | null) {
  if (!left) return dashboardHash(head)
  const route = dashboardHash(`${head}/${encodeSegment(left)}`)
  return right ? `${route}/${encodeSegment(right)}` : route
}

/** Two executions: A (the base) then B. */
export function hashForComparison(
  left: string | null = null,
  right: string | null = null,
): string {
  return pairHash('compare', left, right)
}

/** A run's page opened from a comparison: its link carries the comparison's
 *  hash (its choice of tests included) in `from`, to go back to. */
export function hashFrom(hash: string, origin: string): string {
  return hashWithParams(hash, new URLSearchParams({ from: origin }))
}

/** The comparison a run's page was opened from. Only a comparison of this
 *  dashboard, A and B and nothing more in its path, is taken: never another
 *  hash or a URL. */
export function comparisonOrigin(rawHash: string): string | null {
  const from = routeParams(rawHash).get('from')
  const route = from ? routeFromHash(from) : null
  return from &&
    route?.page === 'compare' &&
    route.left &&
    route.right &&
    from.split('?')[0] === hashForComparison(route.left, route.right)
    ? from
    : null
}

/** Two evaluated system versions of the test catalog. */
export function hashForVersionComparison(
  left: string | null = null,
  right: string | null = null,
): string {
  return pairHash('versions', left, right)
}

export function hashForTestHistory(testId: string): string {
  return dashboardHash(`tests/${encodeSegment(testId)}`)
}

/** Two runs of a test, A (the reference) and B, by execution id. */
export function hashForRunComparison(
  testId: string,
  a: string,
  b: string,
): string {
  return hashWithParams(
    dashboardHash(`tests/${encodeSegment(testId)}/compare`),
    new URLSearchParams({ a, b }),
  )
}

/** The Suites page, open on one suite when `suiteId` is given. */
export function hashForSuites(suiteId?: string | null): string {
  const hash = dashboardHash('suites')
  return suiteId
    ? hashWithParams(hash, new URLSearchParams({ suite: suiteId }))
    : hash
}

export function hashForStacks(): string {
  return dashboardHash('stacks')
}

export function routeRenderIdentity(route: DashboardRoute): string {
  if (route.page === 'execution') return `${route.page}:${route.executionId}`
  if (route.page === 'compare' || route.page === 'versions') {
    return `${route.page}:${route.left ?? ''}:${route.right ?? ''}`
  }
  if (route.page === 'test-history')
    return `${route.page}:${route.testId}${route.compare ? ':compare' : ''}`
  if (route.page === 'workspace') return `workspace:${route.view}`
  return route.page
}

export function useHashRoute(): [DashboardRoute, (targetHash: string) => void] {
  const [route, setRoute] = useState<DashboardRoute>(currentDashboardRoute)

  useEffect(() => {
    const handle = () => {
      const next = routeFromHash(window.location.hash)
      if (!next) return

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
