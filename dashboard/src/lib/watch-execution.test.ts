import { afterEach, describe, expect, it, vi } from 'vitest'
import type { DashboardDataBridge } from '@/lib/dashboard-data-source'
import { watchExecution } from '@/lib/watch-execution'

function fixture() {
  vi.useFakeTimers()
  let handler: (payload: Record<string, unknown>) => void = () => {}
  const off = vi.fn()
  const bridge = {
    subscribeRunChanges: vi.fn(async (next) => {
      handler = next
      return off
    }),
  } as unknown as DashboardDataBridge
  const refresh = vi.fn(async () => {})
  return {
    bridge,
    refresh,
    off,
    emit: (id = 'execution-1') =>
      handler({ kind: 'progress', execution_id: id }),
  }
}

afterEach(() => {
  vi.useRealTimers()
})

describe('live execution refresh', () => {
  it('follows only matching events and polls when events are lost', async () => {
    const { bridge, refresh, off, emit } = fixture()
    const stop = watchExecution(bridge, 'execution-1', refresh)
    emit('other-execution')
    await vi.advanceTimersByTimeAsync(400)
    expect(refresh).not.toHaveBeenCalled()
    emit()
    await vi.advanceTimersByTimeAsync(400)
    expect(refresh).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(5_000)
    expect(refresh).toHaveBeenCalledTimes(2)
    stop()
    expect(off).toHaveBeenCalledOnce()
    await vi.advanceTimersByTimeAsync(10_000)
    expect(refresh).toHaveBeenCalledTimes(2)
  })

  it('recovers from a failed subscription and a failed refresh', async () => {
    const { bridge, refresh } = fixture()
    vi.mocked(bridge.subscribeRunChanges).mockRejectedValue(
      new Error('disconnected'),
    )
    refresh.mockRejectedValueOnce(new Error('offline'))
    const stop = watchExecution(bridge, 'execution-1', refresh)
    await vi.advanceTimersByTimeAsync(10_000)
    expect(refresh).toHaveBeenCalledTimes(2)
    stop()
  })

  it('coalesces rapid events without starving refresh or overlapping requests', async () => {
    const { bridge, refresh, emit } = fixture()
    let finish: () => void = () => {}
    refresh.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          finish = resolve
        }),
    )
    const stop = watchExecution(bridge, 'execution-1', refresh)
    for (let i = 0; i < 6; i += 1) {
      emit()
      await vi.advanceTimersByTimeAsync(100)
    }
    expect(refresh).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(10_000)
    expect(refresh).toHaveBeenCalledTimes(1)
    finish()
    await vi.advanceTimersByTimeAsync(5_000)
    expect(refresh).toHaveBeenCalledTimes(2)
    stop()
  })

  it('disposes a subscription that connects after unmount', async () => {
    const { bridge, refresh, off } = fixture()
    let connect: (off: () => void) => void = () => {}
    vi.mocked(bridge.subscribeRunChanges).mockImplementation(
      () =>
        new Promise((resolve) => {
          connect = resolve
        }),
    )
    const stop = watchExecution(bridge, 'execution-1', refresh)
    stop()
    connect(off)
    await vi.advanceTimersByTimeAsync(6_000)
    expect(off).toHaveBeenCalledOnce()
    expect(refresh).not.toHaveBeenCalled()
  })
})
