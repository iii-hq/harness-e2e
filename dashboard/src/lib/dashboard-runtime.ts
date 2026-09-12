const HASH_BASE = '#/ext/harness-e2e'

export function dashboardHash(path: string): string {
  const suffix = path.replace(/^#?\//, '')
  return suffix ? `${HASH_BASE}/${suffix}` : HASH_BASE
}

export function dashboardRouteHash(rawHash: string): string | null {
  if (rawHash === HASH_BASE || rawHash === `${HASH_BASE}/`) return '#/'
  const prefix = `${HASH_BASE}/`
  return rawHash.startsWith(prefix) ? `#/${rawHash.slice(prefix.length)}` : null
}
