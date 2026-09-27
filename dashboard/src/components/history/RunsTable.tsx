import {
  Checkbox,
  Table,
  TableBody,
  TableCaption,
  TableCell,
  TableFrame,
  TableHead,
  TableHeader,
  TableRow,
  TableViewport,
} from '@iii-dev/console-ui'
import { ChevronDown, ChevronRight, ScrollText } from 'lucide-react'
import { Fragment, type MouseEvent } from 'react'
import { SessionTree } from '@/components/history/SessionTree'
import { isInteractiveTarget, StatusLabel } from '@/design-system'
import { hashForExecution } from '@/hooks/use-hash-route'
import { definitionTitle, shortDefinition } from '@/lib/definition-digest'
import {
  formatDateTime,
  formatDuration,
  formatTokens,
  NOT_REPORTED,
  plural,
} from '@/lib/format'
import {
  childSessions,
  type HistoryObservation,
  type HistoryRun,
  listedExecution,
  lostCriteria,
  lostText,
  modelText,
  observationState,
  profileText,
  runKey,
  scoreStatus,
  scoreText,
} from '@/lib/test-history'
import '@/pages/test-history.css'

export function executionText(observation: HistoryObservation) {
  return (
    observation.execution_label ||
    `execution ${listedExecution(observation)
      .replace(/^plan-/, '')
      .slice(0, 8)}`
  )
}

function count(value: number | null | undefined) {
  return typeof value === 'number' ? String(Math.round(value)) : NOT_REPORTED
}

function tiles(observation: HistoryObservation, run: HistoryRun | undefined) {
  const details = run?.details
  return [
    ['Turns', count(run?.turns ?? observation.median_turns)],
    [
      'Function calls',
      count(run?.function_calls ?? observation.median_function_calls),
    ],
    [
      'Errors',
      count(
        run?.function_call_errors ?? observation.median_function_call_errors,
      ),
    ],
    ['Input tokens', formatTokens(details?.input_tokens)],
    ['Output tokens', formatTokens(details?.output_tokens)],
    ['Cache read', formatTokens(details?.cache_read_tokens)],
  ]
}

/** The links to a run's execution, transcript and evidence record. */
export function runLinks(observation: HistoryObservation, run?: HistoryRun) {
  const execution = listedExecution(observation)
  return {
    execution: hashForExecution(execution),
    transcript: run
      ? hashForExecution(execution, null, run.run_id, 'transcript')
      : null,
    evidence: run ? hashForExecution(execution, null, run.run_id) : null,
  }
}

function RunDetail({
  observation,
  run,
  label,
}: {
  observation: HistoryObservation
  run: HistoryRun | undefined
  label: string | null
}) {
  const lost = run ? lostCriteria(run) : []
  const links = runLinks(observation, run)
  const scored = typeof (run?.score ?? observation.mean_score) === 'number'
  const headingId = `th-subs-${runKey(observation)}-${run?.run_id ?? 'run'}`
  return (
    <div className="th-run-detail" data-run={run?.run_id}>
      {label ? <p className="th-h3">{label}</p> : null}
      <dl className="th-tiles">
        {tiles(observation, run).map(([name, value]) => (
          <div key={name}>
            <dt>{name}</dt>
            <dd>{value}</dd>
          </div>
        ))}
      </dl>
      <div className="th-detail-grid">
        {run?.details ? (
          <SessionTree
            details={run.details}
            turns={run.turns ?? observation.median_turns ?? null}
            headingId={headingId}
          />
        ) : (
          <section className="th-sessions">
            <h3 className="th-h3">Sub-agents</h3>
            <p className="th-faint th-empty-line">
              The run’s native results could not be read, so its sessions are
              not known.
            </p>
          </section>
        )}
        <section className="th-lost" data-criteria-lost={lost.length}>
          <h3 className="th-h3">Criteria lost</h3>
          {run?.details && lost.length === 0 && scored ? (
            <p className="th-faint th-empty-line">Every criterion met.</p>
          ) : null}
          {lost.length > 0 ? (
            <ul className="th-lost-list">
              {lost.map((criterion) => (
                <li key={criterion.id} title={criterion.reason}>
                  <span className="th-mono th-ellipsis">{criterion.id}</span>
                  <span className="th-points">−{criterion.points}</span>
                </li>
              ))}
            </ul>
          ) : null}
          <div className="th-links">
            <a className="th-act th-act-filled" href={links.execution}>
              Open execution
            </a>
            {links.transcript ? (
              <a className="th-act" href={links.transcript}>
                Transcript
              </a>
            ) : null}
            {links.evidence ? (
              <a className="th-act" href={links.evidence}>
                Evidence record
              </a>
            ) : null}
          </div>
        </section>
      </div>
    </div>
  )
}

