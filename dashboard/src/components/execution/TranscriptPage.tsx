import { Copy as CopyIcon, Download, Search, SkipForward } from 'lucide-react'
import { useMemo, useRef, useState } from 'react'
import { PageHeader } from '@/design-system'
import {
  formatTranscriptPayload,
  normalizeTranscript,
  type TranscriptEvent,
  transcriptSummary,
} from '@/lib/transcript-view'
import './execution-page.css'

export type TranscriptBlock =
  | { kind: 'message'; event: TranscriptEvent }
  | { kind: 'tools'; id: string; events: TranscriptEvent[] }

/** Consecutive tool calls fold into one block; messages stand alone. */
export function transcriptBlocks(events: TranscriptEvent[]): TranscriptBlock[] {
  const blocks: TranscriptBlock[] = []
  for (const event of events) {
    const last = blocks.at(-1)
    if (event.kind === 'tool' && last?.kind === 'tools') last.events.push(event)
    else if (event.kind === 'tool')
      blocks.push({ kind: 'tools', id: event.id, events: [event] })
    else blocks.push({ kind: 'message', event })
  }
  return blocks
}

export type ErrorCause = { cause: string; functionId: string; ids: string[] }

/** Tool errors grouped by function and the first line of what they said. */
export function errorsByCause(events: TranscriptEvent[]): ErrorCause[] {
  const causes = new Map<string, ErrorCause>()
  for (const event of events) {
    if (event.kind !== 'tool' || !event.isError) continue
    const text =
      event.result?.text ||
      (event.result?.details
        ? formatTranscriptPayload(event.result.details)
        : '')
    const first = (text.split('\n').find((line) => line.trim()) ?? 'no message')
      .trim()
      .slice(0, 160)
    const functionId = event.functionId ?? 'tool'
    const key = `${functionId}\u0000${first}`
    const cause = causes.get(key) ?? { cause: first, functionId, ids: [] }
    cause.ids.push(event.id)
    causes.set(key, cause)
  }
  return [...causes.values()].sort((a, b) => b.ids.length - a.ids.length)
}

function when(value: TranscriptEvent['timestamp']) {
  if (value === null || value === undefined) return ''
  const time = typeof value === 'number' ? value : Date.parse(String(value))
  return Number.isFinite(time)
    ? new Date(time).toLocaleTimeString('en-US', { hour12: false })
    : ''
}

function ToolEvent({ event }: { event: TranscriptEvent }) {
  return (
    <details
      className="ep-tool"
      id={`ev-${event.id}`}
      data-error={event.isError || undefined}
    >
      <summary>
        <span className="ep-mono">{event.functionId ?? 'tool'}</span>
        <span
          className="ep-tool-state"
          data-tone={event.isError ? 'alert' : 'ok'}
        >
          {event.isError
            ? 'Error'
            : event.status === 'pending'
              ? 'No result'
              : 'Done'}
        </span>
        <span className="ep-faint ep-mono">{when(event.timestamp)}</span>
      </summary>
      {event.arguments != null ? (
        <>
          <p className="ep-label">Input</p>
          <pre className="ep-pre">
            {formatTranscriptPayload(event.arguments)}
          </pre>
        </>
      ) : null}
      {event.result ? (
        <>
          <p className="ep-label">{event.isError ? 'Error' : 'Result'}</p>
          <pre className="ep-pre">
            {event.result.text || formatTranscriptPayload(event.result.details)}
          </pre>
        </>
      ) : null}
    </details>
  )
}

type TranscriptFilter = 'all' | 'messages' | 'tools' | 'errors'

function stamp(value: TranscriptEvent['timestamp']) {
  if (value === null || value === undefined) return null
  const time = typeof value === 'number' ? value : Date.parse(String(value))
  return Number.isFinite(time) ? time : null
}

function offset(start: number | null, value: TranscriptEvent['timestamp']) {
  const time = stamp(value)
  if (start === null || time === null) return ''
  const seconds = Math.max(0, Math.round((time - start) / 1000))
  const m = Math.floor(seconds / 60)
  return m > 0
    ? `+${m}m ${String(seconds % 60).padStart(2, '0')}s`
    : `+${seconds}s`
}

