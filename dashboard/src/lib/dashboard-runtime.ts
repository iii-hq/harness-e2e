export type DashboardRuntime = {
  embedded: boolean
  hashBase: string
}

const standaloneRuntime: DashboardRuntime = {
  embedded: false,
  hashBase: '',
}

let runtime = standaloneRuntime

export function configureDashboardRuntime(next: DashboardRuntime): () => void {
  const previous = runtime
  runtime = {
    embedded: next.embedded,
    hashBase: normalizeHashBase(next.hashBase),
  }
  return () => {
    runtime = previous
  }
}

export function dashboardRuntime(): DashboardRuntime {
  return runtime
}

export function isEmbeddedDashboard(): boolean {
  return runtime.embedded
}

export function dashboardHash(path: string): string {
  const normalized = path.startsWith('#/')
    ? path
    : `#/${path.replace(/^\/+/, '')}`
  if (!runtime.hashBase) return normalized
  const suffix = normalized.slice(2)
  return suffix ? `${runtime.hashBase}/${suffix}` : runtime.hashBase
}

export function dashboardRouteHash(rawHash: string): string | null {
  if (!runtime.hashBase) return rawHash
  if (rawHash === runtime.hashBase || rawHash === `${runtime.hashBase}/`) {
    return '#/'
  }
  const prefix = `${runtime.hashBase}/`
  if (!rawHash.startsWith(prefix)) return null
  return `#/${rawHash.slice(prefix.length)}`
}

function normalizeHashBase(value: string): string {
  const trimmed = value.trim().replace(/\/+$/, '')
  if (!trimmed) return ''
  if (!trimmed.startsWith('#/')) {
    throw new Error(`dashboard hash base must start with '#/': ${value}`)
  }
  return trimmed
}
