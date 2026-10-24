import { afterEach, describe, expect, it, vi } from 'vitest'
import { pollCiSync } from '@/hooks/use-ci-sync'
import { ciSync } from '@/test-fixtures/release-overview'

const { keeping_up: k, off } = ciSync

afterEach(() => {
  vi.useRealTimers()
})

describe('polling CI sync', () => {
  it('asks at once, again each interval while sync is on, and stops when it is off', async () => {
    vi.useFakeTimers()
    const answers = [k, k, off]
    const getCiSyncStatus = vi.fn(async () => answers.shift() ?? off)
    const seen = vi.fn()
    pollCiSync({ getCiSyncStatus }, seen, 1000)
    await vi.advanceTimersByTimeAsync(0)
    expect(seen).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(1000)
    expect(seen).toHaveBeenCalledTimes(2)
    await vi.advanceTimersByTimeAsync(1000)
    expect(seen).toHaveBeenCalledTimes(3)
    expect(seen).toHaveBeenLastCalledWith(off)
    await vi.advanceTimersByTimeAsync(5000)
    expect(getCiSyncStatus).toHaveBeenCalledTimes(3)
  })

  it('asks nothing after the stop', async () => {
    vi.useFakeTimers()
    const getCiSyncStatus = vi.fn(async () => k)
    const seen = vi.fn()
    const stop = pollCiSync({ getCiSyncStatus }, seen, 1000)
    stop()
    await vi.advanceTimersByTimeAsync(5000)
    expect(seen).not.toHaveBeenCalled()
    expect(getCiSyncStatus).not.toHaveBeenCalled()
  })

  it('tries again at the next interval after a failed read', async () => {
    vi.useFakeTimers()
    const getCiSyncStatus = vi
      .fn()
      .mockRejectedValueOnce(new Error('engine unavailable'))
      .mockResolvedValue(k)
    const seen = vi.fn()
    const stop = pollCiSync({ getCiSyncStatus }, seen, 1000)
    await vi.advanceTimersByTimeAsync(0)
    expect(seen).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(1000)
    expect(seen).toHaveBeenCalledTimes(1)
    stop()
  })
})
