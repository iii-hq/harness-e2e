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

function sameOptional(
  left: string | null | undefined,
  right: string | null | undefined,
  label: string,
  reasons: string[],
) {
  if ((left ?? null) !== (right ?? null)) reasons.push(`${label} differs`)
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
  return [
    observation.execution_id,
    observation.case_id,
    observation.contract_sha256,
    observation.scenario_version ?? 'unknown-version',
    observation.seed ?? 'unknown-seed',
  ].join('::')
}

/**
 * Compare two retained observations without pooling them. A different system
 * revision is deliberate: that is the change under inspection. The test
 * contract, case, seed, cohort, and assessment protocol must still match.
 */
export function compareTestObservations(
  baseline: TestObservation,
  candidate: TestObservation,
): ObservationComparison {
  const reasons: string[] = []
  sameRequired(
    baseline.scenario_version,
    candidate.scenario_version,
    'Test version',
    reasons,
  )
  sameRequired(baseline.case_id, candidate.case_id, 'Case', reasons)
  sameRequired(baseline.seed, candidate.seed, 'Seed', reasons)
  sameRequired(
    baseline.contract_sha256,
    candidate.contract_sha256,
    'Scenario contract',
    reasons,
  )
  sameRequired(
    baseline.assessment_profile_sha256,
    candidate.assessment_profile_sha256,
    'Assessment profile',
    reasons,
  )
  sameRequired(
    baseline.analyzer_profile_sha256,
    candidate.analyzer_profile_sha256,
    'Analyzer profile',
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
  sameOptional(
    baseline.judge_provider,
    candidate.judge_provider,
    'Judge provider',
    reasons,
  )
  sameOptional(
    baseline.judge_model,
    candidate.judge_model,
    'Judge model',
    reasons,
  )
  sameOptional(
    baseline.judge_protocol,
    candidate.judge_protocol,
    'Judge protocol',
    reasons,
  )

  return {
    baseline,
    candidate,
    compatible: reasons.length === 0,
    reasons,
    metrics: {
      score: compareMetric(baseline.median_score, candidate.median_score),
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
