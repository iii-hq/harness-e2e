import { useEffect, useState } from 'react'
import { DataTable, DataTableRow, Panel } from '@/design-system'
import type { LiveProgress } from '@/lib/dashboard-data-source'
import { formatDuration, titleCase } from '@/lib/execution-view'

function count(value: number | null) {
  return value === null ? '—' : value.toLocaleString('en-US')
}

export function LiveProgressPanel({
  progress,
  running,
}: {
  progress: LiveProgress
  running: boolean
}) {
  const [now, setNow] = useState(Date.now)
  useEffect(() => {
    if (!running) return
    const timer = window.setInterval(() => setNow(Date.now()), 1_000)
    return () => window.clearInterval(timer)
  }, [running])
  const lastUpdate = Date.parse(progress.updated_at)
  const age = Number.isFinite(lastUpdate)
    ? Math.max(0, (now - lastUpdate) / 1_000)
    : null
  const pending = Math.max(
    0,
    progress.planned_slots - progress.runs_committed - progress.slots_deferred,
  )
  const determined = progress.completed_runs + progress.task_incomplete_runs
  const facts = [
    [
      'Runs recorded',
      `${progress.runs_committed}/${progress.planned_slots}`,
      `${pending} pending · ${progress.slots_deferred} deferred`,
    ],
    [
      'Attempts finished',
      `${progress.attempts_finished}/${progress.attempts_started}`,
      'Includes retries',
    ],
    [
      'Completion observed',
      progress.completion_rate === null
        ? '—'
        : `${Math.round(progress.completion_rate * 100)}%`,
      `${progress.completed_runs}/${determined} determined runs · ${progress.undetermined_runs} undetermined`,
    ],
    [
      'Incomplete tasks',
      count(progress.task_incomplete_runs),
      `${progress.technical_invalid_runs} technically invalid runs`,
    ],
    [
      'Tokens observed',
      count(progress.observed_tokens),
      `${progress.token_observed_attempts}/${progress.attempts_started} attempts with complete token telemetry`,
    ],
    [
      'Cost observed',
      progress.observed_cost_usd === null
        ? '—'
        : `$${progress.observed_cost_usd.toFixed(4)}`,
      `${progress.cost_observed_runs}/${progress.runs_committed} recorded runs with cost, including retries`,
    ],
    [
      'Quality on completed tasks',
      progress.quality_score_completed === null
        ? '—'
        : `${progress.quality_score_completed}/100`,
      `${progress.quality_scored_completed_runs}/${progress.completed_runs} completed runs scored`,
    ],
  ] as const
  return (
    <Panel
      as="section"
      className="mt-5"
      aria-label="Execution progress"
      data-live-progress
    >
      <div className="flex flex-wrap items-baseline justify-between gap-3">
        <h2 className="m-0 text-base font-semibold text-ink">
          {running ? 'Live progress' : 'Preserved progress'}
        </h2>
        <span className="text-xs text-ink-muted">
          {running ? 'Provisional results' : 'Partial evidence'}
        </span>
      </div>
      <p className="mt-2 mb-0 text-sm text-ink" aria-live="polite">
        {titleCase(progress.phase ?? 'requested')}
        {progress.active_attempt ? (
          <>
            {' '}
            · {titleCase(progress.active_attempt.scenario_id)} · attempt{' '}
            {progress.active_attempt.attempt_id.slice(0, 8)}
          </>
        ) : null}
      </p>
      <p className="mt-1 mb-0 text-xs text-ink-muted">
        {running && age !== null
          ? `Last recorded progress ${formatDuration(age)} ago. Long phases can continue between checkpoints.`
          : `Last recorded progress: ${progress.updated_at}`}
      </p>
      <dl className="m-0 mt-5 grid grid-cols-2 gap-5 lg:grid-cols-4">
        {facts.map(([label, value, description]) => (
          <div key={label}>
            <dt className="ds-label">{label}</dt>
            <dd className="m-0 mt-1 font-mono text-lg font-semibold text-ink">
              {value}
            </dd>
            <dd className="m-0 mt-1 text-xs text-ink-muted">{description}</dd>
          </div>
        ))}
      </dl>
      <p className="mt-4 mb-0 text-xs leading-5 text-ink-muted">
        Metrics cover preserved checkpoints. Usage from the active attempt may
        not be included yet. Pending runs have no outcome; scores remain
        provisional until the execution finishes.
      </p>
      {progress.terminal_reason ? (
        <p className="mt-3 mb-0 text-sm text-ink-soft">
          {progress.terminal_reason}
        </p>
      ) : null}
      {progress.slots.length > 0 ? (
        <details className="mt-4 pt-3">
          <summary className="cursor-pointer text-sm font-semibold text-ink">
            Results recorded so far · {progress.runs_committed}/
            {progress.planned_slots}
          </summary>
          <div className="mt-3 overflow-x-auto">
            <DataTable
              minWidth="640px"
              caption="Recorded results and pending runs"
            >
              <thead>
                <tr>
                  {[
                    'Scenario / repetition',
                    'Progress',
                    'Completion',
                    'Technical',
                    'Objective',
                    'Quality',
                  ].map((label) => (
                    <th
                      key={label}
                      className="px-3 py-2 font-semibold"
                      scope="col"
                    >
                      {label}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {progress.slots.map((slot) => (
                  <DataTableRow key={slot.slot_id}>
                    <td className="px-3 py-3">
                      {titleCase(slot.scenario_id)} · #{slot.repetition + 1}
                    </td>
                    <td className="px-3 py-3">
                      {slot.state === 'committed'
                        ? 'Recorded'
                        : titleCase(slot.state)}
                      {slot.reason ? (
                        <span className="mt-1 block text-ink-muted">
                          {slot.reason}
                        </span>
                      ) : null}
                    </td>
                    <td className="px-3 py-3">
                      {slot.completion ? titleCase(slot.completion) : '—'}
                    </td>
                    <td className="px-3 py-3">
                      {slot.technical ? titleCase(slot.technical) : '—'}
                    </td>
                    <td className="px-3 py-3">
                      {slot.objective_score === null
                        ? '—'
                        : `${slot.objective_score}/100`}
                    </td>
                    <td className="px-3 py-3">
                      {slot.completion !== 'completed' ||
                      slot.quality_score_completed === null
                        ? '—'
                        : `${slot.quality_score_completed}/100`}
                    </td>
                  </DataTableRow>
                ))}
              </tbody>
            </DataTable>
          </div>
        </details>
      ) : null}
    </Panel>
  )
}
