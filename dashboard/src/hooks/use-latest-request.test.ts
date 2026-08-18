import { describe, expect, it } from 'vitest'
import { createLatestRequestGate } from './use-latest-request'

describe('latest request gate', () => {
  it('aborts and invalidates the previous request', () => {
    const gate = createLatestRequestGate()
    const first = gate.begin()
    const second = gate.begin()

    expect(first.signal.aborted).toBe(true)
    expect(first.isCurrent()).toBe(false)
    expect(second.signal.aborted).toBe(false)
    expect(second.isCurrent()).toBe(true)
  })

  it('invalidates outstanding work during teardown', () => {
    const gate = createLatestRequestGate()
    const request = gate.begin()
    gate.dispose()

    expect(request.signal.aborted).toBe(true)
    expect(request.isCurrent()).toBe(false)
  })
})
