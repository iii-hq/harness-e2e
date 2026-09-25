import { ExternalLink } from 'lucide-react'
import { useState } from 'react'
import { Callout, Dialog } from '@/design-system'
import type { DashboardDataBridge } from '@/lib/dashboard-data-source'
import { type PlanExecution, running } from '@/lib/plan-execution'
import './where-it-ran.css'
import {
  cancelCopy,
  dockerSteps,
  jobDuration,
  jobLabel,
  jobTests,
  placeOf,
  type TestRow,
  testRows,
} from './where-it-ran-model'

const STATE_LABEL: Record<TestRow['state'], string> = {
  reported: 'Reported',
  running: 'Running',
  waiting: 'Waiting',
  stopped: 'Stopped',
  'at-import': 'Finished',
}

function Dot({ tone }: { tone: 'ok' | 'live' | 'idle' | 'alert' }) {
  return <span className="wr-dot" data-tone={tone} aria-hidden="true" />
}

function TestList({ rows }: { rows: TestRow[] }) {
  if (rows.length === 0) return null
  return (
    <ul className="wr-tests" aria-label="Tests">
      {rows.map((row) => (
        <li key={row.id} className="wr-test" data-state={row.state}>
          <Dot
            tone={
              row.state === 'running'
                ? 'live'
                : row.state === 'reported' || row.state === 'at-import'
                  ? 'ok'
                  : 'idle'
            }
          />
          <span className="wr-mono wr-ellipsis" title={row.id}>
            {row.id}
          </span>
          <span className="wr-state">{STATE_LABEL[row.state]}</span>
          <span className="wr-faint wr-ellipsis" title={row.detail}>
            {row.detail}
          </span>
        </li>
      ))}
    </ul>
  )
}

/** Where the execution runs and what happens there: this harness, Docker
 *  (steps and groups) or GitHub (run, ref and group jobs). */
