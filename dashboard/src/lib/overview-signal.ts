import type { DashboardExecutionSummary } from '@/lib/dashboard-data-source'
import {
  buildExecutionPresentation,
  type ExecutionPresentation,
  formatDate,
} from '@/lib/execution-view'

/** Attention rows the Overview shows before the ledger takes over (audit O-06). */
export const ATTENTION_LIMIT = 5
/** Executions the Overview lists before "view all executions" (audit O-01). */
export const RECENT_LIMIT = 5
/** Sample the trend captions compare against (audit O-16). */
export const TREND_SAMPLE = 10

/**
 * Audit O-09: 14 of 20 executions share the workflow glob as their label.
 * Without a real label the title becomes "<subject> · <date>" and the glob
 * drops to the secondary line.
 */
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
    return {
      title: `${subject.model} · ${formatDate(presentation.completedAt)}`,
      detail: workflow || null,
    }
  }
  return { title: workflow || 'Harness E2E execution', detail: null }
}

function finite(value: number | null | undefined): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

/**
 * Rates arrive either as a 0–1 fraction or as 0–100 points depending on the
 * publisher; the signal always works in points so a delta reads as "pts".
 */
export function percentPoints(value: number | null | undefined): number | null {
  const known = finite(value)
  if (known === null) return null
  return Math.abs(known) <= 1 ? known * 100 : known
}

export function median(
  values: Array<number | null | undefined>,
): number | null {
  const known = values
    .map(finite)
    .filter((value): value is number => value !== null)
    .sort((left, right) => left - right)
  if (known.length === 0) return null
  const middle = Math.floor(known.length / 2)
  return known.length % 2
    ? known[middle]
    : (known[middle - 1] + known[middle]) / 2
}

export type SignalMetric = {
  /** Difference against the previous comparable execution, null when unknown. */
  delta: number | null
  /** Median across the recent sample, for the caption. */
  median: number | null
  sampleSize: number
}

/**
 * Audit O-16: each headline number carries how it moved. The delta compares
 * the latest execution with the previous one that reported the same metric;
 * the median comes from the recent sample.
 */
export function signalMetric(
  presentations: ExecutionPresentation[],
  read: (presentation: ExecutionPresentation) => number | null,
  sample = TREND_SAMPLE,
): SignalMetric {
  const [latest, ...rest] = presentations
  const current = latest ? finite(read(latest)) : null
  const previous = rest
    .map(read)
    .map(finite)
    .find((value) => value !== null)
  const window = rest.slice(0, sample).map(read)
  return {
    delta:
      current === null || previous === undefined || previous === null
        ? null
        : current - previous,
    median: median(window),
    sampleSize: window.map(finite).filter((value) => value !== null).length,
  }
}

export type AttentionEntry = {
  presentation: ExecutionPresentation
  category: string
  count: number
}

/**
 * Audit O-06: the executions that need a person, newest first, out of the
 * window the Overview loaded. Running and cancelled runs are not attention.
 */
export function attentionQueue(
  presentations: ExecutionPresentation[],
  limit = ATTENTION_LIMIT,
): AttentionEntry[] {
  return presentations
    .filter(
      (presentation) =>
        presentation.attention === 'needs_attention' &&
        presentation.primaryIssue !== null,
    )
    .slice(0, limit)
    .map((presentation) => ({
      presentation,
      category: presentation.primaryIssue?.category ?? 'inconclusive',
      count: presentation.primaryIssue?.count ?? 0,
    }))
}

export function runningExecutions(presentations: ExecutionPresentation[]) {
  return presentations.filter(
    (presentation) =>
      presentation.attention === 'running' ||
      presentation.attention === 'cancelling',
  )
}

export type OverviewSignal = {
  presentations: ExecutionPresentation[]
  latest: ExecutionPresentation | null
  running: ExecutionPresentation[]
  attention: AttentionEntry[]
  attentionTotal: number
  recent: ExecutionPresentation[]
  passRate: SignalMetric
  coverage: SignalMetric
  runtime: SignalMetric
  tokens: SignalMetric
}

export function buildOverviewSignal(
  executions: DashboardExecutionSummary[],
): OverviewSignal {
  const presentations = executions.map(buildExecutionPresentation)
  const running = runningExecutions(presentations)
  // The headline is the newest execution that finished; a running one gets
  // its own strip so the numbers never mix states (audit O-17).
  const settled = presentations.filter(
    (presentation) =>
      presentation.attention !== 'running' &&
      presentation.attention !== 'cancelling',
  )
  return {
    presentations,
    latest: settled[0] ?? presentations[0] ?? null,
    running,
    attention: attentionQueue(presentations),
    attentionTotal: presentations.filter(
      (presentation) => presentation.attention === 'needs_attention',
    ).length,
    recent: presentations.slice(0, RECENT_LIMIT),
    passRate: signalMetric(settled, (item) => percentPoints(item.passRate)),
    coverage: signalMetric(settled, (item) => percentPoints(item.coverage)),
    runtime: signalMetric(settled, (item) => item.modelRuntimeSeconds),
    tokens: signalMetric(settled, (item) =>
      finite(item.execution.totals?.total_tokens as number | undefined),
    ),
  }
}
