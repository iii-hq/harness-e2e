import {
  AlertCircle,
  ArrowRight,
  ArrowUpRight,
  Check,
  ExternalLink,
  GitBranch,
  LoaderCircle,
  RefreshCw,
  Search,
  TriangleAlert,
  X,
} from 'lucide-react'
import { type ReactNode, useCallback, useEffect, useId, useState } from 'react'
import { Picker } from '@/components/run-dialog/Picker'
import {
  checkState,
  plural,
  selectionText,
  toggleAll,
} from '@/components/run-dialog/run-dialog-model'
import { Box } from '@/components/run-dialog/TestsColumn'
import '@/components/run-dialog/run-dialog.css'
import '@/components/github-import.css'
import { Dialog, StatusLabel } from '@/design-system'
import { hashForExecution } from '@/hooks/use-hash-route'
import type {
  DashboardDataBridge,
  GithubRun,
  GithubStatus,
} from '@/lib/dashboard-data-source'
import { formatDay, formatDayLabel, formatTime } from '@/lib/format'
import type { ResultState } from '@/lib/result-status'

const WORKFLOW = 'exact-stack-e2e.yml'
/** Runs GitHub answers per page (`github_runs`). */
const PAGE_SIZE = 20
const SKELETON = [
  [220, 320],
  [180, 290],
  [240, 330],
  [160, 270],
  [210, 300],
  [190, 310],
  [230, 280],
]

export type Phase = 'loading' | 'failed' | 'ready'
export type Show = 'all' | 'new' | 'imported'
export type Filters = { query: string; branch: string; show: Show }

const NO_FILTERS: Filters = { query: '', branch: '', show: 'all' }

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
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

/** Where a run stands here: not imported, being imported (this dialog is
 *  starting it, or the worker is downloading it), or held by an execution. */
export function githubRunAction(run: GithubRun, starting = false) {
  if (starting || run.execution_state === 'importing') return 'importing'
  return run.execution_id ? 'imported' : 'import'
}

export function shortSha(sha: string | null | undefined) {
  return sha ? sha.slice(0, 7) : ''
}

/** What the run's contract says it ran, in one line. */
export function contractDetail(run: GithubRun) {
  return [
    [run.provider, run.model].filter(Boolean).join('/'),
    `profile ${run.agent || 'default'}`,
    run.stack ? `stack ${run.stack}` : '',
    run.runner_version ? `runner ${run.runner_version}` : '',
  ]
    .filter(Boolean)
    .join(' · ')
}

/** Whether a run shows under the text filter, the branch and All / Not
 *  imported / Imported. */
export function runMatches(
  run: GithubRun,
  { query, branch, show }: Filters,
  starting = false,
) {
  const q = query.trim().toLowerCase()
  const text = [
    run.run_id,
    run.suite_label,
    run.suite,
    run.title,
    run.head_branch,
    run.head_sha,
    run.model,
    run.agent,
    run.stack,
    run.runner_version,
    run.release_control_execution_id,
  ]
    .filter(Boolean)
    .join(' ')
    .toLowerCase()
  const imported = githubRunAction(run, starting) !== 'import'
  return (
    (!q || text.includes(q)) &&
    (!branch || run.head_branch === branch) &&
    (show === 'all' || (show === 'imported') === imported)
  )
}

export type RunDay = {
  key: string
  label: string
  /** The day under a relative label (`Sep 24` under `Today`), else empty. */
  date: string
  runs: GithubRun[]
}

/** Runs by the local day they were created, in the order given. */
export function runDays(runs: GithubRun[], now = new Date()): RunDay[] {
  const days: RunDay[] = []
  for (const run of runs) {
    const created = new Date(run.created_at ?? '')
    const dated = !Number.isNaN(created.getTime())
    const key = dated ? created.toDateString() : 'undated'
    let day = days.find((entry) => entry.key === key)
    if (!day) {
      const label = dated ? formatDayLabel(created, now) : 'Date not reported'
      const date = dated ? formatDay(created, now) : ''
      day = { key, label, date: date === label ? '' : date, runs: [] }
      days.push(day)
    }
    day.runs.push(run)
  }
  return days
}

/** The footer's two lines: what is selected, what is importing, or why
 *  nothing can be yet. */
