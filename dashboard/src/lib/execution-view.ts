import type {
  DashboardExecutionDetail,
  DashboardExecutionSummary,
  DashboardSubjectSummary,
  ExecutionTotals,
  JsonObject,
} from '@/lib/dashboard-data-source'

export type ExecutionAttentionState =
  | 'passed'
  | 'needs_attention'
  | 'incomplete'
  | 'running'
  | 'cancelling'
  | 'cancelled'
  | 'unavailable'

export type FailureCategory =
  | 'infrastructure'
  | 'resource_limit'
  | 'subject'
  | 'judge'
  | 'hard_gate'
  | 'inconclusive'

export type FailureBreakdown = Record<FailureCategory, number> & {
  passed: number
  passed_with_concerns: number
  total: number
  issues: number
}

export type ExecutionModel = {
  provider: string
  model: string
}

export type ExecutionPresentation = {
  execution: DashboardExecutionSummary
  label: string
  subjects: ExecutionModel[]
  judges: ExecutionModel[]
  attention: ExecutionAttentionState
  breakdown: FailureBreakdown
  primaryIssue: { category: FailureCategory; count: number } | null
  expectedReports: number | null
  receivedReports: number | null
  passRate: number | null
  coverage: number | null
  modelRuntimeSeconds: number | null
  workflowRuntimeSeconds: number | null
  completedAt: string
  startedAt: string
  available: boolean
}

const CATEGORY_ORDER: FailureCategory[] = [
  'infrastructure',
  'resource_limit',
  'subject',
  'judge',
  'hard_gate',
  'inconclusive',
]

const STATUS_KEYS = [
  'passed',
  'passed_with_concerns',
  'hard_gate_failed',
  'infrastructure_error',
  'resource_limit',
  'subject_error',
  'judge_error',
  'unavailable',
] as const

function numberValue(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function stringValue(value: unknown): string {
  return typeof value === 'string' ? value : ''
}

function objectValue(value: unknown): JsonObject {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as JsonObject)
    : {}
}

function countStatus(source: JsonObject, key: string): number {
  return numberValue(source[key]) ?? 0
}

function totalsFor(execution: DashboardExecutionSummary): ExecutionTotals {
  return objectValue(execution.totals) as ExecutionTotals
}

function subjectsFor(
  execution: DashboardExecutionSummary,
): DashboardSubjectSummary[] {
  return Array.isArray(execution.subjects) ? execution.subjects : []
}

function assessmentStatuses(execution: DashboardExecutionSummary): JsonObject {
  const summary = objectValue(execution.assessment_summary)
  const statuses = objectValue(summary.system_statuses)
  return Object.keys(statuses).length > 0 ? statuses : {}
}

function fallbackStatusCounts(
  execution: DashboardExecutionSummary,
): JsonObject {
  const totals = totalsFor(execution)
  const expected = numberValue(totals.expected_reports)
  const received = numberValue(totals.received_reports)
  const missing =
    numberValue(totals.missing_reports) ??
    (expected !== null && received !== null
      ? Math.max(0, expected - received)
      : 0)
  return {
    passed: numberValue(totals.passed_scenarios) ?? 0,
    hard_gate_failed: numberValue(totals.hard_gate_failures) ?? 0,
    infrastructure_error:
      numberValue(totals.infra_failures) ??
      numberValue(totals.technical_failures) ??
      0,
    resource_limit: numberValue(totals.resource_limit_failures) ?? 0,
    unavailable: missing,
  }
}

export function failureBreakdown(
  execution: DashboardExecutionSummary,
): FailureBreakdown {
  const source = assessmentStatuses(execution)
  const counts =
    Object.keys(source).length > 0 ? source : fallbackStatusCounts(execution)
  const breakdown: FailureBreakdown = {
    infrastructure: countStatus(counts, 'infrastructure_error'),
    resource_limit: countStatus(counts, 'resource_limit'),
    subject: countStatus(counts, 'subject_error'),
    judge: countStatus(counts, 'judge_error'),
    hard_gate: countStatus(counts, 'hard_gate_failed'),
    inconclusive: countStatus(counts, 'unavailable'),
    passed: countStatus(counts, 'passed'),
    passed_with_concerns: countStatus(counts, 'passed_with_concerns'),
    total: 0,
    issues: 0,
  }
  breakdown.total = STATUS_KEYS.reduce(
    (total, key) => total + countStatus(counts, key),
    0,
  )
  if (breakdown.total === 0) {
    breakdown.total =
      breakdown.passed +
      breakdown.passed_with_concerns +
      breakdown.hard_gate +
      breakdown.infrastructure +
      breakdown.resource_limit +
      breakdown.subject +
      breakdown.judge +
      breakdown.inconclusive
  }
  breakdown.issues =
    breakdown.infrastructure +
    breakdown.resource_limit +
    breakdown.subject +
    breakdown.judge +
    breakdown.hard_gate +
    breakdown.inconclusive
  return breakdown
}