function eventText(event: TranscriptEvent) {
  return event.kind === 'message'
    ? (event.text ?? '')
    : `${event.functionId ?? ''} ${formatTranscriptPayload(event.arguments)} ${event.result?.text ?? ''}`
}

const CLAMP = 900

function MessageText({ text }: { text: string }) {
  const [open, setOpen] = useState(false)
  const long = text.length > CLAMP
  return (
    <>
      <p className="ep-message-text">
        {long && !open ? `${text.slice(0, CLAMP).trimEnd()}…` : text}
      </p>
      {long ? (
        <button
          type="button"
          className="ds-button ds-button-quiet ds-button-compact"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
        >
          {open ? 'Show less' : 'Show the whole message'}
        </button>
      ) : null}
    </>
  )
}

/** A run's transcript as its own page: search, filter, outline, grouped
 *  tool calls with their errors by cause, next error, copy and download. */
export function TranscriptPage({
  title,
  runLine,
  messages,
  backHref,
  backLabel,
  evidenceHref,
}: {
  title: string
  runLine: string
  messages: unknown
  backHref: string
  backLabel: string
  evidenceHref?: string
}) {
  const events = useMemo(() => normalizeTranscript(messages), [messages])
  const blocks = useMemo(() => transcriptBlocks(events), [events])
  const summary = transcriptSummary(events)
  const errorIds = useMemo(
    () =>
      events
        .filter((event) => event.kind === 'tool' && event.isError)
        .map((event) => event.id),
    [events],
  )
  const [cursor, setCursor] = useState(-1)
  const [copied, setCopied] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [filter, setFilter] = useState<TranscriptFilter>('all')
  const main = useRef<HTMLOListElement>(null)
  const start = useMemo(
    () =>
      events.map((event) => stamp(event.timestamp)).find((t) => t !== null) ??
      null,
    [events],
  )
  // One outline entry per message, with the tool calls that follow it.
  const chapters = useMemo(() => {
    const list: Array<{
      id: string
      t: string
      label: string
      calls: number
      errors: number
    }> = []
    for (const block of blocks) {
      if (block.kind === 'message')
        list.push({
          id: block.event.id,
          t: offset(start, block.event.timestamp),
          label:
            (block.event.text ?? '').split('\n')[0].slice(0, 80) ||
            block.event.role ||
            'message',
          calls: 0,
          errors: 0,
        })
      else {
        if (list.length === 0)
          list.push({
            id: block.id,
            t: offset(start, block.events[0]?.timestamp),
            label: 'Start',
            calls: 0,
            errors: 0,
          })
        const last = list[list.length - 1]
        last.calls += block.events.length
        last.errors += block.events.filter((event) => event.isError).length
      }
    }
    return list
  }, [blocks, start])
  const needle = query.trim().toLowerCase()
  const visible = blocks.flatMap((block): TranscriptBlock[] => {
    if (block.kind === 'message') {
      if (filter === 'tools' || filter === 'errors') return []
      if (needle && !eventText(block.event).toLowerCase().includes(needle))
        return []
      return [block]
    }
    if (filter === 'messages') return []
    const kept = block.events.filter(
      (event) =>
        (filter !== 'errors' || event.isError) &&
        (!needle || eventText(event).toLowerCase().includes(needle)),
    )
    return kept.length ? [{ ...block, events: kept }] : []
  })
  const filters: Array<[TranscriptFilter, string, number]> = [
    ['all', 'All', summary.messages + summary.calls],
    ['messages', 'Messages', summary.messages],
    ['tools', 'Tool calls', summary.calls],
    ['errors', 'Errors', summary.errors],
  ]

  const reveal = (id: string) => {
    const node = document.getElementById(`ev-${id}`)
    if (!node) return
    node.closest('details.ep-tools')?.setAttribute('open', '')
    if (node instanceof HTMLDetailsElement) node.open = true
    node.scrollIntoView({ block: 'center' })
    node.querySelector('summary')?.focus()
  }
  const nextError = () => {
    if (errorIds.length === 0) return
    if (filter === 'messages') setFilter('all')
    const next = (cursor + 1) % errorIds.length
    setCursor(next)
    window.setTimeout(() => reveal(errorIds[next]), 0)
  }
  const plain = () =>
    events
      .map((event) =>
        event.kind === 'message'
          ? `${event.role}: ${event.text}`
          : `${event.functionId}${event.isError ? ' (error)' : ''}: ${formatTranscriptPayload(event.arguments)}\n→ ${event.result?.text ?? ''}`,
      )
      .join('\n\n')
  const copy = async (key: string, value: string) => {
    try {
      await navigator.clipboard.writeText(value)
      setCopied(key)
      window.setTimeout(() => setCopied(null), 1500)
    } catch {
      setCopied(null)
    }
  }
  const download = () => {
    const blob = new Blob([JSON.stringify(messages ?? [], null, 2)], {
      type: 'application/json',
    })
    const link = document.createElement('a')
    link.href = URL.createObjectURL(blob)
    link.download = `${title.replace(/[^a-z0-9_-]+/gi, '-')}-transcript.json`
    link.click()
    URL.revokeObjectURL(link.href)
  }

  return (
    <div className="ds-root page-shell ep-page" data-transcript-page>
      <PageHeader
        variant="detail"
        mono
        back={{ label: backLabel, href: backHref }}
        title={title}
        summary={`${runLine} · ${summary.messages} messages · ${summary.calls} tool calls · ${summary.errors} errors`}
        context="Transcript"
        actions={
          <>
            {evidenceHref ? (
              <a
                className="ds-button ds-button-quiet ds-button-default"
                href={evidenceHref}
              >
                Evidence record
              </a>
            ) : null}
            <button
              type="button"
              className="ds-button ds-button-quiet ds-button-default"
              onClick={() => void copy('all', plain())}
            >
              <CopyIcon size={14} aria-hidden="true" />
              {copied === 'all' ? 'Copied' : 'Copy all'}
            </button>
            <button
              type="button"
              className="ds-button ds-button-quiet ds-button-default"
              onClick={download}
            >
              <Download size={14} aria-hidden="true" />
              Download JSON
            </button>
          </>
        }
      />
      {events.length === 0 ? (
        <p className="ep-faint">This run recorded no transcript.</p>
      ) : (
        <>
          <div
            className="ep-toolbar"
            role="toolbar"
            aria-label="Filter the transcript"
          >
            <label className="ep-search">
              <Search size={14} aria-hidden="true" />
              <span className="ep-sr">Search the transcript</span>
              <input
                type="search"
                value={query}
                placeholder="Search messages, functions, inputs and errors"
                onChange={(event) => setQuery(event.target.value)}
              />
            </label>
            <fieldset className="ep-segments">
              <legend className="ep-sr">Show</legend>
              {filters.map(([key, label, count]) => (
                <button
                  key={key}
                  type="button"
                  aria-pressed={filter === key}
                  data-selected={filter === key}
                  data-transcript-filter={key}
                  onClick={() => setFilter(key)}
                >
                  {label}{' '}
                  <span
                    className={
                      key === 'errors' && count ? 'er-miss' : 'ep-faint'
                    }
                  >
                    {count}
                  </span>
                </button>
              ))}
            </fieldset>
            <button
              type="button"
              className="ds-button ds-button-secondary ds-button-compact"
              disabled={errorIds.length === 0}
              onClick={nextError}
            >
              <SkipForward size={14} aria-hidden="true" />
              Next error
              {errorIds.length
                ? ` (${cursor + 1 > 0 ? cursor + 1 : 0}/${errorIds.length})`
                : ''}
            </button>
            <span className="ep-faint ep-toolbar-note">
              Consecutive tool calls are grouped
            </span>
          </div>
          <div className="ep-transcript">
            <nav className="ep-outline" aria-label="Outline">
              <p className="ep-label">Outline</p>
              <ol>
                {chapters.map((chapter) => (
                  <li key={chapter.id}>
                    <a
                      href={`#ev-${chapter.id}`}
                      onClick={(click) => {
                        click.preventDefault()
                        setFilter('all')
                        window.setTimeout(() => reveal(chapter.id), 0)
                      }}
                    >
                      <span className="ep-faint ep-mono">{chapter.t}</span>
                      <span className="ep-outline-copy">
                        <span className="ep-outline-label">
                          {chapter.label}
                        </span>
                        <span className="ep-faint">
                          {chapter.calls
                            ? `${chapter.calls} tool call${chapter.calls === 1 ? '' : 's'}`
                            : 'no tool calls'}
                          {chapter.errors ? (
                            <span className="er-miss">
                              {' '}
                              · {chapter.errors} error
                              {chapter.errors === 1 ? '' : 's'}
                            </span>
                          ) : null}
                        </span>
                      </span>
                    </a>
                  </li>
                ))}
              </ol>
            </nav>
            <ol className="ep-events" ref={main} aria-label="Transcript events">
              {visible.length === 0 ? (
                <li className="ep-faint">Nothing matches this search.</li>
              ) : null}
              {visible.map((block) =>
                block.kind === 'message' ? (
                  <li key={block.event.id}>
                    <article
                      id={`ev-${block.event.id}`}
                      className="ep-message"
                      data-role={block.event.role}
                    >
                      <header>
                        <span className="ep-strong">{block.event.role}</span>
                        <span className="ep-faint ep-mono">
                          {offset(start, block.event.timestamp)}
                        </span>
                        {block.event.model ? (
                          <span className="ep-faint ep-mono">
                            {block.event.model}
                          </span>
                        ) : null}
                        <button
                          type="button"
                          className="ds-button ds-button-quiet ds-button-compact ep-message-copy"
                          onClick={() =>
                            void copy(block.event.id, block.event.text ?? '')
                          }
                        >
                          {copied === block.event.id ? 'Copied' : 'Copy'}
                        </button>
                      </header>
                      <MessageText text={block.event.text ?? ''} />
                    </article>
                  </li>
                ) : (
                  <li key={block.id}>
                    <details
                      className="ep-tools"
                      open={
                        block.events.some((event) => event.isError) || undefined
                      }
                    >
                      <summary>
                        <span className="ep-strong">{block.events.length}</span>
                        <span className="ep-mono ep-tools-fns">
                          {[
                            ...new Set(
                              block.events.map(
                                (event) => event.functionId ?? 'tool',
                              ),
                            ),
                          ]
                            .slice(0, 3)
                            .join(', ')}
                        </span>
                        {block.events.some((event) => event.isError) ? (
                          <span className="ep-tool-state" data-tone="alert">
                            {
                              block.events.filter((event) => event.isError)
                                .length
                            }{' '}
                            error
                            {block.events.filter((event) => event.isError)
                              .length === 1
                              ? ''
                              : 's'}
                          </span>
                        ) : null}
                        <span className="ep-faint ep-mono">
                          {offset(start, block.events[0]?.timestamp)}
                        </span>
                      </summary>
                      {errorsByCause(block.events).length ? (
                        <div className="ep-group-causes">
                          <p className="ep-label">Errors by cause</p>
                          <ul className="ep-causes">
                            {errorsByCause(block.events).map((cause) => (
                              <li key={`${cause.functionId}:${cause.cause}`}>
                                <button
                                  type="button"
                                  className="ep-cause"
                                  onClick={() => reveal(cause.ids[0])}
                                >
                                  <span className="ep-count">
                                    {cause.ids.length}
                                  </span>
                                  <span className="ep-mono">
                                    {cause.functionId}
                                  </span>
                                  <span className="ep-faint ep-clamp">
                                    {cause.cause}
                                  </span>
                                </button>
                              </li>
                            ))}
                          </ul>
                        </div>
                      ) : null}
                      {block.events.map((event) => (
                        <ToolEvent key={event.id} event={event} />
                      ))}
                    </details>
                  </li>
                ),
              )}
            </ol>
          </div>
        </>
      )}
    </div>
  )
}
