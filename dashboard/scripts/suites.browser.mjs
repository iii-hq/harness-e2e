// Deterministic browser coverage for suites: list, copy, edit and delete
// them, run tests from one, see it on the execution and run it again. The
// suites of the master plan come from the built runner. No models run.
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright'
import { createConsoleTestHost } from './console-test-host.mjs'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..')
const master = JSON.parse(
  execFileSync(
    process.env.HARNESS_E2E_BIN ?? path.join(root, 'target/debug/harness-e2e'),
    ['test-plan', 'list'],
    { encoding: 'utf8' },
  ),
)
const repository = master.suites.map((suite) => ({
  id: suite.id,
  label: suite.label,
  source: 'repository',
  purpose: suite.purpose,
  scenarios: suite.scenario_ids,
  repetitions: suite.repetitions,
  technical_retries: suite.technical_retries,
  sha256: suite.sha256,
  updated_at: null,
}))
const local = []
const calls = { create: [], update: [], remove: [], start: [] }
const executions = new Map()

/** What the runner records for a start: the suite with the digest it
 *  materialized (the reviewed one for a master plan suite as it is). */
function start(request) {
  const id = `plan-${String(executions.size + 1).padStart(32, '0')}`
  const reviewed = repository.find(
    (suite) => suite.id === request.parameters.suite?.id,
  )
  const parameters = {
    ...request.parameters,
    suite: request.parameters.suite
      ? {
          ...request.parameters.suite,
          sha256: reviewed?.sha256 ?? 'sha256:local',
        }
      : { label: '', sha256: 'sha256:unnamed' },
  }
  const execution = {
    id,
    label: request.label,
    status: 'passed',
    state: 'completed',
    started_at: '2026-09-24T10:00:00Z',
    completed_at: '2026-09-24T10:05:00Z',
    availability: 'aggregate',
    parameters,
    subjects: [
      {
        id: parameters.model,
        model: parameters.model,
        provider: parameters.provider,
        scenarios: [],
      },
    ],
    reports: [],
    totals: {},
    plan_execution: {
      id,
      label: request.label || null,
      parameters,
      source: { kind: 'local' },
      stack: [],
      warnings: [],
      state: 'completed',
      started_at: '2026-09-24T10:00:00Z',
      finished_at: '2026-09-24T10:05:00Z',
      error: null,
      slots: [],
      measurements: null,
    },
  }
  executions.set(id, execution)
  return { execution_id: id }
}