export function importSummary({
  phase,
  selected,
  again,
  started,
}: {
  phase: Phase
  selected: number
  /** Selected runs this worker already imported. */
  again: number
  /** Imports this dialog started. */
  started: number
}) {
  if (selected > 0)
    return {
      title: `${plural(selected, 'run', 'runs')} selected`,
      detail: again
        ? `${again === 1 ? '1 was' : `${again} were`} imported before. Importing again replaces ${again === 1 ? 'its' : 'their'} evidence.`
        : 'Evidence downloads in the background. You can keep working.',
      busy: false,
    }
  if (started > 0)
    return {
      title: `Importing ${plural(started, 'run', 'runs')} in the background`,
      detail:
        'You can close this dialog. Each run shows up in Executions as soon as its evidence is in.',
      busy: true,
    }
  return {
    title: 'No runs selected',
    detail:
      phase === 'loading'
        ? 'Runs appear as soon as GitHub answers.'
        : phase === 'failed'
          ? 'Nothing can be imported until GitHub answers.'
          : 'Tick the runs to import.',
    busy: false,
  }
}

/** The line under the list: asking, unavailable, or how many are loaded. */
export function listStatus(
  phase: Phase,
  loaded: number,
  total: number | null,
  repository: string | null,
) {
  if (phase === 'loading') return 'Asking GitHub for completed runs…'
  if (phase === 'failed') return 'GitHub unavailable'
  const count =
    total === null
      ? `${plural(loaded, 'run', 'runs')} loaded`
      : `${loaded} of ${plural(total, 'run', 'runs')} loaded`
  return repository ? `${count} · ${repository}` : count
}

function conclusionOf(conclusion: string | null): {
  state: ResultState
  label: string
} {
  if (conclusion === 'success') return { state: 'passed', label: 'Succeeded' }
  if (conclusion === 'failure') return { state: 'failed', label: 'Failed' }
  if (conclusion === 'timed_out') return { state: 'failed', label: 'Timed out' }
  const label = (conclusion ?? 'unknown').replace(/_/g, ' ')
  return {
    state: 'cancelled',
    label: label.charAt(0).toUpperCase() + label.slice(1),
  }
}

/** `gh` text with its `commands` set in mono. */
function withCode(text: string): ReactNode {
  return text.split('`').map((part, index) =>
    index % 2 ? (
      // biome-ignore lint/suspicious/noArrayIndexKey: fixed pieces of one message
      <code key={index} className="gi-code">
        {part}
      </code>
    ) : (
      part
    ),
  )
}

/** One run: what it ran (from its contract), where and when, and whether
 *  this worker holds it. */
