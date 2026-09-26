import { ExternalLink } from 'lucide-react'
import { useState } from 'react'
import { describeStartError } from '@/components/LocalRunnerDialog'
import { buttonClassName, Dialog } from '@/design-system'
import { hashForExecution } from '@/hooks/use-hash-route'
import type { DashboardDataBridge } from '@/lib/dashboard-data-source'
import { type PlanExecution, rerunGroup } from '@/lib/plan-execution'

/** Run one scenario of a finished execution again. A local execution runs it
 *  here, a Docker one in a new container as its next attempt, a GitHub one
 *  as its job's next attempt; one Release Control dispatched says how to run
 *  it again from there instead. */
export function ScenarioRerunDialog({
  bridge,
  execution,
  scenarioId,
  onClose,
  onStarted,
}: {
  bridge: DashboardDataBridge | null
  execution: PlanExecution
  /** The scenario to run again; the dialog is closed without one. */
  scenarioId: string | null
  onClose: () => void
  onStarted: () => void
}) {
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [running, setRunning] = useState<{ id: string; title: string } | null>(
    null,
  )
  const close = () => {
    if (submitting) return
    setError(null)
    setRunning(null)
    onClose()
  }
  const source = execution.source
  if (source.kind === 'github' && source.release_control_execution_id)
    return (
      <Dialog
        open={scenarioId !== null}
        onClose={close}
        size="sm"
        title={`Run ${scenarioId} again from Release Control`}
        description="Release Control dispatched this run on GitHub and reads its reports."
        bodyPadding
        footer={
          <div className="flex justify-end gap-2">
            <button
              type="button"
              className={buttonClassName({ variant: 'secondary' })}
              onClick={close}
            >
              close
            </button>
          </div>
        }
      >
        <p className="m-0 text-sm text-ink">
          Run it again from Release Control, which re-runs its job on GitHub,
          then import the run again: the import takes the run's highest attempt
          and replaces this execution's runs.
        </p>
        <a
          className="mt-3 inline-flex items-center gap-1 text-sm text-ink"
          href={source.url}
          target="_blank"
          rel="noreferrer"
        >
          open GitHub run #{source.run_id}
          <ExternalLink size={12} aria-hidden="true" />
        </a>
      </Dialog>
    )
  const group = scenarioId ? rerunGroup(execution, scenarioId) : []
  const rounds = execution.slots.filter(
    (slot) => slot.scenario_id === scenarioId,
  ).length
  const run = async () => {
    if (!bridge || !scenarioId) return
    setSubmitting(true)
    setError(null)
    setRunning(null)
    try {
      await bridge.rerunScenario(execution.id, scenarioId)
      setSubmitting(false)
      onStarted()
    } catch (cause) {
      const described = await describeStartError(bridge, cause)
      setRunning(described.running)
      setError(described.error)
      setSubmitting(false)
    }
  }
  return (
    <Dialog
      open={scenarioId !== null}
      onClose={close}
      size="sm"
      title={`Run ${scenarioId} again`}
      description={
        source.kind === 'docker'
          ? "Its group runs again in a new container with this execution's contract, stack lock and executor image, as attempt " +
            `${source.attempt + 1}; then the execution is aggregated and imported again.`
          : source.kind === 'github'
            ? `GitHub re-runs its group's job in run #${source.run_id}, then the finalizer, as the run's next attempt; the run is imported again when it ends.`
            : "It runs on this stack with this execution's model, profile, runs and technical retries."
      }
      bodyPadding
      footer={
        <div className="flex flex-wrap items-center justify-end gap-2">
          {running ? (
            <a
              className={buttonClassName({
                variant: 'quiet',
                className: 'no-underline',
              })}
              href={hashForExecution(running.id)}
            >
              open {running.title}
            </a>
          ) : null}
          <button
            type="button"
            className={buttonClassName({ variant: 'secondary' })}
            disabled={submitting}
            onClick={close}
          >
            cancel
          </button>
          <button
            type="button"
            className={buttonClassName({ variant: 'primary' })}
            disabled={submitting || !bridge}
            aria-busy={submitting}
            onClick={() => void run()}
          >
            {submitting ? 'starting…' : 'run again'}
          </button>
        </div>
      }
    >
      <div className="grid gap-2 text-sm text-ink" data-scenario-rerun>
        <p className="m-0">
          The last attempt counts, even when it does worse. The current one
          stays under previous attempts, out of the score, the totals and the
          comparison.
        </p>
        {rounds > 1 ? (
          <p className="m-0">All {rounds} of its rounds run again.</p>
        ) : null}
        {group.length > 1 ? (
          <p className="m-0 text-warning">
            {group.join(' then ')} run only together, in this order; the whole
            group runs again.
          </p>
        ) : null}
        {error ? (
          <p className="m-0 text-danger" role="alert">
            {error}
          </p>
        ) : null}
      </div>
    </Dialog>
  )
}