type Group = {
  definition: string
  current: boolean
  rows: HistoryObservation[]
}

/** Consecutive runs of one definition, newest first. */
export function groupByDefinition(
  observations: HistoryObservation[],
  current: string | null,
): Group[] {
  const groups: Group[] = []
  for (const observation of observations) {
    const last = groups.at(-1)
    if (last && last.definition === observation.behavior_sha256)
      last.rows.push(observation)
    else
      groups.push({
        definition: observation.behavior_sha256,
        current: observation.behavior_sha256 === current,
        rows: [observation],
      })
  }
  return groups
}

export function RunsTable({
  observations,
  current,
  grouped,
  selected,
  onToggleSelected,
  open,
  onToggleOpen,
  narrow,
}: {
  observations: HistoryObservation[]
  current: string | null
  /** Heads each definition's runs: when more than one is shown. */
  grouped: boolean
  selected: string[]
  onToggleSelected: (key: string) => void
  open: Set<string>
  onToggleOpen: (key: string) => void
  narrow: boolean
}) {
  const columns = narrow ? 4 : 9
  return (
    <TableViewport className="th-table-viewport">
      <TableFrame>
        <Table
          density="compact"
          inset
          className="th-table"
          data-narrow={narrow || undefined}
          data-history-table
        >
          <TableCaption className="ds-visually-hidden">
            {`Runs of this test, newest first, ${observations.length} shown`}
          </TableCaption>
          <TableHeader>
            <TableRow>
              <TableHead className="th-col-pick" scope="col">
                <span className="ds-visually-hidden">Pick for A/B</span>
              </TableHead>
              <TableHead scope="col">Run</TableHead>
              {narrow ? null : (
                <TableHead scope="col" className="th-col-model">
                  Model · profile
                </TableHead>
              )}
              <TableHead scope="col" className="th-col-score">
                Score
              </TableHead>
              {narrow ? null : (
                <>
                  <TableHead scope="col" className="th-col-lost">
                    Criteria lost
                  </TableHead>
                  <TableHead scope="col" className="th-num th-col-duration">
                    Duration
                  </TableHead>
                  <TableHead scope="col" className="th-num th-col-tokens">
                    Tokens
                  </TableHead>
                  <TableHead scope="col" className="th-num th-col-turns">
                    Turns
                  </TableHead>
                </>
              )}
              <TableHead scope="col" className="th-col-actions">
                <span className="ds-visually-hidden">Actions</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {groupByDefinition(observations, current).map((group) => (
              <Fragment key={runKey(group.rows[0])}>
                {grouped ? (
                  <TableRow className="th-group">
                    <TableHead colSpan={columns} scope="colgroup">
                      <span
                        className="th-mono"
                        title={definitionTitle(group.definition)}
                      >
                        Definition {shortDefinition(group.definition)}
                      </span>
                      {group.current ? (
                        <span className="th-tag">current</span>
                      ) : null}
                      <span className="th-count">
                        {plural(group.rows.length, 'run')}
                      </span>
                    </TableHead>
                  </TableRow>
                ) : null}
                {group.rows.map((observation) => (
                  <RunRow
                    key={runKey(observation)}
                    observation={observation}
                    current={current}
                    columns={columns}
                    narrow={narrow}
                    side={
                      selected[0] === runKey(observation)
                        ? 'A'
                        : selected[1] === runKey(observation)
                          ? 'B'
                          : null
                    }
                    locked={
                      selected.length >= 2 &&
                      !selected.includes(runKey(observation))
                    }
                    onToggleSelected={onToggleSelected}
                    expanded={open.has(runKey(observation))}
                    onToggleOpen={onToggleOpen}
                  />
                ))}
              </Fragment>
            ))}
          </TableBody>
        </Table>
      </TableFrame>
    </TableViewport>
  )
}

