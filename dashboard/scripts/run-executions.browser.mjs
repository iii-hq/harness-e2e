// Deterministic browser coverage for Run tests, Run again and the GitHub
// import list. No models run.
import assert from 'node:assert/strict'
import { chromium } from 'playwright'
import { createConsoleTestHost } from './console-test-host.mjs'

const imported = {
  id: 'plan-0123456789abcdef0123456789abcdef',
  label: 'Software engineering',
  plan_id: null,
  status: 'passed',
  state: 'completed',
  started_at: '2026-09-20T10:00:00Z',
  availability: 'aggregate',
  subjects: [
    {
      id: 'terra',
      model: 'gpt-5.6-terra',
      provider: 'openai-codex',
      scenarios: [],
    },
  ],
  reports: [],
  totals: { expected_reports: 2, received_reports: 2 },
  plan_execution: {
    id: 'plan-0123456789abcdef0123456789abcdef',
    label: 'Software engineering',
    parameters: {
      // A suite this runner does not list: Run again offers it as recorded.
      suite: {
        id: 'software-engineering-2025',
        label: 'Software engineering 2025',
        sha256: 'sha256:0123456789abcdef0123456789abcdef',
      },
      scenarios: ['minimal_path', 'retired_scenario'],
      runs: 2,
      technical_retries: 0,
      model: 'gpt-5.6-terra',
      provider: 'openai-codex',
      agent: 'tech-lead',
    },
    source: {
      kind: 'github',
      repository: 'iii-hq/harness-e2e',
      run_id: 42,
      run_attempt: 1,
      url: 'https://github.com/iii-hq/harness-e2e/actions/runs/42',
      release_control_execution_id: null,
      stack: 'default',
    },
    stack: [
      {
        name: 'harness',
        source: 'package',
        requested: '1.8.31',
        observed: '1.8.8',
        commit: null,
        dirty: null,
      },
      {
        name: 'harness-e2e',
        source: 'package',
        requested: null,
        observed: '0.11.28',
        commit: null,
        dirty: null,
      },
    ],
    warnings: [],
    state: 'completed',
    started_at: '2026-09-20T10:00:00Z',
    finished_at: '2026-09-20T11:00:00Z',
    error: null,
    slots: [],
    measurements: null,
  },
}
const slot = (scenario_id, state) => ({
  round: 1,
  group_id: scenario_id,
  scenario_id,
  execution_id: `native-${scenario_id}`,
  state,
  observed: state === 'finished' ? 1 : 0,
  completed: state === 'finished' ? 1 : 0,
  passed: 0,
  technical_valid: 0,
  result_path: null,
  error: null,
})
/** A started execution: running, no plan, no role. */
const running = (id) => ({
  ...imported,
  id,
  label: '',
  status: 'running',
  state: 'running',
  started_at: '2026-09-23T07:51:00Z',
  completed_at: '',
  generated_at: '2026-09-23T08:04:00Z',
  plan_execution: {
    ...imported.plan_execution,
    id,
    label: null,
    source: { kind: 'local' },
    state: 'running',
    slots: [
      slot('context_pressure', 'finished'),
      slot('minimal_path', 'running'),
    ],
  },
})
const nightly = 'plan-22222222222222222222222222222222'
const runningSummary = {
  ...running(nightly),
  label: 'Nightly',
  // The last execution: Run tests starts from its model.
  parameters: {
    ...imported.plan_execution.parameters,
    model: 'deepseek-v4-flash',
    provider: 'deepseek',
  },
  plan_execution: { planned: 9, finished: 1 },
  totals: { expected_reports: 9, received_reports: 1, missing_reports: 8 },
}
/** Stopped by its user after three slots; the stopped run failed technically. */
const cancelledSummary = {
  ...imported,
  id: 'plan-33333333333333333333333333333333',
  label: 'Stopped early',
  status: 'cancelled',
  state: 'cancelled',
  started_at: '2026-09-22T09:58:00Z',
  completed_at: '2026-09-22T10:00:00Z',
  parameters: imported.plan_execution.parameters,
  plan_execution: { planned: 9, finished: 3 },
  totals: {
    expected_reports: 9,
    received_reports: 3,
    missing_reports: 6,
    technical_failures: 1,
    wall_time_seconds: 119.6,
  },
}
// Ran in Docker on the default stack; its stack as its contract recorded it.
const recordedStack =
  'iii: 0.24.2\ncontainers:\n  harness:\n    worker: package://harness\n    version: 1.8.31\n'
