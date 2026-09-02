import { useEffect, useMemo, useRef, useState } from 'react'
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
  const ref = useRef<HTMLDialogElement>(null)
  const titleRef = useRef<HTMLHeadingElement>(null)
  const eventRefs = useRef(new Map<string, HTMLElement>())
  const heading = splitTranscriptTitle(title)
  const events = useMemo(() => normalizeTranscript(messages), [messages])
  const summary = transcriptSummary(events)
  const [filter, setFilter] = useState<TranscriptFilter>('all')
  const [errorCursor, setErrorCursor] = useState(0)
  const [openToolId, setOpenToolId] = useState<string | null>(null)
  const errorEvents = useMemo(
    () => events.filter((event) => event.kind === 'tool' && event.isError),
    [events],
  )
  const visibleEvents = useMemo(() => {
    if (filter === 'messages')
      return events.filter((event) => event.kind === 'message')
    if (filter === 'errors') return errorEvents
    return events
  }, [errorEvents, events, filter])

  useEffect(() => {
    if (messages === undefined) return
    setFilter('all')
    setErrorCursor(0)
    setOpenToolId(null)
    eventRefs.current.clear()
  }, [messages])

  useEffect(() => {
    const dialog = ref.current
    if (!dialog) return
    if (open && !dialog.open) {
      dialog.showModal()
      titleRef.current?.focus()
    }
    if (!open && dialog.open) dialog.close()
  }, [open])

  const registerEvent = (id: string, node: HTMLElement | null) => {
    if (node) eventRefs.current.set(id, node)
    else eventRefs.current.delete(id)
  }

  const focusNextError = () => {
    if (errorEvents.length === 0) return
    const nextIndex = errorCursor % errorEvents.length
    const event = errorEvents[nextIndex]
    setFilter('errors')
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
    <dialog
      ref={ref}
      className="session-transcript-dialog h-[min(760px,calc(100dvh-48px))] w-[min(1120px,calc(100%-32px))] rounded-[6px] border border-line-strong bg-panel shadow-panel backdrop:bg-app-backdrop backdrop:backdrop-blur-[5px] max-[560px]:m-0 max-[560px]:h-dvh max-[560px]:w-screen max-[560px]:max-w-none max-[560px]:rounded-none max-[560px]:border-0"
      onClose={onClose}
      aria-labelledby="transcript-title"
    >
      <div className="session-transcript-shell flex h-full min-h-0 flex-col">
        <header className="session-transcript-header border-b border-line bg-panel">
          <div className="session-transcript-header-content">
            <div className="section-kicker mb-[7px]">
              Run evidence · Transcript
            </div>
            <h2
              id="transcript-title"
              ref={titleRef}
              tabIndex={-1}
              className="m-0 break-words text-[1.35rem] font-[570] tracking-[-0.025em] outline-none"
            >
              {heading.name}
            </h2>
            {heading.runId ? (
              <p
                className="m-0 mt-1 font-mono text-label text-ink-muted"
                title={heading.runId}
              >
                run {shortRunId(heading.runId)}
              </p>
            ) : null}
          </div>
          <button
            className="session-transcript-close"
            type="button"
            onClick={onClose}
            aria-label="Close session transcript"
          >
            ×
          </button>
        </header>
        <div className="session-transcript-body min-h-0 flex-1">
          <div
            className={`conversation-shell ${visibleEvents.length === 0 ? 'conversation-filter-empty' : ''}`}
            data-filter={filter}
          >
            <div className="conversation-toolbar">
              <div className="conversation-stats">
                <span>
                  <strong>{summary.messages}</strong> message
                  {summary.messages === 1 ? '' : 's'}
                </span>
                <span>
                  <strong>{summary.calls}</strong> tool
                  {summary.calls === 1 ? '' : 's'}
                </span>
                <span className={summary.errors ? 'has-errors' : undefined}>
                  <strong>{summary.errors}</strong> error
                  {summary.errors === 1 ? '' : 's'}
                </span>
              </div>
              <div className="conversation-actions">
                <div className="conversation-filters">
                  {(['all', 'messages', 'errors'] as const).map((candidate) => (
                    <button
                      key={candidate}
                      className={`conversation-filter ${filter === candidate ? 'is-active' : ''}`}
                      type="button"
                      aria-pressed={filter === candidate}
                      onClick={() => setFilter(candidate)}
                    >
                      {candidate === 'all'
                        ? 'All'
                        : candidate === 'messages'
                          ? 'Chat'
                          : 'Errors'}
                    </button>
                  ))}
                </div>
                {errorEvents.length > 0 ? (
                  <button
                    className="conversation-next-error"
                    type="button"
                    onClick={focusNextError}
                  >
                    Next error
                  </button>
                ) : null}
              </div>
            </div>
            <div className="conversation-error-position" aria-live="polite">
              {filter === 'errors' && errorEvents.length > 0
                ? `${errorEvents.length} error${errorEvents.length === 1 ? '' : 's'} shown`
                : ''}
            </div>
            {events.length === 0 ? (
              <p className="conversation-empty-filter">
                No transcript messages were retained for this run.
              </p>
            ) : (
              <div className="conversation-list" role="log">
                {visibleEvents.map((event) => (
                  <TranscriptEventCard
                    key={event.id}
                    event={event}
                    onRef={registerEvent}
                    openToolId={openToolId}
                    onToolToggle={(id, nextOpen) =>
                      setOpenToolId(nextOpen ? id : null)
                    }
                  />
                ))}
                <p className="conversation-empty-filter">
                  No events match this filter.
                </p>
              </div>
            )}
          </div>
        </div>
      </div>
    </dialog>
  )
}

