import {
  type DashboardExecutionDetail,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import type { RcReference } from '@/lib/release-control-reference'

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
  /** Digest of the scenario definition the transcript belongs to. */
  behaviorSha256?: string | null
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
  behaviorSha256?: string | null,
): ScenarioChatTarget[] {
  const targets: ScenarioChatTarget[] = []
  const seen = new Set<string>()
  const reference = detail.remote_reference as RcReference | undefined
  const scenarios =
    detail.origin === 'remote'
      ? (reference?.runs ?? []).map((run) => ({
          subjectId: run.identity?.subjectModel ?? '',
          scenarioId: run.scenarioId,
          behaviorSha256: run.behaviorSha256,
          runs: run.record ? [run.record] : [],
        }))
      : (detail.reports ?? []).flatMap((record) =>
          (record.report?.scenarios ?? []).map((scenario) => ({
            subjectId: record.subject_id,
            scenarioId: scenario.scenario_id,
            behaviorSha256: scenario.behavior_sha256,
            runs: scenario.runs ?? [],
          })),
        )

  for (const scenario of scenarios) {
    if (subjectId && scenario.subjectId !== subjectId) continue
    if (scenario.scenarioId !== scenarioId) continue
    if (
      behaviorSha256 !== undefined &&
      (scenario.behaviorSha256 ?? null) !== behaviorSha256
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
          subjectId: scenario.subjectId,
          runId: nonEmpty(attempt.run_id) ?? nonEmpty(run.run_id) ?? '',
          attemptId:
            nonEmpty(attempt.attempt_id) ?? nonEmpty(run.attempt_id) ?? '',
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

  return targets
}

export async function loadScenarioChatTargets({
  executionId,
  scenarioId,
  behaviorSha256,
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
    behaviorSha256,
  )
}
