import {
  type ISdk,
  type RegisterTriggerInput,
  type RemoteFunctionHandler,
  registerWorker,
} from 'iii-browser-sdk'

export type DashboardIiiClient = {
  browserId: string
  trigger<T>(
    functionId: string,
    payload?: Record<string, unknown>,
    options?: { timeoutMs?: number },
  ): Promise<T>
  on<T>(functionId: string, handler: (payload: T) => void): () => void
  registerTrigger(input: RegisterTriggerInput): () => void
  dispose(): Promise<void>
}

let clientPromise: Promise<DashboardIiiClient> | null = null

export function getDashboardIiiClient(): Promise<DashboardIiiClient> {
  clientPromise ??= Promise.resolve(
    wrapSdk(registerWorker(resolveWsUrl()), makeBrowserId()),
  )
  return clientPromise
}

function wrapSdk(sdk: ISdk, browserId: string): DashboardIiiClient {
  const handlers = new Set<() => void>()
  const triggers = new Set<() => void>()
  const fanouts = new Map<
    string,
    {
      listeners: Set<(payload: unknown) => void>
      release: () => void
    }
  >()

  const trigger = <T>(
    functionId: string,
    payload: Record<string, unknown> = {},
    options?: { timeoutMs?: number },
  ) =>
    sdk.trigger<unknown, T>({
      function_id: functionId,
      payload,
      timeoutMs: options?.timeoutMs ?? 15_000,
    })

  const on = <T>(functionId: string, handler: (payload: T) => void) => {
    const id = `${functionId}::${browserId}`
    let fanout = fanouts.get(id)
    if (!fanout) {
      const listeners = new Set<(payload: unknown) => void>()
      const remote: RemoteFunctionHandler = async (payload: unknown) => {
        for (const listener of [...listeners]) listener(payload)
        return null
      }
      const registration = sdk.registerFunction(id, remote, {
        metadata: { internal: true },
      })
      fanout = {
        listeners,
        release: () => registration.unregister(),
      }
      fanouts.set(id, fanout)
    }
    const listener = (payload: unknown) => handler(payload as T)
    fanout.listeners.add(listener)
    let active = true
    const off = () => {
      if (!active) return
      active = false
      handlers.delete(off)
      fanout?.listeners.delete(listener)
      if (fanout?.listeners.size === 0) {
        fanouts.delete(id)
        fanout.release()
      }
    }
    handlers.add(off)
    return off
  }

  const registerTrigger = (input: RegisterTriggerInput) => {
    const registration = sdk.registerTrigger(input)
    let active = true
    const off = () => {
      if (!active) return
      active = false
      triggers.delete(off)
      registration.unregister()
    }
    triggers.add(off)
    return off
  }

  const dispose = async () => {
    for (const off of [...triggers]) off()
    for (const off of [...handlers]) off()
    await sdk.shutdown()
  }

  return { browserId, trigger, on, registerTrigger, dispose }
}

function resolveWsUrl() {
  const url = new URL('./ws', window.location.href)
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
  return url.href
}

function makeBrowserId() {
  return globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random()}`
}
