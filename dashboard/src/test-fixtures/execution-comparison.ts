import type {
  DashboardExecutionDetail,
  StackWorker,
} from '@/lib/dashboard-data-source'

export type Row = {
  scenario?: string
  technical?: string
  completion?: string
  score?: number | null
  /** The case definition, `case.inputs_sha256`. */
  definition?: string
  seed?: number | string
  round?: number
  /** Input and output tokens; a tenth more is read from cache. */
  tokens?: number
  criteria?: Array<{
    id: string
    possible: number
    awarded: number | null
    reason: string
    description?: string
  }>
}

/** One run of a report, with the fields Release Control's ledger reads. */
export function reportRun(id: string, row: Row = {}) {
  const tokens = row.tokens ?? 100
  return {
    run_id: id,
    technical: row.technical ?? 'valid',
    completion: row.completion ?? 'completed',
    score: row.score === undefined ? 80 : row.score,
    wall_time_ms: 2000,
    efficiency: {
      total_tokens: tokens,
      root_turns: 2,
      child_turns: 1,
      function_calls: 3,
      function_call_errors: 0,
    },
    metrics: {
      complete: true,
      totals: { cache_read_tokens: tokens / 10, cache_write_tokens: 0 },
    },
    cost: { subject_usd: 0.01, total_usd: 0.012 },
    criteria: row.criteria ?? [],
  }
}

/** One execution, one report per row, one run per report. */
export function execution(
  id: string,
  rows: Row[],
  extra: Record<string, unknown> = {},
): DashboardExecutionDetail {
  return {
    id,
    label: id,
    status: 'passed',
    subjects: [
      { id: 'subject', model: 'flash', provider: 'deepseek', scenarios: [] },
    ],
    reports: rows.map((row, index) => {
      const scenario = row.scenario ?? `test_${index}`
      return {
        subject_id: 'subject',
        scenario_id: scenario,
        available: true,
        ...(row.round === undefined ? {} : { round: row.round }),
        report: {
          result_contract_sha256: 'contract',
          scenarios: [
            {
              scenario_id: scenario,
              behavior_sha256: `behavior-${scenario}`,
              case_id: `${scenario}:seed-1`,
              case: {
                seed: row.seed ?? 1,
                inputs_sha256: row.definition ?? `definition-${scenario}`,
              },
              aggregate: {
                planned_runs: 1,
                observed_runs: 1,
                deferred_runs: 0,
              },
              runs: [reportRun(`${id}-${index}`, row)],
            },
          ],
        },
      }
    }),
    ...extra,
  } as unknown as DashboardExecutionDetail
}

const stack = (workers: Partial<StackWorker>[]): StackWorker[] =>
  workers.map((worker) => ({
    name: 'worker',
    source: 'package',
    requested: null,
    observed: null,
    commit: null,
    dirty: null,
    ...worker,
  }))

/** A GitHub import: parameters, source and stack recorded, no plan. */
export function imported() {
  const parameters = {
    scenarios: ['minimal_path', 'persistent_state', 'shell_coder_sandbox'],
    runs: 1,
    technical_retries: 1,
    seed: null,
    model: 'flash',
    provider: 'deepseek',
    agent: null,
  }
  const source = {
    kind: 'github',
    repository: 'iii-hq/harness-e2e',
    run_id: 35823421664,
    run_attempt: 1,
    url: 'https://github.com/iii-hq/harness-e2e/actions/runs/35823421664',
    release_control_execution_id: '366030b3-4a4e-4b1a-9f3e-9f7e0c1d2e3f',
  }
  return execution(
    'import-a',
    [
      {
        scenario: 'minimal_path',
        score: 82,
        tokens: 1200,
        criteria: [
          {
            id: 'cites_source',
            description: 'answer cites the source',
            possible: 20,
            awarded: 8,
            reason: 'no source named',
          },
          {
            id: 'answer',
            possible: 80,
            awarded: 74,
            reason: 'correct',
          },
        ],
      },
      {
        scenario: 'persistent_state',
        score: 100,
        tokens: 900,
        criteria: [
          {
            id: 'state_after_restart',
            description: 'state read after restart',
            possible: 50,
            awarded: 50,
            reason: 'read back',
          },
        ],
      },
      {
        scenario: 'shell_coder_sandbox',
        technical: 'technical_invalid',
        completion: 'undetermined',
        score: null,
        tokens: 300,
      },
    ],
    {
      label: 'smoke',
      plan_id: null,
      parameters,
      source,
      stack: stack([
        { name: 'harness-e2e', observed: '0.9.3', requested: '0.9.3' },
        { name: 'llm-router', observed: '1.2.0', requested: '^1.2' },
      ]),
      plan_execution: {
        id: 'import-a',
        plan_id: null,
        role: null,
        label: 'smoke',
        parameters,
        source,
        stack: stack([
          { name: 'harness-e2e', observed: '0.9.3', requested: '0.9.3' },
          { name: 'llm-router', observed: '1.2.0', requested: '^1.2' },
        ]),
      },
    },
  )
}

/** A local run of the current harness with two path workers. */
export function local() {
  const parameters = {
    scenarios: ['minimal_path', 'persistent_state', 'shell_coder_sandbox'],
    runs: 1,
    technical_retries: 1,
    seed: null,
    model: 'flash',
    provider: 'deepseek',
    agent: null,
  }
  const workers = stack([
    { name: 'harness-e2e', observed: '0.9.3', requested: 'latest' },
    {
      name: 'llm-router',
      source: 'path',
      observed: '1.3.0-dev',
      commit: 'a1b2c3d4e5f6',
      dirty: true,
    },
    {
      name: 'session-manager',
      source: 'path',
      observed: '0.4.0',
      commit: 'a1b2c3d4e5f6',
      dirty: true,
    },
  ])
  return execution(
    'local-b',
    [
      {
        scenario: 'minimal_path',
        score: 94,
        tokens: 1000,
        criteria: [
          {
            id: 'cites_source',
            description: 'answer cites the source',
            possible: 20,
            awarded: 20,
            reason: 'names the source',
          },
          {
            id: 'answer',
            possible: 80,
            awarded: 74,
            reason: 'correct',
          },
        ],
      },
      {
        scenario: 'persistent_state',
        score: 62,
        tokens: 900,
        criteria: [
          {
            id: 'state_after_restart',
            description: 'state read after restart',
            possible: 50,
            awarded: 12,
            reason: 'state lost on restart',
          },
        ],
      },
      { scenario: 'shell_coder_sandbox', score: 40, tokens: 500 },
    ],
    {
      label: 'smoke',
      parameters,
      source: { kind: 'local' },
      stack: workers,
      plan_execution: {
        id: 'local-b',
        plan_id: null,
        role: null,
        label: 'smoke',
        parameters,
        source: { kind: 'local' },
        stack: workers,
      },
    },
  )
}
