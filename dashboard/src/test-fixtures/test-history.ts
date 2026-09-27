import type { TestCatalogRow } from '@/lib/test-catalog'
import type { HistoryObservation, HistoryResponse } from '@/lib/test-history'
import fixture from '@/test-fixtures/test-history.json'

/** form_flow_build as the canvas draws it: six runs on three definitions,
 *  newest first; the 9:13 run spawned a Tech Lead and two sub-agents. */
export const formFlow = fixture.history as unknown as HistoryResponse
export const formFlowRow = fixture.catalog_row as unknown as TestCatalogRow

/** A run of the fixture by its canvas time (`9:13`), a copy to change. */
export function run(when: string): HistoryObservation {
  const clock: Record<string, string> = {
    '3:56': '06:56',
    '7:50': '10:50',
    '8:01': '11:01',
    '9:13': '12:13',
    '10:01': '13:01',
    '3:29': '18:29',
  }
  const found = formFlow.observations.find((item) =>
    item.completed_at.includes(`T${clock[when]}`),
  )
  if (!found) throw new Error(`no run at ${when}`)
  return structuredClone(found)
}

export function history(
  overrides: Partial<HistoryResponse> = {},
): HistoryResponse {
  return { ...structuredClone(formFlow), ...overrides }
}