function RunRow({
  observation,
  current,
  columns,
  narrow,
  side,
  locked,
  onToggleSelected,
  expanded,
  onToggleOpen,
}: {
  observation: HistoryObservation
  current: string | null
  columns: number
  narrow: boolean
  side: 'A' | 'B' | null
  locked: boolean
  onToggleSelected: (key: string) => void
  expanded: boolean
  onToggleOpen: (key: string) => void
}) {
  const key = runKey(observation)
  const when = formatDateTime(observation.completed_at)
  const runs = observation.runs ?? []
  const links = runLinks(observation, runs.at(-1))
  const lost = lostText(observation)
  const read = runs.some((run) => run.details)
  const subs = childSessions(observation)
  const isCurrent = observation.behavior_sha256 === current
  const detailId = `th-detail-${key}`
  const toggle = (event: MouseEvent<HTMLTableRowElement>) => {
    if (!isInteractiveTarget(event.target)) onToggleOpen(key)
  }
  return (
    <>
      <TableRow
        className="th-row"
        selected={side !== null}
        data-run-key={key}
        data-result={observationState(observation)}
        onClick={toggle}
      >
        <TableCell className="th-col-pick">
          <Checkbox
            aria-label={`Pick ${when} for A/B`}
            checked={side !== null}
            disabled={locked}
            onChange={() => onToggleSelected(key)}
          />
        </TableCell>
        <TableCell className="th-cell-stack">
          <span className="th-when">
            {side ? (
              <span className="th-side" title={`Compared as ${side}`}>
                {side}
              </span>
            ) : null}
            <span className="th-strong">{when}</span>
            <span
              className="th-tag th-mono"
              data-tone={isCurrent ? undefined : 'plain'}
              title={definitionTitle(observation.behavior_sha256)}
            >
              {shortDefinition(observation.behavior_sha256) ?? 'no definition'}
              {isCurrent ? ' · current' : ''}
            </span>
          </span>
          <a
            className="th-sub th-ellipsis"
            href={links.execution}
            title={executionText(observation)}
          >
            {executionText(observation)}
          </a>
        </TableCell>
        {narrow ? null : (
          <TableCell className="th-cell-stack">
            <span className="th-mono th-ellipsis th-model">
              {modelText(observation)}
            </span>
            <span className="th-profile">
              <span className="th-mono th-faint th-ellipsis">
                {profileText(observation)}
              </span>
              {subs > 0 ? (
                <span className="th-tag" title="Sub-agents this run spawned">
                  {plural(subs, 'sub-agent')}
                </span>
              ) : null}
            </span>
          </TableCell>
        )}
        <TableCell>
          <span className="th-score">
            <StatusLabel
              className="th-score-value"
              state={observationState(observation)}
              label={scoreText(observation)}
            />
            <span className="th-faint">{scoreStatus(observation)}</span>
          </span>
        </TableCell>
        {narrow ? null : (
          <>
            <TableCell
              className="th-cell-lost"
              title={lost || undefined}
              data-lost={lost ? true : undefined}
            >
              <span className="th-mono th-ellipsis">
                {lost ||
                  (typeof observation.mean_score !== 'number'
                    ? 'no score'
                    : read
                      ? NOT_REPORTED
                      : 'not read')}
              </span>
            </TableCell>
            <TableCell className="th-num">
              {formatDuration(
                observation.median_duration_seconds == null
                  ? null
                  : observation.median_duration_seconds * 1000,
              )}
            </TableCell>
            <TableCell className="th-num">
              {formatTokens(observation.median_tokens || null)}
            </TableCell>
            <TableCell className="th-num">
              {count(observation.median_turns)}
            </TableCell>
          </>
        )}
        <TableCell className="th-col-actions">
          <span className="th-row-actions">
            <button
              type="button"
              className="th-icon"
              aria-expanded={expanded}
              aria-controls={detailId}
              aria-label={`Details of the ${when} run`}
              title="Details"
              onClick={() => onToggleOpen(key)}
            >
              <ChevronDown
                size={16}
                aria-hidden="true"
                className={expanded ? 'th-chevron th-rot' : 'th-chevron'}
              />
            </button>
            {links.transcript ? (
              <a
                className="th-icon"
                href={links.transcript}
                aria-label={`Transcript of the ${when} run`}
                title="Transcript"
              >
                <ScrollText size={16} aria-hidden="true" />
              </a>
            ) : null}
            {links.evidence ? (
              <a
                className="th-icon"
                href={links.evidence}
                aria-label={`Evidence record of the ${when} run`}
                title="Evidence record"
              >
                <ChevronRight size={16} aria-hidden="true" />
              </a>
            ) : null}
          </span>
        </TableCell>
      </TableRow>
      <TableRow className="th-detail-row" id={detailId} hidden={!expanded}>
        <TableCell colSpan={columns}>
          {expanded
            ? (runs.length > 0 ? runs : [undefined]).map((run, index) => (
                <RunDetail
                  key={run?.run_id ?? index}
                  observation={observation}
                  run={run}
                  label={
                    runs.length > 1
                      ? `Run ${index + 1} of ${runs.length}`
                      : null
                  }
                />
              ))
            : null}
        </TableCell>
      </TableRow>
    </>
  )
}
