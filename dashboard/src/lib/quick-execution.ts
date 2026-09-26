/** A page asks the Executions page to open Run tests with some tests
 *  already ticked ("Run this test", "New run on B"): the request survives the
 *  route change in session storage and is read once. */
export const QUICK_EXECUTION_INTENT_KEY = 'harness-e2e:quick-execution'

export function requestQuickExecution(scenarioIds: string[] = []) {
  window.sessionStorage.setItem(
    QUICK_EXECUTION_INTENT_KEY,
    JSON.stringify(scenarioIds),
  )
}

/** Returns the requested scope (possibly empty) or null when nothing asked. */
export function consumeQuickExecutionRequest(): string[] | null {
  const raw = window.sessionStorage.getItem(QUICK_EXECUTION_INTENT_KEY)
  if (raw === null) return null
  window.sessionStorage.removeItem(QUICK_EXECUTION_INTENT_KEY)
  if (raw === 'open') return []
  try {
    const parsed = JSON.parse(raw)
    return Array.isArray(parsed)
      ? parsed.filter((item): item is string => typeof item === 'string')
      : []
  } catch {
    return []
  }
}
