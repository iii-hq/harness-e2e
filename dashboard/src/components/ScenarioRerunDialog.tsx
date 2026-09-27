import {
  Button,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from '@iii-dev/console-ui'
import { AlertTriangle, ExternalLink } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import {
  jobTests,
  plural,
  shortImage,
  stackLine,
} from '@/components/execution/where-it-ran-model'
import { describeStartError } from '@/components/LocalRunnerDialog'
import { hashForExecution } from '@/hooks/use-hash-route'
import type { DashboardDataBridge } from '@/lib/dashboard-data-source'
import { providerModel } from '@/lib/execution-view'
import { type PlanExecution, rerunGroup } from '@/lib/plan-execution'

type Fact = { label: string; value: ReactNode; mono?: boolean }

/** What running one test again says where it runs (canvas: Run a test
 *  again · this harness, Docker, GitHub): the body, the facts and the
 *  action, which names the place when it is not this harness. */
export function rerunCopy(execution: PlanExecution, scenarioId: string) {
  const source = execution.source
  const parameters = execution.parameters
  const model = parameters?.model
    ? providerModel({ provider: parameters.provider, model: parameters.model })
    : null
  const slot = execution.slots.find((entry) => entry.scenario_id === scenarioId)
  if (source.kind === 'docker') {
    const facts: Fact[] = [{ label: 'Where', value: 'Docker' }]
    if (slot?.group_id)
      facts.push({ label: 'Group', value: slot.group_id, mono: true })
    if (source.image)
      facts.push({
        label: 'Executor image',
        value: <span title={source.image}>{shortImage(source.image)}</span>,
        mono: true,
      })
    if (parameters?.stack)
      facts.push({
        label: 'Stack',
        value: `${stackLine(parameters.stack)}${parameters.stack.sha256 ? ', locked' : ''}`,
        mono: true,
      })
    facts.push({
      label: 'Attempt',
      value: `${source.attempt + 1} of this execution`,
    })
    return {
      body: `Its group runs again in a new container with this execution’s contract, stack lock and executor image, as attempt ${source.attempt + 1}. Then the execution is aggregated and imported again. The last attempt counts.`,
      action: 'Run again in Docker',
      facts,
      note: null,
    }
  }
  if (source.kind === 'github') {
    const job = source.follow?.jobs?.find((entry) =>
      jobTests(entry, execution).includes(scenarioId),
    )
    const facts: Fact[] = [
      {
        label: 'Where',
        value: (
          <a
            className="ep-confirm-link"
            href={source.url}
            target="_blank"
            rel="noreferrer"
          >
            GitHub · run #{source.run_id}
            <ExternalLink size={12} aria-hidden="true" />
          </a>
        ),
      },
    ]
    if (job || slot?.group_id)
      facts.push({
        label: 'Job',
        value: job?.name ?? slot?.group_id,
        mono: true,
      })
    facts.push({
      label: 'Attempt',
      value: `${source.run_attempt + 1} of the run`,
    })
    if (model) facts.push({ label: 'Model', value: model, mono: true })
    return {
      body: 'Re-runs its group’s job on GitHub, which redoes the aggregate job. The Console imports the run again when it ends. The last attempt counts.',
      action: 'Re-run the job',
      facts,
      note: source.release_control_execution_id
        ? `This run reports to Release Control execution ${source.release_control_execution_id.slice(0, 8)}, so the new attempt reports there too.`
        : null,
    }
  }
  const facts: Fact[] = [{ label: 'Where', value: 'This harness' }]
  if (model) facts.push({ label: 'Model', value: model, mono: true })
  if (parameters)
    facts.push(
      { label: 'Profile', value: parameters.agent ?? 'none', mono: true },
      {
        label: 'Runs',
        value: `${parameters.runs} · ${plural(parameters.technical_retries, 'technical retry', 'technical retries')}`,
      },
    )
  return {
    body: 'It runs on this harness with this execution’s model, profile, runs and technical retries. Its new result replaces this one; the last attempt counts.',
    action: 'Run again',
    facts,
    note: null,
  }
}

/** Run one test of a finished execution again, where it ran: here, in a new
 *  Docker container as its next attempt, or its group's job on GitHub. The
 *  host's dialog, as the list's confirmations. */
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
  const copy = rerunCopy(execution, scenarioId ?? '')
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
  const notes = [
    rounds > 1 ? `All ${rounds} of its rounds run again.` : null,
    group.length > 1
      ? `${group.join(' then ')} run only together, in this order; the whole group runs again.`
      : null,
    copy.note,
  ].filter((note): note is string => Boolean(note))
  return (
    <Dialog
      open={scenarioId !== null}
      onOpenChange={(open) => {
        if (!open) close()
      }}
    >
      <DialogContent
        className="ex-dialog ep-confirm-wide"
        aria-describedby="ep-rerun-body"
        data-scenario-rerun
      >
        <div>
          <DialogTitle className="ex-dialog-title">
            Run <span className="ep-confirm-mono">{scenarioId}</span> again?
          </DialogTitle>
          <DialogDescription id="ep-rerun-body" className="ex-dialog-body">
            {copy.body}
          </DialogDescription>
        </div>
        <dl className="ep-confirm-facts">
          {copy.facts.map((fact) => (
            <div key={fact.label} className="contents">
              <dt>{fact.label}</dt>
              <dd className={fact.mono ? 'ep-confirm-mono' : undefined}>
                {fact.value}
              </dd>
            </div>
          ))}
        </dl>
        {notes.length ? (
          <ul className="ex-dialog-facts">
            {notes.map((note) => (
              <li key={note} data-tone="warn">
                <AlertTriangle size={16} aria-hidden="true" />
                <span>{note}</span>
              </li>
            ))}
          </ul>
        ) : null}
        {error ? (
          <p className="ex-error" role="alert">
            {error}
          </p>
        ) : null}
        <div className="ex-dialog-actions">
          {running ? (
            <a className="ep-confirm-link" href={hashForExecution(running.id)}>
              Open {running.title}
            </a>
          ) : null}
          <Button
            type="button"
            variant="pill"
            size="sm"
            disabled={submitting}
            onClick={close}
          >
            Cancel
          </Button>
          <Button
            type="button"
            variant="primary"
            size="sm"
            disabled={submitting || !bridge}
            aria-busy={submitting}
            onClick={() => void run()}
          >
            {submitting ? 'Starting…' : copy.action}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  )
}
