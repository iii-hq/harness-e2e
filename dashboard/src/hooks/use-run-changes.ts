import { useEffect } from 'react'
import type {
  DashboardDataBridge,
  JsonObject,
} from '@/lib/dashboard-data-source'

type RunChanges = Pick<DashboardDataBridge, 'subscribeRunChanges'>

/**
 * Calls `onChange` once, `delayMs` after the last run change, and never
 * sooner than `minIntervalMs` after its previous call: a change that comes
 * earlier waits for that time, still with the latest payload. Returns the stop.
 */
export function followRunChanges(
  bridge: RunChanges,
  onChange: (payload: JsonObject) => unknown,
  delayMs = 400,
  minIntervalMs = 0,
): () => void {
  let cancelled = false
  let dispose: (() => void) | undefined
  let timer: ReturnType<typeof setTimeout> | undefined
  let lastCall = Number.NEGATIVE_INFINITY
  bridge
    .subscribeRunChanges((payload) => {
      clearTimeout(timer)
      const now = Date.now()
      const at = Math.max(now + delayMs, lastCall + minIntervalMs)
      timer = setTimeout(() => {
        lastCall = Date.now()
        void onChange(payload)
      }, at - now)
    })
    .then((off) => {
      if (cancelled) off()
      else dispose = off
    })
    .catch(() => undefined)
  return () => {
    cancelled = true
    clearTimeout(timer)
    dispose?.()
  }
}

/** Follows run changes while `bridge` is set. Pass a stable `onChange`. */
export function useRunChanges(
  bridge: RunChanges | null,
  onChange: (payload: JsonObject) => unknown,
  delayMs = 400,
  minIntervalMs = 0,
) {
  useEffect(() => {
    if (bridge)
      return followRunChanges(bridge, onChange, delayMs, minIntervalMs)
  }, [bridge, onChange, delayMs, minIntervalMs])
}
