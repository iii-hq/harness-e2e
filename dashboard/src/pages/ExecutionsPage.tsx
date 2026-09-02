import { useCallback, useEffect, useMemo, useState } from 'react'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { StatusBadge } from '@/design-system'
import { hashForExecution } from '@/hooks/use-hash-route'
import { useLatestRequest } from '@/hooks/use-latest-request'
import {
  type DashboardDataBridge,
  type DashboardExecutionSummary,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import {
  buildExecutionPresentation,
  categoryMessage,
  type ExecutionPresentation,
  formatDate,
  formatDuration,
  formatPercent,
} from '@/lib/execution-view'
import { modelNames, statusCopy } from '@/pages/OverviewPage'
import '@/design-system/styles.css'

const sectionLabelClassName =
  'font-mono text-label font-medium uppercase tracking-[0.06em] text-ink-muted'

const triggerLabels: Record<string, string> = {
  schedule: 'Scheduled',
  workflow_dispatch: 'Manual',
  local: 'Local',
}

function triggerLabel(event: string) {
  return triggerLabels[event] ?? event
}

function countBy<T>(items: T[], key: (item: T) => string | null) {
  const counts = new Map<string, number>()
  for (const item of items) {
    const value = key(item)
    if (!value) continue
    counts.set(value, (counts.get(value) ?? 0) + 1)
  }
  return counts
}

function ModelList({ models }: { models: ExecutionPresentation['subjects'] }) {
  if (models.length === 0) return <span className="text-ink-muted">—</span>
  return (
    <div className="grid gap-0.5">
      {models.map((model) => (
        <span key={`${model.provider}/${model.model}`} className="text-ink">
          {model.model}
          {model.provider ? (
            <small className="block text-ink-muted">{model.provider}</small>
          ) : null}
        </span>
      ))}
    </div>
  )
}

function isInteractiveTarget(target: EventTarget | null) {
  return (
    target instanceof Element &&
    target.closest('a, button, input, select, textarea, summary') !== null
  )
}

export function ExecutionHistory({
  executions,
}: {
  executions: DashboardExecutionSummary[]
}) {
  const [query, setQuery] = useState('')
  const [status, setStatus] = useState('all')
  const [event, setEvent] = useState('all')
  // The table shows a status derived from the assessment summary, so the
  // filter offers exactly those labels, with counts, instead of the raw
  // backend status that never matched the column (audit E-02 / O-11).
  const rows = useMemo(
    () =>
      executions.map((execution) => {
        const presentation = buildExecutionPresentation(execution)
        return {
          execution,
          presentation,
          status: statusCopy(presentation),
          searchText: [
            presentation.label,
            execution.workflow_name,
            execution.id,
            execution.run_id,
            execution.completed_at,
            formatDate(presentation.completedAt),
            execution.source?.sha,
            ...presentation.subjects.flatMap((model) => [
              model.model,
              `${model.provider}/${model.model}`,
            ]),
          ]
            .filter(Boolean)
            .join(' ')
            .toLowerCase(),
        }
      }),
    [executions],
  )
  const statusCounts = useMemo(
    () => countBy(rows, (row) => row.status.status),
    [rows],
  )
  const statusOptions = useMemo(
    () =>
      [...statusCounts.entries()].map(([value, count]) => ({
        value,
        count,
        label:
          rows.find((row) => row.status.status === value)?.status.label ??
          value,
      })),
    [rows, statusCounts],
  )
  const eventCounts = useMemo(
    () => countBy(rows, (row) => row.execution.event ?? null),
    [rows],
  )
  const filtered = useMemo(() => {
    const normalized = query.trim().toLowerCase()
    return rows.filter((row) => {
      if (status !== 'all' && row.status.status !== status) return false
      if (event !== 'all' && row.execution.event !== event) return false
      return !normalized || row.searchText.includes(normalized)
    })
  }, [event, rows, query, status])
  return (
    <section className="mt-6 min-w-0" aria-labelledby="executions-heading">
      <div className="flex items-baseline gap-3">
        <h2
          className={`m-0 uppercase ${sectionLabelClassName}`}
          id="executions-heading"
        >
          Recent executions
        </h2>
        <span
          className="ms-auto font-mono text-xs text-ink-muted"
          aria-live="polite"
        >
          {filtered.length} of {executions.length} executions
        </span>
      </div>
      <section
        className="mt-3 grid gap-2 sm:grid-cols-2 md:grid-cols-[minmax(14rem,1fr)_auto_auto] [&_input]:min-h-9 [&_input]:w-full [&_input]:rounded-[6px] [&_input]:border-0 [&_input]:bg-[var(--surface-fill)] [&_input]:px-3 [&_input]:text-[13px] [&_input]:text-ink [&_input]:placeholder:text-ink-muted [&_select]:min-h-9 [&_select]:w-full [&_select]:rounded-[6px] [&_select]:border-0 [&_select]:bg-[var(--surface-fill)] [&_select]:px-3 [&_select]:font-mono [&_select]:text-[13px] [&_select]:lowercase [&_select]:text-ink"
        aria-label="Execution filters"
      >
        <label className="sm:col-span-2 md:col-span-1">
          <span className="visually-hidden">Search executions</span>
          <input
            type="search"
            placeholder="Search label, model, id or date"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
        <label>
          <span className="visually-hidden">Filter by result</span>
          <select
            value={status}
            onChange={(event) => setStatus(event.target.value)}
          >
            <option value="all">all results · {executions.length}</option>
            {statusOptions.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label.toLowerCase()} · {option.count}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span className="visually-hidden">Filter by trigger</span>
          <select
            value={event}
            onChange={(event) => setEvent(event.target.value)}
          >
            <option value="all">all triggers · {executions.length}</option>
            {[...eventCounts.entries()].map(([value, count]) => (
              <option key={value} value={value}>
                {triggerLabel(value).toLowerCase()} · {count}
              </option>
            ))}
          </select>
        </label>
      </section>
      <div className="mt-2 min-w-0 overflow-x-auto">
        <table className="w-full min-w-[52rem] border-collapse text-left font-mono text-xs md:text-[13px] [&_a]:font-medium [&_a]:text-ink [&_a]:underline-offset-4 [&_a:hover]:underline [&_td]:border-0 [&_td]:px-3 [&_td]:py-2.5 [&_th]:border-0 [&_th]:px-3 [&_th]:py-2 [&_th]:font-mono [&_th]:text-[11px] [&_th]:font-medium [&_th]:uppercase [&_th]:tracking-[0.06em] [&_th]:text-ink-muted [&_tbody_tr]:cursor-pointer [&_tbody_tr]:transition-colors [&_tbody_tr:hover]:bg-[var(--surface-soft)]">
          <thead>
            <tr>
              <th scope="col">Execution</th>
              <th scope="col">Result</th>
              <th scope="col">Subject</th>
              <th scope="col">Scope</th>
              <th scope="col">Outcome</th>
              <th scope="col">Efficiency</th>
            </tr>
          </thead>
          <tbody>
            {filtered.map(({ execution, presentation, status }) => {
              const href = hashForExecution(execution.id)
              const evidenceNote =
                execution.availability === 'aggregate'
                  ? 'aggregate report'
                  : execution.availability === 'unavailable'
                    ? 'no report retained'
                    : null
              const partialCoverage =
                presentation.coverage !== null && presentation.coverage < 1
              return (
                // The whole row navigates; the label link keeps the keyboard
                // and screen-reader path (audit E-08 / O-15).
                <tr
                  key={execution.id}
                  onClick={(click) => {
                    if (isInteractiveTarget(click.target)) return
                    window.location.hash = href
                  }}
                >
                  <td data-label="Execution">
                    <a href={href}>{presentation.label}</a>
                    <small className="block text-ink-muted">
                      {formatDate(presentation.completedAt)}
                      {execution.event
                        ? ` · ${triggerLabel(execution.event).toLowerCase()}`
                        : ''}
                    </small>
                  </td>
                  <td data-label="Result">
                    <StatusBadge status={status.status} label={status.label} />
                    {evidenceNote ? (
                      <small className="block text-ink-muted">
                        {evidenceNote}
                      </small>
                    ) : null}
                  </td>
                  <td
                    data-label="Subject"
                    title={modelNames(presentation.subjects)}
                  >
                    <ModelList models={presentation.subjects} />
                  </td>
                  {presentation.available ? (
                    <>
                      <td data-label="Scope">
                        <div className="grid gap-0.5">
                          <strong>
                            {presentation.receivedReports ?? '—'}/
                            {presentation.expectedReports ?? '—'}
                          </strong>
                          {partialCoverage ? (
                            <small>
                              {formatPercent(presentation.coverage)} coverage
                            </small>
                          ) : null}
                        </div>
                      </td>
                      <td data-label="Outcome">
                        <div className="grid gap-0.5">
                          <strong>
                            {formatPercent(presentation.passRate)}
                          </strong>
                          {presentation.primaryIssue ? (
                            <small>
                              {categoryMessage(
                                presentation.primaryIssue.category,
                                presentation.primaryIssue.count,
                              )}
                            </small>
                          ) : null}
                        </div>
                      </td>
                      <td data-label="Efficiency">
                        <div className="grid gap-0.5">
                          <strong>
                            {formatDuration(presentation.modelRuntimeSeconds)}
                          </strong>
                          {presentation.execution.totals?.total_tokens ? (
                            <small>
                              {presentation.execution.totals.total_tokens.toLocaleString()}{' '}
                              tokens
                            </small>
                          ) : null}
                        </div>
                      </td>
                    </>
                  ) : (
                    <td
                      data-label="Report"
                      colSpan={3}
                      className="text-ink-muted"
                    >
                      —
                    </td>
                  )}
                </tr>
              )
            })}
          </tbody>
        </table>
        {filtered.length === 0 && (
          <p className="m-0 py-6 text-center text-[13px] text-[var(--color-ink-faint)]">
            No executions match these filters.{' '}
            <button
              type="button"
              className="font-mono text-[var(--accent)] underline-offset-4 hover:underline"
              onClick={() => {
                setQuery('')
                setStatus('all')
                setEvent('all')
              }}
            >
              clear filters
            </button>
          </p>
        )}
      </div>
    </section>
  )
}

export function ExecutionsPage() {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [executions, setExecutions] = useState<DashboardExecutionSummary[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const beginRequest = useLatestRequest()

  const load = useCallback(async () => {
    const request = beginRequest()
    setError(null)
    try {
      const nextBridge = bridge ?? (await getDashboardDataBridge())
      if (!request.isCurrent()) return
      setBridge(nextBridge)
      const manifest = await nextBridge.listExecutions({ limit: 100 })
      if (!request.isCurrent()) return
      setExecutions(manifest.executions ?? [])
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setLoading(false)
    }
  }, [beginRequest, bridge])

  useEffect(() => {
    void load()
  }, [load])

  return (
    <div className="ds-root min-h-dvh bg-canvas text-ink">
      <DashboardPageActions
        active="executions"
        actionsLabel="Execution actions"
      />
      <div className="page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-16 md:w-[calc(100%_-_3rem)]">
        {error ? (
          <p className="mt-6 text-sm text-danger" role="alert">
            {error}
          </p>
        ) : loading ? (
          <p className="mt-6 text-sm text-ink-soft" role="status">
            Loading executions…
          </p>
        ) : (
          <ExecutionHistory executions={executions} />
        )}
      </div>
    </div>
  )
}
