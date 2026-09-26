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
const stacks = [
  {
    id: 'default',
    label: 'default',
    source: 'repository',
    yaml: 'iii: latest\ncontainers:\n  harness:\n    worker: package://harness\n',
    iii: 'latest',
    template: null,
    containers: [{ name: 'harness', version: null, commit: null }],
    warnings: [],
    updated_at: null,
  },
  {
    id: 'stack-0123456789ab',
    label: 'Pinned harness',
    source: 'local',
    yaml: 'iii: 0.24.1\ncontainers: {}\n',
    iii: '0.24.1',
    template: null,
    containers: [],
    warnings: [],
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
      scenarios: [
        'minimal_path',
        'context_pressure',
        'trend_blog',
        'registry_implementation',
        'registry_verification',
      ],
      // Alphabetically first, never picked for the user.
      models: [
        { provider: 'claude-code', model: 'claude-code/claude-fable-5' },
        { provider: 'deepseek', model: 'deepseek-v4-flash' },
      ],
      scenario_groups: [['registry_implementation', 'registry_verification']],
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
    return { suites: [] }
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
  await fresh.getByText('catalog ready').waitFor()
  await fresh
    .getByText('Before running, choose a model and tick at least one test.')
    .waitFor()
  assert.equal(
    await fresh.getByText('The model of your last execution.').count(),
    0,
  )
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

  // One execution: nothing to compare it with, so no hint, button or column.
  executions = [runningSummary]
  await page.reload()
  await page.getByText('1 of 9 done', { exact: true }).waitFor()
  const compareHint = page.getByText('tick two executions to compare')
  assert.equal(await compareHint.count(), 0)
  assert.equal(
    await page.getByRole('button', { name: 'compare', exact: true }).count(),
    0,
  )
  assert.equal(
    await page.locator('[data-ledger] input[type=checkbox]').count(),
    0,
  )

  // The ledger: a running row reads its progress; a cancelled one reads as
  // cancelled with how far it got, and its runtime rounds whole. Two rows:
  // now they can be compared.
  executions = [runningSummary, cancelledSummary]
  await page.reload()
  await page.getByText('1 of 9 done', { exact: true }).waitFor()
  await compareHint.waitFor()
  assert.equal(await page.getByText(/inconclusive event/).count(), 0)
  const stopped = page.locator(`[data-execution-id="${cancelledSummary.id}"]`)
  await stopped.getByText('cancelled', { exact: true }).waitFor()
  await stopped.getByText('3 of 9 done', { exact: true }).waitFor()
  await stopped.getByText('2m 00s', { exact: true }).waitFor()
  assert.equal(await page.getByText(/infrastructure event/).count(), 0)
  assert.equal(await page.getByText('1m 60s').count(), 0)

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
  assert.equal(
    await runTests.getByLabel('Model').inputValue(),
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
  await again
    .getByRole('status')
    .filter({ hasText: 'Catalog unavailable' })
    .waitFor()
  assert.equal(
    await again.locator('#run-dialog-label').inputValue(),
    'Software engineering',
  )
  // Its suite, as recorded, even though this runner does not list it.
  assert.equal(
    await again.locator('#run-dialog-suite').getAttribute('data-value'),
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
  assert.equal(
    await again.locator('#run-dialog-agent').inputValue(),
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
  await page.getByRole('button', { name: 'run again', exact: true }).click()
  await again.getByText('catalog ready').waitFor()
  await again.getByText('2 of 6', { exact: true }).waitFor()
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
  assert.equal(
    await runTests.locator('#run-dialog-stack').getAttribute('data-value'),
    'default',
  )
  await runTests.locator('#run-dialog-stack').click()
  await runTests.getByRole('option', { name: /^Pinned harness/ }).waitFor()
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

  // A finished one says where it ran and runs again there, on its stack as
  // recorded.
  await page.goto(`${server.url}#/ext/harness-e2e/execution/${dockered.id}`)
  await band.getByText('Docker · attempt 2', { exact: true }).waitFor()
  await band.getByText('default', { exact: true }).waitFor()
  await page.getByRole('button', { name: 'run again', exact: true }).click()
  await again.getByText('catalog ready').waitFor()
  assert.equal(
    await again
      .getByRole('radio', { name: 'Docker' })
      .getAttribute('aria-checked'),
    'true',
  )
  assert.equal(
    await again.locator('#run-dialog-stack').getAttribute('data-value'),
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
    'Run tests, Run again and GitHub import browser flow passed: empty ledger, no model picked without history, quick list with contracts read per row, progress, cancelled row and whole runtime, last model by default, sequential group ticked whole, box/label/Space toggles, no seed, busy runner named with a link, start and follow, cancel, suite, stack and versions in the header, Run again under the recorded suite, selected-first prefill without a catalog, Docker with a stack, Docker groups while running, Run again in Docker on the stack as recorded, delete.',
  )
} finally {
  await browser.close()
  await server.close()
}
