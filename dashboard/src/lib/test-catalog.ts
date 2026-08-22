import type { AssessmentSummary } from '@/lib/assessment-contract'

export type CohortDescriptor = {
  id: string
  lane: string
  subject_provider: string
  subject_model: string
  judge_provider: string | null
  judge_model: string | null
  judge_protocol: string | null
}

export type EvaluatedVersion = {
  id: string
  cohort_id: string
  label: string
  stack_mode: 'source' | 'registry'
  completed_at: string
  execution_count: number
}

export type EvaluatedVersionsResponse = {
  revision: string
  cohorts: CohortDescriptor[]
  versions: EvaluatedVersion[]
}

export type TestSideSummary = {
  evaluated_version_id: string
  execution_count: number
  total_runs: number
  scored_runs: number
  case_count: number
  median_score: number | null
  pass_rate: number | null
  median_cost_usd: number | null
  median_tokens: number | null
  median_duration_seconds: number | null
  outcomes: {
    passed: number
    hard_gate_failed: number
    technical_failed: number
    infra_failed: number
  }
  samples: {
    score: number
    cost_usd: number
    tokens: number
    duration_seconds: number
    turns?: number
  }
  assessment_summary?: AssessmentSummary
}

export type TestHistoryInput = {
  test_id: string
  test_version?: number
  case_id?: string
  subject_provider?: string
  subject_model?: string
  judge_provider?: string
  judge_model?: string
  system_version_id?: string
  result?: string
  cursor?: string
  limit?: number
}

export type HistorySeries = {
  id: string
  case_id: string
  scenario_version: number
  seed: number | null
  contract_sha256: string
  assessment_profile_sha256: string
  analyzer_profile_sha256: string
  system_version_id: string | null
  system_label: string
  stack_mode: string
  harness_revision: string | null
  system_revision: string | null
  engine_revision: string | null
  subject_provider: string
  subject_model: string
  judge_provider: string | null
  judge_model: string | null
  judge_protocol: string | null
  cohort_id: string
  execution_count: number
  run_count: number
  median_score: number | null
  median_cost_usd: number | null
  median_tokens: number | null
  median_duration_seconds: number | null
  median_function_calls: number | null
  median_function_call_errors: number | null
  median_turns: number | null
}

export type HistorySystem = {
  id: string
  label: string
}

export type HistoryModelGroup = {
  provider: string
  models: string[]
}

export type TestHistoryResponse = {
  test_id: string
  test_version: number
  available_versions: TestCatalogRow['available_versions']
  cases: string[]
  subjects: string[]
  subject_models: HistoryModelGroup[]
  judge_models: HistoryModelGroup[]
  systems: HistorySystem[]
  series: HistorySeries[]
  observations: TestObservation[]
  total: number
  next_cursor: string | null
}

export type TestObservation = {
  execution_id: string
  evaluated_version_id: string | null
  cohort_id: string
  completed_at: string
  case_id: string
  contract_sha256: string
  assessment_profile_sha256: string
  analyzer_profile_sha256: string
  status: string
  median_score: number | null
  run_count: number
  scored_runs: number
  assessment_summary?: AssessmentSummary
  scenario_version?: number
  seed?: number | null
  system_version_id?: string | null
  system_label?: string
  stack_mode?: string
  harness_revision?: string | null
  system_revision?: string | null
  engine_revision?: string | null
  subject_provider?: string
  subject_model?: string
  judge_provider?: string | null
  judge_model?: string | null
  judge_protocol?: string | null
  median_cost_usd?: number | null
  median_tokens?: number | null
  median_duration_seconds?: number | null
  median_function_calls?: number | null
  median_function_call_errors?: number | null
  median_turns?: number | null
}

export type TestVersionResult = {
  test_id: string
  test_version: number
  compatibility:
    | 'compatible'
    | 'missing_side'
    | 'contract_changed'
    | 'contract_conflict'
    | 'assessment_changed'
    | 'assessment_conflict'
    | 'analyzer_changed'
    | 'analyzer_conflict'
  compatibility_reasons: string[]
  from: TestSideSummary | null
  to: TestSideSummary | null
  delta: {
    score: number | null
    cost_usd: number | null
    tokens: number | null
    duration_seconds: number | null
  }
  from_observations: TestObservation[]
  to_observations: TestObservation[]
}

export type TestCatalogRow = {
  test_id: string
  lifecycle: 'active' | 'retired' | 'never_run'
  current_version: number | null
  available_versions: Array<{
    version: number
    execution_count: number
    run_count: number
    last_seen: string | null
  }>
  selected_version: number | null
  result: TestVersionResult | null
}

export type TestsListResponse = {
  revision: string
  rows: TestCatalogRow[]
  total: number
  next_cursor: string | null
}

export type TestsListInput = {
  cursor?: string
  limit?: number
  query?: string
  cohort_id?: string
  from_version_id?: string
  to_version_id?: string
}

export type TestVersionInput = {
  test_id: string
  test_version: number
  cohort_id: string
  from_version_id: string
  to_version_id: string
}
