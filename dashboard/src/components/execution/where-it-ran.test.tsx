import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { PlanExecution } from '@/lib/plan-execution'
import {
  CancelExecutionDialog,
  HarnessProgress,
  LiveProgress,
  WhereItRan,
} from './WhereItRan'
import {
  cancelCopy,
  dockerSteps,
  githubSteps,
  jobLabel,
  jobTests,
  liveNotes,
  shortImage,
  testRows,
} from './where-it-ran-model'

const base = {
  id: 'plan-1',
  label: null,
  parameters: {
    scenarios: ['minimal_path', 'timer_wake', 'registry_implementation'],
    runs: 1,
    technical_retries: 0,
    model: 'm',
    provider: 'p',
  },
  stack: [],
  state: 'running',
  started_at: new Date(Date.now() - 220_000).toISOString(),
  finished_at: null,
  error: null,
  slots: [],
  measurements: null,
} as unknown as PlanExecution

const slot = (scenario_id: string, state: string, group_id = '') => ({
  round: 1,
  group_id,
  scenario_id,
  execution_id: '',
  state,
  observed: state === 'finished' ? 1 : 0,
  completed: state === 'finished' ? 1 : 0,
  passed: state === 'finished' ? 1 : 0,
  technical_valid: 1,
  result_path: null,
  error: null,
})

const harness = {
  ...base,
  source: { kind: 'local' },
  slots: [
    slot('minimal_path', 'finished'),
    slot('timer_wake', 'running'),
    slot('registry_implementation', 'queued'),
  ],
} as unknown as PlanExecution

const docker = {
  ...base,
  // Its finished group installed; the others' slots follow their groups.
  slots: [
    {
      ...slot('minimal_path', 'finished', 'case-minimal-path'),
      execution_id: 'native-minimal',
    },
    slot('timer_wake', 'running', 'case-timer-wake'),
    slot('registry_implementation', 'pending', 'case-registry-implementation'),
    slot('registry_verification', 'pending', 'case-registry-implementation'),
  ],
  source: {
    kind: 'docker',
    attempt: 1,
    phase: 'groups',
    image: 'tools-d9a8b54a2c85',
    groups: [
      {
        round: 1,
        campaign_id: '',
        group_id: 'case-minimal-path',
        scenarios: ['minimal_path'],
        state: 'done',
        attempt: 1,
      },
      {
        round: 1,
        campaign_id: '',
        group_id: 'case-timer-wake',
        scenarios: ['timer_wake'],
        state: 'running',
        attempt: 1,
      },
      {
        round: 1,
        campaign_id: '',
        group_id: 'case-registry-implementation',
        scenarios: ['registry_implementation', 'registry_verification'],
        state: 'queued',
        attempt: 1,
      },
    ],
  },
  warnings: [
    'No provider_env_file is configured: the providers start without credentials.',
  ],
} as unknown as PlanExecution

const github = {
  ...base,
  source: {
    kind: 'github',
    repository: 'o/r',
    run_id: 77,
    run_attempt: 1,
    url: 'https://github.com/o/r/actions/runs/77',
    release_control_execution_id: null,
    status: 'in_progress',
    follow: {
      followed: true,
      head_branch: 'main',
      head_sha: '88aee14abcdef',
      jobs: [
        {
          id: 1,
          name: 'E2E / case-minimal-path',
          status: 'completed',
          conclusion: 'success',
          started_at: '2026-09-25T10:00:00Z',
          completed_at: '2026-09-25T10:06:47Z',
          url: 'https://github.com/o/r/actions/runs/77/job/1',
        },
        {
          id: 2,
          name: 'E2E / case-timer-wake',
          status: 'in_progress',
          url: '',
        },
        { id: 3, name: 'aggregate', status: 'queued', url: '' },
      ],
    },
  },
} as unknown as PlanExecution

