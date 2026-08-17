export type SecurityReviewAsset = {
  id: string
  namespaced_id?: string
  kind?: string
  media_type?: string
  size_bytes?: number
  artifact: { path: string; sha256?: string }
}

export type SecurityReviewStep = {
  node_id: string
  step_type?: string
  required?: boolean
  dependencies?: string[]
  status: string
  started_at?: string | null
  completed_at?: string | null
  duration_ms: number
  metrics?: unknown
  assets?: SecurityReviewAsset[]
  hard_gates?: Array<{
    id: string
    passed: boolean
    reason: string
    evidence_ids?: string[]
  }>
  evaluations?: Array<{
    id: string
    outcome: string
    summary: string
    score?: number | null
    evidence_ids?: string[]
  }>
  failures?: Array<{ phase: string; message: string; technical?: boolean }>
  skip_reason?: string | null
}

export type RustFlowSnapshot = {
  kind: 'rust_flow_evidence'
  executable: false
  scenario_id: string
  scenario_version: number
  sha256: string
  tests: Array<{
    id: string
    semantic_test: string
    depends_on: string[]
    required: boolean
  }>
}

export type SecurityReviewRunDetail = {
  execution_id: string
  passed: boolean | null
  security_review_runs: Array<{
    workflow_id: string
    workflow_sha256?: string
    run_id?: string
    attempt_id?: string
    flow_snapshot?: RustFlowSnapshot | null
    active_nodes?: string[]
    cleanup?: {
      status: 'succeeded' | 'failed'
      duration_ms: number
      failure?: string | null
    }
    steps: SecurityReviewStep[]
  }>
}

async function request<T>(path: string): Promise<T> {
  const response = await fetch(path, { cache: 'no-store' })
  const payload = (await response.json().catch(() => ({}))) as T & {
    error?: string
  }
  if (!response.ok) {
    throw new Error(payload.error || `Request failed (${response.status})`)
  }
  return payload
}

export const securityReviewDataSource = {
  runDetail: (executionId: string) =>
    request<SecurityReviewRunDetail>(
      `./api/dashboard/security-review-runs/${encodeURIComponent(executionId)}`,
    ),
}
