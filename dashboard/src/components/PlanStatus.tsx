import { Callout, Panel } from '@/design-system'
import { hashForExecution, hashForPlan } from '@/hooks/use-hash-route'
import {
  type PlanExecution,
  type PlanRequirements,
  running,
} from '@/lib/plan-execution'

export function Requirements({ value }: { value: PlanRequirements }) {
  const active = value.active_execution
  return (
    <Panel aria-label="Execution requirements">
      <h2 className="mt-0 text-sm font-semibold text-ink">
        Execution requirements
      </h2>
      {active ? (
        <Callout tone="warning" title="Another execution is active">
          Your saved draft is preserved.{' '}
          <a
            className="underline"
            href={
              active.plan_id
                ? hashForPlan(active.plan_id)
                : hashForExecution(active.id)
            }
          >
            Follow active execution
          </a>
        </Callout>
      ) : null}
      <ul className="m-0 grid gap-2 pl-5 text-xs leading-5 text-ink-soft">
        {value.checks.map((check) => (
          <li key={check.id}>
            <strong
              className={
                check.status === 'blocked' ? 'text-danger' : 'text-ink'
              }
            >
              {check.status === 'pending'
                ? 'Pending'
                : check.status === 'blocked'
                  ? 'Blocked'
                  : 'Ready'}
            </strong>{' '}
            · {check.message}
          </li>
        ))}
      </ul>
    </Panel>
  )
}

export function PlanProgress({ execution }: { execution: PlanExecution }) {
  const planned = execution.slots.length
  const finished = execution.slots.filter((s) => s.state === 'finished').length
  const observed = execution.slots.reduce((sum, s) => sum + s.observed, 0)
  const active = execution.slots.find(
    (s) => s.state === 'running' || s.state === 'admitting',
  )
  const total = (field: 'passed' | 'completed' | 'technical_valid') =>
    execution.slots.reduce((sum, s) => sum + s[field], 0)
  return (
    <Panel aria-label="Plan execution progress">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="my-0 text-sm font-semibold text-ink">
          {execution.role === 'baseline'
            ? 'Baseline execution'
            : 'Candidate execution'}{' '}
          · {execution.state}
        </h2>
        <span className="text-xs text-ink-soft">
          {finished} / {planned} slots finished
        </span>
      </div>
      <progress
        className="mt-4 h-2 w-full accent-current text-ink"
        value={finished}
        max={planned || 1}
        aria-label="Finished planned slots"
      />
      <p className="text-xs text-ink-soft" aria-live="polite">
        {active
          ? `Round ${active.round} · ${active.scenario_id}`
          : running(execution.state)
            ? 'Preparing the next scenario…'
            : `${planned - observed} slots without observations. ${execution.state === 'interrupted' ? 'Run again starts a new complete execution.' : ''}`}
      </p>
      <dl className="grid grid-cols-2 gap-4 text-xs md:grid-cols-4">
        {[
          ['Execution completion', total('completed')],
          ['Objective correctness', total('passed')],
          ['Technical validity', total('technical_valid')],
          ['Observation coverage', observed],
        ].map(([label, value]) => (
          <div key={label}>
            <dt className="text-ink-soft">{label}</dt>
            <dd className="mx-0 mt-1 text-lg font-semibold text-ink">
              {value} / {planned}
            </dd>
          </div>
        ))}
      </dl>
      {execution.error ? (
        <Callout tone="warning" title="Execution evidence">
          {execution.error}
        </Callout>
      ) : null}
    </Panel>
  )
}
