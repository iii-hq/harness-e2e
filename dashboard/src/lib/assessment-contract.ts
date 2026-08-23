export type AssessmentKind =
  | 'required_check'
  | 'signal'
  | 'asset_validation'
  | 'asset_quality'

export type AssessmentPolicy = 'hard_gate' | 'advisory'
export type AssessmentSource = 'deterministic' | 'judge' | 'asset_analyzer'
export type AssessmentOutcome =
  | 'passed'
  | 'failed'
  | 'partial'
  | 'not_evaluated'
  | 'unavailable'
  | 'error'

export type SystemStatus =
  | 'unavailable'
  | 'passed'
  | 'hard_gate_failed'
  | 'subject_error'
  | 'judge_error'
  | 'resource_limit'
  | 'infrastructure_error'

export type EffectiveStatus = SystemStatus | 'passed_with_concerns'

export type EvidenceReference = {
  artifact_id: string
  artifact_sha256: string
  locator?: string
}

export type AnalyzerIdentity = {
  analyzer: string
  provider?: string
  model?: string
  input_sha256: string
}

export type AnalyzerUsage = {
  latency_ms?: number
  input_tokens?: number
  output_tokens?: number
  cost_usd?: number
}

export type AssessmentResult = {
  criterion_id: string
  target: { kind: 'criterion' | 'asset'; id: string }
  kind: AssessmentKind
  policy: AssessmentPolicy
  dimension:
    | 'deliverable'
    | 'structural_integrity'
    | 'efficiency'
    | 'robustness'
    | 'e2e_infrastructure'
  source: AssessmentSource
  outcome: AssessmentOutcome
  score?: { awarded: number; possible: number }
  confidence?: number
  summary: string
  evidence?: EvidenceReference[]
  analyzer?: AnalyzerIdentity
  analyzer_usage?: AnalyzerUsage
}

export type AssetAssessmentResult = {
  validation: {
    asset_id: string
    outcome:
      | 'valid'
      | 'invalid'
      | 'malformed'
      | 'oversized'
      | 'not_produced'
      | 'unreadable'
      | 'unsafe_path'
      | 'removed_during_cleanup'
      | 'unexpected'
      | 'not_evaluated'
    summary: string
    evidence?: EvidenceReference[]
  }
  qualitative_assessment: AssessmentResult
}

export type AiFinalAssessment = {
  availability:
    | 'not_requested'
    | 'not_evaluated'
    | 'available'
    | 'unavailable'
    | 'malformed'
    | 'failed'
  result?: {
    verdict: 'pass' | 'pass_with_concerns' | 'fail' | 'inconclusive'
    quality_score: number
    confidence: number
    summary: string
    facts: string[]
    strengths?: string[]
    concerns?: string[]
    diagnosis?: string
    recommendation: string
    limitations?: string[]
    evidence?: EvidenceReference[]
  }
  analyzer?: AnalyzerIdentity
  analyzer_usage?: AnalyzerUsage
  reason?: string
}

export type RunAssessmentContract = {
  run_id: string
  attempt_id: string
  system_status: SystemStatus
  assessments?: AssessmentResult[]
  assets?: AssetAssessmentResult[]
  ai_final_assessment: AiFinalAssessment
  effective_status: EffectiveStatus
}

export type AssessmentContract = {
  runs: RunAssessmentContract[]
}

export type AssessmentSummary = {
  run_count: number
  assessment_count: number
  asset_count: number
  evidence_reference_count: number
  system_statuses: Record<EffectiveStatus, number>
  effective_statuses: Record<EffectiveStatus, number>
  assessment_outcomes: Record<AssessmentOutcome, number>
  asset_qualitative_outcomes: Record<AssessmentOutcome, number>
  asset_validation_outcomes: Record<
    AssetAssessmentResult['validation']['outcome'],
    number
  >
  ai_availability: Record<AiFinalAssessment['availability'], number>
  ai_verdicts: Record<
    NonNullable<AiFinalAssessment['result']>['verdict'],
    number
  >
  median_quality_score: number | null
  median_confidence: number | null
}

export function summarizeAssessmentContract(
  contract: AssessmentContract,
): AssessmentSummary {
  const summary = emptyAssessmentSummary()
  const evidence = new Set<string>()
  const qualityScores: number[] = []
  const confidence: number[] = []
  const remember = (references: EvidenceReference[] | undefined) => {
    for (const reference of references ?? []) {
      evidence.add(
        `${reference.artifact_id}\0${reference.artifact_sha256}\0${reference.locator ?? ''}`,
      )
    }
  }

  for (const run of contract.runs) {
    summary.run_count += 1
    summary.system_statuses[run.system_status] += 1
    summary.effective_statuses[run.effective_status] += 1
    summary.ai_availability[run.ai_final_assessment.availability] += 1
    for (const assessment of run.assessments ?? []) {
      summary.assessment_count += 1
      summary.assessment_outcomes[assessment.outcome] += 1
      remember(assessment.evidence)
    }
    for (const asset of run.assets ?? []) {
      summary.asset_count += 1
      summary.asset_validation_outcomes[asset.validation.outcome] += 1
      summary.asset_qualitative_outcomes[
        asset.qualitative_assessment.outcome
      ] += 1
      remember(asset.validation.evidence)
      remember(asset.qualitative_assessment.evidence)
    }
    const result = run.ai_final_assessment.result
    if (result) {
      summary.ai_verdicts[result.verdict] += 1
      qualityScores.push(result.quality_score)
      confidence.push(result.confidence)
      remember(result.evidence)
    }
  }
  summary.evidence_reference_count = evidence.size
  summary.median_quality_score = median(qualityScores)
  summary.median_confidence = median(confidence)
  return summary
}