function TranscriptEventCard({
  event,
  onRef,
  openToolId,
  onToolToggle,
}: {
  event: TranscriptEvent
  onRef: (id: string, node: HTMLElement | null) => void
  openToolId: string | null
  onToolToggle: (id: string, open: boolean) => void
}) {
  if (event.kind === 'message') {
    const isUser = event.role === 'user'
    return (
      <article
        ref={(node) => onRef(event.id, node)}
        className={`conversation-event conversation-message ${isUser ? 'conversation-user' : ''}`}
        data-kind="message"
        data-error="false"
      >
        <div className="conversation-card">
          <header>
            <strong>
              <span>{isUser ? 'You' : 'Assistant'}</span>
            </strong>
            {(event.provider || event.model) && (
              <span>
                {[event.provider, event.model].filter(Boolean).join('/')}
              </span>
            )}
          </header>
          <div className="conversation-copy">{event.text}</div>
        </div>
      </article>
    )
  }

  const resultText = event.result
    ? formatTranscriptPayload(event.result.text || event.result.details)
    : ''
  return (
    <details
      ref={(node) => onRef(event.id, node)}
      className={`conversation-event conversation-tool ${event.isError ? 'conversation-tool-error' : ''}`}
      data-kind="tool"
      data-error={event.isError ? 'true' : 'false'}
      open={openToolId === event.id}
      onToggle={(toggleEvent) =>
        onToolToggle(event.id, toggleEvent.currentTarget.open)
      }
    >
      <summary>
        <span className="conversation-tool-icon" aria-hidden="true">
          {event.isError ? '!' : '›'}
        </span>
        <span className="conversation-tool-name">
          <small>tool</small>
          <strong>{event.functionId}</strong>
          {event.callId && <em>{event.callId}</em>}
        </span>
        <span className="conversation-tool-status">
          {event.isError ? 'error' : (event.status ?? 'no result recorded')}
        </span>
        <span className="conversation-chevron" aria-hidden="true">
          ⌄
        </span>
      </summary>
      <div className="conversation-tool-body">
        {event.arguments != null && (
          <div className="conversation-payload">
            <span>Arguments</span>
            <pre>{formatTranscriptPayload(event.arguments)}</pre>
          </div>
        )}
        {event.result && (
          <div
            className={`conversation-payload ${event.isError ? 'conversation-payload-error' : ''}`}
          >
            <span>Result</span>
            <pre>{resultText || 'No result text'}</pre>
          </div>
        )}
      </div>
    </details>
  )
}
