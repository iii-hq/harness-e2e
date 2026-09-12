import {
  type DashboardExecutionDetail,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'

export type ScenarioChatTarget = {
  executionId: string
  scenarioId: string
  subjectId: string
  runId: string
  attemptId: string
  attemptNumber: number
  sessionId: string
  messages: unknown
  status: string | null
  current: boolean
}

export type ScenarioChatSource = {
  executionId: string
  scenarioId: string
  scenarioVersion?: number | null
  subjectId?: string | null
  runId?: string | null
}

function nonEmpty(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value.trim() : null
}

export function scenarioChatTargets(
  detail: DashboardExecutionDetail,
  scenarioId: string,
  subjectId?: string | null,
  runId?: string | null,
  scenarioVersion?: number | null,
): ScenarioChatTarget[] {
  const targets: ScenarioChatTarget[] = []
  const seen = new Set<string>()

  for (const record of detail.reports ?? []) {
    if (subjectId && record.subject_id !== subjectId) continue
    for (const scenario of record.report?.scenarios ?? []) {
      if (scenario.scenario_id !== scenarioId) continue
      if (
        scenarioVersion !== undefined &&
        (scenario.scenario_version ?? null) !== scenarioVersion
      )
        continue
      for (const run of [...(scenario.runs ?? [])].reverse()) {
        if (runId && run.run_id !== runId) continue
        const attempts = [
          {
            run_id: run.run_id,
            attempt_id: run.attempt_id,
            attempt_number:
              run.attempt_number ?? (run.retry_attempts?.length ?? 0) + 1,
            session_id: run.session_id,
            transcript: run.transcript,
            status: run.status,
            current: true,
          },
          ...[...(run.retry_attempts ?? [])].reverse().map((attempt) => ({
            ...attempt,
            current: false,
          })),
        ]

        for (const attempt of attempts) {
          const sessionId = nonEmpty(attempt.session_id)
          if (!sessionId || seen.has(sessionId)) continue
          seen.add(sessionId)
          targets.push({
            executionId: detail.id,
            scenarioId,
            subjectId: record.subject_id,
            runId: nonEmpty(attempt.run_id) ?? run.run_id,
            attemptId: nonEmpty(attempt.attempt_id) ?? run.attempt_id,
            attemptNumber:
              typeof attempt.attempt_number === 'number'
                ? attempt.attempt_number
                : 1,
            sessionId,
            messages: attempt.transcript?.messages,
            status: nonEmpty(attempt.status),
            current: attempt.current,
          })
        }
      }
    }
  }

  return targets
}

export async function loadScenarioChatTargets({
  executionId,
  scenarioId,
  scenarioVersion,
  subjectId,
  runId,
}: ScenarioChatSource): Promise<ScenarioChatTarget[]> {
  const bridge = await getDashboardDataBridge()
  const detail = await bridge.getExecution(executionId)
  return scenarioChatTargets(
    detail,
    scenarioId,
    subjectId,
    runId,
    scenarioVersion,
  )
}