const dockered = {
  ...imported,
  id: 'plan-44444444444444444444444444444444',
  label: 'In Docker',
  plan_execution: {
    ...imported.plan_execution,
    id: 'plan-44444444444444444444444444444444',
    label: 'In Docker',
    parameters: {
      ...imported.plan_execution.parameters,
      where: 'docker',
      stack: {
        name: 'default',
        yaml: recordedStack,
        sha256:
          'sha256:feedfacefeedfacefeedfacefeedfacefeedfacefeedfacefeedfacefeedface',
      },
    },
    source: {
      kind: 'docker',
      attempt: 2,
      phase: 'done',
      image: 'ghcr.io/iii-hq/harness-e2e@sha256:dd',
      groups: [],
    },
  },
}
const dockerGroup = (scenario, state) => ({
  round: 1,
  campaign_id: 'pr-r01',
  group_id: `case-${scenario.replace(/_/g, '-')}`,
  scenarios: [scenario],
  state,
  attempt: 1,
  error: null,
})
/** What a finished run reports for a test that passed at 100. */
const passedReport = (scenario_id) => ({
  subject_id: 'terra',
  scenario_id,
  native_execution_id: `native-${scenario_id}`,
  round: 1,
  available: true,
  report: {
    report_state: 'complete',
    objective_outcome: 'passed',
    result_contract_sha256: 'sha256:0000',
    assessment_contract: { runs: [] },
    assessment_summary: {},
    scenarios: [
      {
        scenario_id,
        passed: true,
        aggregate: {
          planned_runs: 1,
          observed_runs: 1,
          deferred_runs: 0,
          completed_runs: 1,
          task_incomplete_runs: 0,
          undetermined_runs: 0,
          technical_valid_runs: 1,
          technical_invalid_runs: 0,
          execution_reliability: 1,
          completion_evidence_coverage: 1,
          completion_rate: 1,
          scored_runs: 1,
          mean_score: 100,
          total_tokens_consumed: 1200,
          tokens_completed_p50: 1200,
          failed_attempt_tokens: 0,
          tokens_per_completion: 1200,
          technical_failures: 0,
        },
        runs: [
          {
            run_id: `run-${scenario_id}`,
            attempt_id: `attempt-${scenario_id}`,
            status: 'passed',
            completion: 'completed',
            technical: 'valid',
            evaluators: { completion: 'available' },
            score: 100,
            wall_time_ms: 3_000,
            assessment: { system_status: 'passed', assessments: [] },
          },
        ],
      },
    ],
  },
})
/** Running in Docker: its groups and where each is; the group that ended
 *  is installed, its test reported, while the others run and wait. */
const dockerRunning = {
  ...running('plan-55555555555555555555555555555555'),
  reports: [
    passedReport('minimal_path'),
    {
      subject_id: 'terra',
      scenario_id: 'persistent_state',
      available: false,
      state: 'running',
    },
    {
      subject_id: 'terra',
      scenario_id: 'shell_coder_sandbox',
      available: false,
      state: 'queued',
    },
  ],
  plan_execution: {
    ...running('plan-55555555555555555555555555555555').plan_execution,
    slots: [
      {
        ...slot('minimal_path', 'finished'),
        group_id: 'case-minimal-path',
        passed: 1,
      },
      {
        ...slot('persistent_state', 'running'),
        group_id: 'case-persistent-state',
        execution_id: '',
      },
      {
        ...slot('shell_coder_sandbox', 'pending'),
        group_id: 'case-shell-coder-sandbox',
        execution_id: '',
      },
    ],
    source: {
      kind: 'docker',
      attempt: 1,
      phase: 'groups',
      image: null,
      groups: [
        dockerGroup('minimal_path', 'done'),
        dockerGroup('persistent_state', 'running'),
        dockerGroup('shell_coder_sandbox', 'queued'),
      ],
    },
  },
}
// The canvas fixtures (Main.dc.html): its TESTS, SEQUENCES, SUITES and STACKS.
const TESTS =
  'registry_planning registry_implementation registry_environment registry_verification kanban_c1_foundation kanban_c2_persistence kanban_c3_board kanban_c4_ticket_flow kanban_c5_edit_move kanban_c6_discussion kanban_c7_live linkly_tutorial context_pressure minimal_path persistent_state insert_record sequential_pipeline database_migration_recovery shell_coder_sandbox research_pipeline fanout_ladder incident_response todo_worker_simple todo_worker_planned engineering_endurance_ladder git_regression_forensics alertmanager_route_match mechanical_reaction timer_wake receiving_operation validation_loop subagent_validation subagent_validation_failure validation_self_repair validation_scope_enforcement validation_chain secret_hygiene prompt_injection_resilience moving_target poison_message cleanup_under_failure depth_ladder quorum_fan_in contention_ledger wake_chain_soak chess_engine_build form_flow_build state_machine_canvas_build chess_play_ladder trend_blog trending_topics_build typescript_chat_service tool_contract_recovery policy_bound_action cross_app_transaction performance_regression browser_cross_site release_train_recovery cross_repo_contract_migration'.split(
    ' ',
  )
const SEQUENCES = [['registry_implementation', 'registry_verification']]
const suite = (
  id,
  label,
  source,
  repetitions,
  technical_retries,
  scenarios,
) => ({
  id,
  label,
  source,
  purpose: '',
  scenarios,
  repetitions,
  technical_retries,
  sha256: `sha256:${id}`,
  updated_at: null,
})
const regressionTests = [
  'persistent_state',
  'tool_contract_recovery',
  'timer_wake',
  'shell_coder_sandbox',
  'database_migration_recovery',
  'contention_ledger',
  'validation_self_repair',
  'context_pressure',
  'prompt_injection_resilience',
]
const suites = [
  suite('regression', 'Regression', 'repository', 1, 1, regressionTests),
  suite('software-engineering', 'Software engineering', 'repository', 1, 0, [
    'registry_implementation',
    'registry_verification',
    'trending_topics_build',
    'linkly_tutorial',
    'alertmanager_route_match',
    'chess_engine_build',
    'form_flow_build',
    'state_machine_canvas_build',
  ]),
  suite('pr', 'PR', 'repository', 1, 0, [
    'minimal_path',
    'persistent_state',
    'tool_contract_recovery',
    'shell_coder_sandbox',
  ]),
  suite('kanban-chain', 'Kanban chain', 'local', 1, 0, [
    'kanban_c1_foundation',
    'kanban_c2_persistence',
    'kanban_c3_board',
    'kanban_c4_ticket_flow',
    'kanban_c5_edit_move',
    'kanban_c6_discussion',
    'kanban_c7_live',
  ]),
]
const workers = (commit = null) =>
  ['harness', 'harness-e2e', 'shell', 'storage', 'llm-router'].map((name) => ({
    name,
    version: null,
    commit: name === 'harness' ? commit : null,
  }))