export function WhereItRan({
  execution,
  dockerGroups = 2,
}: {
  execution: PlanExecution
  dockerGroups?: number
}) {
  const source = execution.source
  const live = running(execution.state) || execution.state === 'importing'
  const place = placeOf(execution)
  return (
    <section
      className="wr-card"
      aria-labelledby="where-it-ran-title"
      data-where={place}
    >
      <header className="wr-head">
        <h2 id="where-it-ran-title" className="wr-title">
          Where it ran
        </h2>
        <span className="wr-faint">
          {place === 'github'
            ? 'GitHub'
            : place === 'docker'
              ? 'Docker'
              : 'This harness'}
        </span>
      </header>

      {source.kind === 'github' ? (
        <>
          <dl className="wr-facts">
            <dt>Run</dt>
            <dd>
              <a
                className="wr-link"
                href={source.url}
                target="_blank"
                rel="noreferrer"
              >
                GitHub #{source.run_id}
                {source.run_attempt > 1
                  ? ` · attempt ${source.run_attempt}`
                  : ''}
                <ExternalLink size={12} aria-hidden="true" />
              </a>
            </dd>
            <dt>Workflow</dt>
            <dd className="wr-mono">
              exact-stack-e2e.yml
              {source.follow?.head_branch
                ? ` @ ${source.follow.head_branch}`
                : ''}
              {source.follow?.head_sha
                ? ` ${source.follow.head_sha.slice(0, 7)}`
                : ''}
            </dd>
            {source.release_control_execution_id ? (
              <>
                <dt>Reports</dt>
                <dd className="wr-mono">
                  Release Control{' '}
                  {source.release_control_execution_id.slice(0, 8)}
                </dd>
              </>
            ) : null}
            <dt>Import</dt>
            <dd>
              {execution.state === 'importing'
                ? 'Importing what finished…'
                : execution.state === 'cancelling'
                  ? 'GitHub is finishing the cancel; what finished is imported when the run ends.'
                  : live
                    ? 'Automatic when the run ends'
                    : 'Imported'}
            </dd>
          </dl>
          {source.follow?.jobs && source.follow.jobs.length > 0 ? (
            <ul className="wr-jobs" aria-label="Group jobs">
              {source.follow.jobs.map((job) => {
                const label = jobLabel(job)
                const tests = jobTests(job, execution)
                return (
                  <li
                    key={job.id}
                    className="wr-job"
                    data-job-state={label.toLowerCase()}
                  >
                    <Dot
                      tone={
                        label === 'Running'
                          ? 'live'
                          : label === 'Done'
                            ? 'ok'
                            : label === 'Failed'
                              ? 'alert'
                              : 'idle'
                      }
                    />
                    <span className="wr-job-name">
                      <span className="wr-mono wr-ellipsis" title={job.name}>
                        {job.name}
                      </span>
                      {tests.length ? (
                        <span className="wr-faint wr-ellipsis">
                          {tests.join(', ')}
                        </span>
                      ) : null}
                    </span>
                    <span className="wr-state">{label}</span>
                    <span className="wr-mono wr-faint">{jobDuration(job)}</span>
                    {job.url ? (
                      <a
                        className="wr-icon-link"
                        href={job.url}
                        target="_blank"
                        rel="noreferrer"
                        aria-label={`Open ${job.name} on GitHub`}
                      >
                        <ExternalLink size={14} aria-hidden="true" />
                      </a>
                    ) : null}
                  </li>
                )
              })}
            </ul>
          ) : live ? (
            <p className="wr-faint" role="status">
              Waiting for GitHub to start the jobs…
            </p>
          ) : null}
        </>
      ) : null}

      {source.kind === 'docker'
        ? (execution.warnings ?? [])
            .filter((warning) =>
              /provider_env_file|without credentials/.test(warning),
            )
            .map((warning) => (
              <Callout
                key={warning}
                tone="warning"
                title="No provider credentials"
              >
                {warning} Recorded when the execution started.
              </Callout>
            ))
        : null}

      {source.kind === 'docker' ? (
        <>
          <ol className="wr-steps" aria-label="Steps">
            {dockerSteps(execution, dockerGroups).map((step) => (
              <li
                key={step.phase}
                className="wr-step"
                data-step-state={step.state}
              >
                <Dot
                  tone={
                    step.state === 'done'
                      ? 'ok'
                      : step.state === 'current'
                        ? 'live'
                        : step.state === 'stopped'
                          ? 'alert'
                          : 'idle'
                  }
                />
                <span className="wr-strong">{step.label}</span>
                <span className="wr-faint">{step.detail}</span>
              </li>
            ))}
          </ol>
          <dl className="wr-facts">
            {source.image ? (
              <>
                <dt>Image</dt>
                <dd className="wr-mono">{source.image}</dd>
              </>
            ) : null}
            {execution.parameters?.stack ? (
              <>
                <dt>Stack</dt>
                <dd className="wr-mono">
                  {execution.parameters.stack.name}
                  {execution.parameters.stack.sha256
                    ? ` · ${execution.parameters.stack.sha256.replace('sha256:', '').slice(0, 12)}, locked`
                    : ''}
                </dd>
              </>
            ) : null}
          </dl>
          <ul className="wr-jobs" aria-label="Groups" data-docker-groups>
            {source.groups.map((group) => (
              <li
                key={`${group.round}:${group.group_id}`}
                className="wr-job"
                data-docker-group={group.group_id}
              >
                <Dot
                  tone={
                    group.state === 'running'
                      ? 'live'
                      : group.state === 'done'
                        ? 'ok'
                        : group.state === 'failed'
                          ? 'alert'
                          : 'idle'
                  }
                />
                <span className="wr-job-name">
                  <span className="wr-mono wr-ellipsis">{group.group_id}</span>
                  <span className="wr-faint wr-ellipsis">
                    {group.scenarios.join(', ')}
                  </span>
                  {group.error ? (
                    <span className="wr-error">{group.error}</span>
                  ) : null}
                </span>
                <span className="wr-state" data-group-state>
                  {group.state}
                </span>
                <span className="wr-mono wr-faint">
                  attempt {group.attempt}
                </span>
              </li>
            ))}
          </ul>
        </>
      ) : null}

      {source.kind === 'local' ? (
        <dl className="wr-facts">
          <dt>Runner</dt>
          <dd>This harness, on the stack this Console runs on</dd>
        </dl>
      ) : null}

      {live && source.kind !== 'github' ? (
        <TestList rows={testRows(execution, dockerGroups)} />
      ) : null}
    </section>
  )
}

/** Cancel where it runs, saying what stops and what is kept. */
export function CancelExecutionDialog({
  bridge,
  execution,
  open,
  onClose,
  onCancelled,
}: {
  bridge: DashboardDataBridge | null
  execution: PlanExecution
  open: boolean
  onClose: () => void
  onCancelled: () => void
}) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const copy = cancelCopy(execution)
  const confirm = async () => {
    if (!bridge) return
    setBusy(true)
    setError(null)
    try {
      await bridge.cancelExecution(execution.id)
      onCancelled()
      onClose()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }
  return (
    <Dialog
      open={open}
      onClose={() => (busy ? undefined : onClose())}
      size="sm"
      title={copy.title}
      description={copy.body}
      bodyPadding
      footer={
        <div className="wr-dialog-actions">
          <button
            type="button"
            className="ds-button ds-button-quiet ds-button-default"
            onClick={onClose}
            disabled={busy}
          >
            Keep it running
          </button>
          <button
            type="button"
            className="ds-button ds-button-primary ds-button-default"
            onClick={() => void confirm()}
            disabled={busy}
            aria-busy={busy || undefined}
          >
            {busy ? 'Cancelling…' : copy.action}
          </button>
        </div>
      }
    >
      {error ? (
        <p role="alert" className="wr-error">
          {error}
        </p>
      ) : null}
    </Dialog>
  )
}
