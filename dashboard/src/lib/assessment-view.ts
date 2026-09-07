import type {
  AssessmentKind,
  AssessmentOutcome,
  AssessmentPolicy,
  AssessmentResult,
  AssetValidationOutcome,
  EvidenceReference,
  RunAssessmentContract,
  SystemStatus,
} from '@/lib/assessment-contract'
import type {
  DashboardExecutionDetail,
  DashboardRunProjection,
} from '@/lib/dashboard-data-source'

export type AssessmentFilter = 'all' | 'failed' | 'unavailable' | 'asset'

export type AssessmentEntry = {
  id: string
  criterionId: string
  targetId: string
  kind: AssessmentKind
  /** `objective` marks the deterministic asset validations, which carry no
   *  scoring policy of their own. */
  policy: AssessmentPolicy | 'objective'
  dimension: AssessmentResult['dimension']
  outcome: AssessmentOutcome
  validationOutcome?: AssetValidationOutcome
  score?: AssessmentResult['score']
  summary: string
  evidence: EvidenceReference[]
}

export type AssessmentRunView = {
  key: string
  subjectId: string
  scenarioId: string
  scenarioVersion: number
  runId: string
  attemptId: string
  metrics: AssessmentRunMetrics
  transcript?: { messages?: unknown }
  systemStatus: SystemStatus
  assessments: AssessmentEntry[]
  evidence: EvidenceReference[]
}

export type AssessmentRunMetrics = {
  totalTokens: number | null
  inputTokens: number | null
  outputTokens: number | null
  cacheReadTokens: number | null
  cacheWriteTokens: number | null
  reasoningTokens: number | null
  functionCalls: number | null
  functionCallErrors: number | null
  durationMs: number | null
  sessions: number | null
  turns: number | null
}

export type AssessmentAggregateMetrics = AssessmentRunMetrics

export type AssessmentWorkspaceModel = {
  availability: 'available' | 'unavailable'
  runs: AssessmentRunView[]
}

function sumRunMetric(
  runs: AssessmentRunView[],
  key: keyof AssessmentRunMetrics,
): number | null {
  let total = 0
  let reported = 0
  for (const run of runs) {
    const value = run.metrics[key]
    if (typeof value !== 'number' || !Number.isFinite(value)) continue
    total += value
    reported += 1
  }
  return reported > 0 ? total : null
}

export function aggregateAssessmentMetrics(
  runs: AssessmentRunView[],
): AssessmentAggregateMetrics {
  return {
    totalTokens: sumRunMetric(runs, 'totalTokens'),
    inputTokens: sumRunMetric(runs, 'inputTokens'),
    outputTokens: sumRunMetric(runs, 'outputTokens'),
    cacheReadTokens: sumRunMetric(runs, 'cacheReadTokens'),
    cacheWriteTokens: sumRunMetric(runs, 'cacheWriteTokens'),
    reasoningTokens: sumRunMetric(runs, 'reasoningTokens'),
    functionCalls: sumRunMetric(runs, 'functionCalls'),
    functionCallErrors: sumRunMetric(runs, 'functionCallErrors'),
    durationMs: sumRunMetric(runs, 'durationMs'),
    sessions: sumRunMetric(runs, 'sessions'),
    turns: sumRunMetric(runs, 'turns'),
  }
}

const FAILED_ASSET_OUTCOMES = new Set([
  'invalid',
  'malformed',
  'oversized',
  'not_produced',
  'unreadable',
  'unsafe_path',
  'removed_during_cleanup',
  'unexpected',
])

export function buildAssessmentWorkspace(
  detail: DashboardExecutionDetail | null | undefined,
): AssessmentWorkspaceModel {
  if (!detail) return { availability: 'unavailable', runs: [] }
  const runs: AssessmentRunView[] = []

  for (const record of detail.reports ?? []) {
    if (!record.available || !record.report) continue
    for (const scenario of record.report.scenarios ?? []) {
      for (const projectedRun of scenario.runs ?? []) {
        const contract = projectedRun.assessment
        if (!contract) continue
        runs.push(
          assessmentRunView(
            record.subject_id,
            scenario.scenario_id,
            scenario.scenario_version,
            contract,
            projectedRun,
            projectedRun.transcript,
          ),
        )
      }
    }
  }

  if (runs.length > 0) {
    runs.sort(
      (left, right) =>
        assessmentRunPriority(left) - assessmentRunPriority(right),
    )
    return { availability: 'available', runs }
  }
  return { availability: 'unavailable', runs: [] }
}

function assessmentRunPriority(run: AssessmentRunView) {
  if (run.systemStatus === 'infrastructure_error') return 0
  if (run.systemStatus === 'resource_limit') return 0
  if (
    run.systemStatus === 'subject_error' ||
    run.systemStatus === 'judge_error'
  )
    return 1
  if (run.systemStatus === 'hard_gate_failed') return 2
  if (run.systemStatus === 'unavailable') return 3
  return 4
}

/**
 * The visible next-step guidance, scoped to the harness and the scenario: it
 * names the boundary that broke, never the product quality of the subject.
 */