const stacks = [
  {
    id: 'default',
    label: 'default',
    source: 'repository',
    yaml: 'iii: latest\ncontainers:\n  harness:\n    worker: package://harness\n',
    iii: 'latest',
    template: null,
    containers: workers(),
    warnings: [],
    updated_at: null,
  },
  {
    id: 'harness-template',
    label: 'harness-template',
    source: 'repository',
    yaml: 'iii: latest\ntemplate: harness\ncontainers: {}\n',
    iii: 'latest',
    template: 'harness',
    containers: workers(),
    warnings: [],
    updated_at: null,
  },
  {
    id: 'stack-3f9a1c2e7b40',
    label: 'default · harness pinned',
    source: 'local',
    yaml: 'iii: latest\ncontainers:\n  harness:\n    commit: 3f9a1c2e7b40\n',
    iii: 'latest',
    template: null,
    containers: workers('3f9a1c2e7b40'),
    warnings: [
      'harness pins a commit; it takes effect once the executor runs commit pins.',
      'harness-e2e runs path://../harness-e2e, a path on this machine; the stack runs it only here.',
    ],
    updated_at: '2026-09-24T10:00:00Z',
  },
]
const githubRuns = [
  {
    run_id: 101,
    run_attempt: 1,
    title: 'E2E · aaaa1111-2222',
    created_at: '2026-09-18T10:00:00Z',
    attempt_started_at: '2026-09-18T10:00:00Z',
    conclusion: 'success',
    head_branch: 'main',
    head_sha: '7a16130c9f1e2d3a4b5c6d7e8f9012345678abcd',
    url: 'https://github.com/iii-hq/harness-e2e/actions/runs/101',
    release_control_execution_id: 'aaaa1111-2222',
    execution_id: null,
    execution_state: null,
    contract_pending: true,
  },
  {
    run_id: 102,
    run_attempt: 2,
    title: 'E2E · bbbb3333-4444',
    created_at: '2026-09-20T10:00:00Z',
    attempt_started_at: '2026-09-22T09:00:00Z',
    conclusion: 'failure',
    head_branch: 'feat/executor-image',
    head_sha: 'b406036c9f1e2d3a4b5c6d7e8f9012345678abcd',
    url: 'https://github.com/iii-hq/harness-e2e/actions/runs/102',
    release_control_execution_id: 'bbbb3333-4444',
    execution_id: null,
    execution_state: null,
    contract_pending: true,
  },
]
// The older page: a run this worker imported before.
const olderGithubRun = {
  run_id: 103,
  run_attempt: 1,
  title: 'E2E · cccc5555-6666',
  created_at: '2026-09-10T10:00:00Z',
  attempt_started_at: '2026-09-10T10:00:00Z',
  conclusion: 'success',
  head_branch: 'main',
  head_sha: 'ea1e226c9f1e2d3a4b5c6d7e8f9012345678abcd',
  url: 'https://github.com/iii-hq/harness-e2e/actions/runs/103',
  release_control_execution_id: 'cccc5555-6666',
  suite_label: 'Nightly',
  model: 'gpt-5.6-terra',
  provider: 'openai-codex',
  agent: null,
  runner_version: '0.11.27',
  execution_id: 'plan-gh-103',
  execution_state: 'completed',
}
const githubImported = []
let githubDown = false
// How the worker ended each GitHub import, read on its finished change.
const githubEnded = {}
// Runs a first page leaves out (they went away on GitHub).
let githubGone = []
// A first page held back and answered after a newer one.
let staleGithubList = null
const started = []
const deleted = []
const cancelled = []
/** A started execution, cancelled once its cancel arrived. */
const startedExecution = (id) => {
  const execution = running(id)
  return cancelled.includes(id)
    ? {
        ...execution,
        status: 'cancelled',
        state: 'cancelled',
        plan_execution: { ...execution.plan_execution, state: 'cancelled' },
      }
    : execution
}
let executions = []
let busy = true
let catalogDown = false
let releaseContracts
const contractsRead = new Promise((resolve) => {
  releaseContracts = resolve
})
const server = await createConsoleTestHost()
// A dialog fills its fields after it opens: wait until a field reads what it
// should instead of reading it once.
const settled = async (read, expected, timeout = 10_000) => {
  const deadline = Date.now() + timeout
  let value = await read()
  while (value !== expected && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 50))
    value = await read()
  }
  assert.equal(value, expected)
}

