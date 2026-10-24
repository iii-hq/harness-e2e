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

  it('calls at most once per minimum interval while changes keep coming', async () => {
    const { bridge, emit } = fixture()
    const onChange = vi.fn()
    const stop = followRunChanges(bridge, onChange, 400, 10_000)
    await vi.advanceTimersByTimeAsync(0)
    emit({ at: 0 })
    await vi.advanceTimersByTimeAsync(3000)
    emit({ at: 3000 })
    await vi.advanceTimersByTimeAsync(3000)
    emit({ at: 6000 })
    await vi.advanceTimersByTimeAsync(3000)
    emit({ at: 9000 })
    // 400 ms after the first change it ran; nothing runs again before 10.4 s.
    expect(onChange.mock.calls).toEqual([[{ at: 0 }]])
    await vi.advanceTimersByTimeAsync(1399)
    expect(onChange).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(1)
    expect(onChange.mock.calls).toEqual([[{ at: 0 }], [{ at: 9000 }]])
    // A quiet interval later, a change waits only the delay.
    await vi.advanceTimersByTimeAsync(20_000 - 10_400)
    emit({ at: 20_000 })
    await vi.advanceTimersByTimeAsync(400)
    expect(onChange).toHaveBeenLastCalledWith({ at: 20_000 })
    expect(onChange).toHaveBeenCalledTimes(3)
    stop()
  })

  it('without a minimum interval calls after each delay, as before', async () => {
    const { bridge, emit } = fixture()
    const onChange = vi.fn()
    const stop = followRunChanges(bridge, onChange, 400, 0)
    await vi.advanceTimersByTimeAsync(0)
    emit({ at: 0 })
    await vi.advanceTimersByTimeAsync(400)
    emit({ at: 400 })
    await vi.advanceTimersByTimeAsync(400)
    expect(onChange.mock.calls).toEqual([[{ at: 0 }], [{ at: 400 }]])
    stop()
  })

  it('stops a postponed call with the subscription', async () => {
    const { bridge, emit } = fixture()
    const onChange = vi.fn()
    const stop = followRunChanges(bridge, onChange, 400, 10_000)
    await vi.advanceTimersByTimeAsync(0)
    emit({ at: 0 })
    await vi.advanceTimersByTimeAsync(400)
    emit({ at: 400 })
    await vi.advanceTimersByTimeAsync(400)
    stop()
    await vi.advanceTimersByTimeAsync(20_000)
    expect(onChange).toHaveBeenCalledTimes(1)
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