export function GithubRunRow({
  run,
  selected,
  starting,
  failure,
  onToggle,
  onOpen,
}: {
  run: GithubRun
  selected: boolean
  /** This dialog is asking the worker to import it. */
  starting: boolean
  /** Why the import this dialog asked for did not start. */
  failure: string | null
  onToggle: () => void
  onOpen?: () => void
}) {
  const action = githubRunAction(run, starting)
  const pending = Boolean(run.contract_pending)
  const suite = run.suite_label || run.suite
  const conclusion = conclusionOf(run.conclusion)
  const rc = run.release_control_execution_id
  return (
    <div
      className="gi-row"
      data-github-run={run.run_id}
      data-selected={selected || undefined}
      aria-busy={pending || undefined}
    >
      {/* biome-ignore lint/a11y/noLabelWithoutControl: the checkbox is inside Box */}
      <label className="gi-main">
        <Box
          state={selected ? 'on' : 'off'}
          label={`Import run ${run.run_id}`}
          disabled={action === 'importing'}
          onToggle={onToggle}
        />
        <span className="gi-text">
          <span className="gi-line">
            {pending ? (
              <span
                className="rd-skel gi-skel gi-skel-title"
                role="status"
                aria-label="Reading the run’s contract"
              />
            ) : (
              <span
                className={suite ? 'gi-suite rd-ellipsis' : 'rd-faint'}
                title={suite ? undefined : run.title}
              >
                {suite || run.title}
              </span>
            )}
            {run.head_branch ? (
              <span className="gi-branch" title={run.head_branch}>
                {run.head_branch}
              </span>
            ) : null}
            {run.head_sha ? (
              <span className="rd-meta" title={run.head_sha}>
                {shortSha(run.head_sha)}
              </span>
            ) : null}
            {run.run_attempt > 1 ? (
              <span className="gi-note">attempt {run.run_attempt}</span>
            ) : null}
            {rc ? (
              <span
                className="gi-note"
                title={`Dispatched by Release Control, execution ${rc}`}
              >
                Release Control{' '}
                <span className="rd-mono">{rc.slice(0, 8)}</span>
              </span>
            ) : null}
          </span>
          {pending ? (
            <span className="rd-skel gi-skel gi-skel-detail" />
          ) : run.contract_error ? (
            <span className="rd-hint rd-warning gi-warning">
              <TriangleAlert size={16} aria-hidden="true" />
              <span className="rd-ellipsis" title={run.contract_error}>
                {run.contract_error}
              </span>
            </span>
          ) : (
            <span className="gi-detail rd-ellipsis">{contractDetail(run)}</span>
          )}
          {failure ? (
            <span
              className="rd-hint rd-warning gi-warning"
              data-tone="alert"
              role="alert"
            >
              <AlertCircle size={16} aria-hidden="true" />
              <span className="rd-ellipsis" title={failure}>
                The import didn’t start: {failure}
              </span>
            </span>
          ) : null}
        </span>
      </label>
      <div className="gi-meta">
        <span className="gi-conclusion">
          <StatusLabel
            state={conclusion.state}
            label={conclusion.label}
            tinted
          />
          <span className="rd-meta">{formatTime(run.created_at)}</span>
        </span>
        <a
          className="gi-run-link rd-meta"
          href={run.url}
          target="_blank"
          rel="noreferrer"
          aria-label={`Run ${run.run_id} on GitHub`}
        >
          #{run.run_id}
          <ArrowUpRight size={16} aria-hidden="true" />
        </a>
      </div>
      <div className="gi-state">
        {action === 'imported' ? (
          <span className="gi-state-label">
            <Check size={16} aria-hidden="true" className="gi-ok" />
            Imported
          </span>
        ) : null}
        {action === 'importing' ? (
          <span className="gi-state-label" role="status">
            <LoaderCircle size={16} aria-hidden="true" className="gi-spin" />
            Importing…
          </span>
        ) : null}
        {run.execution_id ? (
          <a
            className="rd-ghost rd-small rd-link-button"
            href={hashForExecution(run.execution_id)}
            onClick={onOpen}
          >
            Open
            <ArrowRight size={16} aria-hidden="true" />
          </a>
        ) : null}
      </div>
    </div>
  )
}

function ListError({
  message,
  status,
  onRetry,
}: {
  message: string
  status: GithubStatus | null
  onRetry: () => void
}) {
  return (
    <div role="alert" className="rd-alert gi-error" data-tone="alert">
      <AlertCircle size={16} aria-hidden="true" className="rd-alert-icon" />
      <div className="rd-grow gi-error-body">
        <p className="rd-strong">GitHub didn’t answer</p>
        <p className="rd-faint">
          {status && !status.ready && status.message ? (
            withCode(status.message)
          ) : (
            <>
              The worker lists runs with the GitHub CLI. Check{' '}
              <code className="gi-code">gh auth status</code> on the machine
              that runs it, then retry.
            </>
          )}
        </p>
        <pre className="gi-pre">{message}</pre>
      </div>
      <button type="button" className="rd-ghost rd-button" onClick={onRetry}>
        <RefreshCw size={16} aria-hidden="true" />
        Retry
      </button>
    </div>
  )
}

/** Completed runs of the exact-stack workflow, newest first by day, to
 *  import as executions, several at once. The list comes from one quick
 *  `gh api` call a page at a time; each run's suite and subject fill in once
 *  its contract is read. Importing continues on the worker in the
 *  background. */