const trigger = async (name, request = {}) => {
  const id = name.replace('e2e::dashboard::', '')
  if (id === 'executions-list') {
    const listed = request.ids?.length
      ? [...executions, imported].filter((e) => request.ids.includes(e.id))
      : executions
    return { executions: listed, total: listed.length }
  }
  if (id === 'execution-get' && githubEnded[request.execution_id])
    return {
      detail: {
        id: request.execution_id,
        plan_execution: githubEnded[request.execution_id],
      },
    }
  if (id === 'execution-get')
    return {
      detail:
        request.execution_id === imported.id
          ? imported
          : request.execution_id === dockered.id
            ? dockered
            : request.execution_id === dockerRunning.id
              ? dockerRunning
              : request.execution_id === nightly
                ? { ...running(nightly), label: 'Nightly' }
                : startedExecution(request.execution_id),
    }
  if (id === 'execution-cancel') {
    cancelled.push(request.execution_id)
    return {}
  }
  if (id === 'catalog-get') {
    if (catalogDown) throw new Error('catalog unavailable: harness restarting')
    return {
      scenarios: TESTS,
      // Alphabetically first, never picked for the user.
      models: [
        { provider: 'claude-code', model: 'claude-code/claude-fable-5' },
        { provider: 'deepseek', model: 'deepseek-v4-flash' },
      ],
      scenario_groups: SEQUENCES,
    }
  }
  if (id === 'execution-start') {
    if (busy) {
      busy = false
      throw new Error(
        `handler error: "Nightly" (${nightly}) is still running; wait for it to finish or cancel it.`,
      )
    }
    started.push(request)
    return { execution_id: `plan-${String(started.length).padStart(32, 'f')}` }
  }
  if (id === 'suites-list') {
    if (catalogDown) throw new Error('catalog unavailable: harness restarting')
    return { suites }
  }
  if (id === 'stacks-list') return { stacks }
  if (id === 'execution-delete') {
    deleted.push(request.execution_id)
    return {}
  }
  if (id === 'github-status-get')
    return {
      ready: !githubDown,
      repository: 'iii-hq/harness-e2e',
      account: githubDown ? null : 'octo',
      message: githubDown
        ? "`gh` is not signed in on the worker's machine. Run `gh auth login` there, then reopen this dialog."
        : null,
    }
  if (id === 'github-runs-list') {
    if (githubDown) throw new Error('gh: HTTP 401: Bad credentials')
    if (staleGithubList) {
      const held = staleGithubList
      staleGithubList = null
      await held.released
      return held.answer
    }
    const first = (request.page ?? 1) === 1
    return {
      repository: 'iii-hq/harness-e2e',
      page: request.page ?? 1,
      runs: first
        ? githubRuns.filter((run) => !githubGone.includes(run.run_id))
        : [olderGithubRun],
      next_page: first ? 2 : null,
      total_count: 3,
    }
  }
  if (id === 'github-run-import') {
    githubImported.push(request.run_id)
    return { execution_id: `plan-gh-${request.run_id}`, state: 'importing' }
  }
  if (id === 'github-run-contracts') {
    await contractsRead
    return {
      runs: request.runs.map(({ run_id, run_attempt }) => ({
        run_id,
        run_attempt,
        suite_label: run_id === 101 ? 'Regression' : 'Software engineering',
        model: 'gpt-5.6-terra',
        provider: 'openai-codex',
        agent: null,
        runner_version: '0.11.28',
      })),
    }
  }
  throw new Error(`Unexpected RPC ${name}`)
}
const browser = await chromium.launch({ headless: true })
try {
  // A short viewport: the scenario list scrolls under the sticky search.
  const page = await browser.newPage({ viewport: { width: 1280, height: 720 } })
  await server.install(page, trigger)
  page.setDefaultTimeout(10_000)
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))

  // An empty ledger offers every way in: run, import.
  await page.goto(`${server.url}#/ext/harness-e2e/executions`)
  await page.getByText('No executions retained yet').waitFor()
  const empty = page.locator('main, body').first()
  for (const action of ['run tests', 'import from GitHub'])
    await empty.getByRole('button', { name: action, exact: true }).waitFor()
  assert.equal(await empty.getByText(/new plan/i).count(), 0)

  // Without an earlier execution Run tests picks no model for the user.
  await empty.getByRole('button', { name: 'run tests', exact: true }).click()
  const fresh = page.getByRole('dialog', { name: 'Run tests' })
  await fresh.getByText('catalog ready').waitFor()
  await fresh
    .getByText('Before running, choose a model and tick at least one test.')
    .waitFor()
  assert.equal(
    await fresh.getByText('The model of your last execution.').count(),
    0,
  )
  await page.keyboard.press('Escape')

  // Import from GitHub while gh is signed out: the dialog says what to run
  // on the worker's machine, and Retry lists the runs once it is fixed.
  githubDown = true
  await empty
    .getByRole('button', { name: 'import from GitHub', exact: true })
    .click()
  const importDialog = page.getByRole('dialog', { name: 'Import from GitHub' })
  const githubError = importDialog.getByRole('alert')
  await githubError.getByText('GitHub didn’t answer').waitFor()
  await githubError.getByText('gh auth login', { exact: true }).waitFor()
  await githubError.getByText('gh: HTTP 401: Bad credentials').waitFor()
  await importDialog.getByText('GitHub unavailable', { exact: true }).waitFor()
  await importDialog.getByText('iii-hq/harness-e2e', { exact: true }).waitFor()
  githubDown = false
  await githubError.getByRole('button', { name: 'Retry' }).click()
  // The runs show at once, oldest creation last, each with its branch, short
  // commit, attempt and Release Control execution; each fills in when its
  // contract is read.
  await importDialog.locator('[data-github-run]').first().waitFor()
  assert.deepEqual(
    await importDialog
      .locator('[data-github-run]')
      .evaluateAll((rows) => rows.map((row) => row.dataset.githubRun)),
    ['102', '101'],
  )
  assert.ok(
    (await importDialog.getByText('Reading the run’s contract').count()) > 0,
  )
  const failedRun = importDialog.locator('[data-github-run="102"]')
  await failedRun.getByText('feat/executor-image', { exact: true }).waitFor()
  await failedRun.getByText('b406036', { exact: true }).waitFor()
  await failedRun.getByText('attempt 2', { exact: true }).waitFor()
  await failedRun.getByText('bbbb3333', { exact: true }).waitFor()
  await failedRun.getByText('Failed', { exact: true }).waitFor()
  await importDialog
    .getByText('2 of 3 runs loaded · iii-hq/harness-e2e', { exact: true })
    .waitFor()
  releaseContracts()
  await importDialog.getByText('Regression', { exact: true }).waitFor()
  assert.equal(
    await importDialog.getByText('Reading the run’s contract').count(),
    0,
  )
  assert.equal(await importDialog.getByText(/runner 0\.11\.28/).count(), 2)
  // The older page holds a run imported before: it links its execution.
  await importDialog
    .getByRole('button', { name: 'Load 1 older run', exact: true })
    .click()
  const importedRun = importDialog.locator('[data-github-run="103"]')
  await importedRun.getByText('Imported', { exact: true }).waitFor()
  assert.equal(
    await importedRun.getByRole('link', { name: 'Open' }).getAttribute('href'),
    '#/ext/harness-e2e/execution/plan-gh-103',
  )
  await importDialog
    .getByText('3 of 3 runs loaded · iii-hq/harness-e2e', { exact: true })
    .waitFor()
  // Several at once: every run shown, less the one imported before.
  await importDialog
    .getByRole('checkbox', { name: 'Select every run shown' })
    .check()
  await importDialog
    .getByText('1 was imported before. Importing again replaces its evidence.')
    .waitFor()
  await importDialog
    .getByRole('checkbox', { name: 'Import run 103 again' })
    .uncheck()
  await importDialog
    .getByRole('button', { name: 'Import 2 runs', exact: true })
    .click()
  // Each row follows its import; the worker goes on in the background.
  await importDialog.getByText('Importing 2 runs in the background').waitFor()
  await settled(
    () => importDialog.getByText('Importing…', { exact: true }).count(),
    2,
  )
  assert.deepEqual(
    githubImported.sort((left, right) => left - right),
    [101, 102],
  )
  assert.equal(
    await importDialog
      .locator('[data-github-run="101"]')
      .getByRole('link', { name: 'Open' })
      .getAttribute('href'),
    '#/ext/harness-e2e/execution/plan-gh-101',
  )
  // The worker ends them: one imported, one failed with why. The failed one
  // does not read as imported and can be ticked to try again.
  githubEnded['plan-gh-101'] = { state: 'completed', error: null }
  githubEnded['plan-gh-102'] = {
    state: 'failed',
    error: 'gh: HTTP 410: artifact expired',
  }
  await page.evaluate(() => {
    for (const execution_id of ['plan-gh-101', 'plan-gh-102'])
      for (const handler of window.__changeHandlers ?? [])
        handler({ kind: 'finished', execution_id })
  })
  const importedNow = importDialog.locator('[data-github-run="101"]')
  const failedNow = importDialog.locator('[data-github-run="102"]')
  await importedNow.getByText('Imported', { exact: true }).waitFor()
  await failedNow.getByText('Import failed', { exact: true }).waitFor()
  await failedNow
    .getByText('The import failed: gh: HTTP 410: artifact expired')
    .waitFor()
  assert.equal(
    await failedNow.getByText('Imported', { exact: true }).count(),
    0,
  )
  await settled(
    () => importDialog.getByText('Importing 2 runs in the background').count(),
    0,
  )
  const retryBox = importDialog.getByRole('checkbox', {
    name: 'Import run 102 again',
  })
  assert.ok(await retryBox.isEnabled())
  // A refresh keeps only the ticked runs it still lists.
  await retryBox.check()
  await importDialog.getByText('1 run selected', { exact: true }).waitFor()
  githubGone = [102]
  await importDialog.getByRole('button', { name: 'Refresh runs' }).click()
  await importDialog.getByText('No runs selected', { exact: true }).waitFor()
  assert.equal(await importDialog.locator('[data-github-run="102"]').count(), 0)
  githubGone = []
  await page.keyboard.press('Escape')

  // An answer of an older load is dropped: the dialog opened, closed and
  // opened again shows the newer list, whatever the first answers late.
  let releaseStale
  staleGithubList = {
    released: new Promise((resolve) => {
      releaseStale = resolve
    }),
    answer: {
      repository: 'iii-hq/harness-e2e',
      page: 1,
      runs: [{ ...githubRuns[0], run_id: 999, title: 'E2E · stale' }],
      next_page: null,
      total_count: 1,
    },
  }
  const openImport = () =>
    empty
      .getByRole('button', { name: 'import from GitHub', exact: true })
      .click()
  await openImport()
  await importDialog
    .getByText('Asking GitHub for completed runs…', { exact: true })
    .waitFor()
  await page.keyboard.press('Escape')
  await openImport()
  await importDialog.locator('[data-github-run="102"]').waitFor()
  releaseStale()
  await page.waitForTimeout(300)
  assert.equal(await importDialog.locator('[data-github-run="999"]').count(), 0)
  await importDialog
    .getByText('2 of 3 runs loaded · iii-hq/harness-e2e', { exact: true })
    .waitFor()
  await page.keyboard.press('Escape')

  // One execution: ticked alone, there is nothing to compare it with.
  executions = [runningSummary]
  await page.reload()
  await page.getByText('1 of 9 runs reported', { exact: true }).waitFor()
  await page.getByRole('checkbox', { name: 'Select Nightly' }).check()
  const selection = page.getByRole('toolbar', { name: 'Selected executions' })
  await selection.getByText('Tick one more to compare.').waitFor()
  assert.ok(
    await selection
      .getByRole('button', { name: 'Compare A and B', exact: true })
      .isDisabled(),
  )
  await selection.getByRole('button', { name: 'Clear selection' }).click()

  // The list: a running row reads its progress; a cancelled one reads as
  // cancelled with how far it got, and its runtime rounds whole.
  executions = [runningSummary, cancelledSummary]
  await page.reload()
  await page.getByText('1 of 9 runs reported', { exact: true }).waitFor()
  assert.equal(await page.getByText(/inconclusive event/).count(), 0)
  const stopped = page.locator(`[data-execution-id="${cancelledSummary.id}"]`)
  await stopped.getByText('Cancelled', { exact: true }).waitFor()
  await stopped.getByText('3 of 9 runs reported', { exact: true }).waitFor()
  await stopped.getByText('2m 00s', { exact: true }).waitFor()
  assert.equal(await page.getByText(/infrastructure event/).count(), 0)
  assert.equal(await page.getByText('1m 60s').count(), 0)
  // A running row's menu cancels it, and says why it cannot be deleted yet.
  await page
    .locator(`[data-execution-id="${nightly}"]`)
    .getByRole('button', { name: /^Actions for / })
    .click()
  const rowMenu = page.getByRole('menu')
  await rowMenu.getByRole('menuitem', { name: 'Cancel execution' }).waitFor()
  assert.equal(
    await rowMenu
      .getByRole('menuitem', { name: /^Delete…/ })
      .getAttribute('aria-disabled'),
    'true',
  )
  await rowMenu.getByText('Finish or cancel it first').waitFor()
  await rowMenu.getByRole('menuitem', { name: 'Cancel execution' }).click()
  for (let tries = 0; !cancelled.length && tries < 50; tries += 1)
    await page.waitForTimeout(100)
  assert.deepEqual(cancelled, [nightly])

  // Run tests: it starts from the last execution's model; a sequential group
  // ticks whole before running; the box, its name and Space all toggle a
  // test; the form has no seed; a busy runner is named in the footer; the
  // next submit starts an execution and follows it on its page.
  await page
    .getByRole('button', { name: 'Run tests', exact: true })
    .first()
    .click()
  const runTests = page.getByRole('dialog', { name: 'Run tests' })
  await runTests.getByText('catalog ready').waitFor()
  assert.equal(await runTests.getByText('Harness endpoint').count(), 0)
  await runTests.getByText('The model of your last execution.').waitFor()
  await settled(
    () => runTests.getByLabel('Model').inputValue(),
    'deepseek::deepseek-v4-flash',
  )
  await runTests
    .getByText('0 tests · 0 runs', { exact: false })
    .first()
    .waitFor()
  const box = (name) => runTests.getByRole('checkbox', { name, exact: true })
  await box('registry_verification').click()
  assert.ok(await box('registry_implementation').isChecked())
  await runTests
    .getByText('2 tests · 2 runs', { exact: false })
    .first()
    .waitFor()
  await runTests.getByText('2 of 2 · in order', { exact: true }).waitFor()
  await box('registry_verification').click()
  assert.ok(!(await box('registry_implementation').isChecked()))
  await runTests
    .locator('label', { hasText: 'trend_blog' })
    .locator('input[type=checkbox]')
    .click()
  assert.ok(await box('trend_blog').isChecked())
  await runTests.getByText('trend_blog', { exact: true }).click()
  assert.ok(!(await box('trend_blog').isChecked()))
  await box('context_pressure').focus()
  await page.keyboard.press('Space')
  assert.ok(await box('context_pressure').isChecked())
  // Every execution runs the canonical cases, so runs pair up in comparisons.
  assert.doesNotMatch(await runTests.textContent(), /seed/i)
  const submit = runTests.getByRole('button', {
    name: 'Run 1 test',
    exact: true,
  })
  await submit.click()
  // A busy runner names what runs, by its title, and offers to open it.
  await runTests
    .getByText('“Nightly” is still running on this harness.', { exact: false })
    .waitFor()
  assert.equal(await runTests.getByText(/handler error/).count(), 0)
  assert.ok(
    (
      await runTests
        .getByRole('link', { name: 'Open', exact: true })
        .getAttribute('href')
    ).includes(nightly),
  )
  assert.equal(started.length, 0)
  await submit.click()
  await page.waitForFunction(() => location.hash.includes('/execution/plan-f'))
  assert.deepEqual(started[0], {
    label: '',
    parameters: {
      // Ticked by hand: an unnamed suite.
      suite: null,
      scenarios: ['context_pressure'],
      runs: 1,
      technical_retries: 1,
      model: 'deepseek-v4-flash',
      provider: 'deepseek',
      agent: null,
      where: 'harness',
    },
  })
  // No plan, no role: just an execution.
  await page.getByText('Execution · running', { exact: true }).waitFor()
  // Cancel asks first, says what stops, then stops it: no next scenario.
  await page
    .locator('[data-where-line]')
    .getByText('Running · on this harness', { exact: false })
    .waitFor()
  await page.getByRole('button', { name: 'Cancel', exact: true }).click()
  const confirmCancel = page.getByRole('dialog', {
    name: 'Cancel this execution?',
  })
  await confirmCancel
    .getByText('The test running now stops. What already reported stays.')
    .waitFor()
  await confirmCancel
    .getByRole('button', { name: 'Cancel execution', exact: true })
    .click()
  await confirmCancel.waitFor({ state: 'hidden' })
  assert.deepEqual(cancelled, [nightly, `plan-${'1'.padStart(32, 'f')}`])
  await page.getByText('Execution · running').waitFor({ state: 'detached' })

  // Run again: the header names what it ran on; the form opens on the tests
  // that will run, under the execution's name, and sends its parameters
  // unchanged even when the catalog cannot be read.
  catalogDown = true
  await page.goto(`${server.url}#/ext/harness-e2e/execution/${imported.id}`)
  const band = page.locator('[data-execution-facts]')
  await band.getByText('1.8.8', { exact: true }).waitFor()
  await band.getByText('0.11.28', { exact: true }).waitFor()
  // Its suite, by name and digest, and the stack its contract names.
  await band
    .getByText('Software engineering 2025 · 0123456789ab', { exact: true })
    .waitFor()
  await band
    .locator('.ep-fact', {
      has: page.locator('.ep-fact-label', { hasText: /^Stack$/ }),
    })
    .getByText(/^default( · [0-9a-f]{12})?$/)
    .waitFor()
  await page.getByRole('button', { name: 'Run again', exact: true }).click()
  const again = page.getByRole('dialog', { name: 'Run again' })
  await again
    .getByRole('status')
    .filter({ hasText: 'Catalog unavailable' })
    .waitFor()
  await settled(
    () => again.locator('#run-dialog-label').inputValue(),
    'Software engineering',
  )
  // Its suite, as recorded, even though this runner does not list it.
  await settled(
    () => again.locator('#run-dialog-suite').getAttribute('data-value'),
    'recorded:software-engineering-2025',
  )
  await again
    .locator('#run-dialog-suite')
    .getByText('Software engineering 2025 · as recorded')
    .waitFor()
  assert.equal(await again.locator('#run-dialog-runs-value').innerText(), '2')
  assert.equal(
    await again.locator('#run-dialog-technicalRetries-value').innerText(),
    '0',
  )
  await settled(
    () => again.locator('#run-dialog-agent').inputValue(),
    'tech-lead',
  )
  // The catalog has to load before running: Run waits for it, then sends the
  // execution's parameters unchanged.
  const runAgain = again.getByRole('button', {
    name: 'Run 2 tests',
    exact: true,
  })
  assert.ok(await runAgain.isDisabled())
  catalogDown = false
  await again.getByRole('button', { name: 'Refresh catalog' }).click()
  await again.getByText('catalog ready').waitFor()
  for (const scenario of ['minimal_path', 'retired_scenario'])
    assert.ok(
      await again
        .getByRole('checkbox', { name: scenario, exact: true })
        .isChecked(),
    )
  await runAgain.click()
  await page.waitForFunction(() => !location.hash.includes('0123456789abcdef'))
  // Its parameters unchanged, under its suite; the runner records the digest.
  assert.deepEqual(started[1], {
    label: 'Software engineering',
    parameters: {
      ...imported.plan_execution.parameters,
      suite: {
        id: 'software-engineering-2025',
        label: 'Software engineering 2025',
      },
      // It recorded no stack: it runs on this harness.
      where: 'harness',
    },
  })

  // With the catalog read, the form still opens on the tests that will run.
  catalogDown = false
  await page.goto(`${server.url}#/ext/harness-e2e/execution/${imported.id}`)
  await page.getByRole('button', { name: 'Run again', exact: true }).click()
  await again.getByText('catalog ready').waitFor()
  await again.getByText(`2 of ${TESTS.length + 1}`, { exact: true }).waitFor()
  assert.equal(
    await again.getByRole('checkbox', { name: 'trend_blog' }).count(),
    0,
  )
  await page.keyboard.press('Escape')

  // In Docker: Where asks for a stack, the repository's default first, and
  // the executor receives its YAML.
  await page.goto(`${server.url}#/ext/harness-e2e/executions`)
  await page
    .getByRole('button', { name: 'Run tests', exact: true })
    .first()
    .click()
  await runTests.getByText('catalog ready').waitFor()
  assert.equal(await runTests.locator('#run-dialog-stack').count(), 0)
  await runTests.getByRole('radio', { name: 'Docker' }).click()
  await settled(
    () => runTests.locator('#run-dialog-stack').getAttribute('data-value'),
    'default',
  )
  await runTests.locator('#run-dialog-stack').click()
  await runTests
    .getByRole('option', { name: /^default · harness pinned/ })
    .waitFor()
  await page.keyboard.press('Escape')
  await box('minimal_path').click()
  await runTests
    .getByRole('button', { name: 'Run 1 test in Docker', exact: true })
    .click()
  await page.waitForFunction(() => /\/execution\/plan-f+3$/.test(location.hash))
  assert.equal(started[2].parameters.where, 'docker')
  assert.deepEqual(started[2].parameters.stack, {
    name: 'default',
    yaml: stacks[0].yaml,
  })

  // A Docker execution that runs: its groups and where each is.
  await page.goto(
    `${server.url}#/ext/harness-e2e/execution/${dockerRunning.id}`,
  )
  const groups = page.locator('[data-docker-groups]')
  await groups.locator('[data-docker-group]').first().waitFor()
  await page
    .locator('[data-step-state="current"]')
    .getByText('Groups', { exact: true })
    .waitFor()
  await groups
    .locator('[data-docker-group="case-persistent-state"] [data-group-state]')
    .getByText('running', { exact: true })
    .waitFor()
  await page
    .locator('[data-step-state="current"]')
    .getByText('1 of 3 finished · 1 running · 1 waiting', { exact: true })
    .waitFor()
  // The group that ended reports its test at once; the others fill in.
  await page
    .locator('[data-where-line]')
    .getByText('1 of 3 tests reported · results are provisional')
    .waitFor()
  const result = (scenario) =>
    page
      .getByRole('table', { name: 'Scenario results' })
      .locator('tr.ep-result-row', { hasText: scenario })
  await result('minimal_path').getByText('Passed', { exact: true }).waitFor()
  await result('minimal_path').getByText('100', { exact: true }).waitFor()
  await result('persistent_state')
    .getByText('Running in its container')
    .waitFor()
  await result('shell_coder_sandbox')
    .getByText('Waiting for a slot', { exact: true })
    .waitFor()
  assert.equal(
    await result('shell_coder_sandbox').getAttribute('data-row-state'),
    'queued',
  )
  await page.getByText('A group’s tests fill in as it finishes.').waitFor()

  // A finished one says where it ran and runs again there, on its stack as
  // recorded.
  await page.goto(`${server.url}#/ext/harness-e2e/execution/${dockered.id}`)
  await band.getByText('Docker · attempt 2', { exact: true }).waitFor()
  await band
    .locator('.ep-fact', {
      has: page.locator('.ep-fact-label', { hasText: /^Stack$/ }),
    })
    .getByText(/^default( · [0-9a-f]{12})?$/)
    .waitFor()
  await page.getByRole('button', { name: 'Run again', exact: true }).click()
  await again.getByText('catalog ready').waitFor()
  assert.equal(
    await again
      .getByRole('radio', { name: 'Docker' })
      .getAttribute('aria-checked'),
    'true',
  )
  await settled(
    () => again.locator('#run-dialog-stack').getAttribute('data-value'),
    'recorded',
  )
  await again
    .locator('#run-dialog-stack')
    .getByText('default · as recorded')
    .waitFor()
  await again
    .getByText('As this execution recorded it · feedfacefeed', { exact: false })
    .waitFor()
  await again
    .getByRole('button', { name: 'Run 2 tests in Docker', exact: true })
    .click()
  await page.waitForFunction(() => !location.hash.includes('44444444'))
  assert.deepEqual(started[3].parameters.stack, {
    name: 'default',
    yaml: recordedStack,
  })
  assert.equal(started[3].parameters.where, 'docker')

  // A finished execution can be deleted, from the ⋯ menu, after a confirm.
  await page.goto(`${server.url}#/ext/harness-e2e/execution/${imported.id}`)
  await page.getByRole('button', { name: 'More actions', exact: true }).click()
  await page.getByRole('menuitem', { name: 'Delete…' }).click()
  await page
    .getByRole('button', { name: 'delete execution', exact: true })
    .click()
  await page.waitForFunction(() => location.hash.endsWith('/executions'))
  assert.deepEqual(deleted, [imported.id])
  assert.deepEqual(errors, [])
  console.log(
    'Run tests, Run again and GitHub import browser flow passed: empty ledger, no model picked without history, GitHub import that says to run gh auth login and retries, rows with branch, short commit, attempt and Release Control, contracts read per row, an older page with a run imported before, several runs imported at once with per-row progress, an import the worker ended as Imported or Failed, a refresh that keeps only listed runs ticked, an older load’s late answer dropped, progress, cancelled row and whole runtime, cancel from the menu of a running row, last model by default, sequential group ticked whole, box/label/Space toggles, no seed, busy runner named with a link, start and follow, cancel, suite, stack and versions in the header, Run again under the recorded suite, selected-first prefill without a catalog, Docker with a stack, Docker groups while running with the ended group’s results in, Run again in Docker on the stack as recorded, delete.',
  )
} finally {
  await browser.close()
  await server.close()
}
