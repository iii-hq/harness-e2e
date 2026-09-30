import { afterEach, describe, expect, it, vi } from 'vitest'
import { followRunChanges } from '@/hooks/use-run-changes'

function fixture() {
  vi.useFakeTimers()
  let emit: (payload: Record<string, unknown>) => void = () => {}
  const off = vi.fn()
  const bridge = {
    subscribeRunChanges: vi.fn(async (handler) => {
      emit = handler
      return off
    }),
  }
  return { bridge, off, emit: (payload = {}) => emit(payload) }
}

afterEach(() => {
  vi.useRealTimers()
})

describe('following run changes', () => {
  it('turns changes inside the delay into one call with the last payload', async () => {
    const { bridge, emit } = fixture()
    const onChange = vi.fn()
    const stop = followRunChanges(bridge, onChange)
    await vi.advanceTimersByTimeAsync(0)
    emit({ kind: 'progress' })
    await vi.advanceTimersByTimeAsync(300)
    emit({ kind: 'finished' })
    await vi.advanceTimersByTimeAsync(399)
    expect(onChange).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(1)
    expect(onChange).toHaveBeenCalledTimes(1)
    expect(onChange).toHaveBeenCalledWith({ kind: 'finished' })
    stop()
  })

  it('drops the subscription that resolves after the stop', async () => {
    const { bridge, off } = fixture()
    const stop = followRunChanges(bridge, vi.fn())
    stop()
    expect(off).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(0)
    expect(off).toHaveBeenCalledOnce()
  })
})
