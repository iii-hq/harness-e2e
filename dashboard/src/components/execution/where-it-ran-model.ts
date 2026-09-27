import type { GithubJob } from '@/lib/dashboard-data-source'
import { type PlanExecution, running } from '@/lib/plan-execution'
import { roundKey } from '@/lib/scenario-matrix'

/** Where an execution runs, and what is happening there, as the page's
 *  "Where it ran" card and the line under the title read it. */

export type Place = 'harness' | 'docker' | 'github'

export function placeOf(execution: PlanExecution): Place {
  if (execution.source.kind === 'docker') return 'docker'
  if (execution.source.kind === 'github') return 'github'
  return 'harness'
}

/** An executor image by its tag, with a long hex cut to 12:
 *  `ghcr.io/iii-hq/harness-e2e:tools-d9a8…` → `tools-d9a8b54a2c85`. */
export function shortImage(image: string) {
  const tag = image.split(/[:@]/).pop() || image
  return tag.replace(/([0-9a-f]{12})[0-9a-f]+$/, '$1')
}

/** A stack as recorded: its name and the first 12 of its lock's digest. */
export function stackLine(stack: { name: string; sha256?: string }) {
  return stack.sha256
    ? `${stack.name} · ${stack.sha256.replace('sha256:', '').slice(0, 12)}`
    : stack.name
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
  | 'not-run'
  | 'running'
  | 'waiting'
  | 'stopped'

export type TestRow = {
  id: string
  state: TestState
  detail: string
  /** A Docker group's round; a test here stands for all its rounds. */
  round?: number
}

/** A live test list: what reported, what runs, what waits. A Docker group's
 *  tests report as it ends, its runs installed then; one that left no run
 *  did not run. After a cancel what did not finish "stopped before it
 *  finished". */
export function testRows(execution: PlanExecution): TestRow[] {
  const cancelled =
    execution.state === 'cancelled' || execution.state === 'cancelling'
  const source = execution.source
  if (source.kind === 'docker') {
    const rows: TestRow[] = []
    for (const group of source.groups) {
      const ended = group.state === 'done' || group.state === 'failed'
      for (const id of group.scenarios) {
        const installed = execution.slots.find(
          (slot) =>
            slot.round === group.round &&
            slot.group_id === group.group_id &&
            slot.scenario_id === id &&
            slot.execution_id,
        )
        const state: TestState = ended
          ? installed
            ? 'reported'
            : 'not-run'
          : group.state === 'running'
            ? 'running'
            : group.state === 'queued' && !cancelled
              ? 'waiting'
              : 'stopped'
        rows.push({
          id,
          round: group.round,
          state,
          detail: installed
            ? `${installed.passed}/${installed.completed || installed.observed} passed`
            : state === 'not-run'
              ? (group.error ?? 'Its group left no run')
              : state === 'running'
                ? `Running in its container${group.attempt > 1 ? ` · attempt ${group.attempt}` : ''}`
                : state === 'waiting'
                  ? 'Waiting for a slot'
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

/** The line under each test without a result yet (running, waiting, or
 *  stopped before it finished), by round and test (`roundKey`). */
export function liveNotes(execution: PlanExecution): Record<string, string> {
  return Object.fromEntries(
    testRows(execution)
      .filter((row) => ['running', 'waiting', 'stopped'].includes(row.state))
      .map((row) => [roundKey(row.round, row.id), row.detail]),
  )
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

export function dockerSteps(execution: PlanExecution) {
  const source = execution.source
  if (source.kind !== 'docker') return []
  const order = ['prepare', 'groups', 'finalize', 'import', 'done']
  const at = order.indexOf(source.phase)
  const count = (...states: string[]) =>
    source.groups.filter((group) => states.includes(group.state)).length
  const finished = count('done', 'failed')
  const cancelled =
    execution.state === 'cancelled' || execution.state === 'cancelling'
  const stopped = count('cancelled', 'interrupted')
  const stoppedGroups = stopped > 0
  // "3 of 9 finished · 2 running · 4 waiting"
  const groupsLine = [
    `${finished} of ${source.groups.length} finished${cancelled ? ' before the cancel' : ''}`,
    count('running') ? `${count('running')} running` : null,
    count('queued') ? `${count('queued')} waiting` : null,
    stopped ? `${stopped} stopped` : null,
  ]
    .filter(Boolean)
    .join(' · ')
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
      detail: phase === 'groups' ? groupsLine : detail,
      time: '',
    }
  })
}

const isGroupJob = (job: GithubJob) => /case-/.test(job.name)
const isPrepareJob = (job: GithubJob) => /prepare|materialize/i.test(job.name)
const isAggregateJob = (job: GithubJob) =>
  /aggregate|finalize/i.test(job.name) && !isGroupJob(job)

/** A GitHub run's four steps as it reports its jobs: the prepare job, the
 *  group jobs, the aggregate job and this Console's import. */
export function githubSteps(execution: PlanExecution) {
  const source = execution.source
  if (source.kind !== 'github') return []
  const jobs = source.follow?.jobs ?? []
  const cancelled =
    execution.state === 'cancelling' || execution.state === 'cancelled'
  const importing = execution.state === 'importing'
  const jobState = (job: GithubJob | undefined, after: boolean): StepState =>
    job?.status === 'completed'
      ? 'done'
      : job?.status === 'in_progress' || (after && !job)
        ? 'current'
        : 'next'
  const prepare = jobs.find(isPrepareJob)
  const groups = jobs.filter(isGroupJob)
  const aggregate = jobs.find(isAggregateJob)
  const finished = groups.filter((job) => job.status === 'completed').length
  const live = groups.filter((job) => job.status === 'in_progress').length
  const groupsDone = groups.length > 0 && finished === groups.length
  const span = (list: GithubJob[]) => {
    const starts = list.flatMap((job) =>
      job.started_at ? [job.started_at] : [],
    )
    if (starts.length === 0) return ''
    const first = starts.sort()[0]
    const ends = list.flatMap((job) =>
      job.completed_at ? [job.completed_at] : [],
    )
    return elapsed(
      first,
      list.every((job) => job.completed_at) ? ends.sort().at(-1) : null,
    )
  }
  const prepareState: StepState =
    prepare?.status === 'completed' || groups.length > 0
      ? 'done'
      : jobState(prepare, !importing)
  let groupsState: StepState = groupsDone
    ? 'done'
    : prepareState === 'done'
      ? 'current'
      : 'next'
  let aggregateState: StepState = importing
    ? 'done'
    : groupsDone
      ? jobState(aggregate, true)
      : 'next'
  if (cancelled && groupsState === 'current') groupsState = 'stopped'
  if (cancelled && aggregateState === 'next') aggregateState = 'current'
  return [
    {
      phase: 'prepare',
      label: 'Prepare job',
      state: prepareState,
      detail: 'Suite materialized, stack assembled and locked',
      time: prepare ? jobDuration(prepare) : '',
    },
    {
      phase: 'groups',
      label: 'Group jobs',
      state: groupsState,
      detail:
        groups.length === 0
          ? 'Waiting for GitHub to start the jobs'
          : cancelled
            ? `${finished} of ${groups.length} finished before the cancel · ${groups.length - finished} stopped`
            : [
                `${finished} of ${groups.length} finished`,
                live ? `${live} running` : null,
              ]
                .filter(Boolean)
                .join(' · '),
      time: span(groups),
    },
    {
      phase: 'aggregate',
      label: 'Aggregate job',
      state: aggregateState,
      detail: cancelled
        ? 'GitHub is finishing the cancel'
        : 'Aggregates the groups and writes the bundle',
      time: aggregate ? jobDuration(aggregate) : '',
    },
    {
      phase: 'import',
      label: 'Import',
      state: importing ? ('current' as StepState) : ('next' as StepState),
      detail: importing
        ? 'Importing what finished'
        : cancelled
          ? 'Imports what finished once the run ends'
          : 'Automatic when the run ends',
      time: '',
    },
  ]
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
      action: 'Cancel run',
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
