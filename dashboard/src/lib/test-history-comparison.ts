import type { TestObservation } from '@/lib/test-catalog'

export type ComparedMetric = {
  baseline: number | null
  candidate: number | null
  delta: number | null
  relativeDelta: number | null
}

export type ObservationComparison = {
  baseline: TestObservation
  candidate: TestObservation
  compatible: boolean
  reasons: string[]
  metrics: {
    score: ComparedMetric
    cost: ComparedMetric
    duration: ComparedMetric
    tokens: ComparedMetric
    functionCalls: ComparedMetric
    functionErrors: ComparedMetric
    turns: ComparedMetric
  }
}

function present(value: string | number | null | undefined) {
  return value !== null && value !== undefined && value !== ''
}

function sameRequired(
  left: string | number | null | undefined,
  right: string | number | null | undefined,
  label: string,
  reasons: string[],
) {
  if (!present(left) || !present(right)) {
    reasons.push(`${label} is not recorded on both executions`)
  } else if (left !== right) {
    reasons.push(`${label} differs`)
  }
}

function asMetric(value: number | null | undefined) {
  return value !== null && value !== undefined && Number.isFinite(value)
    ? value
    : null
}

function compareMetric(
  baselineValue: number | null | undefined,
  candidateValue: number | null | undefined,
): ComparedMetric {
  const baseline = asMetric(baselineValue)
  const candidate = asMetric(candidateValue)
  if (baseline === null || candidate === null) {
    return { baseline, candidate, delta: null, relativeDelta: null }
  }
  const delta = candidate - baseline
  return {
    baseline,
    candidate,
    delta,
    relativeDelta: baseline === 0 ? null : delta / Math.abs(baseline),
  }
}

export function testObservationKey(observation: TestObservation) {
  const key = [
    observation.execution_id,
    observation.case_id,
    observation.contract_sha256,
    observation.behavior_sha256,
    observation.seed ?? 'unknown-seed',
  ].join('::')
  return observation.observation_id
    ? `${key}::${observation.observation_id}`
    : key
}

/**
 * The scenario the two observations answer to. The retained contract digest
 * settles it; an observation that only carries the definition digest is
 * settled by that instead. Neither on both sides means no delta, not an error.
 */
function sameScenario(
  baseline: TestObservation,
  candidate: TestObservation,
  reasons: string[],
) {
  if (present(baseline.contract_sha256) && present(candidate.contract_sha256)) {
    if (baseline.contract_sha256 !== candidate.contract_sha256)
      reasons.push('Scenario contract differs')
    return
  }
  if (present(baseline.behavior_sha256) && present(candidate.behavior_sha256)) {
    if (baseline.behavior_sha256 !== candidate.behavior_sha256)
      reasons.push('Scenario definition differs')
    return
  }
  reasons.push('Scenario definition is not recorded on both executions')
}

/**
 * Compare two retained observations without pooling them. A different system
 * revision is deliberate: that is the change under inspection. The scenario
 * definition, case, seed, cohort, and assessment protocol must still match.
 */
export function compareTestObservations(
  baseline: TestObservation,
  candidate: TestObservation,
): ObservationComparison {
  const reasons: string[] = []
  sameRequired(baseline.case_id, candidate.case_id, 'Case', reasons)
  sameRequired(baseline.seed, candidate.seed, 'Seed', reasons)
  sameScenario(baseline, candidate, reasons)
  sameRequired(
    baseline.assessment_profile_sha256,
    candidate.assessment_profile_sha256,
    'Assessment profile',
    reasons,
  )
  sameRequired(baseline.cohort_id, candidate.cohort_id, 'Cohort', reasons)
  sameRequired(
    baseline.stack_mode,
    candidate.stack_mode,
    'Execution stack',
    reasons,
  )
  sameRequired(
    baseline.subject_provider,
    candidate.subject_provider,
    'Execution provider',
    reasons,
  )
  sameRequired(
    baseline.subject_model,
    candidate.subject_model,
    'Execution model',
    reasons,
  )

  return {
    baseline,
    candidate,
    compatible: reasons.length === 0,
    reasons,
    metrics: {
      score: compareMetric(baseline.mean_score, candidate.mean_score),
      cost: compareMetric(baseline.median_cost_usd, candidate.median_cost_usd),
      duration: compareMetric(
        baseline.median_duration_seconds,
        candidate.median_duration_seconds,
      ),
      tokens: compareMetric(baseline.median_tokens, candidate.median_tokens),
      functionCalls: compareMetric(
        baseline.median_function_calls,
        candidate.median_function_calls,
      ),
      functionErrors: compareMetric(
        baseline.median_function_call_errors,
        candidate.median_function_call_errors,
      ),
      turns: compareMetric(baseline.median_turns, candidate.median_turns),
    },
  }
}
