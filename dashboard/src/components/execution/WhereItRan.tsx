import {
  Button,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from '@iii-dev/console-ui'
import { Check, ExternalLink, Square } from 'lucide-react'
import { useState } from 'react'
import { Callout } from '@/design-system'
import type { DashboardDataBridge } from '@/lib/dashboard-data-source'
import { sentenceCase } from '@/lib/format'
import { type PlanExecution, running } from '@/lib/plan-execution'
import './where-it-ran.css'
import {
  cancelCopy,
  dockerSteps,
  githubSteps,
  jobDuration,
  jobLabel,
  jobTests,
  placeOf,
  plural,
  testRows,
} from './where-it-ran-model'

function Dot({ tone }: { tone: 'ok' | 'live' | 'idle' | 'alert' }) {
  return <span className="wr-dot" data-tone={tone} aria-hidden="true" />
}

/** A GitHub run's group jobs, as GitHub reports them. */
function GroupJobList({ execution }: { execution: PlanExecution }) {
  const source = execution.source
  if (source.kind !== 'github') return null
  return (
    <ul className="wr-jobs" aria-label="Group jobs">
      {(source.follow?.jobs ?? []).map((job) => {
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
                <span className="wr-faint wr-ellipsis">{tests.join(', ')}</span>
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
  )
}

/** A live execution's progress, one representation (canvas: Execution
 *  detail · running): numbered steps in Docker and on GitHub, with GitHub's
 *  group jobs under them. A harness execution's is the bar under its title. */
export function LiveProgress({ execution }: { execution: PlanExecution }) {
  const source = execution.source
  const steps =
    source.kind === 'docker'
      ? dockerSteps(execution)
      : source.kind === 'github'
        ? githubSteps(execution)
        : []
  if (steps.length === 0) return null
  const jobs =
    source.kind === 'github'
      ? (source.follow?.jobs ?? []).filter((job) => /case-/.test(job.name))
      : []
  return (
    <>
      <section
        className="wr-progress"
        aria-labelledby="live-progress-title"
        data-live-progress={source.kind}
      >
        <h2 id="live-progress-title" className="wr-title">
          Progress
        </h2>
        <ol className="wr-step-cards">
          {steps.map((step, index) => (
            <li
              key={step.phase}
              className="wr-step-card"
              data-step-state={step.state}
              aria-current={step.state === 'current' ? 'step' : undefined}
            >
              <span className="wr-step-head">
                <span className="wr-step-mark" aria-hidden="true">
                  {step.state === 'done' ? <Check size={12} /> : null}
                </span>
                <span className="wr-mono wr-faint">{index + 1}</span>
                <span className="wr-strong">{step.label}</span>
                <span className="wr-mono wr-faint wr-step-time">
                  {step.time}
                </span>
              </span>
              <span className="wr-step-detail">{step.detail}</span>
            </li>
          ))}
        </ol>
      </section>
      {source.kind === 'github' ? (
        <section className="wr-progress" aria-labelledby="group-jobs-title">
          <div className="wr-head">
            <h2 id="group-jobs-title" className="wr-title">
              Group jobs
            </h2>
            {jobs.length ? (
              <span className="wr-mono wr-faint">
                {jobs.filter((job) => job.status === 'completed').length}/
                {jobs.length}
              </span>
            ) : null}
          </div>
          {source.follow?.jobs?.length ? (
            <GroupJobList execution={execution} />
          ) : (
            <p className="wr-faint" role="status">
              Waiting for GitHub to start the jobs…
            </p>
          )}
          <p className="wr-faint wr-note">
            GitHub holds the results until the run ends. Then this Console
            imports the run by itself, and the totals and each test’s results
            show here as on any execution.
          </p>
        </section>
      ) : null}
    </>
  )
}

/** A harness execution's progress under its title: the share of tests that
 *  reported, and how many run now. */
export function HarnessProgress({ execution }: { execution: PlanExecution }) {
  const rows = testRows(execution)
  if (rows.length === 0) return null
  const reported = rows.filter(
    (row) => row.state === 'reported' || row.state === 'not-run',
  ).length
  const now = rows.filter((row) => row.state === 'running').length
  return (
    <div className="wr-bar-line" data-harness-progress>
      <span className="wr-bar" aria-hidden="true">
        <span style={{ width: `${(100 * reported) / rows.length}%` }} />
      </span>
      <span role="status">
        {reported} of {plural(rows.length, 'test', 'tests')} reported · {now}{' '}
        running · results are provisional
      </span>
    </div>
  )
}

/** Where the execution runs and what happens there: this harness, Docker
 *  (steps and groups) or GitHub (run, ref and group jobs). */
export function WhereItRan({ execution }: { execution: PlanExecution }) {
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
            <GroupJobList execution={execution} />
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
            {dockerSteps(execution).map((step) => (
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
                  {sentenceCase(group.state)}
                </span>
                <span className="wr-mono wr-faint">
                  attempt {group.attempt}
                </span>
              </li>
            ))}
          </ul>
        </>
      ) : null}
    </section>
  )
}

/** Cancel where it runs, saying what stops and what is kept: the host's
 *  confirmation, as the list's delete, with the safe choice focused. */
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
      onOpenChange={(next) => {
        if (!next && !busy) onClose()
      }}
    >
      <DialogContent
        role="alertdialog"
        className="ex-dialog"
        aria-describedby="ep-cancel-body"
        onOpenAutoFocus={(event) => {
          event.preventDefault()
          document.getElementById('ep-cancel-keep')?.focus()
        }}
      >
        <div className="ex-dialog-head">
          <span className="ex-dialog-icon" aria-hidden="true">
            <Square size={16} />
          </span>
          <div>
            <DialogTitle className="ex-dialog-title">{copy.title}</DialogTitle>
            <DialogDescription id="ep-cancel-body" className="ex-dialog-body">
              {copy.body}
            </DialogDescription>
          </div>
        </div>
        {error ? (
          <p role="alert" className="ex-error">
            {error}
          </p>
        ) : null}
        <div className="ex-dialog-actions">
          <Button
            id="ep-cancel-keep"
            type="button"
            variant="pill"
            size="sm"
            disabled={busy}
            onClick={onClose}
          >
            Keep running
          </Button>
          <Button
            type="button"
            variant="pill"
            size="sm"
            className="ex-danger"
            disabled={busy || !bridge}
            aria-busy={busy || undefined}
            onClick={() => void confirm()}
          >
            {busy ? 'Cancelling…' : copy.action}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  )
}