describe('where it ran · model', () => {
  it('fills the harness tests in as they report', () => {
    const rows = testRows(harness)
    expect(rows.map((row) => row.state)).toEqual([
      'reported',
      'running',
      'waiting',
    ])
  })

  it('reads a Docker execution’s steps and groups; a finished group’s tests report at once', () => {
    const steps = dockerSteps(docker)
    expect(steps.map((step) => step.state)).toEqual([
      'done',
      'current',
      'next',
      'next',
    ])
    expect(steps[1].detail).toBe('1 of 3 finished · 1 running · 1 waiting')
    const rows = testRows(docker)
    expect(rows[0]).toEqual({
      id: 'minimal_path',
      round: 1,
      state: 'reported',
      detail: '1/1 passed',
    })
    expect(rows[1]).toMatchObject({
      state: 'running',
      detail: 'Running in its container',
    })
    // How many groups run at once is the worker's; the line does not guess.
    expect(rows[2].detail).toBe('Waiting for a slot')
    // A group that ended without a run: its tests did not run, and say why.
    const failed = {
      ...docker,
      slots: [slot('minimal_path', 'finished', 'case-minimal-path')],
      source: {
        ...(docker.source as object),
        groups: [
          {
            round: 1,
            campaign_id: '',
            group_id: 'case-minimal-path',
            scenarios: ['minimal_path'],
            state: 'failed',
            attempt: 1,
            error: 'compose::add failed',
          },
        ],
      },
    } as unknown as PlanExecution
    expect(testRows(failed)).toEqual([
      {
        id: 'minimal_path',
        round: 1,
        state: 'not-run',
        detail: 'compose::add failed',
      },
    ])
    const cancelled = { ...docker, state: 'cancelled' } as PlanExecution
    const stopped = {
      ...cancelled,
      source: {
        ...(docker.source as object),
        phase: 'done',
        groups: [
          {
            round: 1,
            campaign_id: '',
            group_id: 'case-a',
            scenarios: ['a'],
            state: 'cancelled',
            attempt: 1,
          },
        ],
      },
    } as unknown as PlanExecution
    expect(dockerSteps(stopped).map((step) => step.state)).toEqual([
      'done',
      'stopped',
      'next',
      'next',
    ])
    expect(dockerSteps(stopped)[1].detail).toBe(
      '0 of 1 finished before the cancel · 1 stopped',
    )
    expect(testRows(cancelled)[2].detail).toBe('Stopped before it finished')
  })

  it('maps GitHub jobs to their tests and states', () => {
    const jobs =
      github.source.kind === 'github' ? (github.source.follow?.jobs ?? []) : []
    expect(jobTests(jobs[0], github)).toEqual(['minimal_path'])
    expect(jobs.map(jobLabel)).toEqual(['Done', 'Running', 'Queued'])
  })

  it('reads a GitHub run’s four steps from its jobs', () => {
    const steps = githubSteps(github)
    expect(steps.map((step) => [step.label, step.state])).toEqual([
      ['Prepare job', 'done'],
      ['Group jobs', 'current'],
      ['Aggregate job', 'next'],
      ['Import', 'next'],
    ])
    expect(steps[1].detail).toBe('1 of 2 finished · 1 running')
    const cancelling = { ...github, state: 'cancelling' } as PlanExecution
    expect(githubSteps(cancelling).map((step) => step.state)).toEqual([
      'done',
      'stopped',
      'current',
      'next',
    ])
    expect(githubSteps(cancelling)[1].detail).toBe(
      '1 of 2 finished before the cancel · 1 stopped',
    )
    const importing = { ...github, state: 'importing' } as PlanExecution
    expect(githubSteps(importing)[3]).toMatchObject({
      state: 'current',
      detail: 'Importing what finished',
    })
  })

  it('names an executor image by its tag, cut to 12', () => {
    expect(shortImage('ghcr.io/iii-hq/harness-e2e:tools-d9a8b54a2c85')).toBe(
      'tools-d9a8b54a2c85',
    )
    expect(
      shortImage(
        'ghcr.io/iii-hq/harness-e2e:tools-d9a8b54a2c85f00dfeedface0123',
      ),
    ).toBe('tools-d9a8b54a2c85')
  })

  it('says what a cancel stops and keeps, per place', () => {
    expect(cancelCopy(harness).body).toBe(
      'The test running now stops. What already reported stays.',
    )
    expect(cancelCopy(docker).body).toContain(
      'The group that finished keeps counting',
    )
    const none = {
      ...docker,
      source: {
        ...(docker.source as object),
        groups: [
          {
            round: 1,
            campaign_id: '',
            group_id: 'case-a',
            scenarios: ['a'],
            state: 'running',
            attempt: 1,
          },
        ],
      },
    } as unknown as PlanExecution
    expect(cancelCopy(none).body).toContain('None has finished yet')
    expect(cancelCopy(github)).toMatchObject({
      title: 'Cancel the run on GitHub?',
    })
    expect(cancelCopy(github).body).toBe(
      'Calls gh run cancel. Its 1 running job stops; the job that finished is imported when the run ends.',
    )
  })
})

