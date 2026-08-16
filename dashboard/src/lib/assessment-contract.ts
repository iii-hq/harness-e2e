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
    strengths?: string[]
    concerns?: string[]
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
