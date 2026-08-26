import type { JsonObject } from '@/lib/dashboard-data-source'

export type LocalScenarioSummary = {
  id: string
  title: string
  version: number
  source_path: string
  source_sha256: string
}

export function localScenariosFromCatalog(
  value: JsonObject,
): LocalScenarioSummary[] {
  if (!Array.isArray(value.local_scenarios)) return []
  return value.local_scenarios
    .flatMap((candidate) => {
      if (!candidate || typeof candidate !== 'object') return []
      const scenario = candidate as JsonObject
      if (
        typeof scenario.id !== 'string' ||
        typeof scenario.title !== 'string' ||
        typeof scenario.version !== 'number' ||
        typeof scenario.source_path !== 'string' ||
        typeof scenario.source_sha256 !== 'string'
      ) {
        return []
      }
      return [scenario as LocalScenarioSummary]
    })
    .sort((left, right) => left.title.localeCompare(right.title))
}
