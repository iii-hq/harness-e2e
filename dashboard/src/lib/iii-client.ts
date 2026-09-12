export type DashboardIiiClient = {
  browserId: string
  trigger<T>(
    functionId: string,
    payload?: Record<string, unknown>,
    options?: { timeoutMs?: number; namespace?: string },
  ): Promise<T>
  on<T>(functionId: string, handler: (payload: T) => void): () => void
  registerTrigger(input: {
    type: string
    function_id: string
    config: Record<string, unknown>
  }): () => void
}

let client: DashboardIiiClient | null = null

export function installDashboardIiiClient(value: DashboardIiiClient) {
  client = value
}

export function getDashboardIiiClient(): Promise<DashboardIiiClient> {
  return client
    ? Promise.resolve(client)
    : Promise.reject(new Error('Console iii client has not been configured'))
}
