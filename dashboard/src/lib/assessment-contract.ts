export type AssessmentKind = 'required_check' | 'signal' | 'asset_validation'

export type AssessmentPolicy = 'hard_gate' | 'advisory'
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

export type EvidenceReference = {
  artifact_id: string
  artifact_sha256: string
  locator?: string
}

/** Identity of a Markdown-scenario analyzer: the instruction-adherence pass
 *  and the opt-in transcript audit are the only producers left. */
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
  target: { kind: 'criterion'; id: string }
  kind: AssessmentKind
  policy: AssessmentPolicy
  dimension:
    | 'deliverable'
    | 'structural_integrity'
    | 'efficiency'
    | 'robustness'
    | 'e2e_infrastructure'
  outcome: AssessmentOutcome
  score?: { awarded: number; possible: number }
  summary: string
  evidence?: EvidenceReference[]
}

export type AssetValidationOutcome =
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

export type AssetAssessmentResult = {
  asset_id: string
  outcome: AssetValidationOutcome
  summary: string
  evidence?: EvidenceReference[]
}

export type RunAssessmentContract = {
  run_id: string
  attempt_id: string
  system_status: SystemStatus
  assessments?: AssessmentResult[]
  assets?: AssetAssessmentResult[]
}

export type AssessmentContract = {
  runs: RunAssessmentContract[]
}

export type AssessmentSummary = {
  run_count: number
  assessment_count: number
  asset_count: number
  evidence_reference_count: number
  system_statuses: Record<SystemStatus, number>
  assessment_outcomes: Record<AssessmentOutcome, number>
  asset_validation_outcomes: Record<AssetValidationOutcome, number>
}

export function summarizeAssessmentContract(
  contract: AssessmentContract,
): AssessmentSummary {
  const summary = emptyAssessmentSummary()
  const evidence = new Set<string>()
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
    for (const assessment of run.assessments ?? []) {
      summary.assessment_count += 1
      summary.assessment_outcomes[assessment.outcome] += 1
      remember(assessment.evidence)
    }
    for (const asset of run.assets ?? []) {
      summary.asset_count += 1
      summary.asset_validation_outcomes[asset.outcome] += 1
      remember(asset.evidence)
    }
  }
  summary.evidence_reference_count = evidence.size
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
  }
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
    const identity = `${runId}\0${attemptId}`
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
