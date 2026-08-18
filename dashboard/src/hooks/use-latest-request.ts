import { useCallback, useEffect, useRef } from 'react'

export type LatestRequest = {
  signal: AbortSignal
  isCurrent: () => boolean
}

export type LatestRequestGate = {
  begin: () => LatestRequest
  dispose: () => void
}

export function createLatestRequestGate(): LatestRequestGate {
  let sequence = 0
  let controller: AbortController | null = null

  return {
    begin() {
      controller?.abort()
      const current = ++sequence
      const next = new AbortController()
      controller = next
      return {
        signal: next.signal,
        isCurrent: () => current === sequence && !next.signal.aborted,
      }
    },
    dispose() {
      controller?.abort()
      sequence += 1
    },
  }
}

/** Abort the previous load and guard state writes from stale responses. */
export function useLatestRequest() {
  const gate = useRef<LatestRequestGate | null>(null)
  if (gate.current === null) gate.current = createLatestRequestGate()
  const activeGate = gate.current
  const begin = useCallback(() => activeGate.begin(), [activeGate])

  useEffect(() => () => activeGate.dispose(), [activeGate])

  return begin
}
