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

/** What the row offers: import, follow an import in progress, or open (and
 *  import again) an execution this worker already has. */
export function githubRunAction(run: GithubRun) {
  if (run.execution_state === 'importing') return 'importing'
  return run.execution_id ? 'imported' : 'import'
}

/** Completed exact-stack workflow runs, newest first, each importable as an
 *  execution. `gh` errors are shown as they come. */
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
      try {
        const response = await bridge.listGithubRuns(page)
        setRepository(response.repository)
        setRuns((current) =>
          page === 1 ? response.runs : [...current, ...response.runs],
        )
        setNextPage(response.next_page)
      } catch (cause) {
        setError(errorText(cause))
      } finally {
        setLoading(false)
      }
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
          <th scope="col">conclusion</th>
          <th scope="col">
            <span className="ds-visually-hidden">Import</span>
          </th>
        </tr>
      </thead>
      <tbody>
        {runs.map((run) => {
          const action = githubRunAction(run)
          return (
            <tr key={run.run_id} data-github-run={run.run_id}>
              <td data-label="run">
                <a
                  className="inline-flex items-center gap-1 font-mono text-xs text-ink"
                  href={run.url}
                  target="_blank"
                  rel="noreferrer"
                >
                  #{run.run_id}
                  {run.run_attempt > 1 ? ` · attempt ${run.run_attempt}` : ''}
                  <ExternalLink size={12} aria-hidden="true" />
                </a>
                <span className="block font-mono text-label text-ink-muted">
                  {run.created_at ? formatDate(run.created_at) : '—'}
                </span>
              </td>
              <td data-label="suite" className="font-mono text-xs">
                {run.suite_label || run.suite || '—'}
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
                {run.model ?? '—'}
                <span className="block text-label text-ink-muted">
                  {run.provider ?? ''}
                </span>
              </td>
              <td data-label="profile" className="font-mono text-xs">
                {run.agent ?? 'default'}
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
