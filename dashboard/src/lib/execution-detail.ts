import type { StatusVariant } from '@iii-dev/console-ui'
import type { SystemOutcome } from '@/components/SystemOutcome'
import type { MetricTone, OperationalStatus } from '@/design-system/primitives'
import type {
  AssessmentRunMetrics,
  AssessmentRunView,
} from '@/lib/assessment-view'
import type {
  DashboardExecutionDetail,
  DashboardExecutionSummary,
} from '@/lib/dashboard-data-source'
import { buildExecutionMetrics } from '@/lib/execution-metrics'
import type { ExecutionVerdict } from '@/lib/execution-verdict'
import {
  type ExecutionPresentation,
  formatDate,
  formatDuration,
  formatPercent,
} from '@/lib/execution-view'
import type {
  ScenarioMatrixItem,
  ScenarioMatrixSummary,
} from '@/lib/scenario-matrix'

/** What the execution detail shows, derived from the retained detail; the
 *  page only renders these. */

export function summaryFromDetail(
  detail: DashboardExecutionDetail,
  fallback?: DashboardExecutionSummary,
): DashboardExecutionSummary {
  const nested =
    detail.execution && typeof detail.execution === 'object'
      ? (detail.execution as Record<string, unknown>)
      : {}
  return {
    ...(fallback ?? {}),
    ...detail,
    id: detail.id || fallback?.id || String(nested.id ?? ''),
    label: detail.label || fallback?.label || String(nested.label ?? ''),
    status: detail.status || fallback?.status || 'incomplete',
    subjects: detail.subjects ?? fallback?.subjects ?? [],
  }
}

export function executionStatus(presentation: ExecutionPresentation): {
  status: OperationalStatus
  label: string
} {
  const attention = presentation.attention
  if (attention === 'passed') return { status: 'passed', label: 'passed' }
  if (attention === 'running') return { status: 'running', label: 'running' }
  if (attention === 'cancelling')
    return { status: 'cancelling', label: 'cancelling' }
  if (attention === 'cancelled')
    return { status: 'cancelled', label: 'cancelled' }
  if (attention === 'incomplete')
    return { status: 'incomplete', label: 'incomplete' }
  if (attention === 'unavailable')
    return { status: 'unavailable', label: 'unavailable' }
  if (
    presentation.breakdown.inconclusive > 0 &&
    presentation.breakdown.inconclusive === presentation.breakdown.issues
  )
    return { status: 'inconclusive', label: 'inconclusive' }
  return { status: 'failed', label: 'failed' }
}

/** The one status the result contract publishes, pooled over the retained runs. */
export function executionOutcome(
  presentation: ExecutionPresentation,
  runs: AssessmentRunView[],
): SystemOutcome {
  const values = runs.length
    ? runs.map((run) => run.systemStatus)
    : [presentation.attention]
  const counts = new Map<string, number>()
  for (const value of values) counts.set(value, (counts.get(value) ?? 0) + 1)
  if (counts.size === 1) return { value: values[0] }
  return {
    value: 'partial',
    label: [...counts]
      .map(
        ([value, count]) =>
          `${count} ${value === 'hard_gate_failed' ? 'failed (legacy result)' : value.replaceAll('_', ' ')}`,
      )
      .join(' · '),
  }
}

