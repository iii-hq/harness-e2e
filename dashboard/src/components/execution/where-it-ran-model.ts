import type { GithubJob } from '@/lib/dashboard-data-source'
import { type PlanExecution, running } from '@/lib/plan-execution'

/** Where an execution runs, and what is happening there, as the page's
 *  "Where it ran" card and the line under the title read it. */

export type Place = 'harness' | 'docker' | 'github'

export function placeOf(execution: PlanExecution): Place {
  if (execution.source.kind === 'docker') return 'docker'
  if (execution.source.kind === 'github') return 'github'
  return 'harness'
}

export function plural(count: number, one: string, many: string) {
  return `${count} ${count === 1 ? one : many}`
}

function duration(seconds: number) {
  if (!Number.isFinite(seconds) || seconds < 0) return '—'
  if (seconds < 60) return `${Math.round(seconds)}s`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60)
    return `${minutes}m ${String(Math.round(seconds - minutes * 60)).padStart(2, '0')}s`
  return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, '0')}m`
}

export function elapsed(from: string | null | undefined, to?: string | null) {
  const start = from ? Date.parse(from) : Number.NaN
  const end = to ? Date.parse(to) : Date.now()
  return Number.isFinite(start) ? duration((end - start) / 1000) : '—'
}

export type TestState =
  | 'reported'
  | 'running'
  | 'waiting'
  | 'stopped'
  | 'at-import'

export type TestRow = { id: string; state: TestState; detail: string }

/** A live test list: what reported, what runs, what waits. A Docker group's
 *  tests wait for the import once the group finished; after a cancel what
 *  did not finish "stopped before it finished". */
export function testRows(
  execution: PlanExecution,
  dockerGroups = 2,
): TestRow[] {
  const cancelled =
    execution.state === 'cancelled' || execution.state === 'cancelling'
  const source = execution.source
  if (source.kind === 'docker') {
    const rows: TestRow[] = []
    for (const group of source.groups) {
      for (const id of group.scenarios) {
        const state: TestState =
          group.state === 'done' || group.state === 'failed'
            ? 'at-import'
            : group.state === 'running'
              ? 'running'
              : group.state === 'queued' && !cancelled
                ? 'waiting'
                : 'stopped'
        rows.push({
          id,
          state,
          detail:
            state === 'at-import'
              ? `${group.group_id} finished · results at import`
              : state === 'running'
                ? `Running in its container${group.attempt > 1 ? ` · attempt ${group.attempt}` : ''}`
                : state === 'waiting'
                  ? `Waiting for a slot · ${plural(dockerGroups, 'group', 'groups')} at a time`
                  : 'Stopped before it finished',
        })
      }
    }
    return rows
  }
  // One row per test; its rounds decide it: any running → Running, all
  // finished → Reported, else waiting (or stopped after a cancel).
  const order = [...new Set(execution.slots.map((slot) => slot.scenario_id))]
  return order.map((id) => {
    const slots = execution.slots.filter((slot) => slot.scenario_id === id)
    const finished = slots.filter((slot) => slot.state === 'finished')
    const active = slots.some(
      (slot) => slot.state === 'running' || slot.state === 'admitting',
    )
    const state: TestState = active
      ? 'running'
      : finished.length === slots.length
        ? 'reported'
        : cancelled || !running(execution.state)
          ? 'stopped'
          : 'waiting'
    const rounds =
      slots.length > 1 ? ` · ${finished.length} of ${slots.length} rounds` : ''
    const passed = finished.reduce((sum, slot) => sum + slot.passed, 0)
    const completed = finished.reduce(
      (sum, slot) => sum + (slot.completed || slot.observed),
      0,
    )
    return {
      id,
      state,
      detail:
        state === 'reported'
          ? `${passed}/${completed} passed${rounds}`
          : state === 'running'
            ? `Running for ${elapsed(execution.started_at)}${rounds}`
            : state === 'waiting'
              ? `Waiting${rounds}`
              : 'Stopped before it finished',
    }
  })
}

/** "Running · on this harness · for 3m 40s" and the provisional count. */
export function whereLine(execution: PlanExecution) {
  const source = execution.source
  const live = running(execution.state)
  const place =
    source.kind === 'github'
      ? `on GitHub · dispatched ${formatWhen(execution.started_at)}`
      : source.kind === 'docker'
        ? 'in Docker'
        : 'on this harness'
  const state =
    execution.state === 'cancelling'
      ? 'Cancelling'
      : live
        ? 'Running'
        : execution.state === 'importing'
          ? 'Importing'
          : null
  const since =
    live && source.kind === 'local'
      ? ` · for ${elapsed(execution.started_at)}`
      : ''
  return state ? `${state} · ${place}${since}` : place
}

export function reportedLine(execution: PlanExecution) {
  if (!running(execution.state)) return null
  const source = execution.source
  if (source.kind === 'github') {
    // GitHub reports per group job; the tests arrive with the import.
    const jobs = (source.follow?.jobs ?? []).filter((job) =>
      /case-/.test(job.name),
    )
    if (jobs.length === 0) return 'Results arrive with the import'
    const done = jobs.filter((job) => job.status === 'completed').length
    return `${done} of ${plural(jobs.length, 'group job', 'group jobs')} finished · results at import`
  }
  const rows = testRows(execution)
  const reported = rows.filter(
    (row) => row.state === 'reported' || row.state === 'at-import',
  ).length
  return `${reported} of ${plural(rows.length, 'test', 'tests')} reported · results are provisional`
}

function formatWhen(value: string | null | undefined) {
  const time = value ? Date.parse(value) : Number.NaN
  if (!Number.isFinite(time)) return '—'
  return new Date(time).toLocaleString('en-US', {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  })
}

export const DOCKER_STEPS = [
  ['prepare', 'Prepare', 'Suite materialized, stack assembled and locked'],
  ['groups', 'Groups', ''],
  ['finalize', 'Aggregate', 'Aggregates the groups into the root bundle'],
  [
    'import',
    'Import',
    'Reads the execution’s folder into this Console, as an import from GitHub',
  ],
] as const

export type StepState = 'done' | 'current' | 'next' | 'stopped'

export function dockerSteps(execution: PlanExecution, dockerGroups = 2) {
  const source = execution.source
  if (source.kind !== 'docker') return []
  const order = ['prepare', 'groups', 'finalize', 'import', 'done']
  const at = order.indexOf(source.phase)
  const finished = source.groups.filter(
    (group) => group.state === 'done' || group.state === 'failed',
  ).length
  const cancelled =
    execution.state === 'cancelled' || execution.state === 'cancelling'
  const stoppedGroups = source.groups.some(
    (group) => group.state === 'cancelled' || group.state === 'interrupted',
  )
  return DOCKER_STEPS.map(([phase, label, detail], index) => {
    let state: StepState =
      index < at || source.phase === 'done'
        ? 'done'
        : index === at
          ? cancelled
            ? 'stopped'
            : 'current'
          : 'next'
    // A cancel stops the groups; with none finished nothing is aggregated.
    if (cancelled && stoppedGroups) {
      if (phase === 'groups') state = 'stopped'
      else if (phase !== 'prepare' && finished === 0) state = 'next'
    }
    return {
      phase,
      label,
      state,
      detail:
        phase === 'groups'
          ? `${finished} of ${source.groups.length} finished · ${plural(dockerGroups, 'group', 'groups')} at a time`
          : detail,
    }
  })
}

/** The tests a GitHub group job runs: the slots of its group once the run
 *  was imported, else the planned test its `case-<first test>` name carries
 *  (dashes for underscores). */
export function jobTests(job: GithubJob, execution: PlanExecution) {
  const bySlot = [
    ...new Set(
      execution.slots
        .filter((slot) => slot.group_id && job.name.includes(slot.group_id))
        .map((slot) => slot.scenario_id),
    ),
  ]
  if (bySlot.length > 0) return bySlot
  const match = job.name.match(/case-([a-z0-9-]+)/)
  if (!match) return []
  return (execution.parameters?.scenarios ?? []).filter(
    (id) => id.replace(/_/g, '-') === match[1],
  )
}

export function jobLabel(job: GithubJob) {
  if (job.status !== 'completed')
    return job.status === 'in_progress' ? 'Running' : 'Queued'
  if (job.conclusion === 'success') return 'Done'
  if (job.conclusion === 'cancelled') return 'Cancelled'
  if (job.conclusion === 'skipped') return 'Skipped'
  return 'Failed'
}

export function jobDuration(job: GithubJob) {
  return job.started_at ? elapsed(job.started_at, job.completed_at) : '—'
}

/** Counts a cancel confirmation quotes: what stops, what is kept. */
export function cancelCounts(execution: PlanExecution) {
  const source = execution.source
  if (source.kind === 'docker') {
    const finished = source.groups.filter(
      (group) => group.state === 'done' || group.state === 'failed',
    ).length
    return { finished, running: source.groups.length - finished }
  }
  if (source.kind === 'github') {
    const jobs = (source.follow?.jobs ?? []).filter((job) =>
      /case-/.test(job.name),
    )
    const finished = jobs.filter((job) => job.status === 'completed').length
    return { finished, running: jobs.length - finished }
  }
  const finished = execution.slots.filter(
    (slot) => slot.state === 'finished',
  ).length
  return { finished, running: execution.slots.length - finished }
}

export function cancelCopy(execution: PlanExecution) {
  const { finished, running: live } = cancelCounts(execution)
  const place = placeOf(execution)
  if (place === 'github')
    return {
      title: 'Cancel the run on GitHub?',
      body: `Calls gh run cancel. Its ${plural(live, 'running job stops', 'running jobs stop')}; ${finished === 1 ? 'the job that finished is' : `the ${finished} jobs that finished are`} imported when the run ends.`,
      action: 'Cancel the run',
    }
  if (place === 'docker')
    return {
      title: 'Cancel this execution?',
      body:
        finished === 0
          ? 'Its running and waiting groups stop. None has finished yet, so nothing is imported.'
          : `Its running and waiting groups stop. ${finished === 1 ? 'The group that finished keeps' : `The ${finished} groups that finished keep`} counting, and the execution is aggregated and imported with ${finished === 1 ? 'it' : 'them'}.`,
      action: 'Cancel execution',
    }
  return {
    title: 'Cancel this execution?',
    body: 'The test running now stops. What already reported stays.',
    action: 'Cancel execution',
  }
}
