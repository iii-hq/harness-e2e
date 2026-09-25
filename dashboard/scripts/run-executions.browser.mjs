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
const dockerGroup = (group_id, state) => ({
  round: 1,
  campaign_id: 'pr-r01',
  group_id,
  scenarios: [group_id],
  state,
  attempt: 1,
  error: null,
})
/** Running in Docker: its groups, and where each is. */
const dockerRunning = {
  ...running('plan-55555555555555555555555555555555'),
  plan_execution: {
    ...running('plan-55555555555555555555555555555555').plan_execution,
    source: {
      kind: 'docker',
      attempt: 1,
      phase: 'groups',
      image: null,
      groups: [
        dockerGroup('case-minimal-path', 'done'),
        dockerGroup('case-persistent-state', 'running'),
        dockerGroup('case-shell-coder-sandbox', 'queued'),
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
    url: 'https://github.com/iii-hq/harness-e2e/actions/runs/102',
    release_control_execution_id: 'bbbb3333-4444',
    execution_id: null,
    execution_state: null,
    contract_pending: true,
  },
]
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
const trigger = async (name, request = {}) => {
  const id = name.replace('e2e::dashboard::', '')
  if (id === 'executions-list') {
    const listed = request.ids?.length
      ? [...executions, imported].filter((e) => request.ids.includes(e.id))
      : executions
    return { executions: listed, total: listed.length }
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
  if (id === 'github-runs-list')
    return {
      repository: 'iii-hq/harness-e2e',
      page: 1,
      runs: githubRuns,
      next_page: null,
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
  await fresh
    .getByText(`Catalog ready · ${TESTS.length} tests · 2 models`)
    .waitFor()
  await fresh.getByText('0 tests · 0 runs', { exact: true }).waitFor()
  assert.equal(
    await fresh.getByText('The model of your last execution.').count(),
    0,
  )
  // The button stays off and says what is missing.
  await fresh
    .getByText('Before running, choose a model and tick at least one test.')
    .waitFor()
  assert.ok(
    await fresh
      .getByRole('button', { name: 'Run tests', exact: true })
      .isDisabled(),
  )
  // Families are blocks with a box and a count; sequences say their order.
  const registry = fresh.getByRole('group', { name: 'registry' })
  await registry.getByText('4', { exact: true }).waitFor()
  await registry.getByText('1 of 2 · in order').waitFor()
  await fresh.getByRole('group', { name: 'Standalone' }).waitFor()
  // A suite ticks its tests and names itself; a change makes it custom, and
  // Reset puts it back.
  await fresh.locator('#run-tests-suite').click()
  await fresh.getByRole('option', { name: 'Regression', exact: true }).click()
  await fresh
    .getByText('Repository suite · 1 run per test · 1 retry', { exact: true })
    .waitFor()
  await fresh.getByText('9 selected', { exact: true }).waitFor()
  await fresh.getByRole('checkbox', { name: 'timer_wake', exact: true }).click()
  await fresh
    .getByText('Changed from Regression. Runs as a custom selection.')
    .waitFor()
  assert.equal(await fresh.locator('#run-tests-suite').textContent(), 'Custom')
  await fresh.getByRole('button', { name: 'Reset', exact: true }).click()
  await fresh.getByText('9 selected', { exact: true }).waitFor()
  assert.equal(
    await fresh.locator('#run-tests-suite').textContent(),
    'Regression',
  )
  // Filtered to what is ticked; the family box ticks what it shows.
  await fresh.getByRole('radio', { name: /^Selected/ }).click()
  await fresh.getByText(`9 of ${TESTS.length}`, { exact: true }).waitFor()
  await fresh.getByRole('button', { name: 'Clear', exact: true }).click()
  await fresh.getByText('No tests ticked yet.').waitFor()
  await fresh.getByRole('button', { name: 'Show all tests' }).click()
  await fresh
    .getByRole('checkbox', { name: 'Select every test in kanban' })
    .click()
  await fresh.getByText('7 selected', { exact: true }).waitFor()
  await fresh.getByRole('searchbox', { name: 'Filter tests' }).fill('zzz')
  await fresh.getByText('No tests match “zzz”.').waitFor()
  await fresh.getByRole('button', { name: 'Clear filter' }).click()
  await fresh.getByRole('button', { name: 'Clear', exact: true }).click()
  await page.keyboard.press('Escape')

  // Import from GitHub: the runs show at once, oldest creation last, and
  // each row fills in when its contract is read.
  await empty
    .getByRole('button', { name: 'import from GitHub', exact: true })
    .click()
  const importDialog = page.getByRole('dialog', { name: 'Import from GitHub' })
  await importDialog.locator('[data-github-run]').first().waitFor()
  assert.deepEqual(
    await importDialog
      .locator('[data-github-run]')
      .evaluateAll((rows) => rows.map((row) => row.dataset.githubRun)),
    ['102', '101'],
  )
  assert.ok((await importDialog.getByText('reading…').count()) > 0)
  await importDialog
    .getByText('attempt 2 · Sep 22, 2026', { exact: false })
    .waitFor()
  await importDialog.getByText('RC bbbb3333', { exact: true }).waitFor()
  releaseContracts()
  await importDialog.getByText('Regression', { exact: true }).waitFor()
  assert.equal(await importDialog.getByText('reading…').count(), 0)
  assert.equal(await importDialog.getByText('0.11.28').count(), 2)
  await page.keyboard.press('Escape')

  // One execution: ticked alone, there is nothing to compare it with.
  executions = [runningSummary]
  await page.reload()
  await page.getByText('1 of 9 runs reported', { exact: true }).waitFor()
  await page
    .getByRole('checkbox', { name: /^Select gpt-5\.6-terra · / })
    .check()
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
  await page.keyboard.press('Escape')

  // Run tests: it starts from the last execution's model; this harness is
  // busy, which the footer says with a way to open what runs; a sequential
  // group ticks whole; the box, its name and Space all toggle a test; the
  // form has no seed; the next submit starts an execution and follows it on
  // its page.
  await page
    .getByRole('button', { name: 'Run tests', exact: true })
    .first()
    .click()
  const runTests = page.getByRole('dialog', { name: 'Run tests' })
  await runTests.getByText('Catalog ready', { exact: false }).waitFor()
  assert.equal(await runTests.getByText('Harness endpoint').count(), 0)
  await runTests.getByText('The model of your last execution.').waitFor()
  assert.equal(
    await runTests.locator('#run-tests-model').textContent(),
    'deepseek / deepseek-v4-flash',
  )
  const busyAlert = runTests.getByRole('alert').filter({
    hasText: '“Nightly” is still running on this harness.',
  })
  await busyAlert.waitFor()
  assert.ok(
    (
      await busyAlert
        .getByRole('link', { name: 'Open Nightly', exact: true })
        .getAttribute('href')
    ).includes(nightly),
  )
  // A test in a sequence also says where it runs in it ("2 of 2 · in order").
  const box = (name) =>
    runTests.getByRole('checkbox', { name: new RegExp(`^${name}(\\s|$)`) })
  await box('registry_verification').click()
  assert.ok(await box('registry_implementation').isChecked())
  await runTests
    .getByRole('button', { name: 'Run 2 tests', exact: true })
    .waitFor()
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
  // The steppers keep runs and retries within their range.
  const runsStepper = runTests.getByRole('group', { name: 'Runs per test' })
  assert.ok(
    await runsStepper.getByRole('button', { name: 'Fewer runs' }).isDisabled(),
  )
  const submit = runTests.getByRole('button', {
    name: 'Run 1 test',
    exact: true,
  })
  await submit.click()
  // The runner said it is busy: the footer still names what runs, by its
  // title, never the raw handler error.
  await busyAlert.waitFor()
  assert.equal(await runTests.getByText(/handler error/).count(), 0)
  assert.equal(started.length, 0)
  // Run in Docker drops the refusal: the footer sums up Docker.
  await busyAlert.getByRole('button', { name: 'Run in Docker' }).click()
  await runTests
    .getByText(
      '1 run per test · 1 retry · custom selection · in Docker on default',
    )
    .waitFor()
  assert.equal(await runTests.getByText(/still running/).count(), 0)
  await runTests.getByRole('radio', { name: 'This harness' }).click()
  await busyAlert.waitFor()
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
  // Cancel stops it: no next scenario is admitted.
  const cancel = page.getByRole('button', { name: 'cancel execution' })
  await cancel.click()
  await cancel.waitFor({ state: 'detached' })
  assert.deepEqual(cancelled, [`plan-${'1'.padStart(32, 'f')}`])
  await page.getByText('Execution · running').waitFor({ state: 'detached' })

  // Run again: the header names what it ran on; the form opens on the tests
  // that will run, under the execution's name, and sends its parameters
  // unchanged even when the catalog cannot be read.
  catalogDown = true
  await page.goto(`${server.url}#/ext/harness-e2e/execution/${imported.id}`)
  const band = page.locator('[data-identity-band]')
  await band.getByText('1.8.8', { exact: true }).waitFor()
  await band.getByText('0.11.28', { exact: true }).waitFor()
  // Its suite, by name and digest, and the stack its contract names.
  await band
    .getByText('Software engineering 2025 · 0123456789ab', { exact: true })
    .waitFor()
  await band.getByText('default', { exact: true }).waitFor()
  await page.getByRole('button', { name: 'run again', exact: true }).click()
  const again = page.getByRole('dialog', { name: 'Run again' })
  await again.getByText('Couldn’t load the test catalog').waitFor()
  await again.getByText('catalog unavailable: harness restarting').waitFor()
  await again.getByText('Catalog unavailable', { exact: true }).waitFor()
  assert.equal(
    await again.locator('#run-tests-label').inputValue(),
    'Software engineering',
  )
  // Its suite, as recorded, even though this runner does not list it.
  assert.equal(
    await again.locator('#run-tests-suite').textContent(),
    'Software engineering 2025 · as recorded',
  )
  await again
    .getByText('As this execution ran · 2 runs per test · 0 retries')
    .waitFor()
  await again.getByText('The model this execution ran with.').waitFor()
  for (const scenario of ['minimal_path', 'retired_scenario'])
    assert.ok(
      await again
        .getByRole('checkbox', { name: new RegExp(`^${scenario}(\\s|$)`) })
        .isChecked(),
    )
  const output = (name) =>
    again.getByRole('group', { name }).locator('output').textContent()
  assert.equal(await output('Runs per test'), '2')
  assert.equal(await output('Retries on crash'), '0')
  assert.equal(
    await again.locator('#run-tests-agent').inputValue(),
    'tech-lead',
  )
  await again.getByRole('button', { name: 'Run 2 tests', exact: true }).click()
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
  await page.getByRole('button', { name: 'run again', exact: true }).click()
  await again.getByText('Catalog ready', { exact: false }).waitFor()
  await again.getByText(`2 of ${TESTS.length + 1}`, { exact: true }).waitFor()
  assert.equal(
    await again.getByRole('checkbox', { name: 'trend_blog' }).count(),
    0,
  )
  await page.keyboard.press('Escape')

  // In Docker: Where asks for a stack, the repository's default first, and
  // the executor receives its YAML. Docker does not wait for this harness.
  await page.goto(`${server.url}#/ext/harness-e2e/executions`)
  await page
    .getByRole('button', { name: 'Run tests', exact: true })
    .first()
    .click()
  await runTests.getByText('Catalog ready', { exact: false }).waitFor()
  assert.equal(await runTests.locator('#run-tests-stack').count(), 0)
  await busyAlert.getByRole('button', { name: 'Run in Docker' }).click()
  assert.equal(
    await runTests
      .getByRole('radio', { name: 'Docker', exact: true })
      .getAttribute('aria-checked'),
    'true',
  )
  assert.equal(await busyAlert.count(), 0)
  assert.equal(
    await runTests.locator('#run-tests-stack').textContent(),
    'default',
  )
  await runTests
    .getByText(
      'iii latest · 5 workers. Its YAML is what the executor assembles.',
    )
    .waitFor()
  // Its warnings show under the field once picked.
  await runTests.locator('#run-tests-stack').click()
  await runTests
    .getByRole('group', { name: 'This Console' })
    .getByRole('option', { name: 'default · harness pinned' })
    .click()
  await runTests
    .getByText('harness pins a commit; it takes effect', { exact: false })
    .waitFor()
  await runTests.locator('#run-tests-stack').click()
  await runTests.getByRole('option', { name: 'default', exact: true }).click()
  assert.equal(
    await runTests.getByText('harness pins a commit', { exact: false }).count(),
    0,
  )
  await box('minimal_path').click()
  await runTests
    .getByText(
      '1 run per test · 1 retry · custom selection · in Docker on default',
    )
    .waitFor()
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
  await groups.getByText('Running the groups…').waitFor()
  await groups
    .locator('[data-docker-group="case-persistent-state"] [data-group-state]')
    .getByText('running', { exact: true })
    .waitFor()

  // A finished one says where it ran and runs again there, on its stack as
  // recorded.
  await page.goto(`${server.url}#/ext/harness-e2e/execution/${dockered.id}`)
  await band.getByText('Docker · attempt 2', { exact: true }).waitFor()
  await band.getByText('default', { exact: true }).waitFor()
  await page.getByRole('button', { name: 'run again', exact: true }).click()
  await again.getByText('Catalog ready', { exact: false }).waitFor()
  assert.equal(
    await again
      .getByRole('radio', { name: 'Docker', exact: true })
      .getAttribute('aria-checked'),
    'true',
  )
  assert.equal(
    await again.locator('#run-tests-stack').textContent(),
    'default · as recorded',
  )
  await again
    .getByText('As this execution recorded it · feedfacefeed')
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

  // On a phone the dialog is one column: the tests follow the fields, at
  // the height of their content, and can be ticked.
  await page.goto(`${server.url}#/ext/harness-e2e/executions`)
  await page
    .getByRole('button', { name: 'Run tests', exact: true })
    .first()
    .click()
  await runTests.getByText('Catalog ready', { exact: false }).waitFor()
  await page.setViewportSize({ width: 390, height: 844 })
  // The host fixes the dialog to the viewport (a bottom sheet here); the
  // double renders it in the flow, so pin it as the host does.
  await runTests.evaluate((dialog) => {
    dialog.style.position = 'fixed'
  })
  const phoneList = runTests.getByRole('region', { name: 'Tests' })
  assert.ok((await phoneList.boundingBox()).height > 300)
  const phoneBox = box('timer_wake')
  await phoneBox.scrollIntoViewIfNeeded()
  assert.ok(await phoneBox.isVisible())
  await phoneBox.click()
  assert.ok(await phoneBox.isChecked())
  await runTests
    .getByRole('button', { name: 'Run 1 test', exact: true })
    .waitFor()
  await page.keyboard.press('Escape')
  await page.setViewportSize({ width: 1280, height: 720 })

  // A finished execution can be deleted.
  await page.goto(`${server.url}#/ext/harness-e2e/execution/${imported.id}`)
  await page
    .getByRole('button', { name: 'Delete execution', exact: true })
    .click()
  await page
    .getByRole('button', { name: 'delete execution', exact: true })
    .click()
  await page.waitForFunction(() => location.hash.endsWith('/executions'))
  assert.deepEqual(deleted, [imported.id])
  assert.deepEqual(errors, [])
  console.log(
    'Run tests, Run again and GitHub import browser flow passed: empty ledger, no model picked without history and the button off with the reason, family blocks and sequences, suite custom and reset, filters and clear, quick list with contracts read per row, progress, cancelled row and whole runtime, the menu of a running row, last model by default, busy harness named with a link before and after a submit, sequential group ticked whole, box/label/Space toggles, no seed, start and follow, cancel, suite, stack and versions in the header, Run again under the recorded suite without a catalog, selected-first prefill, Run in Docker from the busy alert, stacks with warnings, Docker groups while running, Run again in Docker on the stack as recorded, delete.',
  )
} finally {
  await browser.close()
  await server.close()
}