const trigger = (name, request = {}) => {
  const id = name.replace('e2e::dashboard::', '')
  if (id === 'suites-list') return { suites: [...repository, ...local] }
  if (id === 'suite-create') {
    calls.create.push(request)
    const from = [...repository, ...local].find(
      (suite) => suite.id === request.from,
    )
    const suite = {
      ...from,
      id: `suite-${local.length + 1}`,
      label: request.label || `${from.label} copy`,
      source: 'local',
      purpose: '',
      sha256: 'sha256:local',
      updated_at: '2026-09-24T09:00:00Z',
    }
    local.push(suite)
    return suite
  }
  if (id === 'suite-update') {
    calls.update.push(request)
    const suite = local.find((entry) => entry.id === request.suite_id)
    const { suite_id: _, ...changes } = request
    return Object.assign(suite, changes)
  }
  if (id === 'suite-delete') {
    calls.remove.push(request)
    local.splice(
      local.findIndex((entry) => entry.id === request.suite_id),
      1,
    )
    return {}
  }
  if (id === 'catalog-get')
    return {
      scenarios: [...new Set(repository.flatMap((suite) => suite.scenarios))],
      scenario_groups: [['registry_implementation', 'registry_verification']],
      models: [{ provider: 'deepseek', model: 'deepseek-v4-flash' }],
    }
  if (id === 'executions-list') {
    const listed = [...executions.values()].filter(
      (execution) => !request.ids?.length || request.ids.includes(execution.id),
    )
    return { executions: listed, total: listed.length }
  }
  if (id === 'execution-get')
    return { detail: executions.get(request.execution_id) }
  if (id === 'execution-start') {
    calls.start.push(request)
    return start(request)
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

  // The Suites tab lists the master plan's suites, read-only.
  await page.goto(`${server.url}#/ext/harness-e2e/executions`)
  await page.getByRole('link', { name: 'Suites', exact: true }).first().click()
  await page.locator('[data-suites]').waitFor()
  for (const suite of repository) {
    const row = page.locator(`[data-suite="${suite.id}"]`)
    await row.getByText(suite.label, { exact: true }).waitFor()
    await row.getByText('repository', { exact: true }).waitFor()
    await row
      .getByText(suite.sha256.replace(/^sha256:/, '').slice(0, 12))
      .waitFor()
    assert.equal(await row.getByRole('button', { name: /^Edit / }).count(), 0)
    assert.equal(await row.getByRole('button', { name: /^Delete / }).count(), 0)
  }
  const regression = repository.find((suite) => suite.id === 'regression')

  // A copy of a repository suite is a suite of this Console, opened to edit.
  await page
    .getByRole('button', { name: `Copy ${regression.label}`, exact: true })
    .click()
  const editor = page.getByRole('dialog', {
    name: `Edit ${regression.label} copy`,
  })
  await editor.waitFor()
  assert.deepEqual(calls.create, [{ from: 'regression', label: '' }])
  // A suite holds no model.
  assert.equal(await editor.getByText('Choose the model').count(), 0)
  await editor.locator('#suite-editor-label').fill('Regression, fast')
  await editor.locator('#suite-editor-runs').fill('2')
  const dropped = regression.scenarios[0]
  await editor.getByRole('checkbox', { name: dropped, exact: true }).click()
  await editor.getByRole('button', { name: 'save suite', exact: true }).click()
  await editor.waitFor({ state: 'hidden' })
  assert.deepEqual(calls.update, [
    {
      suite_id: 'suite-1',
      label: 'Regression, fast',
      scenarios: regression.scenarios.slice(1),
      repetitions: 2,
      technical_retries: regression.technical_retries,
    },
  ])
  const copy = page.locator('[data-suite="suite-1"]')
  await copy.getByText('Regression, fast', { exact: true }).waitFor()
  await copy.getByText('this Console', { exact: true }).waitFor()
  await copy
    .getByText(`${regression.scenarios.length - 1} tests · 2 runs each`, {
      exact: false,
    })
    .waitFor()

  // Run tests: the suite is the first field. Picking one ticks what it
  // holds; changing that makes it unnamed until it is picked again.
  const pr = repository.find((suite) => suite.id === 'pr')
  await page.goto(`${server.url}#/ext/harness-e2e/executions`)
  await page
    .getByRole('button', { name: 'Run tests', exact: true })
    .first()
    .click()
  const run = page.getByRole('dialog', { name: 'Run tests' })
  await run.getByText('catalog ready').waitFor()
  const suiteField = run.locator('#run-dialog-suite')
  const pickSuite = async (dialog, name) => {
    await dialog.locator('#run-dialog-suite').click()
    const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
    await dialog
      .getByRole('option', { name: new RegExp(`^${escaped}(\\s|$)`) })
      .click()
  }
  await pickSuite(run, 'PR')
  for (const scenario of pr.scenarios)
    assert.ok(
      await run
        .getByRole('checkbox', { name: scenario, exact: true })
        .isChecked(),
    )
  await run
    .getByText(`${pr.scenarios.length} tests ·`, { exact: false })
    .first()
    .waitFor()
  await run
    .getByRole('checkbox', { name: pr.scenarios[0], exact: true })
    .click()
  await run.getByText('Changed from PR. Runs as a custom selection.').waitFor()
  await run.getByRole('button', { name: 'Reset', exact: true }).click()
  assert.equal(await suiteField.getAttribute('data-value'), 'pr')
  await run
    .getByText(`${pr.scenarios.length} tests ·`, { exact: false })
    .first()
    .waitFor()
  await run.getByLabel('Model').selectOption('deepseek::deepseek-v4-flash')
  await run
    .getByRole('button', {
      name: `Run ${pr.scenarios.length} tests`,
      exact: true,
    })
    .click()
  await page.waitForFunction(() => location.hash.includes('/execution/plan-'))
  assert.deepEqual(calls.start[0], {
    label: '',
    parameters: {
      suite: { id: 'pr', label: 'PR' },
      scenarios: pr.scenarios,
      runs: pr.repetitions,
      technical_retries: pr.technical_retries,
      model: 'deepseek-v4-flash',
      provider: 'deepseek',
      agent: null,
      where: 'harness',
    },
  })

  // The execution names its suite in the header, with its digest.
  const band = page.locator('[data-identity-band]')
  await band
    .getByText(`PR · ${pr.sha256.replace(/^sha256:/, '').slice(0, 12)}`, {
      exact: true,
    })
    .waitFor()

  // Run again keeps the suite.
  await page.getByRole('button', { name: 'run again', exact: true }).click()
  const again = page.getByRole('dialog', { name: 'Run again' })
  await again.getByText('catalog ready').waitFor()
  assert.equal(
    await again.locator('#run-dialog-suite').getAttribute('data-value'),
    'pr',
  )
  await again
    .getByRole('button', {
      name: `Run ${pr.scenarios.length} tests`,
      exact: true,
    })
    .click()
  await page.waitForFunction(() => location.hash.endsWith('0002'))
  assert.deepEqual(calls.start[1].parameters, calls.start[0].parameters)

  // Run tests from the suite of this Console (the model of the last
  // execution comes along), then edit the suite: Run again keeps the suite
  // as the execution ran it, under its name, not what it holds now.
  const fast = regression.scenarios.length - 1
  await page.goto(`${server.url}#/ext/harness-e2e/executions`)
  await page
    .getByRole('button', { name: 'Run tests', exact: true })
    .first()
    .click()
  const fromLocal = page.getByRole('dialog', { name: 'Run tests' })
  await fromLocal.getByText('catalog ready').waitFor()
  await pickSuite(fromLocal, 'Regression, fast')
  await fromLocal
    .getByRole('button', { name: `Run ${fast} tests`, exact: true })
    .click()
  await page.waitForFunction(() => location.hash.endsWith('0003'))
  const ranLocal = calls.start[2].parameters
  assert.deepEqual(ranLocal.suite, { id: 'suite-1', label: 'Regression, fast' })
  assert.equal(ranLocal.runs, 2)
  await band.getByText('Regression, fast · local', { exact: true }).waitFor()
  const localExecution = await page.evaluate(() => location.hash)
  await page.goto(`${server.url}#/ext/harness-e2e/suites`)
  await page
    .getByRole('button', { name: 'Edit Regression, fast', exact: true })
    .click()
  const edit = page.getByRole('dialog', { name: 'Edit Regression, fast' })
  await edit.locator('#suite-editor-runs').fill('3')
  await edit.getByRole('button', { name: 'save suite', exact: true }).click()
  await edit.waitFor({ state: 'hidden' })
  await page.goto(`${server.url}${localExecution}`)
  await page.getByRole('button', { name: 'run again', exact: true }).click()
  await again.getByText('catalog ready').waitFor()
  assert.equal(
    await again.locator('#run-dialog-suite').getAttribute('data-value'),
    'recorded:suite-1',
  )
  await again
    .locator('#run-dialog-suite')
    .getByText('Regression, fast · as recorded')
    .waitFor()
  await again
    .getByRole('button', { name: `Run ${fast} tests`, exact: true })
    .click()
  await page.waitForFunction(() => location.hash.endsWith('0004'))
  assert.deepEqual(calls.start[3].parameters, ranLocal)

  // A suite of this Console is deleted after a confirmation.
  await page.goto(`${server.url}#/ext/harness-e2e/suites`)
  await page
    .getByRole('button', { name: 'Delete Regression, fast', exact: true })
    .click()
  const confirm = page.getByRole('dialog', { name: 'Delete Regression, fast?' })
  await confirm.getByRole('button', { name: 'cancel', exact: true }).click()
  assert.deepEqual(calls.remove, [])
  await page
    .getByRole('button', { name: 'Delete Regression, fast', exact: true })
    .click()
  await confirm
    .getByRole('button', { name: 'delete suite', exact: true })
    .click()
  await copy.waitFor({ state: 'detached' })
  assert.deepEqual(calls.remove, [{ suite_id: 'suite-1' }])

  // Narrow: the suites table fits.
  await page.goto(`${server.url}#/ext/harness-e2e/suites`)
  await page.locator('[data-suites]').waitFor()
  await page.setViewportSize({ width: 390, height: 844 })
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
    true,
  )
  assert.deepEqual(errors, [])
  console.log(
    'Suites browser flow passed: repository suites listed read-only, copy, edit and delete a suite of this Console, Run tests from a suite (changed makes it unnamed), the suite in the execution header, Run again keeps it, even after its suite was edited (as recorded), narrow viewport.',
  )
} catch (error) {
  console.error(
    await browser.contexts()[0]?.pages()[0]?.locator('body').innerText(),
  )
  throw error
} finally {
  await browser.close()
  await server.close()
}
