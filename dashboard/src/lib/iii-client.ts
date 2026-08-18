export type DashboardIiiClient = {
  browserId: string
  trigger<T>(
    functionId: string,
    payload?: Record<string, unknown>,
    options?: { timeoutMs?: number },
  ): Promise<T>
  on<T>(functionId: string, handler: (payload: T) => void): () => void
  registerTrigger(input: {
    type: string
    function_id: string
    config: Record<string, unknown>
  }): () => void
}

let clientPromise: Promise<DashboardIiiClient> | null = null
let clientFactory:
  | (() => DashboardIiiClient | Promise<DashboardIiiClient>)
  | null = null

export function installDashboardIiiClient(client: DashboardIiiClient) {
  clientPromise = Promise.resolve(client)
}

export function installDashboardIiiClientFactory(
  factory: () => DashboardIiiClient | Promise<DashboardIiiClient>,
) {
  clientFactory = factory
  clientPromise = null
}

export function getDashboardIiiClient(): Promise<DashboardIiiClient> {
  if (!clientPromise) {
    if (!clientFactory) {
      return Promise.reject(
        new Error('dashboard iii client has not been configured'),
      )
    }
    clientPromise = Promise.resolve(clientFactory())
  }
  return clientPromise
}
