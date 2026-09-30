import { useEffect } from 'react'
import type {
  DashboardDataBridge,
  JsonObject,
} from '@/lib/dashboard-data-source'

type RunChanges = Pick<DashboardDataBridge, 'subscribeRunChanges'>

/** Calls `onChange` once, `delayMs` after the last run change; returns the stop. */
export function followRunChanges(
  bridge: RunChanges,
  onChange: (payload: JsonObject) => unknown,
  delayMs = 400,
): () => void {
  let cancelled = false
  let dispose: (() => void) | undefined
  let timer: ReturnType<typeof setTimeout> | undefined
  bridge
    .subscribeRunChanges((payload) => {
      clearTimeout(timer)
      timer = setTimeout(() => void onChange(payload), delayMs)
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
) {
  useEffect(() => {
    if (bridge) return followRunChanges(bridge, onChange, delayMs)
  }, [bridge, onChange, delayMs])
}