function emptyAssessmentSummary(): AssessmentSummary {
  return {
    run_count: 0,
    assessment_count: 0,
    asset_count: 0,
    evidence_reference_count: 0,
    system_statuses: {
      unavailable: 0,
      passed: 0,
      passed_with_concerns: 0,
      hard_gate_failed: 0,
      subject_error: 0,
      judge_error: 0,
      resource_limit: 0,
      infrastructure_error: 0,
    },
    effective_statuses: {
      unavailable: 0,
      passed: 0,
      passed_with_concerns: 0,
      hard_gate_failed: 0,
      subject_error: 0,
      judge_error: 0,
      resource_limit: 0,
      infrastructure_error: 0,
    },
    assessment_outcomes: {
      passed: 0,
      failed: 0,
      partial: 0,
      not_evaluated: 0,
      unavailable: 0,
      error: 0,
    },
    asset_qualitative_outcomes: {
      passed: 0,
      failed: 0,
      partial: 0,
      not_evaluated: 0,
      unavailable: 0,
      error: 0,
    },
    asset_validation_outcomes: {
      valid: 0,
      invalid: 0,
      malformed: 0,
      oversized: 0,
      not_produced: 0,
      unreadable: 0,
      unsafe_path: 0,
      removed_during_cleanup: 0,
      unexpected: 0,
      not_evaluated: 0,
    },
    ai_availability: {
      not_requested: 0,
      not_evaluated: 0,
      available: 0,
      unavailable: 0,
      malformed: 0,
      failed: 0,
    },
    ai_verdicts: {
      pass: 0,
      pass_with_concerns: 0,
      fail: 0,
      inconclusive: 0,
    },
    median_quality_score: null,
    median_confidence: null,
  }
}

function median(values: number[]) {
  if (values.length === 0) return null
  const ordered = [...values].sort((left, right) => left - right)
  const midpoint = Math.floor(ordered.length / 2)
  return ordered.length % 2 === 0
    ? (ordered[midpoint - 1] + ordered[midpoint]) / 2
    : ordered[midpoint]
}

export type AnalysisBundle = {
  scope: 'execution' | 'test' | 'comparison'
  input_sha256: string
  subjects: Array<{
    execution_id: string
    run_id: string
    attempt_id: string
    scenario_id: string
    scenario_version: number
    case_id: string
    system_status: SystemStatus
    effective_status: EffectiveStatus
  }>
  assessments?: AssessmentResult[]
  assets?: AssetAssessmentResult[]
  dimensions?: unknown[]
  failures?: unknown[]
  evidence?: unknown[]
  metrics?: Array<{ id: string; value: number; unit: string }>
  excerpts?: Array<{
    kind: string
    summary: string
    evidence: EvidenceReference
  }>
  limitations?: string[]
}

export type AnalysisResponse = {
  input_sha256: string
  analyzer: AnalyzerIdentity
  facts?: Array<{ summary: string; evidence: EvidenceReference[] }>
  interpretations?: Array<{
    summary: string
    confidence: number
    evidence: EvidenceReference[]
  }>
  opportunities?: Array<{
    priority: number
    summary: string
    expected_impact: string
    validation_method: string
    evidence: EvidenceReference[]
  }>
  limitations?: Array<{ summary: string; evidence?: EvidenceReference[] }>
}

export class AssessmentContractError extends Error {}

export function readAssessmentContract(result: unknown): AssessmentContract {
  if (!isRecord(result)) {
    throw new AssessmentContractError('E2E result must be an object')
  }
  if ('schema_version' in result) {
    throw new AssessmentContractError(
      'versioned E2E payloads are not supported',
    )
  }
  const contract = result.assessment_contract
  if (!isRecord(contract)) {
    throw new AssessmentContractError('results require assessment_contract')
  }
  if ('contract_version' in contract) {
    throw new AssessmentContractError(
      'versioned assessment contracts are not supported',
    )
  }
  if (!Array.isArray(contract.runs)) {
    throw new AssessmentContractError(
      'assessment_contract.runs must be an array',
    )
  }
  validateRunIdentities(contract.runs)
  return contract as unknown as AssessmentContract
}

function validateRunIdentities(runs: unknown[]) {
  const seen = new Set<string>()
  for (const run of runs) {
    if (!isRecord(run)) {
      throw new AssessmentContractError(
        'assessment contract run must be an object',
      )
    }
    const runId = nonemptyString(run.run_id)
    const attemptId = nonemptyString(run.attempt_id)
    if (!runId || !attemptId) {
      throw new AssessmentContractError(
        'assessment contract run_id and attempt_id are required',
      )
    }
    const identity = `${runId}\u0000${attemptId}`
    if (seen.has(identity)) {
      throw new AssessmentContractError(
        'assessment contract repeats a run identity',
      )
    }
    seen.add(identity)
  }
}

function nonemptyString(value: unknown) {
  return typeof value === 'string' && value.trim() ? value : undefined
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}
