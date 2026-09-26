import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { PlanExecution } from '@/lib/plan-execution'
import { CancelExecutionDialog, WhereItRan } from './WhereItRan'
import {
  cancelCopy,
  dockerSteps,
  jobLabel,
  jobTests,
  reportedLine,
  testRows,
  whereLine,
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
    expect(whereLine(harness)).toMatch(/^Running · on this harness · for 3m/)
    expect(reportedLine(harness)).toBe(
      '1 of 3 tests reported · results are provisional',
    )
  })

  it('reads a Docker execution’s steps and groups; finished groups wait for the import', () => {
    const steps = dockerSteps(docker)
    expect(steps.map((step) => step.state)).toEqual([
      'done',
      'current',
      'next',
      'next',
    ])
    expect(steps[1].detail).toBe('1 of 3 finished · 2 groups at a time')
    const rows = testRows(docker)
    expect(rows[0]).toMatchObject({ id: 'minimal_path', state: 'at-import' })
    expect(rows[0].detail).toContain('results at import')
    expect(rows[1].state).toBe('running')
    expect(rows[2].detail).toBe('Waiting for a slot · 2 groups at a time')
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
    expect(testRows(cancelled)[2].detail).toBe('Stopped before it finished')
  })

  it('maps GitHub jobs to their tests and states', () => {
    const jobs =
      github.source.kind === 'github' ? (github.source.follow?.jobs ?? []) : []
    expect(jobTests(jobs[0], github)).toEqual(['minimal_path'])
    expect(jobs.map(jobLabel)).toEqual(['Done', 'Running', 'Queued'])
    expect(whereLine(github)).toMatch(/^Running · on GitHub · dispatched /)
    expect(reportedLine(github)).toBe(
      '1 of 2 group jobs finished · results at import',
    )
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
  it('shows the GitHub run, workflow ref and group jobs', () => {
    const html = renderToStaticMarkup(<WhereItRan execution={github} />)
    expect(html).toContain('GitHub #77')
    expect(html).toContain('exact-stack-e2e.yml @ main 88aee14')
    expect(html).toContain('Automatic when the run ends')
    expect(html).toContain('E2E / case-timer-wake')
    expect(html).toContain('data-job-state="running"')
    expect(html).toContain('6m 47s')
  })

  it('shows the Docker steps, groups, image and the missing credentials', () => {
    const html = renderToStaticMarkup(<WhereItRan execution={docker} />)
    expect(html).toContain('No provider credentials')
    expect(html).toContain('Suite materialized, stack assembled and locked')
    expect(html).toContain('data-docker-group="case-timer-wake"')
    expect(html).toContain('tools-d9a8b54a2c85')
    expect(html).toContain('results at import')
  })

  it('lists the harness tests while it runs', () => {
    const html = renderToStaticMarkup(<WhereItRan execution={harness} />)
    expect(html).toContain('This harness, on the stack this Console runs on')
    expect(html).toContain('data-state="running"')
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
    expect(html).toContain('Keep it running')
  })
})
