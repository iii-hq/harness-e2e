import type { ReactNode } from 'react'
import { Callout, Panel } from '@/design-system'
import { formatDate, formatDuration } from '@/lib/execution-view'
import { type PlanExecution, running } from '@/lib/plan-execution'

const dockerPhases: Record<string, string> = {
  prepare: 'Materializing the suite and assembling the stack…',
  groups: 'Running the groups…',
  finalize: 'Aggregating the groups…',
  import: 'Importing the results…',
  done: 'Done',
}

/** A running execution: how many of its slots finished, and which runs now.
 *  One in Docker lists its groups; its results arrive with the import. */
export function ExecutionProgress({
  execution,
  actions,
}: {
  execution: PlanExecution
  actions?: ReactNode
}) {
  const planned = execution.slots.length
  const finished = execution.slots.filter((s) => s.state === 'finished').length
  const observed = execution.slots.reduce((sum, s) => sum + s.observed, 0)
  const active = execution.slots.find(
    (s) => s.state === 'running' || s.state === 'admitting',
  )
  const total = (field: 'passed' | 'completed' | 'technical_valid') =>
    execution.slots.reduce((sum, s) => sum + s[field], 0)
  return (
    <Panel aria-label="Execution progress">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="my-0 text-sm font-semibold text-ink">
          Execution · {execution.state}
        </h2>
        <span className="text-xs text-ink-soft">
          {finished} / {planned} slots finished
        </span>
        {actions}
      </div>
      <progress
        className="mt-4 h-2 w-full accent-current text-ink"
        value={finished}
        max={planned || 1}
        aria-label="Finished planned slots"
      />
      {execution.rerun && running(execution.state) ? (
        <p className="mb-0 text-xs text-ink" data-rerun-progress>
          {`${execution.rerun.scenarios.join(', ')} running again since ${formatDate(execution.rerun.started_at)}`}
          {Number.isFinite(Date.parse(execution.rerun.started_at))
            ? ` · ${formatDuration((Date.now() - Date.parse(execution.rerun.started_at)) / 1000)} elapsed`
            : ''}
          {' · totals update when it finishes'}
        </p>
      ) : null}
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
      {execution.source?.kind === 'docker' ? (
        <div className="grid gap-2 text-xs" data-docker-groups>
          <p className="m-0 text-ink-soft">
            {dockerPhases[execution.source.phase] ?? execution.source.phase}
          </p>
          <ul className="m-0 grid list-none gap-1 p-0">
            {execution.source.groups.map((group) => (
              <li
                key={`${group.round}:${group.group_id}`}
                className="flex min-w-0 flex-wrap items-baseline gap-x-3 font-mono"
                data-docker-group={group.group_id}
              >
                <span className="text-ink">{group.group_id}</span>
                <span className="text-ink-soft" data-group-state>
                  {group.state}
                  {group.attempt > 1 ? ` · attempt ${group.attempt}` : ''}
                </span>
                {group.error ? (
                  <span className="min-w-0 break-words font-sans text-warning">
                    {group.error}
                  </span>
                ) : null}
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      {execution.error ? (
        <Callout tone="warning" title="Execution evidence">
          {execution.error}
        </Callout>
      ) : null}
    </Panel>
  )
}
