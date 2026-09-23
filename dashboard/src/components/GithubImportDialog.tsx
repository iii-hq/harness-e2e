import { ExternalLink } from 'lucide-react'
import { useCallback, useEffect, useState } from 'react'
import {
  buttonClassName,
  Callout,
  DataTable,
  Dialog,
  StatusBadge,
} from '@/design-system'
import { hashForExecution } from '@/hooks/use-hash-route'
import type {
  DashboardDataBridge,
  GithubRun,
} from '@/lib/dashboard-data-source'
import { formatDate } from '@/lib/execution-view'

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

function conclusionStatus(conclusion: string | null) {
  if (conclusion === 'success') return 'passed' as const
  if (conclusion === 'cancelled') return 'cancelled' as const
  if (conclusion === 'failure') return 'failed' as const
  return 'unavailable' as const
}

/** Newest run first, by when the run was created (a re-run attempt does not
 *  move it). */
export function sortGithubRuns(runs: GithubRun[]): GithubRun[] {
  const created = (run: GithubRun) => Date.parse(run.created_at ?? '') || 0
  return [...runs].sort(
    (left, right) =>
      created(right) - created(left) || right.run_id - left.run_id,
  )
}

/** Runs with what their contracts said merged in; a run the answer left out
 *  stops waiting and says its contract could not be read. */
export function withContracts(
  runs: GithubRun[],
  read: Array<Partial<GithubRun> & { run_id: number }>,
  asked: number[],
): GithubRun[] {
  return runs.map((run) => {
    if (!asked.includes(run.run_id)) return run
    const contract = read.find((entry) => entry.run_id === run.run_id)
    return {
      ...run,
      ...contract,
      contract_pending: false,
      contract_error:
        contract?.contract_error ??
        (contract ? undefined : 'The contract could not be read'),
    }
  })
}

/** What the row offers: import, follow an import in progress, or open (and
 *  import again) an execution this worker already has. */
export function githubRunAction(run: GithubRun) {
  if (run.execution_state === 'importing') return 'importing'
  return run.execution_id ? 'imported' : 'import'
}

/** Completed exact-stack workflow runs, newest first, each importable as an
 *  execution. The list comes from one quick `gh api` call; each run's suite,
 *  model, profile and runner fill in once its contract is read. `gh` errors
 *  are shown as they come. */
export function GithubImportDialog({
  bridge,
  open,
  onClose,
  onImported,
}: {
  bridge: DashboardDataBridge | null
  open: boolean
  onClose: () => void
  onImported: (executionId: string) => void
}) {
  const [runs, setRuns] = useState<GithubRun[]>([])
  const [repository, setRepository] = useState<string | null>(null)
  const [nextPage, setNextPage] = useState<number | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [importing, setImporting] = useState<number | null>(null)

  const load = useCallback(
    async (page: number) => {
      if (!bridge) return
      setLoading(true)
      setError(null)
      let pending: GithubRun[] = []
      try {
        const response = await bridge.listGithubRuns(page)
        setRepository(response.repository)
        setRuns((current) =>
          sortGithubRuns(
            page === 1 ? response.runs : [...current, ...response.runs],
          ),
        )
        setNextPage(response.next_page)
        pending = response.runs.filter((run) => run.contract_pending)
      } catch (cause) {
        setError(errorText(cause))
      } finally {
        setLoading(false)
      }
      if (pending.length === 0) return
      const asked = pending.map((run) => run.run_id)
      const read = await bridge
        .readGithubRunContracts(pending)
        .then((answer) => answer.runs)
        .catch((cause) =>
          asked.map((run_id) => ({ run_id, contract_error: errorText(cause) })),
        )
      setRuns((current) => withContracts(current, read, asked))
    },
    [bridge],
  )

  useEffect(() => {
    if (open) void load(1)
  }, [open, load])

  const importRun = async (run: GithubRun) => {
    if (!bridge) return
    setImporting(run.run_id)
    setError(null)
    try {
      const accepted = await bridge.importGithubRun(run.run_id)
      setRuns((current) =>
        current.map((entry) =>
          entry.run_id === run.run_id
            ? {
                ...entry,
                execution_id: accepted.execution_id,
                execution_state: accepted.state,
              }
            : entry,
        ),
      )
      onImported(accepted.execution_id)
    } catch (cause) {
      setError(errorText(cause))
    } finally {
      setImporting(null)
    }
  }

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="xl"
      tall
      bodyPadding
      title="Import from GitHub"
      description={`Completed exact-stack runs${repository ? ` of ${repository}` : ''}. Importing downloads the run's evidence into this worker; importing again replaces it.`}
      data-github-import
    >
      <div className="grid gap-4">
        {error ? (
          <Callout tone="danger" title="GitHub request failed">
            <span className="whitespace-pre-wrap break-words font-mono text-xs">
              {error}
            </span>
          </Callout>
        ) : null}
        {runs.length === 0 && loading ? (
          <p className="m-0 font-mono text-xs text-ink-muted" role="status">
            loading runs from GitHub…
          </p>
        ) : runs.length === 0 && !error ? (
          <p className="m-0 text-sm text-ink-soft">
            No completed exact-stack runs found.
          </p>
        ) : null}
        {runs.length > 0 ? (
          <GithubRunsTable
            runs={runs}
            importing={importing}
            onImport={(run) => void importRun(run)}
            onOpen={onClose}
          />
        ) : null}
        {nextPage ? (
          <button
            type="button"
            className={buttonClassName({
              variant: 'secondary',
              className: 'justify-self-start',
            })}
            disabled={loading}
            aria-busy={loading}
            onClick={() => void load(nextPage)}
          >
            {loading ? 'loading…' : 'load older runs'}
          </button>
        ) : null}
      </div>
    </Dialog>
  )
}

