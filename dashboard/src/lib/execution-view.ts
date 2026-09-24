import type {
  DashboardExecutionDetail,
  DashboardExecutionSummary,
  DashboardSubjectSummary,
  ExecutionSuite,
  ExecutionTotals,
  JsonObject,
  StackWorker,
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
  | 'inconclusive'

export type FailureBreakdown = Record<FailureCategory, number> & {
  passed: number
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
  'inconclusive',
]

const STATUS_KEYS = [
  'passed',
  'infrastructure_error',
  'resource_limit',
  'subject_error',
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
    inconclusive: countStatus(counts, 'unavailable'),
    passed: countStatus(counts, 'passed'),
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
      breakdown.infrastructure +
      breakdown.resource_limit +
      breakdown.subject +
      breakdown.inconclusive
  }
  breakdown.issues =
    breakdown.infrastructure +
    breakdown.resource_limit +
    breakdown.subject +
    breakdown.inconclusive
  return breakdown
}

export function attentionState(
  execution: DashboardExecutionSummary,
  breakdown = failureBreakdown(execution),
): ExecutionAttentionState {
  const status = stringValue(execution.status)
  // An import in progress is followed like a running execution.
  if (status === 'running' || status === 'importing') return 'running'
  if (status === 'cancelling') return 'cancelling'
  if (status === 'cancelled') return 'cancelled'
  if (status === 'incomplete') return 'incomplete'
  if (status === 'unavailable') return 'unavailable'
  if (status === 'failed') return 'needs_attention'
  const hasAttention = CATEGORY_ORDER.some(
    (category) => breakdown[category] > 0,
  )
  if (hasAttention) return 'needs_attention'
  if (breakdown.passed > 0) return 'passed'
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
  const attention = attentionState(execution, breakdown)
  return {
    execution,
    label: executionLabel(execution),
    subjects: executionSubjects(execution),
    attention,
    breakdown,
    // A run stopped by its user reads as cancelled, not as a failure.
    primaryIssue:
      attention === 'cancelled' || attention === 'cancelling'
        ? null
        : primaryIssue(breakdown),
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
  if (seconds < 59.5) return `${seconds.toFixed(seconds < 10 ? 1 : 0)}s`
  // Round the total first, so 119.6s reads "2m 00s", never "1m 60s".
  const total = Math.round(seconds)
  return `${Math.floor(total / 60)}m ${String(total % 60).padStart(2, '0')}s`
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

export function statusCopy(presentation: ExecutionPresentation) {
  if (presentation.execution.status === 'importing')
    return { label: 'importing', status: 'running' as const }
  if (presentation.attention === 'passed')
    return { label: 'passed', status: 'passed' as const }
  if (presentation.attention === 'running')
    return { label: 'running', status: 'running' as const }
  if (presentation.attention === 'cancelling')
    return { label: 'cancelling', status: 'cancelling' as const }
  if (presentation.attention === 'cancelled')
    return { label: 'cancelled', status: 'cancelled' as const }
  if (presentation.attention === 'incomplete')
    return { label: 'incomplete', status: 'incomplete' as const }
  if (presentation.attention === 'unavailable')
    return { label: 'no report', status: 'unavailable' as const }
  if (
    presentation.breakdown.inconclusive > 0 &&
    presentation.breakdown.issues === presentation.breakdown.inconclusive
  )
    return { label: 'inconclusive', status: 'inconclusive' as const }
  return { label: 'failed', status: 'failed' as const }
}

/** Where an execution came from, as text with a link when it has one. Native
 *  runs and executions planned here read as local. */
export function executionOrigin(execution: DashboardExecutionSummary): {
  label: string
  href: string | null
} {
  const source = objectValue(execution.source)
  if (source.kind === 'github')
    return {
      label: `GitHub #${String(source.run_id ?? '')}`,
      href: stringValue(source.url) || null,
    }
  return { label: 'local', href: null }
}

/** How far a running (or cancelled) execution got, as its slots (or a
 *  native run's slots) finished of those planned; null otherwise. */
export function executionProgress(
  execution: DashboardExecutionSummary,
): string | null {
  const status = stringValue(execution.status)
  if (!['running', 'cancelling', 'cancelled'].includes(status)) return null
  const plan = objectValue(execution.plan_execution)
  const live = objectValue(execution.live_progress)
  const done =
    numberValue(plan.finished) ?? numberValue(live.runs_committed) ?? null
  const planned =
    numberValue(plan.planned) ?? numberValue(live.planned_slots) ?? null
  return done === null || !planned ? null : `${done} of ${planned} done`
}

/** The version a stack worker ran: its checkout for a `path://` worker,
 *  else the version the engine reported (or the one asked for). Versions
 *  that differed between groups are all listed. */
/** A suite as the Console names it: its name, or "unnamed suite", and the
 *  start of the digest of what it materialized. */
export function suiteText(
  suite: ExecutionSuite | null | undefined,
): string | null {
  if (!suite) return null
  const name = suite.id ? suite.label || suite.id : 'unnamed suite'
  const digest = suite.sha256?.replace(/^sha256:/, '').slice(0, 12)
  return digest ? `${name} · ${digest}` : name
}

export function workerVersion(
  stack: StackWorker[] | undefined,
  name: string,
): string | null {
  const versions = (stack ?? [])
    .filter((worker) => worker.name === name)
    .map((worker) =>
      worker.source === 'path' && worker.commit
        ? `path @${worker.commit.slice(0, 12)}${worker.dirty ? ' (dirty)' : ''}`
        : (worker.observed ?? worker.requested),
    )
    .filter((version): version is string => Boolean(version))
  return versions.length > 0 ? [...new Set(versions)].join(', ') : null
}

/** Mean of the per-scenario mean scores; null unless every scenario has one. */
export function executionScore(
  execution: DashboardExecutionSummary | null,
): number | null {
  const scores =
    execution?.subjects.flatMap((subject) =>
      subject.scenarios.map((scenario) => numberValue(scenario.mean_score)),
    ) ?? []
  return scores.length > 0 && scores.every((score) => score !== null)
    ? scores.reduce<number>((total, score) => total + (score ?? 0), 0) /
        scores.length
    : null
}

/** "provider/model", without repeating a provider the model id carries. */
export function providerModel({
  provider,
  model,
}: {
  provider?: string | null
  model: string
}) {
  return !provider || model.startsWith(`${provider}/`)
    ? model
    : `${provider}/${model}`
}

export function modelNames(models: ExecutionPresentation['subjects']) {
  if (models.length === 0) return 'not reported'
  return models.map(providerModel).join(', ')
}

export function executionTitle(presentation: ExecutionPresentation): {
  title: string
  detail: string | null
} {
  const execution = presentation.execution
  const label =
    typeof execution.label === 'string' ? execution.label.trim() : ''
  const workflow =
    typeof execution.workflow_name === 'string'
      ? execution.workflow_name.trim()
      : ''
  if (label) return { title: label, detail: workflow || null }
  const subject = presentation.subjects[0]
  if (subject) {
    // Dated by its creation: the same title while it runs, when it ends and
    // on every page.
    return {
      title: `${subject.model} · ${formatDate(presentation.startedAt || presentation.completedAt)}`,
      detail: workflow || null,
    }
  }
  return { title: workflow || 'Harness E2E execution', detail: null }
}

/**
 * Rates arrive either as a 0–1 fraction or as 0–100 points depending on the
 * publisher; the signal always works in points so a delta reads as "pts".
 */
export function percentPoints(value: number | null | undefined): number | null {
  const known = numberValue(value)
  if (known === null) return null
  return Math.abs(known) <= 1 ? known * 100 : known
}
