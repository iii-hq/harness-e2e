// Deterministic browser coverage for running one scenario of an execution
// again. No models run.
import assert from 'node:assert/strict'
import { chromium } from 'playwright'
import { createConsoleTestHost } from './console-test-host.mjs'

function run(id, { score = 80, technical = 'valid', failure = null } = {}) {
  return {
    run_id: id,
    attempt_id: `${id}-attempt`,
    status: technical === 'valid' ? 'passed' : 'infrastructure_error',
    completion: technical === 'valid' ? 'completed' : 'undetermined',
    technical,
    score,
    wall_time_ms: 2000,
    efficiency: { total_tokens: 100, root_turns: 2, function_calls: 3 },
    metrics: { complete: true, totals: { cache_read_tokens: 10 } },
    cost: { subject_usd: 0.01 },
    criteria: [],
    failures: failure ? [{ phase: 'setup', message: failure }] : [],
  }
}

/** One native run's report for one slot, as execution-get carries it. */
function report(scenario, native, runValue) {
  const valid = runValue.technical === 'valid'
  return {
    subject_id: 'flash',
    scenario_id: scenario,
    native_execution_id: native,
    round: 1,
    available: true,
    report: {
      result_contract_sha256: 'sha256:contract',
      report_state: 'complete',
      objective_outcome: valid ? 'passed' : 'inconclusive',
      scenarios: [
        {
          scenario_id: scenario,
          passed: valid,
          case: { seed: 1, inputs_sha256: `inputs-${scenario}` },
          aggregate: {
            planned_runs: 1,
            observed_runs: 1,
            deferred_runs: 0,
            completed_runs: valid ? 1 : 0,
            task_incomplete_runs: 0,
            undetermined_runs: valid ? 0 : 1,
            technical_valid_runs: valid ? 1 : 0,
            technical_invalid_runs: valid ? 0 : 1,
            scored_runs: valid ? 1 : 0,
            technical_failures: valid ? 0 : 1,
            execution_reliability: null,
            completion_evidence_coverage: null,
            completion_rate: null,
            mean_score: runValue.score,
            total_tokens_consumed: 100,
            tokens_completed_p50: null,
            failed_attempt_tokens: null,
            tokens_per_completion: null,
          },
          runs: [runValue],
        },
      ],
    },
  }
}

const slot = (scenario_id, execution_id, previous = []) => ({
  round: 1,
  group_id: scenario_id,
  scenario_id,
  execution_id,
  state: 'finished',
  observed: 1,
  completed: 1,
  passed: 1,
  technical_valid: 1,
  result_path: null,
  error: null,
  previous_attempts: previous,
})

const localId = 'plan-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
const importedId = 'plan-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'
const nightly = 'plan-22222222222222222222222222222222'
const parameters = {
  scenarios: [
    'minimal_path',
    'timer_wake',
    'registry_implementation',
    'registry_verification',
  ],
  runs: 1,
  technical_retries: 1,
  model: 'flash',
  provider: 'deepseek',
  agent: null,
}
const invalidTimer = run('timer-1', {
  technical: 'technical_invalid',
  score: null,
  failure: 'scenario setup failed: UNKNOWN_DB primary',
})