export function GithubImportDialog({
  bridge,
  open,
  onClose,
  onImported,
}: {
  bridge: DashboardDataBridge | null
  open: boolean
  onClose: () => void
  /** Some import started; the executions list has a new row. */
  onImported: () => void
}) {
  const id = useId()
  const [runs, setRuns] = useState<GithubRun[]>([])
  const [repository, setRepository] = useState<string | null>(null)
  const [github, setGithub] = useState<GithubStatus | null>(null)
  const [total, setTotal] = useState<number | null>(null)
  const [nextPage, setNextPage] = useState<number | null>(null)
  const [phase, setPhase] = useState<Phase>('loading')
  const [loadingOlder, setLoadingOlder] = useState(false)
  const [error, setError] = useState<{ page: number; message: string } | null>(
    null,
  )
  const [filters, setFilters] = useState<Filters>(NO_FILTERS)
  const [selected, setSelected] = useState<number[]>([])
  const [starting, setStarting] = useState<number[]>([])
  const [started, setStarted] = useState<number[]>([])
  const [failures, setFailures] = useState<Record<number, string>>({})

  const checkGithub = useCallback(() => {
    bridge?.getGithubStatus().then(setGithub, () => setGithub(null))
  }, [bridge])

  const load = useCallback(
    async (page: number) => {
      if (!bridge) return
      setError(null)
      if (page === 1) {
        setPhase('loading')
        setRuns([])
      } else setLoadingOlder(true)
      let pending: GithubRun[] = []
      try {
        const response = await bridge.listGithubRuns(page)
        setRepository(response.repository)
        setTotal(response.total_count ?? null)
        setNextPage(response.next_page)
        setRuns((current) => {
          const known = page === 1 ? [] : current
          // A run created meanwhile shifts the pages by one.
          const added = response.runs.filter(
            (run) => !known.some((entry) => entry.run_id === run.run_id),
          )
          return sortGithubRuns([...known, ...added])
        })
        setPhase('ready')
        pending = response.runs.filter((run) => run.contract_pending)
      } catch (cause) {
        setError({ page, message: errorText(cause) })
        if (page === 1) setPhase('failed')
      } finally {
        setLoadingOlder(false)
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
    if (!open) return
    setFilters(NO_FILTERS)
    setSelected([])
    setStarted([])
    setFailures({})
    checkGithub()
    void load(1)
  }, [open, load, checkGithub])

  const retry = (page: number) => {
    if (page === 1) checkGithub()
    void load(page)
  }

  const importSelected = async () => {
    if (!bridge || selected.length === 0) return
    const ids = selected
    setSelected([])
    setStarting((current) => [...current, ...ids])
    setStarted((current) => [
      ...current,
      ...ids.filter((runId) => !current.includes(runId)),
    ])
    setFailures((current) =>
      Object.fromEntries(
        Object.entries(current).filter(([runId]) => !ids.includes(+runId)),
      ),
    )
    const accepted = await Promise.all(
      ids.map(async (runId) => {
        try {
          const execution = await bridge.importGithubRun(runId)
          setRuns((current) =>
            current.map((run) =>
              run.run_id === runId
                ? {
                    ...run,
                    execution_id: execution.execution_id,
                    execution_state: execution.state,
                  }
                : run,
            ),
          )
          return true
        } catch (cause) {
          setFailures((current) => ({ ...current, [runId]: errorText(cause) }))
          setStarted((current) => current.filter((entry) => entry !== runId))
          return false
        } finally {
          setStarting((current) => current.filter((entry) => entry !== runId))
        }
      }),
    )
    if (accepted.some(Boolean)) onImported()
  }

  const ready = phase === 'ready'
  const repo = repository ?? github?.repository ?? null
  const isStarting = (run: GithubRun) => starting.includes(run.run_id)
  const visible = runs.filter((run) =>
    runMatches(run, filters, isStarting(run)),
  )
  const selectable = visible
    .filter((run) => githubRunAction(run, isStarting(run)) !== 'importing')
    .map((run) => run.run_id)
  const shown = checkState(selectable, selected)
  const hidden = selected.filter(
    (runId) => !visible.some((run) => run.run_id === runId),
  ).length
  const imported = runs.filter(
    (run) => githubRunAction(run, isStarting(run)) !== 'import',
  ).length
  const again = runs.filter(
    (run) =>
      selected.includes(run.run_id) && githubRunAction(run) === 'imported',
  ).length
  const branches = new Map<string, number>()
  for (const run of runs)
    if (run.head_branch)
      branches.set(run.head_branch, (branches.get(run.head_branch) ?? 0) + 1)
  const summary = importSummary({
    phase,
    selected: selected.length,
    again,
    started: started.length,
  })
  const older = total === null ? null : Math.min(PAGE_SIZE, total - runs.length)
  const filtered =
    filters.query.trim() !== '' ||
    filters.branch !== '' ||
    filters.show !== 'all'
  const segments: Array<[Show, string, number]> = [
    ['all', 'All', runs.length],
    ['new', 'Not imported', runs.length - imported],
    ['imported', 'Imported', imported],
  ]

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="xl"
      tall
      title="Import from GitHub"
      description={
        <>
          Completed runs of <span className="rd-mono gi-ink">{WORKFLOW}</span>
          {repo ? (
            <>
              {' '}
              in <span className="rd-mono gi-ink">{repo}</span>
            </>
          ) : null}
          . Importing copies a run’s evidence into this worker.
        </>
      }
      actions={
        repo ? (
          <a
            className="ds-dialog-close"
            href={`https://github.com/${repo}/actions/workflows/${WORKFLOW}`}
            target="_blank"
            rel="noreferrer"
            aria-label="Open the workflow on GitHub"
            title="Open the workflow on GitHub"
          >
            <ExternalLink size={16} aria-hidden="true" />
          </a>
        ) : null
      }
      closeLabel="Close"
      className="ds-root rd-dialog gi-dialog"
      bodyClassName="rd-body"
      footer={
        <div className="rd-footer">
          <div className="gi-summary rd-grow" aria-live="polite">
            {summary.busy ? (
              <LoaderCircle size={16} aria-hidden="true" className="gi-spin" />
            ) : null}
            <div className="rd-summary">
              <p className="rd-summary-counts">{summary.title}</p>
              <p className="rd-summary-line rd-faint">{summary.detail}</p>
            </div>
          </div>
          <div className="rd-actions">
            <button
              type="button"
              className="rd-ghost rd-button rd-cancel"
              onClick={onClose}
            >
              {summary.busy ? 'Close' : 'Cancel'}
            </button>
            <button
              type="button"
              className="rd-primary"
              disabled={!ready || selected.length === 0}
              onClick={() => void importSelected()}
            >
              {selected.length
                ? `Import ${plural(selected.length, 'run', 'runs')}`
                : 'Import runs'}
            </button>
          </div>
        </div>
      }
    >
      <section className="rd-tests gi-runs" aria-label="Runs">
        <div className="rd-tests-toolbar">
          <div className="rd-search">
            <Search size={16} aria-hidden="true" className="rd-search-icon" />
            <input
              type="text"
              className="rd-control rd-input rd-search-input"
              aria-label="Filter runs"
              placeholder="Filter by suite, branch, commit or run"
              value={filters.query}
              disabled={!ready}
              onChange={(event) =>
                setFilters({ ...filters, query: event.target.value })
              }
            />
            {filters.query ? (
              <button
                type="button"
                className="rd-ghost rd-search-clear"
                aria-label="Clear filter"
                onClick={() => setFilters({ ...filters, query: '' })}
              >
                <X size={16} aria-hidden="true" />
              </button>
            ) : null}
          </div>
          <div className="gi-branches">
            <label htmlFor={`${id}-branch`} className="ds-visually-hidden">
              Branch
            </label>
            <Picker
              id={`${id}-branch`}
              label="Branches"
              icon={
                <GitBranch size={16} aria-hidden="true" className="rd-faint" />
              }
              groups={[
                {
                  label: null,
                  options: [
                    {
                      value: '',
                      label: 'All branches',
                      meta: String(runs.length),
                    },
                    ...[...branches].map(([branch, count]) => ({
                      value: branch,
                      label: branch,
                      meta: String(count),
                    })),
                  ],
                },
              ]}
              value={filters.branch}
              valueLabel={filters.branch || 'All branches'}
              disabled={!ready}
              onPick={(branch) => setFilters({ ...filters, branch })}
            />
          </div>
          {/* biome-ignore lint/a11y/useSemanticElements: a toggle group, not a fieldset */}
          <div
            role="group"
            aria-label="Show"
            className="rd-control rd-segments"
          >
            {segments.map(([value, label, count]) => (
              <button
                key={value}
                type="button"
                className="rd-segment"
                aria-pressed={filters.show === value}
                disabled={!ready}
                onClick={() => setFilters({ ...filters, show: value })}
              >
                {label} <span className="rd-meta">{ready ? count : '–'}</span>
              </button>
            ))}
          </div>
        </div>

        <div className="rd-tests-head">
          {/* biome-ignore lint/a11y/noLabelWithoutControl: the checkbox is inside Box */}
          <label className="rd-tests-all">
            <Box
              state={shown}
              label="Select every run shown"
              disabled={!ready || selectable.length === 0}
              onToggle={() => setSelected(toggleAll(selectable, selected))}
            />
            <span className="rd-strong">Runs</span>
            <span className="rd-meta">
              {!ready
                ? ''
                : visible.length === runs.length
                  ? `${runs.length} loaded`
                  : `${visible.length} of ${runs.length} loaded`}
            </span>
          </label>
          <span className="rd-meta rd-push">
            {selectionText(selected.length, hidden)}
          </span>
          <button
            type="button"
            className="rd-ghost rd-small"
            disabled={selected.length === 0}
            onClick={() => setSelected([])}
          >
            Clear
          </button>
        </div>

        <div className="rd-tests-list rd-scroll">
          {runDays(visible).map((day) => (
            // biome-ignore lint/a11y/useSemanticElements: a day of runs, not a fieldset
            <div
              key={day.key}
              role="group"
              aria-label={day.label}
              className="rd-family"
            >
              <div className="rd-family-head">
                <span className="rd-family-name">{day.label}</span>
                {day.date ? (
                  <span className="rd-faint gi-date">{day.date}</span>
                ) : null}
                <span className="rd-count">{day.runs.length}</span>
              </div>
              {day.runs.map((run) => (
                <GithubRunRow
                  key={run.run_id}
                  run={run}
                  selected={selected.includes(run.run_id)}
                  starting={isStarting(run)}
                  failure={failures[run.run_id] ?? null}
                  onToggle={() =>
                    setSelected(
                      selected.includes(run.run_id)
                        ? selected.filter((entry) => entry !== run.run_id)
                        : [...selected, run.run_id],
                    )
                  }
                  onOpen={onClose}
                />
              ))}
            </div>
          ))}

          {ready && visible.length === 0 ? (
            <div className="rd-empty">
              <Search size={16} aria-hidden="true" className="rd-faint" />
              {filtered ? (
                <>
                  <p className="rd-strong">No loaded run matches.</p>
                  <p className="rd-faint">
                    {nextPage
                      ? 'Older runs may. Clear the filters or load older runs.'
                      : 'Clear the filters to see every run.'}
                  </p>
                  <button
                    type="button"
                    className="rd-control rd-button"
                    onClick={() => setFilters(NO_FILTERS)}
                  >
                    Clear filters
                  </button>
                </>
              ) : (
                <>
                  <p className="rd-strong">No completed runs yet.</p>
                  <p className="rd-faint">
                    Runs of {WORKFLOW} show up here once they complete on
                    GitHub.
                  </p>
                </>
              )}
            </div>
          ) : null}

          {error && error.page > 1 ? (
            <ListError
              message={error.message}
              status={github}
              onRetry={() => retry(error.page)}
            />
          ) : ready && nextPage ? (
            <button
              type="button"
              className="rd-ghost gi-older"
              disabled={loadingOlder}
              aria-busy={loadingOlder || undefined}
              onClick={() => void load(nextPage)}
            >
              {loadingOlder
                ? 'Loading older runs…'
                : older === null
                  ? 'Load older runs'
                  : `Load ${plural(older, 'older run', 'older runs')}`}
            </button>
          ) : null}

          {phase === 'loading' ? (
            <div
              className="gi-loading"
              role="status"
              aria-busy="true"
              aria-label="Loading runs"
            >
              <div className="gi-loading-head">
                <span className="rd-skel gi-skel" style={{ width: 120 }} />
              </div>
              {SKELETON.map(([first, second], index) => (
                // biome-ignore lint/suspicious/noArrayIndexKey: fixed placeholder rows
                <div key={index} className="gi-loading-row">
                  <span className="rd-skel rd-skel-box" />
                  <span className="gi-loading-lines">
                    <span
                      className="rd-skel gi-skel"
                      style={{ width: first }}
                    />
                    <span
                      className="rd-skel gi-skel gi-skel-thin"
                      style={{ width: second }}
                    />
                  </span>
                  <span className="rd-skel gi-skel" style={{ width: 90 }} />
                </div>
              ))}
            </div>
          ) : null}

          {phase === 'failed' && error ? (
            <ListError
              message={error.message}
              status={github}
              onRetry={() => retry(1)}
            />
          ) : null}
        </div>

        <div className="rd-catalog">
          <span className="rd-dot" data-status={phase} aria-hidden="true" />
          <span role="status" className="rd-ellipsis rd-grow">
            {listStatus(phase, runs.length, total, repo)}
          </span>
          <button
            type="button"
            className="rd-ghost rd-icon-button"
            aria-label="Refresh runs"
            disabled={phase === 'loading'}
            onClick={() => retry(1)}
          >
            <RefreshCw size={16} aria-hidden="true" />
          </button>
        </div>
      </section>
    </Dialog>
  )
}
