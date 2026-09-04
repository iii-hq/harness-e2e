import type { DashboardDataBridge } from '@/lib/dashboard-data-source'

/** Events keep latency low; polling recovers missed events and disconnected tabs. */
export function watchExecution(
  bridge: DashboardDataBridge,
  executionId: string,
  refresh: () => Promise<unknown>,
): () => void {
  let disposed = false
  let refreshing = false
  let unsubscribe: (() => void) | undefined
  let timer: ReturnType<typeof setTimeout> | undefined
  let dueAt = Number.POSITIVE_INFINITY
  const schedule = (delay: number) => {
    if (disposed || refreshing) return
    const nextDue = Date.now() + delay
    if (nextDue >= dueAt) return
    clearTimeout(timer)
    dueAt = nextDue
    timer = setTimeout(() => {
      dueAt = Number.POSITIVE_INFINITY
      if (disposed) return
      refreshing = true
      void Promise.resolve()
        .then(refresh)
        .catch(() => undefined)
        .finally(() => {
          refreshing = false
          schedule(5_000)
        })
    }, delay)
  }
  schedule(5_000)
  void bridge
    .subscribeRunChanges((payload) => {
      if (!payload.execution_id || payload.execution_id === executionId) {
        schedule(400)
      }
    })
    .then((off) => {
      if (disposed) off()
      else unsubscribe = off
    })
    .catch(() => undefined)
  return () => {
    disposed = true
    clearTimeout(timer)
    unsubscribe?.()
  }
}
