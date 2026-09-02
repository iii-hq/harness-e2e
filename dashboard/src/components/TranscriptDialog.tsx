import { ChevronDown, Copy, Search, X } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import {
  buttonClassName,
  Dialog,
  FilterChip,
  FilterChipGroup,
  Input,
} from '@/design-system'
import {
  formatTranscriptPayload,
  normalizeTranscript,
  type TranscriptEvent,
  transcriptSummary,
} from '@/lib/transcript-view'

type TranscriptFilter = 'all' | 'messages' | 'errors'

/**
 * Audit TR-07: callers pass "Scenario · <run id>". The run id is kept in a
 * secondary line, shortened when it is a long hash, with the full value in a
 * tooltip.
 */
export function splitTranscriptTitle(title: string) {
  const separator = title.lastIndexOf(' · ')
  if (separator === -1) return { name: title, runId: null }
  return {
    name: title.slice(0, separator),
    runId: title.slice(separator + 3),
  }
}

function shortRunId(value: string) {
  return value.length > 20 ? `${value.slice(0, 12)}…${value.slice(-4)}` : value
}

/** Audit TR-02: each event carries when it happened, relative to the first. */
export function relativeTime(
  timestamp: string | number | null | undefined,
  origin: number | null,
) {
  if (timestamp == null || origin === null) return null
  const value =
    typeof timestamp === 'number' ? timestamp : Date.parse(String(timestamp))
  if (!Number.isFinite(value)) return null
  const seconds = Math.max(0, Math.round((value - origin) / 1000))
  if (seconds < 60) return `+${seconds}s`
  return `+${Math.floor(seconds / 60)}m ${String(seconds % 60).padStart(2, '0')}s`
}

function transcriptOrigin(events: TranscriptEvent[]) {
  for (const event of events) {
    if (event.timestamp == null) continue
    const value =
      typeof event.timestamp === 'number'
        ? event.timestamp
        : Date.parse(String(event.timestamp))
    if (Number.isFinite(value)) return value
  }
  return null
}

export function eventText(event: TranscriptEvent) {
  if (event.kind === 'message') return event.text ?? ''
  return [
    event.functionId,
    formatTranscriptPayload(event.arguments),
    event.result
      ? formatTranscriptPayload(event.result.text || event.result.details)
      : '',
  ]
    .filter(Boolean)
    .join('\n')
}

/** Audit TR-03: search narrows the log and says how many events matched. */
export function matchesQuery(event: TranscriptEvent, query: string) {
  const normalized = query.trim().toLowerCase()
  if (!normalized) return true
  return eventText(event).toLowerCase().includes(normalized)
}

const ROLE_LABELS: Record<string, string> = {
  user: 'user',
  assistant: 'assistant',
  system: 'system',
  tool: 'tool',
}

function CopyButton({ value, label }: { value: string; label: string }) {
  const [copied, setCopied] = useState(false)
  return (
    <button
      className={buttonClassName({ variant: 'quiet', size: 'compact' })}
      type="button"
      aria-label={label}
      title={label}
      onClick={(click) => {
        click.stopPropagation()
        void navigator.clipboard?.writeText(value).then(() => {
          setCopied(true)
          window.setTimeout(() => setCopied(false), 1500)
        })
      }}
    >
      <Copy size={12} aria-hidden="true" />
      {copied ? 'copied' : 'copy'}
    </button>
  )
}