/** One row per run: suite, subject, profile, conclusion, and the import
 *  action or the execution that already holds it. */
export function GithubRunsTable({
  runs,
  importing,
  onImport,
  onOpen,
}: {
  runs: GithubRun[]
  importing: number | null
  onImport: (run: GithubRun) => void
  onOpen?: () => void
}) {
  return (
    <DataTable
      caption={`GitHub runs, ${runs.length} loaded`}
      collapse
      minWidth="52rem"
    >
      <thead>
        <tr>
          <th scope="col">run</th>
          <th scope="col">suite</th>
          <th scope="col">model</th>
          <th scope="col">profile</th>
          <th scope="col">runner</th>
          <th scope="col">conclusion</th>
          <th scope="col">
            <span className="ds-visually-hidden">Import</span>
          </th>
        </tr>
      </thead>
      <tbody>
        {runs.map((run) => {
          const action = githubRunAction(run)
          const pending = Boolean(run.contract_pending)
          // Until the contract is read, its cells say so instead of "—".
          const contract = (value: string | null | undefined, empty = '—') =>
            pending ? (
              <span className="text-ink-muted" role="status">
                reading…
              </span>
            ) : (
              value || empty
            )
          return (
            <tr
              key={run.run_id}
              data-github-run={run.run_id}
              aria-busy={pending || undefined}
            >
              <td data-label="run">
                <a
                  className="inline-flex items-center gap-1 font-mono text-xs text-ink"
                  href={run.url}
                  target="_blank"
                  rel="noreferrer"
                >
                  #{run.run_id}
                  <ExternalLink size={12} aria-hidden="true" />
                </a>
                <span className="block font-mono text-label text-ink-muted">
                  {run.created_at ? formatDate(run.created_at) : '—'}
                </span>
                {run.run_attempt > 1 ? (
                  <span className="block font-mono text-label text-ink-muted">
                    attempt {run.run_attempt}
                    {run.attempt_started_at
                      ? ` · ${formatDate(run.attempt_started_at)}`
                      : ''}
                  </span>
                ) : null}
                {run.release_control_execution_id ? (
                  <span
                    className="block font-mono text-label text-ink-muted"
                    title={`Release Control execution ${run.release_control_execution_id}`}
                  >
                    RC {run.release_control_execution_id.slice(0, 8)}
                  </span>
                ) : null}
              </td>
              <td data-label="suite" className="font-mono text-xs">
                {contract(run.suite_label || run.suite)}
                {run.contract_error ? (
                  <span
                    className="block text-label text-warning"
                    title={run.contract_error}
                  >
                    contract unavailable
                  </span>
                ) : null}
              </td>
              <td data-label="model" className="font-mono text-xs">
                {contract(run.model)}
                <span className="block text-label text-ink-muted">
                  {pending ? '' : (run.provider ?? '')}
                </span>
              </td>
              <td data-label="profile" className="font-mono text-xs">
                {contract(run.agent, run.contract_error ? '—' : 'default')}
              </td>
              <td data-label="runner" className="font-mono text-xs">
                {contract(run.runner_version)}
              </td>
              <td data-label="conclusion">
                <StatusBadge
                  status={conclusionStatus(run.conclusion)}
                  label={run.conclusion ?? 'unknown'}
                />
              </td>
              <td className="text-right">
                <span className="inline-flex flex-wrap items-center justify-end gap-2">
                  {action === 'imported' ? (
                    <span className="font-mono text-label text-ink-muted">
                      imported
                    </span>
                  ) : null}
                  {run.execution_id ? (
                    <a
                      className={buttonClassName({
                        variant: 'quiet',
                        size: 'compact',
                        className: 'no-underline',
                      })}
                      href={hashForExecution(run.execution_id)}
                      onClick={onOpen}
                    >
                      {action === 'importing' ? 'importing…' : 'open'}
                    </a>
                  ) : null}
                  <button
                    type="button"
                    className={buttonClassName({
                      variant: action === 'import' ? 'secondary' : 'quiet',
                      size: 'compact',
                    })}
                    disabled={importing !== null || action === 'importing'}
                    aria-busy={importing === run.run_id}
                    onClick={() => onImport(run)}
                  >
                    {importing === run.run_id
                      ? 'importing…'
                      : action === 'import'
                        ? 'import'
                        : 'import again'}
                  </button>
                </span>
              </td>
            </tr>
          )
        })}
      </tbody>
    </DataTable>
  )
}
