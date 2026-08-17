import type {
  AiFinalAssessment,
  AnalyzerIdentity,
  AnalyzerUsage,
  AssessmentKind,
  AssessmentOutcome,
  AssessmentPolicy,
  AssessmentResult,
  AssessmentSource,
  EffectiveStatus,
  EvidenceReference,
  RunAssessmentContract,
  SystemStatus,
} from '@/lib/assessment-contract'
import type {
  DashboardExecutionDetail,
  DashboardRunProjection,
} from '@/lib/dashboard-data-source'

export type AssessmentFilter =
  | 'all'
  | 'failed'
  | 'low_confidence'
  | 'unavailable'
  | 'asset'
  | 'ai'

export type AssessmentEntry = {
  id: string
  criterionId: string
  targetId: string
  kind: AssessmentKind
  policy: AssessmentPolicy | 'objective'
  dimension: AssessmentResult['dimension']
  source: AssessmentSource | 'deterministic_asset_validation'
  outcome: AssessmentOutcome
  validationOutcome?: string
  score?: AssessmentResult['score']
  confidence?: number
  summary: string
  evidence: EvidenceReference[]
  analyzer?: AnalyzerIdentity
  analyzerUsage?: AnalyzerUsage
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
  effectiveStatus: EffectiveStatus
  assessments: AssessmentEntry[]
  finalAssessment: AiFinalAssessment
  evidence: EvidenceReference[]
  hasAiDisagreement: boolean
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
  availability: 'available' | 'unavailable' | 'legacy'
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

export const LOW_CONFIDENCE_THRESHOLD = 0.75

export function buildAssessmentWorkspace(
  detail: DashboardExecutionDetail | null | undefined,
): AssessmentWorkspaceModel {
  if (!detail) return { availability: 'legacy', runs: [] }
  const runs: AssessmentRunView[] = []
  let unavailable = false

  for (const record of detail.reports ?? []) {
    if (!record.available || !record.report) continue
    if (record.report.assessment_availability === 'unavailable') {
      unavailable = true
    }
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
  return { availability: unavailable ? 'unavailable' : 'legacy', runs: [] }
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
  if (run.hasAiDisagreement) return 3
  if (run.systemStatus === 'unavailable') return 3
  return 4
}

/**
 * Keep the visible next-step guidance scoped to the harness and the scenario.
 * The persisted AI recommendation remains raw evidence; this presentation
 * model prevents it from becoming a release or product-quality instruction.
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
    return 'Fix the subject execution or transport path, confirm a complete response is captured, and rerun the scenario before judging quality.'
  }
  if (run.systemStatus === 'judge_error') {
    return 'Fix the judge invocation or assessment-schema path, validate the JSON contract, and rerun the scenario.'
  }
  if (run.systemStatus === 'hard_gate_failed') {
    return 'Fix the scenario or fixture that violates the hard gate, add a regression assertion for that condition, and rerun the scenario.'
  }
  if (run.systemStatus === 'unavailable') {
    return 'Restore the missing report or assessment contract, add a readiness check, and rerun the scenario.'
  }
  if (run.hasAiDisagreement) {
    return 'Keep objective gates as the authority, review the assessment input if the disagreement persists, and rerun a comparable scenario.'
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
    const validationOutcome = asset.validation.outcome
    assessments.push({
      id: `asset-validation:${asset.validation.asset_id}`,
      criterionId: `asset:${asset.validation.asset_id}`,
      targetId: asset.validation.asset_id,
      kind: 'asset_validation',
      policy: 'objective',
      dimension: 'structural_integrity',
      source: 'deterministic_asset_validation',
      outcome:
        validationOutcome === 'valid'
          ? 'passed'
          : validationOutcome === 'not_evaluated'
            ? 'not_evaluated'
            : 'failed',
      validationOutcome,
      summary: asset.validation.summary,
      evidence: asset.validation.evidence ?? [],
    })
    assessments.push(
      assessmentEntry(asset.qualitative_assessment, 'asset-quality'),
    )
  }

  const evidence = uniqueEvidence([
    ...assessments.flatMap((assessment) => assessment.evidence),
    ...(contract.ai_final_assessment.result?.evidence ?? []),
  ])
  const verdict = contract.ai_final_assessment.result?.verdict
  const objectiveFailure = contract.system_status !== 'passed'
  const positiveAi = verdict === 'pass' || verdict === 'pass_with_concerns'

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
    effectiveStatus: contract.effective_status,
    assessments,
    finalAssessment: contract.ai_final_assessment,
    evidence,
    hasAiDisagreement:
      (objectiveFailure && positiveAi) ||
      (!objectiveFailure && verdict === 'fail'),
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

function assessmentEntry(
  assessment: AssessmentResult,
  prefix = 'assessment',
): AssessmentEntry {
  return {
    id: `${prefix}:${assessment.criterion_id}:${assessment.target.kind}:${assessment.target.id}`,
    criterionId: assessment.criterion_id,
    targetId: assessment.target.id,
    kind: assessment.kind,
    policy: assessment.policy,
    dimension: assessment.dimension,
    source: assessment.source,
    outcome: assessment.outcome,
    score: assessment.score,
    confidence: assessment.confidence,
    summary: assessment.summary,
    evidence: assessment.evidence ?? [],
    analyzer: assessment.analyzer,
    analyzerUsage: assessment.analyzer_usage,
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
  if (filter === 'low_confidence') {
    return (
      entry.confidence != null && entry.confidence < LOW_CONFIDENCE_THRESHOLD
    )
  }
  if (filter === 'unavailable') {
    return (
      entry.outcome === 'unavailable' ||
      entry.outcome === 'not_evaluated' ||
      entry.outcome === 'error'
    )
  }
  if (filter === 'asset') {
    return entry.kind === 'asset_validation' || entry.kind === 'asset_quality'
  }
  return entry.source === 'judge' || entry.source === 'asset_analyzer'
}

export function assessmentFilterCounts(runs: AssessmentRunView[]) {
  const entries = runs.flatMap((run) => run.assessments)
  return {
    all: entries.length,
    failed: entries.filter((entry) => matchesAssessmentFilter(entry, 'failed'))
      .length,
    low_confidence: entries.filter((entry) =>
      matchesAssessmentFilter(entry, 'low_confidence'),
    ).length,
    unavailable: entries.filter((entry) =>
      matchesAssessmentFilter(entry, 'unavailable'),
    ).length,
    asset: entries.filter((entry) => matchesAssessmentFilter(entry, 'asset'))
      .length,
    ai: entries.filter((entry) => matchesAssessmentFilter(entry, 'ai')).length,
  } satisfies Record<AssessmentFilter, number>
}