export function TranscriptDialog({
  title,
  messages,
  open,
  onClose,
}: {
  title: string
  messages: unknown
  open: boolean
  onClose: () => void
}) {
  const eventRefs = useRef(new Map<string, HTMLElement>())
  const heading = splitTranscriptTitle(title)
  const events = useMemo(() => normalizeTranscript(messages), [messages])
  const summary = transcriptSummary(events)
  const origin = useMemo(() => transcriptOrigin(events), [events])
  const [filter, setFilter] = useState<TranscriptFilter>('all')
  const [query, setQuery] = useState('')
  const [errorCursor, setErrorCursor] = useState(0)
  const [openToolId, setOpenToolId] = useState<string | null>(null)
  const errorEvents = useMemo(
    () => events.filter((event) => event.kind === 'tool' && event.isError),
    [events],
  )
  const visibleEvents = useMemo(() => {
    const base =
      filter === 'messages'
        ? events.filter((event) => event.kind === 'message')
        : filter === 'errors'
          ? errorEvents
          : events
    return base.filter((event) => matchesQuery(event, query))
  }, [errorEvents, events, filter, query])

  useEffect(() => {
    if (messages === undefined) return
    setFilter('all')
    setQuery('')
    setErrorCursor(0)
    setOpenToolId(null)
    eventRefs.current.clear()
  }, [messages])

  const registerEvent = (id: string, node: HTMLElement | null) => {
    if (node) eventRefs.current.set(id, node)
    else eventRefs.current.delete(id)
  }

  const focusNextError = () => {
    if (errorEvents.length === 0) return
    const nextIndex = errorCursor % errorEvents.length
    const event = errorEvents[nextIndex]
    setFilter('errors')
    setQuery('')
    setOpenToolId(event.id)
    setErrorCursor((nextIndex + 1) % errorEvents.length)
    window.requestAnimationFrame(() => {
      eventRefs.current.get(event.id)?.scrollIntoView({
        behavior: 'smooth',
        block: 'center',
      })
    })
  }

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="lg"
      tall
      kicker="run evidence · transcript"
      title={heading.name}
      description={
        heading.runId ? (
          <span className="font-mono text-label" title={heading.runId}>
            run {shortRunId(heading.runId)}
          </span>
        ) : undefined
      }
      closeLabel="Close session transcript"
      className="ds-root"
    >
      <div className="grid min-w-0 gap-4" data-transcript>
        <div className="sticky top-0 z-10 grid gap-3 bg-panel pb-2">
          <div className="flex flex-wrap items-center gap-2">
            <FilterChipGroup label="Transcript events">
              <FilterChip
                active={filter === 'all'}
                count={events.length}
                onClick={() => setFilter('all')}
              >
                all
              </FilterChip>
              <FilterChip
                active={filter === 'messages'}
                count={summary.messages}
                onClick={() => setFilter('messages')}
              >
                messages
              </FilterChip>
              {/* Audit TR-03: no "next error" control when there are none. */}
              {summary.errors > 0 ? (
                <FilterChip
                  active={filter === 'errors'}
                  count={summary.errors}
                  onClick={() => setFilter('errors')}
                >
                  errors
                </FilterChip>
              ) : null}
            </FilterChipGroup>
            {errorEvents.length > 0 ? (
              <button
                className={buttonClassName({
                  variant: 'secondary',
                  size: 'compact',
                })}
                type="button"
                onClick={focusNextError}
              >
                next error
              </button>
            ) : null}
            <span className="ms-auto font-mono text-label text-ink-muted">
              {summary.calls} tool {summary.calls === 1 ? 'call' : 'calls'}
            </span>
          </div>
          <div className="relative">
            <Search
              className="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-ink-muted"
              size={14}
              aria-hidden="true"
            />
            <Input
              className="pr-9 pl-9"
              type="text"
              value={query}
              placeholder="Search the transcript"
              aria-label="Search the transcript"
              onChange={(event) => setQuery(event.target.value)}
            />
            {query ? (
              <button
                className="absolute top-1/2 right-1 inline-grid size-7 -translate-y-1/2 place-items-center rounded-[6px] border-0 bg-transparent text-ink-muted hover:bg-[var(--surface-soft)] hover:text-ink"
                type="button"
                onClick={() => setQuery('')}
                aria-label="Clear search"
              >
                <X size={13} aria-hidden="true" />
              </button>
            ) : null}
          </div>
          <output
            className="font-mono text-label text-ink-muted"
            aria-live="polite"
          >
            {events.length === 0
              ? 'no transcript retained'
              : `${visibleEvents.length} of ${events.length} events shown`}
          </output>
        </div>

        {events.length === 0 ? (
          <p className="m-0 text-xs text-ink-soft">
            No transcript messages were retained for this run.
          </p>
        ) : visibleEvents.length === 0 ? (
          <p className="m-0 text-xs text-ink-soft">
            No events match this filter.
          </p>
        ) : (
          // Audit TR-02: one left-aligned column with a role rail, never a
          // centred 760px block inside a 1120px dialog.
          <div className="grid min-w-0 max-w-[52rem] gap-3" role="log">
            {visibleEvents.map((event) => (
              <TranscriptEventCard
                key={event.id}
                event={event}
                origin={origin}
                onRef={registerEvent}
                openToolId={openToolId}
                onToolToggle={(id, nextOpen) =>
                  setOpenToolId(nextOpen ? id : null)
                }
              />
            ))}
          </div>
        )}
      </div>
    </Dialog>
  )
}