/** The local execution: before, during and after timer_wake runs again. */
function localExecution(phase) {
  const timer =
    phase === 'after'
      ? report('timer_wake', 'native-timer-2', run('timer-2', { score: 75 }))
      : phase === 'running'
        ? // Admitted and running: nothing to report yet.
          {
            subject_id: 'flash',
            scenario_id: 'timer_wake',
            native_execution_id: 'native-timer-2',
            round: 1,
            available: false,
            state: 'running',
          }
        : report('timer_wake', 'native-timer-1', invalidTimer)
  const state = phase === 'running' ? 'running' : 'completed'
  const timerSlot =
    phase === 'before'
      ? slot('timer_wake', 'native-timer-1')
      : {
          ...slot('timer_wake', 'native-timer-2', [
            { execution_id: 'native-timer-1', error: null },
          ]),
          ...(phase === 'running'
            ? { state: 'running', observed: 0, completed: 0, passed: 0 }
            : {}),
        }
  const source = { kind: 'local' }
  return {
    id: localId,
    label: 'nightly smoke',
    status: phase === 'running' ? 'running' : 'technical_failed',
    state,
    started_at: '2026-09-23T10:00:00Z',
    completed_at: phase === 'running' ? '' : '2026-09-23T11:00:00Z',
    subjects: [
      { id: 'flash', model: 'flash', provider: 'deepseek', scenarios: [] },
    ],
    totals: {},
    parameters,
    source,
    stack: [],
    reports: [
      report('minimal_path', 'native-minimal', run('minimal-1', { score: 90 })),
      timer,
      report('registry_implementation', 'native-group', run('group-1')),
      report('registry_verification', 'native-group', run('group-2')),
    ],
    previous_reports:
      phase === 'after'
        ? [report('timer_wake', 'native-timer-1', invalidTimer)]
        : [],
    plan_execution: {
      id: localId,
      label: 'nightly smoke',
      parameters,
      source,
      stack: [],
      warnings: [],
      state,
      started_at: '2026-09-23T10:00:00Z',
      finished_at: null,
      error: null,
      measurements: null,
      rerun:
        phase === 'running'
          ? {
              scenarios: ['timer_wake'],
              runs: ['native-timer-1'],
              started_at: new Date().toISOString(),
              state: 'completed',
              error: null,
              finished_at: '2026-09-23T11:00:00Z',
            }
          : null,
      slots: [
        slot('minimal_path', 'native-minimal'),
        timerSlot,
        slot('registry_implementation', 'native-group'),
        slot('registry_verification', 'native-group'),
      ],
    },
  }
}

const importedSource = {
  kind: 'github',
  repository: 'iii-hq/harness-e2e',
  run_id: 42,
  run_attempt: 1,
  url: 'https://github.com/iii-hq/harness-e2e/actions/runs/42',
  release_control_execution_id: null,
}
const imported = {
  ...localExecution('before'),
  id: importedId,
  label: 'imported smoke',
  source: importedSource,
  plan_execution: {
    ...localExecution('before').plan_execution,
    id: importedId,
    label: 'imported smoke',
    source: importedSource,
  },
}

let phase = 'before'
let busy = true
const reruns = []
const detailOf = (id) =>
  id === localId
    ? localExecution(phase)
    : id === importedId
      ? imported
      : {
          id: nightly,
          label: 'Nightly',
          status: 'running',
          subjects: [],
          reports: [],
        }
const trigger = (name, request = {}) => {
  const id = name.replace('e2e::dashboard::', '')
  if (id === 'executions-list') {
    const listed = (request.ids ?? []).map((one) => {
      const { reports, previous_reports, ...summary } = detailOf(one)
      return summary
    })
    return { executions: listed, total: listed.length }
  }
  if (id === 'execution-get') return { detail: detailOf(request.execution_id) }
  if (id === 'execution-slot-rerun') {
    if (busy) {
      busy = false
      throw new Error(
        `handler error: "Nightly" (${nightly}) is still running; wait for it to finish or cancel it.`,
      )
    }
    reruns.push(request)
    phase = 'running'
    return { execution_id: request.execution_id }
  }
  throw new Error(`Unexpected RPC ${name}`)
}