export function finiteMetric(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

export function formatMetricCount(value: number | null) {
  return value === null ? '—' : Math.round(value).toLocaleString('en-US')
}

export function formatReportedCost(value: number | null) {
  if (value == null) return '—'
  if (value > 0 && value < 0.0001) return '<$0.0001'
  return `$${value.toFixed(4)}`
}

export function runCountFromDetail(detail: DashboardExecutionDetail) {
  return (detail.reports ?? []).reduce(
    (total, record) =>
      total +
      (record.report?.scenarios ?? []).reduce(
        (scenarioTotal, scenario) =>
          scenarioTotal + (scenario.runs?.length ?? 0),
        0,
      ),
    0,
  )
}

/** The facts that identify the execution, one pair each. */
export function identityEntries(
  detail: DashboardExecutionDetail,
  presentation: ExecutionPresentation,
): Array<[string, string]> {
  return [
    [
      'subject',
      presentation.subjects
        .map((model) => `${model.provider}/${model.model}`)
        .join(', ') || 'not reported',
    ],
    [
      'started',
      presentation.startedAt ? formatDate(presentation.startedAt) : '—',
    ],
    [
      'trigger',
      [detail.event, detail.actor].filter(Boolean).join(' · ') ||
        'not reported',
    ],
    ['id', `${detail.id.slice(0, 8)}…${detail.id.slice(-6)}`],
  ]
}

/** One sentence under the title: what the page holds. */
export function executionSummarySentence({
  detail,
  live,
  scenarioSummary,
  runCount,
}: {
  detail: DashboardExecutionDetail
  live: boolean
  scenarioSummary: ScenarioMatrixSummary | null
  runCount: number
}): string {
  if (detail.live_progress) {
    return `${detail.live_progress.runs_committed} of ${detail.live_progress.planned_slots} runs recorded · ${
      live ? 'results are provisional' : 'partial evidence preserved'
    }`
  }
  if (live) return 'Execution in progress · results are provisional'
  const tests = scenarioSummary?.total ?? 0
  return `${tests} test${tests === 1 ? '' : 's'} · ${runCount} run${runCount === 1 ? '' : 's'}`
}

/** The tone of the verdict panel follows the aggregate outcome. */
export function verdictVariant(
  summary: ScenarioMatrixSummary | null,
): StatusVariant {
  if (!summary || summary.total === 0) return 'warn'
  if (summary.failed > 0 || summary.unavailable > 0) return 'alert'
  if (summary.passed === summary.total) return 'success'
  return 'warn'
}

export type ExecutionMetricCard = {
  label: string
  value: string
  detail: string
  tone: MetricTone
}

/** The five numbers that describe an execution at a glance. */
export function executionMetricCards(
  detail: DashboardExecutionDetail,
  scenarioSummary: ScenarioMatrixSummary | null,
): ExecutionMetricCard[] {
  const metrics = buildExecutionMetrics(detail)
  const tests = scenarioSummary?.total ?? 0
  const passed = scenarioSummary?.passed ?? 0
  const wallSeconds = finiteMetric(detail.totals?.wall_time_seconds)
  const durationSeconds =
    metrics.durationMs.total === null
      ? wallSeconds
      : metrics.durationMs.total / 1000
  return [
    {
      label: 'tests',
      value: tests ? `${passed}/${tests}` : '—',
      detail: tests
        ? [
            scenarioSummary?.failed ? `${scenarioSummary.failed} failed` : null,
            scenarioSummary?.inconclusive
              ? `${scenarioSummary.inconclusive} inconclusive`
              : null,
            scenarioSummary?.unavailable
              ? `${scenarioSummary.unavailable} without evidence`
              : null,
          ]
            .filter(Boolean)
            .join(' · ') || 'every test passed'
        : 'no scenario report retained',
      tone:
        tests === 0
          ? 'unavailable'
          : passed === tests
            ? 'positive'
            : 'negative',
    },
    {
      label: 'score',
      value:
        metrics.scoreMean === null
          ? '—'
          : String(Math.round(metrics.scoreMean)),
      detail:
        metrics.scoreSamples > 0
          ? `mean of ${metrics.scoreSamples} scored run${metrics.scoreSamples === 1 ? '' : 's'}`
          : 'nothing was scored',
      tone: metrics.scoreMean === null ? 'unavailable' : 'neutral',
    },
    {
      label: 'completion',
      value:
        metrics.completionRate === null
          ? '—'
          : formatPercent(metrics.completionRate * 100, false),
      detail: `${metrics.completed} of ${metrics.observed} run${metrics.observed === 1 ? '' : 's'} completed`,
      tone:
        metrics.completionRate === null
          ? 'unavailable'
          : metrics.completionRate >= 1
            ? 'positive'
            : 'warning',
    },
    {
      label: 'runtime',
      value: durationSeconds === null ? '—' : formatDuration(durationSeconds),
      detail:
        metrics.durationMs.total === null
          ? wallSeconds === null
            ? 'not reported'
            : 'wall time of the execution'
          : `${metrics.durationMs.samples} of ${metrics.durationMs.expected} runs reported`,
      tone: durationSeconds === null ? 'unavailable' : 'neutral',
    },
    {
      label: 'tokens',
      value: formatMetricCount(metrics.subjectTokens.total),
      detail:
        tokenBreakdownLines({
          input: metrics.inputTokens.total,
          output: metrics.outputTokens.total,
          cacheRead: metrics.cacheReadTokens.total,
          cacheWrite: metrics.cacheWriteTokens.total,
        }).join(' · ') || 'input and output not reported',
      tone: metrics.subjectTokens.total === null ? 'unavailable' : 'neutral',
    },
    {
      label: 'cost',
      value: formatReportedCost(metrics.cost.total),
      detail:
        metrics.cost.total === null
          ? 'not reported'
          : `reported by ${metrics.cost.samples} of ${metrics.cost.expected} run${metrics.cost.expected === 1 ? '' : 's'}`,
      tone: metrics.cost.total === null ? 'unavailable' : 'neutral',
    },
    {
      label: 'turns',
      value: formatMetricCount(metrics.turns.total),
      detail:
        metrics.functionCalls.total === null
          ? 'function calls not reported'
          : `${formatMetricCount(metrics.functionCalls.total)} function call${
              metrics.functionCalls.total === 1 ? '' : 's'
            }${
              metrics.functionErrors.total
                ? ` · ${formatMetricCount(metrics.functionErrors.total)} failed`
                : ''
            }`,
      tone: metrics.turns.total === null ? 'unavailable' : 'neutral',
    },
  ]
}

export type TokenBreakdown = {
  input: number | null
  output: number | null
  cacheRead: number | null
  cacheWrite: number | null
}

/** The subject tokens of the retained runs split as their terminal attempts
 *  report them; each part null when no run reported it. */
export function tokenBreakdownOfRuns(
  runs: ReadonlyArray<{ metrics: AssessmentRunMetrics }>,
): TokenBreakdown {
  return {
    input: sumOfRuns(runs, 'inputTokens'),
    output: sumOfRuns(runs, 'outputTokens'),
    cacheRead: sumOfRuns(runs, 'cacheReadTokens'),
    cacheWrite: sumOfRuns(runs, 'cacheWriteTokens'),
  }
}

/** One line for what the subject sent and received, one for what the cache
 *  read and wrote, each naming only the parts a run reported; nothing when no
 *  part was reported. */
export function tokenBreakdownLines(breakdown: TokenBreakdown): string[] {
  const lines: string[] = []
  const usage = [
    breakdown.input === null
      ? null
      : `${formatMetricCount(breakdown.input)} in`,
    breakdown.output === null
      ? null
      : `${formatMetricCount(breakdown.output)} out`,
  ].filter(Boolean)
  if (usage.length > 0) lines.push(usage.join(' · '))
  const cache = [
    breakdown.cacheRead === null
      ? null
      : `${formatMetricCount(breakdown.cacheRead)} read`,
    breakdown.cacheWrite === null
      ? null
      : `${formatMetricCount(breakdown.cacheWrite)} write`,
  ].filter(Boolean)
  if (cache.length > 0) lines.push(`cache ${cache.join(' · ')}`)
  else if (lines.length > 0) lines.push('cache not reported')
  return lines
}

/** The turns the retained runs of a test spent, summed; null when none reported them. */
export function turnsOfRuns(
  runs: ReadonlyArray<{ metrics: AssessmentRunMetrics }>,
): number | null {
  return sumOfRuns(runs, 'turns')
}

function sumOfRuns(
  runs: ReadonlyArray<{ metrics: AssessmentRunMetrics }>,
  key: keyof AssessmentRunMetrics,
): number | null {
  const known = runs
    .map((run) => run.metrics[key])
    .filter(
      (value): value is number => value !== null && Number.isFinite(value),
    )
  return known.length === 0
    ? null
    : known.reduce((sum, value) => sum + value, 0)
}

/** The totals that survive an unavailable evidence bundle. */
export function snapshotMetricCards(
  detail: DashboardExecutionDetail,
): ExecutionMetricCard[] {
  const reports = finiteMetric(detail.totals?.received_reports)
  const tokens = finiteMetric(detail.totals?.total_tokens)
  const cost = finiteMetric(detail.totals?.total_cost_usd)
  const duration = finiteMetric(detail.totals?.wall_time_seconds)
  const turns = finiteMetric(detail.totals?.turns)
  const card = (
    label: string,
    value: string,
    reported: boolean,
  ): ExecutionMetricCard => ({
    label,
    value,
    detail: 'retained execution total',
    tone: reported ? 'neutral' : 'unavailable',
  })
  return [
    card('received reports', formatMetricCount(reports), reports !== null),
    card('tokens', formatMetricCount(tokens), tokens !== null),
    card('reported cost', formatReportedCost(cost), cost !== null),
    card(
      'runtime',
      duration === null ? '—' : formatDuration(duration),
      duration !== null,
    ),
    card('turns', formatMetricCount(turns), turns !== null),
  ]
}

/** Live state copy: the scope reached and the time elapsed. */
export function liveStateCopy(
  presentation: ExecutionPresentation,
  hasProgress: boolean,
  now = Date.now(),
): { headline: string; note: string } {
  const running =
    presentation.attention === 'running' ||
    presentation.attention === 'cancelling'
  const scope =
    presentation.expectedReports !== null &&
    presentation.receivedReports !== null
      ? `${presentation.receivedReports} of ${presentation.expectedReports} tests`
      : null
  const started = presentation.startedAt
    ? Date.parse(presentation.startedAt)
    : Number.NaN
  const elapsed =
    running && Number.isFinite(started)
      ? `${formatDuration((now - started) / 1000)} elapsed`
      : null
  return {
    headline:
      [presentation.attention, scope, elapsed].filter(Boolean).join(' · ') ||
      presentation.attention,
    note: running
      ? 'This page follows recorded progress automatically. The final report and decision appear when the execution finishes.'
      : hasProgress
        ? 'The final report is unavailable. Recorded checkpoints remain visible as partial evidence, not a final verdict.'
        : 'No report or verified progress is available for this execution.',
  }
}

function compactObject(value: unknown): string | null {
  if (!value || typeof value !== 'object') return null
  const entries = Object.entries(value as Record<string, unknown>).filter(
    ([, entry]) => entry !== null && entry !== undefined && entry !== '',
  )
  if (entries.length === 0) return null
  return entries
    .map(([key, entry]) =>
      typeof entry === 'string' && /^[0-9a-f]{40}$/i.test(entry)
        ? `${key} ${entry.slice(0, 12)}`
        : `${key} ${typeof entry === 'object' ? JSON.stringify(entry) : String(entry)}`,
    )
    .join(' · ')
}

/** Provenance rows: only fields with a value, timestamps in the reader's
 *  locale next to the duration, nested records flattened without null keys. */
export function provenanceEntries(
  detail: DashboardExecutionDetail,
  presentation: ExecutionPresentation,
): Array<[string, string]> {
  const started = Date.parse(presentation.startedAt)
  const completed = Date.parse(presentation.completedAt)
  const duration =
    Number.isFinite(started) &&
    Number.isFinite(completed) &&
    completed >= started
      ? formatDuration((completed - started) / 1000)
      : null
  const rows: Array<[string, string | null | undefined]> = [
    ['execution id', detail.id],
    ['run id', detail.run_id],
    ['attempt', detail.attempt == null ? null : String(detail.attempt)],
    ['status', detail.status],
    ['availability', detail.availability],
    [
      'slot start deadline',
      detail.slot_start_deadline_seconds == null
        ? null
        : `${detail.slot_start_deadline_seconds}s (soft limit for starting new slots)`,
    ],
    ['event', detail.event],
    ['actor', detail.actor],
    [
      'started',
      presentation.startedAt ? formatDate(presentation.startedAt) : null,
    ],
    [
      'completed',
      presentation.completedAt
        ? `${formatDate(presentation.completedAt)}${duration ? ` · ${duration}` : ''}`
        : null,
    ],
    ['source', compactObject(detail.source)],
    ['release', compactObject(detail.release)],
  ]
  return rows.filter((row): row is [string, string] => Boolean(row[1]))
}

export type ResultFilter = 'all' | OperationalStatus

/** Counts per result for the filter row, in severity order. */
export function resultFilterCounts(
  items: ScenarioMatrixItem[],
): Array<[OperationalStatus, number]> {
  const order: OperationalStatus[] = [
    'failed',
    'unavailable',
    'inconclusive',
    'incomplete',
    'running',
    'cancelling',
    'cancelled',
    'recommendation',
    'passed',
  ]
  const counts = new Map<OperationalStatus, number>()
  for (const item of items)
    counts.set(
      item.objective.status,
      (counts.get(item.objective.status) ?? 0) + 1,
    )
  return order
    .filter((status) => counts.has(status))
    .map((status) => [status, counts.get(status) ?? 0])
}

export function filterResults(
  items: ScenarioMatrixItem[],
  filter: ResultFilter,
): ScenarioMatrixItem[] {
  return filter === 'all'
    ? items
    : items.filter((item) => item.objective.status === filter)
}

/** Artifacts an imported execution retained, as report and path pairs. */
export function retainedArtifacts(
  detail: DashboardExecutionDetail,
): Array<{ reportId: string; path: string }> {
  return (detail.retained_reports ?? []).flatMap((value) => {
    if (!value || typeof value !== 'object' || Array.isArray(value)) return []
    const report = value as Record<string, unknown>
    const reportId =
      typeof report.id === 'string'
        ? report.id
        : typeof report.report_id === 'string'
          ? report.report_id
          : null
    const paths = Array.isArray(report.artifacts) ? report.artifacts : []
    if (!reportId) return []
    return paths.flatMap((artifact) => {
      const path =
        typeof artifact === 'string'
          ? artifact
          : artifact &&
              typeof artifact === 'object' &&
              typeof (artifact as Record<string, unknown>).path === 'string'
            ? String((artifact as Record<string, unknown>).path)
            : null
      return path ? [{ reportId, path }] : []
    })
  })
}

/** One line for the verdict panel headline when the report exists. */
export function verdictHeadline(verdict: ExecutionVerdict): string {
  return verdict.headline
}