function TranscriptEventCard({
  event,
  origin,
  onRef,
  openToolId,
  onToolToggle,
}: {
  event: TranscriptEvent
  origin: number | null
  onRef: (id: string, node: HTMLElement | null) => void
  openToolId: string | null
  onToolToggle: (id: string, open: boolean) => void
}) {
  const at = relativeTime(event.timestamp, origin)
  if (event.kind === 'message') {
    const role = ROLE_LABELS[event.role ?? ''] ?? event.role ?? 'assistant'
    return (
      <article
        ref={(node) => onRef(event.id, node)}
        className="grid min-w-0 grid-cols-[5.5rem_minmax(0,1fr)] gap-3"
        data-kind="message"
        data-role={role}
        data-error="false"
      >
        <span className="grid content-start gap-0.5 pt-0.5 text-right">
          <span className="ds-label">{role}</span>
          {at ? (
            <span className="font-mono text-label text-ink-muted">{at}</span>
          ) : null}
        </span>
        <div className="grid min-w-0 gap-2 rounded-[6px] bg-[var(--surface-fill)] p-3">
          {event.provider || event.model ? (
            <span className="font-mono text-label text-ink-muted">
              {[event.provider, event.model].filter(Boolean).join('/')}
            </span>
          ) : null}
          <p className="m-0 whitespace-pre-wrap break-words text-sm leading-6 text-ink">
            {event.text}
          </p>
          <span className="justify-self-start">
            <CopyButton value={event.text ?? ''} label="Copy this message" />
          </span>
        </div>
      </article>
    )
  }

  const resultText = event.result
    ? formatTranscriptPayload(event.result.text || event.result.details)
    : ''
  const argumentsText =
    event.arguments == null ? '' : formatTranscriptPayload(event.arguments)
  return (
    <article
      ref={(node) => onRef(event.id, node)}
      className="grid min-w-0 grid-cols-[5.5rem_minmax(0,1fr)] gap-3"
      data-kind="tool"
      data-error={event.isError ? 'true' : 'false'}
    >
      <span className="grid content-start gap-0.5 pt-0.5 text-right">
        <span className="ds-label">tool</span>
        {at ? (
          <span className="font-mono text-label text-ink-muted">{at}</span>
        ) : null}
      </span>
      <details
        className={`group grid min-w-0 rounded-[6px] p-3 ${
          event.isError
            ? 'bg-[color-mix(in_srgb,var(--danger)_8%,transparent)]'
            : 'bg-[var(--surface-fill)]'
        }`}
        open={openToolId === event.id}
        onToggle={(toggleEvent) =>
          onToolToggle(event.id, toggleEvent.currentTarget.open)
        }
      >
        <summary className="flex min-w-0 cursor-pointer list-none items-center gap-2 text-xs marker:hidden">
          <ChevronDown
            className="size-4 shrink-0 -rotate-90 text-ink-muted transition-transform duration-[var(--ds-duration-fast)] group-open:rotate-0 motion-reduce:transition-none"
            aria-hidden="true"
          />
          <strong
            className="min-w-0 truncate font-mono font-medium text-ink"
            title={event.callId ?? undefined}
          >
            {event.functionId}
          </strong>
          <span
            className={`ms-auto font-mono text-label ${event.isError ? 'text-danger' : 'text-ink-muted'}`}
          >
            {event.isError ? 'error' : (event.status ?? 'no result recorded')}
          </span>
        </summary>
        <div className="mt-3 grid gap-3">
          {argumentsText ? (
            <div className="grid gap-1">
              <span className="flex items-center gap-2">
                <span className="ds-label">arguments</span>
                <CopyButton value={argumentsText} label="Copy the arguments" />
              </span>
              <pre className="m-0 max-h-72 overflow-auto rounded-[6px] bg-canvas p-3 font-mono text-xs leading-5 text-ink-soft">
                {argumentsText}
              </pre>
            </div>
          ) : null}
          <div className="grid gap-1">
            <span className="flex items-center gap-2">
              <span className="ds-label">result</span>
              {resultText ? (
                <CopyButton value={resultText} label="Copy the result" />
              ) : null}
            </span>
            <pre className="m-0 max-h-72 overflow-auto rounded-[6px] bg-canvas p-3 font-mono text-xs leading-5 text-ink-soft">
              {resultText || 'no result recorded'}
            </pre>
          </div>
        </div>
      </details>
    </article>
  )
}
