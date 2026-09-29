import {
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
import { ChevronRight, X } from 'lucide-react'
import type { CSSProperties } from 'react'
import { isInteractiveTarget } from '@/design-system'
import { hashForExecution, hashForTestHistory } from '@/hooks/use-hash-route'
import { formatDay, formatTime } from '@/lib/format'
import {
  counted,
  notRun,
  pointTime,
  releaseControlId,
  seriesModel,
  sourceText,
  stackText,
  type TestCell,
  type TrendChange,
  type TrendPoint,
  testCell,
  testIds,
  trendMetric,
} from '@/lib/trends'

function cellText(cell: TestCell) {
  switch (cell.kind) {
    case 'uncounted':
      return '—'
    case 'absent':
      return ''
    case 'not_run':
      return 'not run'
    case 'technical_invalid':
      return (
        <>
          <X size={16} aria-hidden="true" />
          <span className="ds-visually-hidden">technically invalid</span>
        </>
      )
    case 'no_score':
      return 'no score'
    case 'scored':
      return String(cell.score)
  }
}

function cellTitle(cell: TestCell, id: string, when: string) {
  switch (cell.kind) {
    case 'uncounted':
      return `${when} · no counted run`
    case 'absent':
      return `${id} was not in the suite on ${when}`
    case 'not_run':
      return `${id} was planned on ${when} and did not run`
    case 'technical_invalid':
      return `${id} · technically invalid on ${when}`
    case 'no_score':
      return `${id} · no score on ${when}`
    case 'scored':
      return `${id} · ${cell.score} on ${when}`
  }
}

/** The score of each test in each execution, oldest first; a column picks
 *  its execution, a test opens its history. */
export function ByTest({
  points,
  changes,
  selected,
  narrow,
  onPick,
}: {
  points: TrendPoint[]
  changes: TrendChange[][]
  selected: number
  narrow: boolean
  onPick: (index: number) => void
}) {
  const ids = testIds(points)
  const widths = {
    '--tr-test-width': `calc(var(--spacing) * ${narrow ? 42 : 60})`,
    '--tr-column-width': `calc(var(--spacing) * ${narrow ? 14 : 15.5})`,
  } as CSSProperties
  return (
    <section className="tr-section" aria-labelledby="tr-tests">
      <div className="tr-section-head">
        <h2 id="tr-tests" className="tr-h2">
          By test
        </h2>
        <span className="tr-faint">
          The score of each run. Blank: not in the suite then. Not run: planned,
          no run. A cross: technically invalid. The diamond marks a changed
          definition.
        </span>
      </div>
      <div className="tr-grid-scroll">
        <table
          aria-label="Score by test and execution"
          className="tr-grid"
          style={widths}
          data-by-test
        >
          <thead>
            <tr>
              <th scope="col" className="tr-grid-corner">
                Test
              </th>
              {points.map((point, index) => (
                <th
                  scope="col"
                  key={point.execution_id}
                  className="tr-grid-head"
                >
                  <button
                    type="button"
                    className="tr-column"
                    data-selected={index === selected || undefined}
                    aria-pressed={index === selected}
                    aria-label={`Select ${pointTime(point)}`}
                    onClick={() => onPick(index)}
                  >
                    <span>{formatDay(point.started_at)}</span>
                    <span>{formatTime(point.started_at)}</span>
                  </button>
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {ids.map((id) => (
              <tr key={id}>
                <th scope="row" className="tr-grid-test">
                  <a href={hashForTestHistory(id)} title={id}>
                    {id}
                  </a>
                </th>
                {points.map((point, index) => {
                  const cell = testCell(point, id, changes[index])
                  return (
                    <td
                      key={point.execution_id}
                      className="tr-cell"
                      data-kind={cell.kind}
                      data-tone={cell.kind === 'scored' ? cell.tone : undefined}
                      data-selected={index === selected || undefined}
                      title={cellTitle(cell, id, pointTime(point))}
                    >
                      {cellText(cell)}
                      {'redefined' in cell && cell.redefined ? (
                        <>
                          <span className="tr-redefined" aria-hidden="true" />
                          <span className="ds-visually-hidden">
                            , definition changed
                          </span>
                        </>
                      ) : null}
                    </td>
                  )
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  )
}

const score = trendMetric('score')
const duration = trendMetric('duration')
const tokens = trendMetric('input_tokens')
const calls = trendMetric('function_calls')
const errors = trendMetric('error_rate')
const turns = trendMetric('turns')

function figure(metric: typeof score, point: TrendPoint) {
  const value = metric.value(point)
  return value === null ? '—' : metric.figure(value, point)
}

/** The executions in the view, newest first; a version new since the row
 *  below is tinted, and a row opens its execution. */
export function ExecutionsTable({
  points,
  selected,
  narrow,
}: {
  points: TrendPoint[]
  selected: number
  narrow: boolean
}) {
  const rows = points
    .map((point, index) => ({ point, index, before: points[index - 1] }))
    .reverse()
  // Under `any` the view mixes models or profiles: say each row's.
  const mixed =
    new Set(points.map((item) => `${seriesModel(item)}|${item.profile ?? ''}`))
      .size > 1
  const open = (point: TrendPoint) => () => {
    window.location.hash = hashForExecution(point.execution_id)
  }
  return (
    <section className="tr-section" aria-labelledby="tr-ex">
      <div className="tr-section-head">
        <h2 id="tr-ex" className="tr-h2">
          Executions in this view
        </h2>
        <span className="tr-mono tr-faint">{points.length}</span>
        <span className="tr-faint">
          newest first · a tinted version is new since the row below
        </span>
      </div>
      <TableViewport>
        <TableFrame>
          <Table
            density="compact"
            inset
            className="tr-table"
            data-narrow={narrow || undefined}
            data-trend-executions
          >
            <TableCaption className="ds-visually-hidden">
              Executions in this view
            </TableCaption>
            <TableHeader>
              <TableRow>
                <TableHead scope="col">When</TableHead>
                {narrow ? null : (
                  <>
                    <TableHead scope="col" className="tr-w-26">
                      iii
                    </TableHead>
                    <TableHead scope="col" className="tr-w-38">
                      Stack
                    </TableHead>
                  </>
                )}
                <TableHead scope="col" className="tr-num tr-w-14">
                  Score
                </TableHead>
                <TableHead scope="col" className="tr-num tr-w-16">
                  Tests
                </TableHead>
                {narrow ? null : (
                  <>
                    <TableHead scope="col" className="tr-num tr-w-18">
                      Duration
                    </TableHead>
                    <TableHead scope="col" className="tr-num tr-w-18">
                      Input
                    </TableHead>
                    <TableHead scope="col" className="tr-num tr-w-14">
                      Calls
                    </TableHead>
                    <TableHead scope="col" className="tr-num tr-w-16">
                      Errors
                    </TableHead>
                    <TableHead scope="col" className="tr-num tr-w-14">
                      Turns
                    </TableHead>
                  </>
                )}
                <TableHead scope="col" className="tr-col-open">
                  <span className="ds-visually-hidden">Open</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map(({ point, index, before }) => {
                const rc = releaseControlId(point)
                const missing = notRun(point)
                const newStack =
                  before?.stack.name &&
                  point.stack.name &&
                  before.stack.name !== point.stack.name
                return (
                  <TableRow
                    key={point.execution_id}
                    interactive
                    tabIndex={-1}
                    selected={index === selected}
                    className="tr-row"
                    data-execution-id={point.execution_id}
                    onClick={(event) => {
                      if (!isInteractiveTarget(event.target)) open(point)()
                    }}
                  >
                    <TableCell className="tr-cell-stack">
                      <a
                        className="tr-strong"
                        href={hashForExecution(point.execution_id)}
                        aria-label={`Open the execution of ${pointTime(point)}`}
                      >
                        {pointTime(point)}
                      </a>
                      <span
                        className="tr-mono tr-small tr-sub"
                        data-tone={counted(point) ? undefined : 'alert'}
                      >
                        {[
                          counted(point) ? null : 'no counted run',
                          sourceText(point),
                          rc ? `RC ${rc}` : null,
                          mixed ? seriesModel(point) : null,
                          mixed && point.profile
                            ? `profile ${point.profile}`
                            : null,
                          point.label,
                        ]
                          .filter(Boolean)
                          .join(' · ')}
                      </span>
                    </TableCell>
                    {narrow ? null : (
                      <>
                        <TableCell>
                          <span
                            className="tr-version"
                            data-new={
                              (before?.engine &&
                                point.engine &&
                                before.engine !== point.engine) ||
                              undefined
                            }
                          >
                            {point.engine ?? '—'}
                          </span>
                        </TableCell>
                        <TableCell>
                          <span
                            className="tr-version"
                            data-new={newStack || undefined}
                          >
                            {stackText(point)}
                          </span>
                        </TableCell>
                      </>
                    )}
                    <TableCell className="tr-num">
                      {figure(score, point)}
                    </TableCell>
                    <TableCell
                      className="tr-num"
                      title={
                        missing.length
                          ? `not run: ${missing.join(', ')}`
                          : undefined
                      }
                    >
                      {point.measures
                        ? `${point.measures.completed} of ${point.measures.planned}`
                        : '—'}
                    </TableCell>
                    {narrow ? null : (
                      <>
                        <TableCell className="tr-num">
                          {figure(duration, point)}
                        </TableCell>
                        <TableCell className="tr-num">
                          {figure(tokens, point)}
                        </TableCell>
                        <TableCell className="tr-num">
                          {figure(calls, point)}
                        </TableCell>
                        <TableCell className="tr-num">
                          {figure(errors, point)}
                        </TableCell>
                        <TableCell className="tr-num">
                          {figure(turns, point)}
                        </TableCell>
                      </>
                    )}
                    <TableCell className="tr-col-open">
                      <ChevronRight
                        size={16}
                        aria-hidden="true"
                        className="tr-faint-ink"
                      />
                    </TableCell>
                  </TableRow>
                )
              })}
            </TableBody>
          </Table>
        </TableFrame>
      </TableViewport>
    </section>
  )
}