describe('where it ran · rounds', () => {
  it('keys each Docker test’s line by its round', () => {
    const group = (round: number, state: string) => ({
      round,
      campaign_id: `pr-r0${round}`,
      group_id: 'case-minimal-path',
      scenarios: ['minimal_path'],
      state,
      attempt: 1,
    })
    const rounds = {
      ...docker,
      slots: [],
      source: {
        ...(docker.source as object),
        groups: [group(1, 'queued'), group(2, 'running')],
      },
    } as unknown as PlanExecution
    expect(liveNotes(rounds)).toEqual({
      '1:minimal_path': 'Waiting for a slot',
      '2:minimal_path': 'Running in its container',
    })
  })

  it('keeps a test running until all its rounds report', () => {
    const rounds = {
      ...harness,
      slots: [
        slot('minimal_path', 'finished'),
        slot('minimal_path', 'running'),
      ],
    } as PlanExecution
    const [row] = testRows(rounds)
    expect(row.state).toBe('running')
    expect(row.detail).toContain('1 of 2 rounds')
  })
})

describe('where it ran · card', () => {
  it('shows the workflow ref and group jobs, not the run the band names', () => {
    const html = renderToStaticMarkup(<WhereItRan execution={github} />)
    expect(html).not.toContain('GitHub #77')
    expect(html).not.toContain('Release Control')
    expect(html).toContain('exact-stack-e2e.yml @ main 88aee14')
    expect(html).toContain('E2E / case-timer-wake')
    expect(html).toContain('data-job-state="running"')
    expect(html).toContain('6m 47s')
  })

  it('shows the Docker steps and groups; image and credentials are said once, elsewhere', () => {
    const html = renderToStaticMarkup(<WhereItRan execution={docker} />)
    // Needs attention says the missing credentials; the band, the image.
    expect(html).not.toContain('No provider credentials')
    expect(html).not.toContain('tools-d9a8b54a2c85')
    expect(html).toContain('Suite materialized, stack assembled and locked')
    expect(html).toContain('data-docker-group="case-timer-wake"')
    expect(html).toContain('data-group-state="true">Running<')
    // Its tests are in the results table, filled in as each group ends.
    expect(html).not.toContain('aria-label="Tests"')
    expect(html).not.toContain('results at import')
  })

  it('shows one live progress: steps in Docker and on GitHub, a bar here', () => {
    const docked = renderToStaticMarkup(<LiveProgress execution={docker} />)
    expect(docked).toContain('Progress')
    expect(docked).toContain('data-step-state="current"')
    expect(docked).not.toContain('Group jobs')
    const hub = renderToStaticMarkup(<LiveProgress execution={github} />)
    expect(hub).toContain('Aggregate job')
    expect(hub).toContain('E2E / case-timer-wake')
    expect(renderToStaticMarkup(<LiveProgress execution={harness} />)).toBe('')
    expect(
      renderToStaticMarkup(<HarnessProgress execution={harness} />),
    ).toContain('1 of 3 tests reported · 1 running · results are provisional')
  })

  it('confirms a GitHub cancel naming gh run cancel', () => {
    const html = renderToStaticMarkup(
      <CancelExecutionDialog
        bridge={null}
        execution={github}
        open
        onClose={() => {}}
        onCancelled={() => {}}
      />,
    )
    expect(html).toContain('Cancel the run on GitHub?')
    expect(html).toContain('Calls gh run cancel.')
    expect(html).toContain('Keep running')
    expect(html).toContain('role="alertdialog"')
    expect(html).toContain('>Cancel run<')
  })
})