export function attentionState(
  execution: DashboardExecutionSummary,
  breakdown = failureBreakdown(execution),
): ExecutionAttentionState {
  const status = stringValue(execution.status)
  if (status === 'running') return 'running'
  if (status === 'cancelling') return 'cancelling'
  if (status === 'cancelled') return 'cancelled'
  if (status === 'incomplete') return 'incomplete'
  if (status === 'unavailable') return 'unavailable'
  const hasAttention = CATEGORY_ORDER.some(
    (category) => breakdown[category] > 0,
  )
  if (hasAttention) return 'needs_attention'
  if (breakdown.passed > 0 || breakdown.passed_with_concerns > 0)
    return 'passed'
  return 'unavailable'
}

export function primaryIssue(
  breakdown: FailureBreakdown,
): { category: FailureCategory; count: number } | null {
  const category = CATEGORY_ORDER.find((candidate) => breakdown[candidate] > 0)
  return category ? { category, count: breakdown[category] } : null
}

function modelFrom(value: unknown): ExecutionModel | null {
  const model = objectValue(value)
  const name = stringValue(model.model)
  if (!name) return null
  return { model: name, provider: stringValue(model.provider) }
}

export function executionSubjects(
  execution: DashboardExecutionSummary,
): ExecutionModel[] {
  const seen = new Set<string>()
  return subjectsFor(execution).flatMap((subject) => {
    const model = modelFrom(subject)
    if (!model) return []
    const key = `${model.provider}/${model.model}`
    if (seen.has(key)) return []
    seen.add(key)
    return [model]
  })
}

export function executionJudges(
  execution: DashboardExecutionSummary,
): ExecutionModel[] {
  const seen = new Set<string>()
  return subjectsFor(execution).flatMap((subject) => {
    const judge = modelFrom(subject.judge)
    if (!judge) return []
    const key = `${judge.provider}/${judge.model}`
    if (seen.has(key)) return []
    seen.add(key)
    return [judge]
  })
}

export function executionLabel(execution: DashboardExecutionSummary): string {
  return (
    stringValue(execution.label) ||
    stringValue(execution.workflow_name) ||
    'Harness E2E execution'
  )
}

export function buildExecutionPresentation(
  execution: DashboardExecutionSummary,
): ExecutionPresentation {
  const totals = totalsFor(execution)
  const breakdown = failureBreakdown(execution)
  return {
    execution,
    label: executionLabel(execution),
    subjects: executionSubjects(execution),
    judges: executionJudges(execution),
    attention: attentionState(execution, breakdown),
    breakdown,
    primaryIssue: primaryIssue(breakdown),
    expectedReports: numberValue(totals.expected_reports),
    receivedReports: numberValue(totals.received_reports),
    passRate: numberValue(totals.scenario_pass_rate),
    coverage: numberValue(totals.report_coverage),
    modelRuntimeSeconds: numberValue(totals.wall_time_seconds),
    workflowRuntimeSeconds:
      numberValue(execution.workflow_duration_seconds) ??
      numberValue(totals.workflow_duration_seconds),
    completedAt: stringValue(execution.completed_at || execution.generated_at),
    startedAt: stringValue(execution.started_at),
    available: execution.availability !== 'unavailable',
  }
}

export function titleCase(value: string): string {
  return value
    .replaceAll('.', ' / ')
    .replaceAll('_', ' ')
    .replaceAll('-', ' ')
    .replace(/\b\w/g, (letter) => letter.toUpperCase())
}

export function formatPercent(value: number | null, fraction = true): string {
  if (value === null || !Number.isFinite(value)) return 'Not reported'
  const percent = fraction && Math.abs(value) <= 1 ? value * 100 : value
  return `${percent.toFixed(percent % 1 === 0 ? 0 : 1)}%`
}

export function formatDuration(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds)) return 'Not reported'
  if (seconds < 60) return `${seconds.toFixed(seconds < 10 ? 1 : 0)}s`
  const minutes = Math.floor(seconds / 60)
  const remainder = Math.round(seconds % 60)
  return `${minutes}m ${String(remainder).padStart(2, '0')}s`
}

export function formatDate(value: string): string {
  if (!value) return 'Not reported'
  const timestamp = Date.parse(value)
  return Number.isFinite(timestamp)
    ? new Date(timestamp).toLocaleString(undefined, {
        dateStyle: 'medium',
        timeStyle: 'short',
      })
    : value
}

export function categoryLabel(category: FailureCategory): string {
  return {
    infrastructure: 'Infrastructure',
    resource_limit: 'Resource limit',
    subject: 'Subject model',
    judge: 'Judge model',
    hard_gate: 'Hard gate',
    inconclusive: 'Inconclusive',
  }[category]
}

export function categoryMessage(
  category: FailureCategory,
  count: number,
): string {
  const label = categoryLabel(category).toLowerCase()
  return `${count} ${label} ${count === 1 ? 'event' : 'events'}`
}

export function isExecutionAttention(
  presentation: ExecutionPresentation,
): boolean {
  return (
    presentation.attention === 'needs_attention' ||
    presentation.attention === 'incomplete' ||
    presentation.attention === 'unavailable'
  )
}

export function detailHasAttention(detail: DashboardExecutionDetail): boolean {
  return isExecutionAttention(buildExecutionPresentation(detail))
}