export function buildHarnessRecommendation(run: AssessmentRunView): string {
  const failedAsset = run.assessments.some(
    (entry) =>
      entry.kind === 'asset_validation' && entry.validationOutcome !== 'valid',
  )

  if (run.systemStatus === 'infrastructure_error' || failedAsset) {
    return 'Fix the harness collection or serialization path, validate every expected artifact against its schema before assessment, and rerun the scenario.'
  }
  if (run.systemStatus === 'resource_limit') {
    return 'Reduce the scenario resource footprint or adjust its execution budget, verify collection completes within the limit, and rerun the scenario.'
  }
  if (run.systemStatus === 'subject_error') {
    return 'Fix the subject execution or transport path, confirm a complete response is captured, and rerun the scenario.'
  }
  if (run.systemStatus === 'judge_error') {
    return 'Fix the Markdown validator invocation or its schema path, validate the JSON contract, and rerun the scenario.'
  }
  if (run.systemStatus === 'hard_gate_failed') {
    return 'Fix the scenario or fixture that violates the hard gate, add a regression assertion for that condition, and rerun the scenario.'
  }
  if (run.systemStatus === 'unavailable') {
    return 'Restore the missing report or assessment contract, add a readiness check, and rerun the scenario.'
  }
  return 'Repeat a comparable scenario to confirm harness stability before expanding test coverage.'
}

function assessmentRunView(
  subjectId: string,
  scenarioId: string,
  scenarioVersion: number,
  contract: RunAssessmentContract,
  projectedRun: DashboardRunProjection,
  transcript?: { messages?: unknown },
): AssessmentRunView {
  const assessments = (contract.assessments ?? []).map((assessment) =>
    assessmentEntry(assessment),
  )
  for (const asset of contract.assets ?? []) {
    assessments.push({
      id: `asset-validation:${asset.asset_id}`,
      criterionId: `asset:${asset.asset_id}`,
      targetId: asset.asset_id,
      kind: 'asset_validation',
      policy: 'objective',
      dimension: 'structural_integrity',
      outcome:
        asset.outcome === 'valid'
          ? 'passed'
          : asset.outcome === 'not_evaluated'
            ? 'not_evaluated'
            : 'failed',
      validationOutcome: asset.outcome,
      summary: asset.summary,
      evidence: asset.evidence ?? [],
    })
  }

  const evidence = uniqueEvidence(
    assessments.flatMap((assessment) => assessment.evidence),
  )

  return {
    key: `${subjectId}:${scenarioId}:${contract.run_id}:${contract.attempt_id}`,
    subjectId,
    scenarioId,
    scenarioVersion,
    runId: contract.run_id,
    attemptId: contract.attempt_id,
    metrics: assessmentRunMetrics(projectedRun),
    ...(transcript ? { transcript } : {}),
    systemStatus: contract.system_status,
    assessments,
    evidence,
  }
}

function finiteNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function assessmentRunMetrics(
  projectedRun: DashboardRunProjection,
): AssessmentRunMetrics {
  const totals = projectedRun.metrics?.totals
  const efficiency = projectedRun.efficiency
  const inputTokens = finiteNumber(totals?.input_tokens)
  const outputTokens = finiteNumber(totals?.output_tokens)
  return {
    totalTokens:
      finiteNumber(efficiency?.total_tokens) ??
      (inputTokens !== null && outputTokens !== null
        ? inputTokens + outputTokens
        : null),
    inputTokens,
    outputTokens,
    cacheReadTokens: finiteNumber(totals?.cache_read_tokens),
    cacheWriteTokens: finiteNumber(totals?.cache_write_tokens),
    reasoningTokens: finiteNumber(totals?.reasoning_tokens),
    functionCalls:
      finiteNumber(totals?.function_calls) ??
      finiteNumber(efficiency?.function_calls),
    functionCallErrors:
      finiteNumber(totals?.function_call_errors) ??
      finiteNumber(efficiency?.function_call_errors),
    durationMs:
      finiteNumber(projectedRun.wall_time_ms) ??
      finiteNumber(efficiency?.wall_time_ms),
    sessions:
      finiteNumber(totals?.sessions) ?? finiteNumber(efficiency?.sessions),
    turns: finiteNumber(totals?.turns) ?? finiteNumber(efficiency?.turns),
  }
}

function assessmentEntry(assessment: AssessmentResult): AssessmentEntry {
  return {
    id: `assessment:${assessment.criterion_id}:${assessment.target.kind}:${assessment.target.id}`,
    criterionId: assessment.criterion_id,
    targetId: assessment.target.id,
    kind: assessment.kind,
    policy: assessment.policy,
    dimension: assessment.dimension,
    outcome: assessment.outcome,
    score: assessment.score,
    summary: assessment.summary,
    evidence: assessment.evidence ?? [],
  }
}

function uniqueEvidence(references: EvidenceReference[]) {
  const unique = new Map<string, EvidenceReference>()
  for (const reference of references) {
    const key = `${reference.artifact_id}\0${reference.artifact_sha256}\0${reference.locator ?? ''}`
    unique.set(key, reference)
  }
  return [...unique.values()]
}

export function matchesAssessmentFilter(
  entry: AssessmentEntry,
  filter: AssessmentFilter,
) {
  if (filter === 'all') return true
  if (filter === 'failed') {
    return (
      entry.outcome === 'failed' ||
      entry.outcome === 'error' ||
      (entry.validationOutcome != null &&
        FAILED_ASSET_OUTCOMES.has(entry.validationOutcome))
    )
  }
  if (filter === 'unavailable') {
    return (
      entry.outcome === 'unavailable' ||
      entry.outcome === 'not_evaluated' ||
      entry.outcome === 'error'
    )
  }
  return entry.kind === 'asset_validation'
}

export function assessmentFilterCounts(runs: AssessmentRunView[]) {
  const entries = runs.flatMap((run) => run.assessments)
  return {
    all: entries.length,
    failed: entries.filter((entry) => matchesAssessmentFilter(entry, 'failed'))
      .length,
    unavailable: entries.filter((entry) =>
      matchesAssessmentFilter(entry, 'unavailable'),
    ).length,
    asset: entries.filter((entry) => matchesAssessmentFilter(entry, 'asset'))
      .length,
  } satisfies Record<AssessmentFilter, number>
}
