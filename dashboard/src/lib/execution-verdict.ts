import type { AssessmentRunView } from '@/lib/assessment-view'
import type { ExecutionPresentation } from '@/lib/execution-view'
import { unsupportedExecutionReason } from '@/lib/execution-view'
import type {
  ScenarioMatrixItem,
  ScenarioMatrixSummary,
} from '@/lib/scenario-matrix'

/** States where no scenario report exists, so nothing can be decided. */
export const NO_RUN_STATES = new Set([
  'cancelled',
  'cancelling',
  'running',
  'incomplete',
  'unavailable',
  'unsupported',
])

export type ExecutionVerdict = {
  /** One sentence for the whole execution, not per scenario (audit ED-03). */
  headline: string
  /** The scenario the next step comes from, when there is one. */
  worst: ScenarioMatrixItem | null
  /** What to do next, from the worst scenario's advisory or a fallback. */
  nextStep: string
  /** What happened, from the worst scenario's diagnosis, when retained. */
  diagnosis: string | null
}

const SEVERITY: Array<{ status: string; label: string; plural: string }> = [
  { status: 'failed', label: 'failure', plural: 'failures' },
  {
    status: 'hard_gate',
    label: 'hard gate failed',
    plural: 'hard gates failed',
  },
  {
    status: 'unavailable',
    label: 'without evidence',
    plural: 'without evidence',
  },
  { status: 'inconclusive', label: 'inconclusive', plural: 'inconclusive' },
  { status: 'incomplete', label: 'incomplete', plural: 'incomplete' },
  { status: 'running', label: 'still running', plural: 'still running' },
]

function countFor(summary: ScenarioMatrixSummary, status: string) {
  if (status === 'failed') return summary.failed
  if (status === 'hard_gate') return summary.hardGate
  if (status === 'unavailable') return summary.unavailable
  if (status === 'inconclusive') return summary.inconclusive
  if (status === 'incomplete') return summary.incomplete
  if (status === 'running') return summary.running
  return 0
}

/**
 * Audit ED-03: one aggregated verdict for the execution. The page used to
 * show a per-scenario AI headline next to an objective one, which
 * contradicted itself whenever an execution held more than one scenario.
 */
export function executionVerdict(
  presentation: ExecutionPresentation,
  summary: ScenarioMatrixSummary | null,
  items: ScenarioMatrixItem[] = [],
  primaryRun: AssessmentRunView | null = null,
): ExecutionVerdict {
  const attention = presentation.attention
  if (attention === 'unsupported') {
    return {
      headline: 'unsupported result contract · historical evidence retained',
      worst: null,
      nextStep:
        'Read the original artifacts with a compatible reader. This execution cannot be used as a baseline.',
      diagnosis: unsupportedExecutionReason(presentation.execution),
    }
  }
  if (!summary || summary.total === 0) {
    return {
      headline: NO_RUN_STATES.has(attention)
        ? `${attention.replace(/_/g, ' ')} · no scenario report retained`
        : 'no scenario report retained',
      worst: null,
      nextStep:
        attention === 'cancelled'
          ? 'Re-run the same scope to obtain a report.'
          : attention === 'running' || attention === 'cancelling'
            ? 'The report appears when the run finishes.'
            : 'Re-run the execution to obtain a report.',
      diagnosis: null,
    }
  }
  const parts = SEVERITY.map(({ status, label, plural }) => {
    const count = countFor(summary, status)
    return count > 0 ? `${count} ${count === 1 ? label : plural}` : null
  }).filter(Boolean) as string[]
  if (summary.passed > 0) parts.push(`${summary.passed} passed`)
  const worst =
    SEVERITY.map(({ status }) =>
      items.find((item) => item.objective.status === status),
    ).find(Boolean) ?? null
  const aiResult = primaryRun?.finalAssessment.result
  const worstRun = worst?.primaryRun ?? null
  const worstAssessment =
    worstRun && typeof worstRun === 'object'
      ? (
          worstRun as {
            ai_final_assessment?: {
              result?: { diagnosis?: string; recommendation?: string }
            }
          }
        ).ai_final_assessment?.result
      : undefined
  const diagnosis = worstAssessment?.diagnosis ?? aiResult?.diagnosis ?? null
  const recommendation =
    worstAssessment?.recommendation ?? aiResult?.recommendation ?? null
  return {
    headline: parts.join(' · '),
    worst,
    nextStep:
      recommendation ??
      (parts.length === 1 && summary.passed === summary.total
        ? 'Nothing to act on: every scenario passed.'
        : 'Inspect the retained evidence of the failing scenario before deciding whether to re-run.'),
    diagnosis,
  }
}
