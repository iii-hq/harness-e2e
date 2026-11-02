export type DashboardIiiClient = {
  browserId: string
  trigger<T>(
    functionId: string,
    payload?: Record<string, unknown>,
    options?: { timeoutMs?: number; namespace?: string },
  ): Promise<T>
  on<T>(functionId: string, handler: (payload: T) => void): () => void
  observe(
    source: {
      function_id: string
      namespace?: string
      deliver?: 'envelope' | 'input' | 'output'
    },
    target: string,
  ): () => void
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
