export type PortKind = 'boolean' | 'json' | 'text_utf8' | 'assessment'

export type StepPortDescriptor = {
  kind: PortKind
  optional: boolean
  control_source?: 'deterministic' | 'ai' | null
}

export type StepTypeDescriptor = {
  id: string
  version: number
  description: string
  config_schema: Record<string, unknown>
  inputs: Record<string, StepPortDescriptor>
  outputs: Record<string, StepPortDescriptor>
  capabilities: string[]
  required_functions: Array<{ function_id: string }>
  replay_policy: 'idempotent' | 'compensable' | 'non_repeatable'
  operational_kind: 'harness' | 'product' | 'assessment' | 'transformation'
}

export type WorkflowInputBinding =
  | { source: 'literal'; kind: PortKind; value: unknown }
  | { source: 'output'; node_id: string; port: string }

export type WorkflowCondition = {
  node_id: string
  port: string
  equals: boolean
}

export type WorkflowNodeDefinition = {
  id: string
  step_type: string
  step_version: number
  config: Record<string, unknown>
  depends_on: string[]
  inputs: Record<string, WorkflowInputBinding>
  activation:
    | { policy: 'always' }
    | { policy: 'all' | 'any'; conditions: WorkflowCondition[] }
  dependency_policy: 'succeeded' | 'terminal'
  required: boolean
}

export type WorkflowDefinition = {
  schema_version: 1
  id: string
  scenario_version: number
  description: string
  limits: {
    max_parallel: number
    max_nodes: number
    step_timeout_seconds: number
    workflow_timeout_seconds: number
    max_total_tokens?: number | null
    max_cost_usd?: number | null
    technical_retries: number
  }
  nodes: WorkflowNodeDefinition[]
  criteria: Array<{
    id: string
    weight: number
    producer_node_id: string
    output_port: string
    advisory: boolean
  }>
}

export type WorkflowLayout = Record<string, { x: number; y: number }>

export type WorkflowDraft = {
  schema_version: 1
  id: string
  label: string
  created_at: string
  updated_at: string
  definition_sha256: string
  definition: WorkflowDefinition
  layout: WorkflowLayout
}

export type OfficialWorkflow = {
  id: string
  scenario_version: number
  description: string
  definition_sha256: string
  definition: WorkflowDefinition
}

export type WorkflowCatalog = {
  mode: 'local' | 'observed'
  drafts: WorkflowDraft[]
  official: OfficialWorkflow[]
  step_types: StepTypeDescriptor[]
  definition_schema: Record<string, unknown>
}

export type WorkflowRunSnapshot = {
  job?: {
    id: string
    status: 'running' | 'cancelling' | 'cancelled' | 'completed' | 'failed'
    log?: string
    log_offset?: number
    error?: string
  } | null
}

export type WorkflowObservedStep = {
  node_id: string
  status: string
  duration_ms: number
  assets?: Array<{ id: string; artifact: { path: string } }>
  hard_gates?: Array<{ id: string; passed: boolean; reason: string }>
  evaluations?: Array<{ id: string; outcome: string; summary: string }>
  failures?: Array<{ phase: string; message: string }>
}

export type WorkflowRunDetail = {
  execution_id: string
  passed: boolean | null
  workflow_definition?: WorkflowDefinition | null
  workflow_runs: Array<{
    workflow_id: string
    workflow_sha256?: string
    steps: WorkflowObservedStep[]
  }>
}

async function request<T>(path: string, options: RequestInit = {}): Promise<T> {
  const response = await fetch(path, {
    cache: 'no-store',
    headers: { 'Content-Type': 'application/json' },
    ...options,
  })
  const payload = (await response.json().catch(() => ({}))) as T & {
    error?: string
  }
  if (!response.ok) {
    throw new Error(payload.error || `Request failed (${response.status})`)
  }
  return payload
}

export const workflowDataSource = {
  catalog: () => request<WorkflowCatalog>('./api/dashboard/workflows'),
  create: (
    label: string,
    definition: WorkflowDefinition,
    layout: WorkflowLayout,
  ) =>
    request<WorkflowDraft>('./api/dashboard/workflows', {
      method: 'POST',
      body: JSON.stringify({ label, definition, layout }),
    }),
  update: (
    draft: WorkflowDraft,
    label: string,
    definition: WorkflowDefinition,
    layout: WorkflowLayout,
  ) =>
    request<WorkflowDraft>(
      `./api/dashboard/workflows/${encodeURIComponent(draft.id)}`,
      {
        method: 'PATCH',
        body: JSON.stringify({
          label,
          definition,
          layout,
          expected_definition_sha256: draft.definition_sha256,
        }),
      },
    ),
  duplicate: (draftId: string) =>
    request<WorkflowDraft>(
      `./api/dashboard/workflows/${encodeURIComponent(draftId)}/duplicate`,
      { method: 'POST', body: '{}' },
    ),
  remove: async (draftId: string) => {
    const response = await fetch(
      `./api/dashboard/workflows/${encodeURIComponent(draftId)}`,
      { method: 'DELETE' },
    )
    if (!response.ok) throw new Error(`Delete failed (${response.status})`)
  },
  validate: (definition: WorkflowDefinition) =>
    request<{ valid: true; definition_sha256: string }>(
      './api/dashboard/workflows/validate',
      { method: 'POST', body: JSON.stringify({ definition }) },
    ),
  run: (
    draft: WorkflowDraft,
    identity: { url: string; model: string; provider: string },
  ) =>
    request<WorkflowRunSnapshot>(
      `./api/dashboard/workflows/${encodeURIComponent(draft.id)}/run`,
      {
        method: 'POST',
        body: JSON.stringify({
          expected_definition_sha256: draft.definition_sha256,
          ...identity,
        }),
      },
    ),
  runStatus: (after = 0) =>
    request<WorkflowRunSnapshot>(`./api/local/run?after=${after}`),
  runDetail: (executionId: string) =>
    request<WorkflowRunDetail>(
      `./api/dashboard/workflow-runs/${encodeURIComponent(executionId)}`,
    ),
}

export function canonicalWorkflowJson(definition: WorkflowDefinition) {
  return `${JSON.stringify(sortJson(definition), null, 2)}\n`
}

function sortJson(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortJson)
  if (!value || typeof value !== 'object') return value
  return Object.fromEntries(
    Object.entries(value as Record<string, unknown>)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, nested]) => [key, sortJson(nested)]),
  )
}
