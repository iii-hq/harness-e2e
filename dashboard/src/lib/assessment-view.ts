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
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'

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
  systemStatus: SystemStatus
  effectiveStatus: EffectiveStatus
  assessments: AssessmentEntry[]
  finalAssessment: AiFinalAssessment
  evidence: EvidenceReference[]
  hasAiDisagreement: boolean
}

export type AssessmentWorkspaceModel = {
  availability: 'available' | 'unavailable' | 'legacy'
  runs: AssessmentRunView[]
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
          ),
        )
      }
    }
  }

  if (runs.length > 0) return { availability: 'available', runs }
  return { availability: unavailable ? 'unavailable' : 'legacy', runs: [] }
}

function assessmentRunView(
  subjectId: string,
  scenarioId: string,
  scenarioVersion: number,
  contract: RunAssessmentContract,
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
