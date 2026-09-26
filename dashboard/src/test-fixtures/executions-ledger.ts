import type {
  DashboardExecutionSummary,
  DockerGroup,
} from '@/lib/dashboard-data-source'

// The executions list as the redesign canvas draws it (Executions.dc.html,
// ROWS): three executions running (in Docker, an import from GitHub, on this
// harness), then today, yesterday and two older days. Dates are local, so
// the day groups and times read the same in any time zone.

/** The evening of Sep 24, 2026, when the canvas was drawn. */
export const LEDGER_NOW = new Date(2026, 8, 24, 21, 0)

/** Executions retained on the canvas, of which the rows below are loaded. */
export const LEDGER_TOTAL = 58

const at = (day: number, hours: number, minutes: number) =>
  new Date(2026, 8, day, hours, minutes).toISOString()

const FLASH = { provider: 'deepseek', model: 'deepseek-flash' }
const OPUS = { provider: 'claude-code', model: 'claude-code/claude-opus-5-5' }

type Row = {
  id: string
  title: string
  at: string
  /** When an untitled execution was created: its title is dated by it. */
  started?: string
  origin: 'local' | 'docker' | { github: number }
  result: 'running' | 'importing' | 'passed' | 'failed' | 'incomplete'
  subject: { provider: string; model: string }
  profile: string | null
  tests: [received: number, expected: number]
  score?: number | null
  pass?: number | null
  runtime?: number | null
  tokens?: number | null
  groups?: DockerGroup[]
}

function group(index: number, state: DockerGroup['state']): DockerGroup {
  return {
    round: 1,
    campaign_id: 'regression',
    group_id: `case-${index}`,
    scenarios: [`scenario_${index}`],
    state,
    attempt: 1,
  }
}

function source(origin: Row['origin'], groups: DockerGroup[] = []) {
  if (origin === 'local') return { kind: 'local' }
  if (origin === 'docker')
    return { kind: 'docker', attempt: 1, phase: 'groups', groups }
  return {
    kind: 'github',
    repository: 'iii-hq/harness-e2e',
    run_id: origin.github,
    run_attempt: 1,
    url: `https://github.com/iii-hq/harness-e2e/actions/runs/${origin.github}`,
    release_control_execution_id: null,
  }
}

function summary(row: Row): DashboardExecutionSummary {
  const [received, expected] = row.tests
  const live = row.result === 'running' || row.result === 'importing'
  const scenarios = Array.from({ length: expected }, (_, index) => ({
    id: `scenario_${index + 1}`,
    mean_score: row.score ?? null,
  }))
  const statuses =
    row.result === 'passed'
      ? { passed: expected }
      : row.result === 'failed'
        ? { infrastructure_error: 1, passed: received - 1 }
        : row.result === 'incomplete'
          ? { unavailable: expected }
          : {}
  return {
    id: row.id,
    label: row.title,
    status:
      row.result === 'failed'
        ? 'technical_failed'
        : row.result === 'running'
          ? 'running'
          : row.result,
    state: live ? row.result : 'completed',
    availability: 'available',
    started_at: row.started ?? row.at,
    completed_at: live ? '' : row.at,
    source: source(row.origin, row.groups),
    parameters: {
      scenarios: scenarios.map((scenario) => scenario.id),
      runs: 1,
      technical_retries: 1,
      model: row.subject.model,
      provider: row.subject.provider,
      agent: row.profile,
    },
    plan_execution: { planned: expected, finished: received },
    subjects: [{ id: 'subject', ...row.subject, scenarios }],
    assessment_summary: live
      ? undefined
      : ({ system_statuses: statuses } as never),
    totals: {
      expected_reports: expected,
      received_reports: received,
      scenario_pass_rate:
        row.pass === undefined || row.pass === null ? null : row.pass / 100,
      wall_time_seconds: row.runtime ?? null,
      total_tokens: row.tokens ?? null,
    },
  }
}

