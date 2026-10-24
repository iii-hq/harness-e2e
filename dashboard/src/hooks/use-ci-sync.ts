import { useCallback, useEffect, useState } from 'react'
import { useRunChanges } from '@/hooks/use-run-changes'
import type {
  CiSyncStatus,
  DashboardDataBridge,
} from '@/lib/dashboard-data-source'

/** Asks for the sync's status at once and again each interval, while sync
 *  is on; returns the stop. A failed read tries again at the next interval. */
export function pollCiSync(
  bridge: Pick<DashboardDataBridge, 'getCiSyncStatus'>,
  onStatus: (status: CiSyncStatus) => void,
  intervalMs = 15_000,
): () => void {
  let stopped = false
  let timer: ReturnType<typeof setTimeout> | undefined
  const ask = async () => {
    try {
      const status = await bridge.getCiSyncStatus()
      if (stopped) return
      onStatus(status)
      if (!status.enabled) return
    } catch {
      if (stopped) return
    }
    timer = setTimeout(ask, intervalMs)
  }
  timer = setTimeout(ask, 0)
  return () => {
    stopped = true
    clearTimeout(timer)
  }
}

const messageOf = (cause: unknown) =>
  cause instanceof Error ? cause.message : String(cause)

/** CI sync's status, kept current while sync is on, and the two things a
 *  reader can ask of it. A failed read leaves `status` null: no line. */
export function useCiSync(bridge: DashboardDataBridge | null): {
  status: CiSyncStatus | null
  error: string | null
  set: (enabled: boolean) => void
  check: () => void
} {
  const [status, setStatus] = useState<CiSyncStatus | null>(null)
  const [error, setError] = useState<string | null>(null)
  const enabled = status?.enabled

  // Turning sync on starts the poll again; turning it off lets it end.
  // biome-ignore lint/correctness/useExhaustiveDependencies: `enabled` restarts the poll
  useEffect(() => {
    if (bridge) return pollCiSync(bridge, setStatus)
  }, [bridge, enabled])

  // Imports change the counters between two polls.
  const refresh = useCallback(async () => {
    try {
      if (bridge) setStatus(await bridge.getCiSyncStatus())
    } catch {
      // The next poll tries again.
    }
  }, [bridge])
  useRunChanges(bridge, refresh)

  const ask = useCallback((call: () => Promise<CiSyncStatus>) => {
    setError(null)
    call().then(setStatus, (cause) => setError(messageOf(cause)))
  }, [])
  const set = useCallback(
    (on: boolean) => {
      if (bridge) ask(() => bridge.setCiSync(on))
    },
    [bridge, ask],
  )
  const check = useCallback(() => {
    if (bridge) ask(() => bridge.checkCiSync())
  }, [bridge, ask])

  return { status, error, set, check }
}
