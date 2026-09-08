import { type OperationalStatus, StatusBadge } from '@/design-system'
import { titleCase } from '@/lib/execution-view'

/** The result contract publishes one outcome: the system status. It used to
 *  be rendered beside an advisory AI verdict and an "effective" combination
 *  of the two, which read as three findings instead of one (audit ED-21).
 *  Both of those are gone, so a single badge says everything there is. */
export type SystemOutcome = { value: string; label?: string }

const PASSING = new Set(['passed', 'complete', 'available', 'valid'])
const CONCERNED = new Set(['partial', 'inconclusive'])
const FAILING = new Set([
  'failed',
  'hard_gate_failed',
  'subject_error',
  'judge_error',
  'resource_limit',
  'infrastructure_error',
  'error',
  'malformed',
])

/** Maps a system status onto the design system's status vocabulary. Anything
 *  unrecognised reads as unavailable rather than inventing a verdict
 *  (audit AW-04). */
export function outcomeStatus(value: string): OperationalStatus {
  if (PASSING.has(value)) return 'passed'
  if (CONCERNED.has(value)) return 'inconclusive'
  if (FAILING.has(value)) return 'failed'
  return 'unavailable'
}

export function SystemOutcomeBadge({
  outcome,
  className,
}: {
  outcome: SystemOutcome
  className?: string
}) {
  const label =
    outcome.label ??
    (outcome.value === 'hard_gate_failed'
      ? 'failed (legacy result)'
      : titleCase(outcome.value).toLowerCase())
  return (
    <div
      className={`grid items-baseline gap-x-4 gap-y-0.5 ${className ?? ''}`}
      data-system-outcome
    >
      <span className="min-w-0 [&_.ds-status-badge]:whitespace-normal">
        <StatusBadge label={label} status={outcomeStatus(outcome.value)} />
      </span>
      <span className="font-mono text-label text-ink-muted">
        system · completion, execution and infrastructure
      </span>
    </div>
  )
}
