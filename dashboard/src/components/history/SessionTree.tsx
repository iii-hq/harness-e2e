import { formatDuration, plural } from '@/lib/format'
import {
  type RunDetails,
  sessionName,
  sessionRole,
  subAgentsNote,
} from '@/lib/test-history'
import '@/pages/test-history.css'

/** Two thin bars per row, in the style of "Calls by worker": the first
 *  faint, the second in the accent, each against the longest in its list. */
export function PairedBars({
  first,
  second,
}: {
  first: number
  second: number
}) {
  return (
    <span className="th-bars" aria-hidden="true">
      <span className="th-bar-first" style={{ width: `${first}%` }} />
      <span className="th-bar-second" style={{ width: `${second}%` }} />
    </span>
  )
}

export function BarsLegend({
  first,
  second,
}: {
  first: string
  second: string
}) {
  return (
    <div className="th-bars-legend" aria-hidden="true">
      <span>
        <span className="th-bar-first" />
        {first}
      </span>
      <span>
        <span className="th-bar-second" />
        {second}
      </span>
    </div>
  )
}

function share(value: number, max: number) {
  return max > 0 ? Math.round((value / max) * 100) : 0
}

/** The session tree of a run, indented by depth, with each session's turns
 *  and calls as paired bars and its errors and duration. */
export function SessionTree({
  details,
  turns,
  heading = 'Sub-agents',
  headingId,
}: {
  details: RunDetails
  turns: number | null
  heading?: string
  headingId?: string
}) {
  const sessions = details.sessions
  const root = sessions.find((session) => session.depth === 0) ?? null
  const maxTurns = Math.max(0, ...sessions.map((session) => session.turns))
  const maxCalls = Math.max(
    0,
    ...sessions.map((session) => session.function_calls),
  )
  return (
    <section
      className="th-sessions"
      aria-labelledby={headingId}
      data-sessions={details.child_sessions}
    >
      <div className="th-section-head">
        <h3 className="th-h3" id={headingId}>
          {heading}
        </h3>
        <span className="th-faint">{subAgentsNote(details, turns)}</span>
      </div>
      {sessions.length > 0 ? (
        <div className="th-inset">
          <ul className="th-session-list">
            {sessions.map((session) => (
              <li
                key={session.session_id}
                data-session={session.session_id}
                data-depth={session.depth}
              >
                <span
                  className="th-session-name"
                  style={{
                    paddingInlineStart: `calc(var(--spacing) * ${4 * session.depth})`,
                  }}
                >
                  <span
                    className="th-mono th-ellipsis"
                    title={session.session_id}
                  >
                    {session.depth > 0 ? (
                      <span className="th-ghost" aria-hidden="true">
                        ↳{' '}
                      </span>
                    ) : null}
                    {sessionName(session)}
                  </span>
                  <span className="th-faint th-ellipsis">
                    {sessionRole(session, root?.session_id ?? null)}
                  </span>
                </span>
                <PairedBars
                  first={share(session.turns, maxTurns)}
                  second={share(session.function_calls, maxCalls)}
                />
                <span className="th-session-figures">
                  <span>
                    <span className="th-faint-num">{session.turns} · </span>
                    {session.function_calls}
                  </span>
                  <span
                    data-errors={session.function_call_errors > 0 || undefined}
                  >
                    {plural(session.function_call_errors, 'error')} ·{' '}
                    {formatDuration(session.duration_ms)}
                  </span>
                </span>
              </li>
            ))}
          </ul>
          <BarsLegend first="turns" second="function calls" />
        </div>
      ) : null}
    </section>
  )
}
