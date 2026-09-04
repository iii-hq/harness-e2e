import { type OperationalStatus, StatusBadge } from '@/design-system'
import { titleCase } from '@/lib/execution-view'

/** The three values are one derivation, not three findings: the system decides,
 *  the AI comments, and the contract publishes the combination. Rendering them
 *  as peers — three cards, or a flat run of label/value pairs — made readers
 *  work out the relationship themselves, and the two screens that show them
 *  disagreed on how (audit ED-21). */
export type OutcomeRole = 'system' | 'advisory' | 'effective'

const ROLE_CAPTIONS: Record<OutcomeRole, string> = {
  system: 'deterministic gates, execution and infrastructure — authoritative',
  advisory: 'separate qualitative conclusion, never overrides the system',
  effective: 'the status the result contract publishes',
}

const PASSING = new Set(['passed', 'pass', 'complete', 'available'])
const CONCERNED = new Set([
  'pass_with_concerns',
  'passed_with_concerns',
  'inconclusive',
  'partial',
])
const FAILING = new Set([
  'fail',
  'failed',
  'hard_gate_failed',
  'subject_error',
  'judge_error',
  'resource_limit',
  'infrastructure_error',
  'error',
  'malformed',
])

/** Maps a system status, an AI verdict or an availability value onto the
 *  design system's status vocabulary. Anything unrecognised reads as
 *  unavailable rather than inventing a verdict (audit AW-04). */
export function outcomeStatus(value: string): OperationalStatus {
  if (PASSING.has(value)) return 'passed'
  if (CONCERNED.has(value)) return 'inconclusive'
  if (FAILING.has(value)) return 'failed'
  return 'unavailable'
}

export type OutcomeRow = { role: OutcomeRole; value: string }

export function OutcomeDerivation({
  rows,
  className,
}: {
  rows: OutcomeRow[]
  className?: string
}) {
  const inputs = rows.filter((row) => row.role !== 'effective')
  const effective = rows.find((row) => row.role === 'effective')
  return (
    <dl className={`m-0 grid gap-2 ${className ?? ''}`} data-outcome-derivation>
      {inputs.map((row) => (
        <div
          className="grid items-baseline gap-x-4 gap-y-0.5 @[560px]:grid-cols-[13rem_minmax(0,1fr)]"
          key={row.role}
        >
          <dt className="m-0">
            <StatusBadge
              label={titleCase(row.value).toLowerCase()}
              status={outcomeStatus(row.value)}
            />
          </dt>
          <dd className="m-0 font-mono text-label text-ink-muted">
            {row.role} · {ROLE_CAPTIONS[row.role]}
          </dd>
        </div>
      ))}
      {effective ? (
        // The published status carries the weight; the two above are its inputs.
        <div className="mt-1 grid items-baseline gap-x-4 gap-y-0.5 @[560px]:grid-cols-[13rem_minmax(0,1fr)]">
          <dt className="m-0 text-[0.9375rem] font-semibold [&_.ds-status-badge]:text-[0.9375rem] [&_.ds-status-badge]:font-semibold">
            <StatusBadge
              label={titleCase(effective.value).toLowerCase()}
              status={outcomeStatus(effective.value)}
            />
          </dt>
          <dd className="m-0 font-mono text-label text-ink-soft">
            effective · {ROLE_CAPTIONS.effective}
          </dd>
        </div>
      ) : null}
    </dl>
  )
}
