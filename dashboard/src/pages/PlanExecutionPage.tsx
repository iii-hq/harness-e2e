import { useEffect, useState } from 'react'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import {
  buttonClassName,
  Callout,
  DataTable,
  PageHeader,
  Panel,
} from '@/design-system'
import { hashForExecution, hashForPlan } from '@/hooks/use-hash-route'
import { getDashboardDataBridge } from '@/lib/dashboard-data-source'
import {
  type PlanExecution,
  type PlanRequirements,
  profileAction,
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
          {execution.role === 'run'
            ? 'Execution'
            : execution.role === 'baseline'
              ? 'Reference execution'
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

export function PlanExecutionPage({ executionId }: { executionId: string }) {
  const [execution, setExecution] = useState<PlanExecution | null>(null)
  const [error, setError] = useState('')
  useEffect(() => {
    let active = true
    const load = async () => {
      try {
        const bridge = await getDashboardDataBridge()
        const value = await profileAction<PlanExecution>(bridge, {
          action: 'execution',
          execution_id: executionId,
        })
        if (active) setExecution(value)
      } catch (cause) {
        if (active) setError(String(cause))
      }
    }
    void load()
    const timer = setInterval(() => void load(), 2500)
    return () => {
      active = false
      clearInterval(timer)
    }
  }, [executionId])
  const cancel = async () => {
    try {
      const bridge = await getDashboardDataBridge()
      setExecution(
        await profileAction<PlanExecution>(bridge, {
          action: 'cancel',
          execution_id: executionId,
        }),
      )
    } catch (cause) {
      setError(String(cause))
    }
  }
  return (
    <>
      <DashboardPageActions active="plans" />
      <div className="ds-root page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-16 md:w-[calc(100%_-_3rem)]">
        <PageHeader
          title="Plan execution"
          summary="Every planned slot and its native evidence."
        />
        {error ? (
          <Callout tone="warning" title="Execution unavailable">
            {error}
          </Callout>
        ) : null}
        {execution ? (
          <div className="mt-5 grid gap-5">
            <a href={hashForPlan(execution.plan_id)}>Back to plan</a>
            <PlanProgress execution={execution} />
            {running(execution.state) ? (
              <button
                type="button"
                className={buttonClassName({
                  variant: 'secondary',
                  className: 'justify-self-start',
                })}
                disabled={execution.state === 'cancelling'}
                onClick={() => void cancel()}
              >
                Cancel execution
              </button>
            ) : null}
            <DataTable
              caption="Planned scenario slots and native evidence"
              collapse
            >
              <thead>
                <tr>
                  <th>Round</th>
                  <th>Scenario</th>
                  <th>State</th>
                  <th>Objective</th>
                  <th>Technical validity</th>
                  <th>Evidence</th>
                </tr>
              </thead>
              <tbody>
                {execution.slots.map((slot) => (
                  <tr key={slot.execution_id}>
                    <td data-label="Round">{slot.round}</td>
                    <td data-label="Scenario" className="break-words">
                      {slot.scenario_id}
                    </td>
                    <td data-label="State">
                      {slot.state.replaceAll('_', ' ')}
                      {slot.error ? (
                        <p className="text-xs text-danger">{slot.error}</p>
                      ) : null}
                    </td>
                    <td data-label="Objective">
                      {slot.observed === 0 || slot.technical_valid === 0 ? (
                        'Unavailable'
                      ) : slot.passed === 1 ? (
                        'Passed'
                      ) : (
                        <span className="text-danger">Failed</span>
                      )}
                    </td>
                    <td data-label="Technical validity">
                      {slot.observed === 0
                        ? 'Unavailable'
                        : slot.technical_valid === 1
                          ? 'Valid'
                          : 'Not valid'}
                    </td>
                    <td data-label="Evidence">
                      {slot.state !== 'pending' && slot.state !== 'not_run' ? (
                        <a
                          className="text-xs text-ink underline"
                          href={hashForExecution(slot.execution_id)}
                        >
                          Open native execution
                        </a>
                      ) : (
                        'Not produced'
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </DataTable>
          </div>
        ) : (
          <p role="status">Loading execution…</p>
        )}
      </div>
    </>
  )
}