const ROWS: Row[] = [
  {
    id: 'plan-9c41d07be2a3f6150000000000000000',
    title: 'Regression',
    at: at(24, 20, 12),
    origin: 'docker',
    result: 'running',
    subject: FLASH,
    profile: null,
    tests: [3, 9],
    groups: [
      group(1, 'done'),
      group(2, 'done'),
      group(3, 'failed'),
      group(4, 'running'),
      group(5, 'running'),
      group(6, 'queued'),
      group(7, 'queued'),
      group(8, 'queued'),
      group(9, 'queued'),
    ],
  },
  {
    id: 'plan-e5b0a2c47d913f080000000000000000',
    title: 'Software engineering',
    at: at(24, 20, 33),
    origin: { github: 36073359724 },
    result: 'importing',
    subject: FLASH,
    profile: null,
    tests: [6, 14],
  },
  {
    id: 'plan-2b7e41c09d5a8f310000000000000000',
    title: 'Opus 5.5 · ade-worker-builder · retry',
    at: at(24, 11, 2),
    origin: 'local',
    result: 'running',
    subject: OPUS,
    profile: 'ade-worker-builder',
    tests: [1, 2],
  },
  {
    id: 'plan-cf6ab5f943136bb54a954bc763a26eff',
    title: 'Opus 5.5 · ade-worker-builder · wake fix',
    at: at(24, 10, 10),
    origin: 'local',
    result: 'passed',
    subject: OPUS,
    profile: 'ade-solo-builder',
    tests: [2, 2],
    score: 100,
    pass: 100,
    runtime: 1049,
    tokens: 85_395,
  },
  {
    id: 'plan-7706993fb0b832a97df123065d77fff1',
    title: 'Opus 5.5 · ade-worker-builder · wake fix',
    at: at(24, 9, 51),
    origin: 'local',
    result: 'incomplete',
    subject: OPUS,
    profile: 'meu-profile',
    tests: [0, 2],
  },
  {
    id: 'plan-f811eb1f31c9ab47bca5de86e3c895e5',
    title: 'Opus 5.5 · ade-worker-builder · wake fix',
    at: at(24, 9, 46),
    origin: 'local',
    result: 'passed',
    subject: OPUS,
    profile: 'ade-worker-builder',
    tests: [2, 2],
    score: 95,
    pass: 100,
    runtime: 4205,
    tokens: 353_987,
  },
  {
    id: 'plan-81960bf0afbe493e793920f06fe3a169',
    title: 'Opus 5.5 · template only',
    at: at(24, 8, 9),
    origin: 'local',
    result: 'passed',
    subject: OPUS,
    profile: null,
    tests: [2, 2],
    score: 100,
    pass: 100,
    runtime: 922,
    tokens: 94_419,
  },
  {
    id: 'plan-684b8f754aa50b61006e2415b061b254',
    title: 'Opus 5.5 · ade-worker-builder',
    at: at(24, 7, 53),
    origin: 'local',
    result: 'passed',
    subject: OPUS,
    profile: 'ade-worker-builder',
    tests: [2, 2],
    score: 0,
    pass: 100,
    runtime: 385,
    tokens: 35_038,
  },
  {
    id: 'plan-00ec987715b2647da4bd386eefc230b6',
    title: 'no profile',
    at: at(24, 6, 54),
    origin: 'local',
    result: 'failed',
    subject: OPUS,
    profile: null,
    tests: [1, 1],
    pass: 0,
    runtime: 113,
    tokens: 7_336,
  },
  {
    // Untitled: its title is its model and when it was created.
    id: 'plan-3ef1b6a76e54d12e4936a8ecd1dda7e5',
    title: '',
    at: at(24, 6, 49),
    started: at(24, 6, 43),
    origin: 'local',
    result: 'failed',
    subject: OPUS,
    profile: null,
    tests: [1, 1],
    pass: 0,
    runtime: 406,
    tokens: 44_586,
  },
  {
    id: 'plan-1d32074494c8ae86a7fec7e2c0f96447',
    title: 'Software engineering',
    at: at(24, 4, 25),
    origin: { github: 35965100994 },
    result: 'failed',
    subject: FLASH,
    profile: 'ade-worker-builder',
    tests: [13, 15],
    pass: 80,
    runtime: 8951,
    tokens: 3_339_305,
  },
  {
    id: 'plan-958b154359944826a6d5bb3c30a18c0e',
    title: 'Software engineering',
    at: at(23, 20, 0),
    origin: { github: 35925167026 },
    result: 'failed',
    subject: FLASH,
    profile: 'ade-worker-builder',
    tests: [13, 13],
    pass: 61.5,
    runtime: 12_148,
    tokens: 11_878_141,
  },
  {
    id: 'c3cdb199cf8ecf05ab82c7352a3a4212',
    title: 'e2e::* control-plane run',
    at: at(21, 0, 18),
    origin: 'local',
    result: 'passed',
    subject: FLASH,
    profile: null,
    tests: [1, 1],
    score: 90,
    pass: 100,
    runtime: 8.8,
    tokens: 4_224,
  },
  {
    id: 'a1d33f6958975feedf83ce5c1e1c3ed4',
    title: 'validation-ui-1-2-4-deepseek-flash-20260921',
    at: at(21, 0, 17),
    origin: 'local',
    result: 'passed',
    subject: FLASH,
    profile: null,
    tests: [1, 1],
    score: 100,
    pass: 100,
    runtime: 12,
    tokens: 4_505,
  },
  {
    id: '33c4f270b6057d14b349a9f38af5a821',
    title: 'ux-review-deepseek-flash-persistent-state',
    at: at(20, 22, 37),
    origin: 'local',
    result: 'passed',
    subject: FLASH,
    profile: null,
    tests: [1, 1],
    score: 90,
    pass: 100,
    runtime: 11,
    tokens: 4_583,
  },
  {
    id: 'c92c4cd47ac1bd4a24f68e489b0d234a',
    title: 'qa-ux-20260920-quick-retest',
    at: at(20, 22, 10),
    origin: 'local',
    result: 'passed',
    subject: FLASH,
    profile: null,
    tests: [1, 1],
    score: 40,
    pass: 100,
    runtime: 19,
    tokens: 6_087,
  },
]

/** The canvas's sixteen executions, newest first as the list receives them. */
export const LEDGER_EXECUTIONS: DashboardExecutionSummary[] = ROWS.map(summary)

/** One execution of the fixture by the start of its id. */
export function ledgerExecution(id: string): DashboardExecutionSummary {
  const found = LEDGER_EXECUTIONS.find((execution) =>
    execution.id.startsWith(id),
  )
  if (!found) throw new Error(`no ledger fixture ${id}`)
  return found
}
