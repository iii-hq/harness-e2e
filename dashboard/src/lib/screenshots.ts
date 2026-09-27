import type {
  DashboardDataBridge,
  DashboardExecutionDetail,
  JsonObject,
} from '@/lib/dashboard-data-source'

export type ScreenshotEntry = {
  key: string
  /** The native execution whose report declares the deliverable. */
  executionId: string
  runId: string
  path: string
  pointer: string
  caption: string
}

function objects(value: unknown): JsonObject[] {
  return Array.isArray(value)
    ? value.filter(
        (item): item is JsonObject =>
          Boolean(item) && typeof item === 'object' && !Array.isArray(item),
      )
    : []
}

/** Screenshots each run's deliverables name, as the report lists them. */
export function screenshotsOf(
  detail: DashboardExecutionDetail,
  scenarioId: string,
): ScreenshotEntry[] {
  return detail.reports
    .filter((record) => record.scenario_id === scenarioId)
    .flatMap((record) => {
      const executionId =
        typeof record.native_execution_id === 'string' &&
        record.native_execution_id
          ? record.native_execution_id
          : detail.id
      return (record.report?.scenarios ?? []).flatMap((scenario) =>
        scenario.runs.flatMap((run) =>
          objects(run.deliverables).flatMap((deliverable) => {
            const path = objects([deliverable.artifact])[0]?.path
            if (typeof path !== 'string') return []
            return objects(deliverable.screenshots).map((screenshot) => ({
              key: `${executionId}:${run.run_id}:${path}:${screenshot.pointer}`,
              executionId,
              runId: run.run_id,
              path,
              pointer: String(screenshot.pointer ?? ''),
              caption: String(screenshot.caption ?? screenshot.pointer ?? ''),
            }))
          }),
        ),
      )
    })
}

/** A screenshot's bytes as a data URL, read through `e2e::dashboard::evidence-read`. */
export async function screenshotSource(
  read: DashboardDataBridge['readEvidence'],
  screenshot: ScreenshotEntry,
): Promise<string> {
  const file = await read({
    execution_id: screenshot.executionId,
    path: screenshot.path,
    pointer: screenshot.pointer,
  })
  return `data:${file.media_type};base64,${file.base64}`
}
