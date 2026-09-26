import { AlertCircle } from 'lucide-react'
import type { GithubStatus } from '@/lib/dashboard-data-source'

/** Where a GitHub execution is dispatched, and whether the worker's `gh`
 *  can do it: signed in, still checking, or how to fix it. */
export function GithubCard({
  status,
  onCheckAgain,
}: {
  status: GithubStatus | 'loading' | null
  onCheckAgain: () => void
}) {
  const known = status && status !== 'loading' ? status : null
  return (
    <div className="rd-github">
      <dl className="rd-facts">
        <dt>Workflow</dt>
        <dd className="rd-mono">exact-stack-e2e.yml</dd>
        <dt>From</dt>
        <dd className="rd-mono">
          {known?.repository || 'this worker’s repository'} · default branch
        </dd>
        <dt>Credentials</dt>
        <dd>the workflow’s environment secrets</dd>
        <dt>Reports</dt>
        <dd>none to Release Control</dd>
        {known?.ready ? (
          <>
            <dt>Sign-in</dt>
            <dd>
              gh on this worker’s machine
              {known.account ? ` · ${known.account}` : ''}
            </dd>
          </>
        ) : null}
      </dl>
      {status === 'loading' ? (
        <p className="rd-hint" role="status">
          Checking gh on this worker’s machine…
        </p>
      ) : null}
      {known && !known.ready ? (
        <div role="alert" className="rd-alert" data-tone="alert">
          <AlertCircle size={16} aria-hidden="true" className="rd-alert-icon" />
          <div className="rd-grow">
            <p className="rd-strong">
              gh isn’t ready on this worker’s machine.
            </p>
            <p className="rd-faint">{known.message}</p>
          </div>
          <button
            type="button"
            className="rd-ghost rd-small"
            onClick={onCheckAgain}
          >
            Check again
          </button>
        </div>
      ) : null}
    </div>
  )
}
