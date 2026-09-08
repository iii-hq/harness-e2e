import { useCallback, useState } from 'react'
import { dashboardHeaderActionClassName } from '@/components/DashboardPageActions'
import { Callout } from '@/design-system'
import { hashForPlan } from '@/hooks/use-hash-route'
import type {
  DashboardDataBridge,
  ReleaseControlOutcome,
  ReleaseControlPulledExecution,
  ReleaseControlPulledGroup,
  ReleaseControlPullResponse,
} from '@/lib/dashboard-data-source'

/**
 * One click brings Release Control executions into this dashboard: the server
 * downloads their GitHub Actions bundles and installs each group's native run
 * unchanged. The panel says, group by group, what happened and why.
 */
export type ReleaseControlSyncState = {
  pending: boolean
  result: ReleaseControlPullResponse | null
  error: string | null
  run: () => Promise<void>
  dismiss: () => void
}

export function useReleaseControlSync(
  bridge: DashboardDataBridge | null,
  onDone: () => Promise<void> | void,
): ReleaseControlSyncState {
  const [pending, setPending] = useState(false)
  const [result, setResult] = useState<ReleaseControlPullResponse | null>(null)
  const [error, setError] = useState<string | null>(null)

  const run = useCallback(async () => {
    if (bridge?.mode !== 'local') return
    setPending(true)
    setError(null)
    try {
      const response = await bridge.pullReleaseControl()
      setResult(response)
      await onDone()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setPending(false)
    }
  }, [bridge, onDone])

  const dismiss = useCallback(() => {
    setResult(null)
    setError(null)
  }, [])

  return { pending, result, error, run, dismiss }
}

const OUTCOME_LABEL: Record<ReleaseControlOutcome, string> = {
  imported: 'imported',
  exists: 'already present',
  unreadable: 'unreadable',
  not_importable: 'not importable',
  expired: 'expired',
  failed: 'failed (retried on the next click)',
}

export function outcomeLabel(outcome: string): string {
  return (OUTCOME_LABEL as Record<string, string>)[outcome] ?? outcome
}

export function summarizeExecution(
  execution: ReleaseControlPulledExecution,
): string {
  const counts = new Map<string, number>()
  for (const group of execution.groups) {
    counts.set(group.outcome, (counts.get(group.outcome) ?? 0) + 1)
  }
  const parts = [...counts.entries()].map(
    ([outcome, count]) => `${count} ${outcomeLabel(outcome)}`,
  )
  return [execution.execution_id, ...parts].join(' · ')
}

export function groupLine(group: ReleaseControlPulledGroup): string {
  const name =
    [group.campaign_id, group.group_id].filter(Boolean).join('/') ||
    group.native_execution_id ||
    'run'
  const runner = group.runner_version ? ` · runner ${group.runner_version}` : ''
  const reason = group.reason ? `: ${group.reason}` : ''
  return `${name} · ${outcomeLabel(group.outcome)}${runner}${reason}`
}

function countOutcomes(
  result: ReleaseControlPullResponse,
  outcomes: ReleaseControlOutcome[],
): number {
  return result.executions
    .flatMap((execution) => execution.groups)
    .filter((group) => outcomes.includes(group.outcome)).length
}

export function ReleaseControlSyncButton({
  sync,
}: {
  sync: ReleaseControlSyncState
}) {
  return (
    <button
      type="button"
      className={dashboardHeaderActionClassName({ primary: true })}
      disabled={sync.pending}
      aria-busy={sync.pending}
      title="Download the newest Release Control executions from GitHub Actions into this dashboard"
      onClick={() => void sync.run()}
    >
      {sync.pending ? 'syncing release control…' : 'sync release control'}
    </button>
  )
}

export function ReleaseControlSyncResult({
  sync,
}: {
  sync: ReleaseControlSyncState
}) {
  if (sync.error) {
    return (
      <Callout
        tone="danger"
        title="Release Control sync failed"
        className="mt-6"
      >
        <span className="flex flex-wrap items-center justify-between gap-3">
          {sync.error}
          <DismissButton onClick={sync.dismiss} />
        </span>
      </Callout>
    )
  }
  const { result } = sync
  if (!result) return null
  const executions = result.executions.length
  const added = countOutcomes(result, ['imported'])
  const unreadable = countOutcomes(result, ['unreadable'])
  const tone =
    executions > 0 && added === 0 && unreadable > 0 ? 'warning' : 'success'
  const title = `Release Control: ${executions} execution${executions === 1 ? '' : 's'} checked · ${added} run${added === 1 ? '' : 's'} added`
  return (
    <Callout tone={tone} title={title} className="mt-6">
      <ul className="m-0 list-none p-0 font-mono text-xs">
        {result.executions.map((execution) => (
          <li
            key={`${execution.run_id}-${execution.run_attempt}`}
            className="mt-1"
          >
            <details>
              <summary className="cursor-pointer">
                {summarizeExecution(execution)}
              </summary>
              <ul className="m-0 list-none p-0 pl-4 text-ink-soft">
                {execution.groups.map((group) => (
                  <li
                    key={
                      group.native_execution_id ??
                      `${group.campaign_id}/${group.group_id}/${group.outcome}`
                    }
                  >
                    {groupLine(group)}
                  </li>
                ))}
                {execution.url ? (
                  <li>
                    <a href={execution.url} target="_blank" rel="noreferrer">
                      GitHub run {execution.run_id}
                    </a>
                  </li>
                ) : null}
                {execution.plan_id ? (
                  <li>
                    <a href={hashForPlan(execution.plan_id)}>
                      filed under plan {execution.plan_id}
                    </a>
                  </li>
                ) : null}
                {execution.plan_error ? (
                  <li>not filed under a plan: {execution.plan_error}</li>
                ) : null}
              </ul>
            </details>
          </li>
        ))}
      </ul>
      <p className="mt-2 flex flex-wrap items-center justify-between gap-3 text-xs text-ink-muted">
        <span>
          Runs land in {result.runs_dir}.
          {result.remaining_runs > 0
            ? ` ${result.remaining_runs} older execution${result.remaining_runs === 1 ? '' : 's'} did not fit this click's time budget: sync again to continue.`
            : ''}
        </span>
        <DismissButton onClick={sync.dismiss} />
      </p>
    </Callout>
  )
}

function DismissButton({ onClick }: { onClick: () => void }) {
  return (
    <button
      type="button"
      className="cursor-pointer border-0 bg-transparent p-0 font-mono text-xs text-ink-muted underline hover:text-ink"
      onClick={onClick}
    >
      dismiss
    </button>
  )
}