const server = await createConsoleTestHost()
const browser = await chromium.launch({ headless: true })
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } })
  await server.install(page, trigger)
  page.setDefaultTimeout(10_000)
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))

  // Every scenario can run again; the one that did not pass says so.
  await page.goto(`${server.url}#/ext/harness-e2e/execution/${localId}`)
  const timerAgain = page.getByRole('button', {
    name: 'Run Timer Wake again',
    exact: true,
  })
  await timerAgain.waitFor()
  assert.equal((await timerAgain.innerText()).trim(), 'run again')
  assert.equal(
    (
      await page
        .getByRole('button', { name: 'Run Minimal Path again', exact: true })
        .innerText()
    ).trim(),
    '',
  )

  // A scenario of a sequential group says its group runs with it.
  await page
    .getByRole('button', { name: 'Run Registry Verification again' })
    .click()
  const group = page.getByRole('dialog', {
    name: 'Run registry_verification again',
    exact: true,
  })
  await group
    .getByText(
      'registry_implementation then registry_verification run only together, in this order; the whole group runs again.',
    )
    .waitFor()
  await group.getByRole('button', { name: 'cancel', exact: true }).click()
  await group.waitFor({ state: 'hidden' })

  // A busy runner is named; the next try runs timer_wake again and the page
  // follows the execution while it runs.
  await timerAgain.click()
  const dialog = page.getByRole('dialog', {
    name: 'Run timer_wake again',
    exact: true,
  })
  await dialog.getByText('The last attempt counts', { exact: false }).waitFor()
  const confirm = dialog.getByRole('button', { name: 'run again', exact: true })
  await confirm.click()
  await dialog
    .getByText(
      '"Nightly" is still running. Wait for it to finish or cancel it.',
    )
    .waitFor()
  assert.ok(
    (
      await dialog
        .getByRole('link', { name: 'open Nightly', exact: true })
        .getAttribute('href')
    ).includes(nightly),
  )
  assert.equal(reruns.length, 0)
  await confirm.click()
  await page.getByText('Execution · running', { exact: true }).waitFor()
  assert.deepEqual(reruns, [
    { execution_id: localId, scenario_id: 'timer_wake' },
  ])
  // While it runs: timed from the rerun, the scenario reads as running, the
  // others keep their results, and nothing can run again.
  await page
    .getByText('timer_wake running again since', { exact: false })
    .waitFor()
  const running = page.locator('[aria-label="Timer Wake scenario result"]')
  await running.getByText('Running', { exact: true }).waitFor()
  await page
    .locator('[aria-label="Minimal Path scenario result"]')
    .getByText('90/100')
    .waitFor()
  assert.equal(await page.locator('[data-rerun-scenario]').count(), 0)

  // Finished: the last attempt counts, the previous one is listed apart.
  phase = 'after'
  await page.reload()
  await page
    .getByText('1 scenario run again, the last attempt counts', {
      exact: false,
    })
    .waitFor()
  const row = page.locator('[aria-label="Timer Wake scenario result"]')
  await row.getByText('rerun ×1', { exact: true }).waitFor()
  await row
    .getByRole('button', { name: /Timer Wake/ })
    .first()
    .click()
  const previous = page.locator('[data-previous-attempt="native-timer-1"]')
  await previous.waitFor()
  await previous
    .getByText('scenario setup failed: UNKNOWN_DB primary')
    .waitFor()
  assert.ok(
    (
      await previous
        .getByRole('link', { name: 'evidence record' })
        .getAttribute('href')
    ).includes('execution/native-timer-1/run/timer-1'),
  )
  await page.getByText('previous attempts · not counted').waitFor()

  // An imported execution runs again on GitHub, never here.
  await page.goto(`${server.url}#/ext/harness-e2e/execution/${importedId}`)
  await page
    .getByRole('button', { name: 'Run Timer Wake again', exact: true })
    .click()
  const github = page.getByRole('dialog', {
    name: 'Run timer_wake again on GitHub',
    exact: true,
  })
  await github.getByText('import the run again', { exact: false }).waitFor()
  assert.equal(
    await github
      .getByRole('link', { name: 'open GitHub run #42' })
      .getAttribute('href'),
    importedSource.url,
  )
  assert.equal(
    await github
      .getByRole('button', { name: 'run again', exact: true })
      .count(),
    0,
  )
  assert.equal(reruns.length, 1)
  assert.deepEqual(errors, [])
  console.log(
    'Rerun scenario browser flow passed: every row offers it, prominent where it failed, group warned, busy runner named, running followed with the scenario running and the others kept, last attempt counted with the previous one listed and linked, imported execution sent to GitHub.',
  )
} finally {
  await browser.close()
  await server.close()
}
