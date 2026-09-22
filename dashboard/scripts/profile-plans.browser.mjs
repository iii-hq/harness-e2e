// Deterministic browser coverage for the executable-plan journey. No models run.
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
const plans = []
const executions = new Map()
const runStarts = []
let active = null
let requirementsBlocked = false
const configuration = {
  url: 'ws://localhost:49134',
  model: '',
  provider: '',
}
const requirements = () => ({
  ready: !active && !requirementsBlocked,
  checks: requirementsBlocked
    ? [
        {
          id: 'fixture',
          status: 'blocked',
          message: 'Fixture requirement blocked.',
        },
      ]
    : [],
  active_execution: active && {
    id: active.id,
    kind: 'plan',
    plan_id: active.plan_id,
  },
})
function createPlan(request) {
  const plan = {
    ...configuration,
    ...request,
    id: `saved-${plans.length + 1}`,
    scenarios: request.scenarios.map((scenario_id) => ({
      scenario_id,
      behavior_sha256:
        'sha256:a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1',
      case_id: scenario_id,
    })),
    scenario_ids: request.scenarios,
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
    locked: false,
    compatible: true,
    state: 'draft',
    scope_hash: 'scope',
    baseline_execution_id: null,
    candidate_execution_ids: [],
    incomplete_execution_ids: [],
    last_attempt_id: null,
  }
  plans.push(plan)
  return plan
}
createPlan({
  ...configuration,
  label: 'Existing manual plan',
  purpose: 'Existing workflow',
  model: 'deepseek-v4-flash',
  provider: 'deepseek',
  scenarios: ['minimal_path'],
  runs: 1,
  technical_retries: 0,
  seed: null,
})
function startPlan(plan, role) {
  assert.equal(active, null)
  active = {
    id: `plan-${executions.size + 1}`,
    plan_id: plan.id,
    state: 'running',
    role,
    started_at: new Date().toISOString(),
    slots: [],
    measurements: null,
  }
  for (let round = 1; round <= plan.runs; round++)
    for (const scenario_id of plan.scenario_ids)
      active.slots.push({
        scenario_id,
        round,
        execution_id: `native-${active.slots.length}`,
        state: active.slots.length ? 'pending' : 'running',
        observed: 0,
        completed: 0,
        passed: 0,
        technical_valid: 0,
      })
  executions.set(active.id, active)
  plan.state = `${role}_running`
  plan.locked = true
  plan.last_attempt_id = active.id
  return plan
}
function operation(request) {
  if (request.action === 'requirements') return requirements()
  if (request.action === 'execution')
    return executions.get(request.execution_id)
  if (request.action === 'export')
    return plans.find((p) => p.id === request.plan_id)
  if (request.action === 'cancel') {
    const execution = executions.get(request.execution_id)
    execution.state = 'cancelled'
    execution.slots.forEach((s) => {
      s.state = 'not_run'
    })
    const plan = plans.find((p) => p.id === execution.plan_id)
    plan.state = plan.baseline_execution_id ? 'baseline_ready' : 'draft'
    plan.incomplete_execution_ids.push(execution.id)
    active = null
    return execution
  }
  throw Error(`Unexpected operation ${request.action}`)
}
function executionDetail(execution) {
  const plan = plans.find((p) => p.id === execution.plan_id)
  return {
    id: execution.id,
    label: plan.label,
    plan_id: plan.id,
    plan_execution: execution,
    status: execution.state,
    event: 'local',
    availability: 'aggregate',
    subjects: [
      {
        id: plan.model,
        model: plan.model,
        provider: plan.provider,
        scenarios: [],
      },
    ],
    reports: execution.slots.map((slot) => ({
      subject_id: plan.model,
      scenario_id: slot.scenario_id,
      native_execution_id: slot.execution_id,
      available: false,
    })),
    totals: { expected_reports: execution.slots.length, received_reports: 0 },
  }
}
const server = await createConsoleTestHost()
const trigger = (name, request = {}) => {
  const id = name.replace('e2e::dashboard::', '')
  if (id === 'plan-control') return operation(request)
  if (id === 'plans-list') return { plans, master_plan: master }
  if (id === 'plan-create') return createPlan(request)
  if (id === 'plan-get')
    return plans.find((plan) => plan.id === request.plan_id)
  if (id === 'plan-update')
    return Object.assign(
      plans.find((plan) => plan.id === request.plan_id),
      request,
    )
  if (id === 'plan-run-start') {
    runStarts.push({ planId: request.plan_id, role: request.role })
    return startPlan(
      plans.find((plan) => plan.id === request.plan_id),
      request.role,
    )
  }
  if (id === 'executions-list')
    return { executions: [...executions.values()].map(executionDetail) }
  if (id === 'execution-get')
    return { detail: executionDetail(executions.get(request.execution_id)) }
  if (id === 'catalog-get')
    return {
      url: configuration.url,
      scenarios: [...new Set(master.profiles.flatMap((p) => p.scenario_ids))],
      models: [
        { provider: 'deepseek', model: 'deepseek-v4-flash' },
        { provider: 'openai-codex', model: 'codex/gpt-5.6-terra' },
      ],
    }
  if (id === 'run-status') return { job: null, defaults: configuration }
  if (name === 'release-control::test-plans::list') return { plans: [] }
  throw new Error(`Unexpected RPC ${name}`)
}
const browser = await chromium.launch({ headless: true })
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } })
  await server.install(page, trigger)
  page.setDefaultTimeout(10_000)
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))
  await page.goto(`${server.url}#/ext/harness-e2e/plans`)
  await page.getByRole('tab', { name: 'My plans', exact: true }).waitFor()
  assert.equal(
    await page.getByRole('link', { name: 'Create After release plan' }).count(),
    0,
  )
  await page
    .getByRole('link', { name: 'new plan', exact: true })
    .first()
    .click()
  await page
    .getByRole('combobox', { name: 'Start from a template' })
    .selectOption('after-release')
  await page.getByRole('button', { name: 'Save and run', exact: true }).click()
  await page
    .getByText('Choose an execution model.', { exact: true })
    .first()
    .waitFor()
  async function select(label, model) {
    await page.getByRole('button', { name: label, exact: true }).click()
    const search = page.getByRole('searchbox', {
      name: `Search ${label}`,
      exact: true,
    })
    await search.fill(model)
    await search.press('ArrowDown')
    await page.keyboard.press('Enter')
  }
  await select('Execution model', 'deepseek-v4-flash')
  await page.getByRole('button', { name: 'Save plan', exact: true }).click()
  await page.locator('[data-plan-scope]').waitFor()
  assert.equal(plans.length, 2)
  assert.equal(plans[1].scenario_ids.length, 5)
  assert.equal(plans[1].baseline_execution_id, null)
  const runButton = page.getByRole('button', { name: 'Run plan', exact: true })
  await runButton.click()
  const runDialog = page.getByRole('dialog', { name: 'Run baseline?' })
  await runDialog.waitFor()
  assert.equal(runStarts.length, 0)
  await runDialog.getByRole('button', { name: 'Cancel', exact: true }).click()
  await runDialog.waitFor({ state: 'hidden' })
  assert.equal(runStarts.length, 0)
  await runButton.click()
  await runDialog.waitFor()
  await page.keyboard.press('Escape')
  await runDialog.waitFor({ state: 'hidden' })
  assert.equal(runStarts.length, 0)
  requirementsBlocked = true
  await runButton.click()
  await runDialog
    .getByRole('button', { name: 'Run baseline', exact: true })
    .click()
  await runDialog
    .getByText('Execution requirements need attention.', { exact: false })
    .waitFor()
  assert.equal(runStarts.length, 0)
  assert.equal(active, null)
  requirementsBlocked = false
  await runDialog.getByRole('button', { name: 'Cancel', exact: true }).click()
  await runButton.click()
  await runDialog.waitFor()
  assert.equal(
    await runDialog
      .getByText('Fixture requirement blocked.', { exact: true })
      .count(),
    0,
  )
  assert.equal(
    await runDialog
      .getByText('Execution requirements need attention.', { exact: false })
      .count(),
    0,
  )
  await runDialog
    .getByRole('button', { name: 'Run baseline', exact: true })
    .click()
  await page.getByRole('button', { name: /^cancel execution$/i }).waitFor()
  assert.deepEqual(runStarts.at(-1), { planId: plans[1].id, role: 'baseline' })
  await page.getByRole('button', { name: /^cancel execution$/i }).click()
  await page
    .locator('[data-plan-run-history]')
    .getByText('cancelled', { exact: true })
    .first()
    .waitFor()
  assert.equal(active, null)
  // Template and manual plans use the same table, detail and lifecycle.
  await page.goto(`${server.url}#/ext/harness-e2e/plans`)
  await page.getByText('After release', { exact: true }).first().waitFor()
  assert.equal(await page.getByRole('table').count(), 1)
  await page.getByText('Existing manual plan', { exact: true }).first().click()
  await page.locator('[data-plan-scope]').waitFor()
  await page.goto(`${server.url}#/ext/harness-e2e/plans/${plans[1].id}`)
  await page.getByRole('link', { name: 'Duplicate plan', exact: true }).click()
  await page
    .getByRole('button', { name: 'Execution model', exact: true })
    .waitFor()
  assert.match(
    await page
      .getByRole('button', { name: 'Execution model', exact: true })
      .innerText(),
    /Choose a model/,
  )
  await select('Execution model', 'codex/gpt-5.6-terra')
  await page.getByRole('button', { name: 'Save and run', exact: true }).click()
  await page.getByRole('button', { name: /^cancel execution$/i }).waitFor()
  assert.equal(plans.length, 3)
  assert.equal(active.role, 'baseline')
  assert.equal(active.slots.length, 5)
  assert.equal(await page.locator('progress').getAttribute('max'), '5')
  await page.goto(`${server.url}#/ext/harness-e2e/plans/new/profile/after-release`)
  await page
    .getByRole('button', { name: 'Execution model', exact: true })
    .waitFor()
  await select('Execution model', 'deepseek-v4-flash')
  await page.getByRole('button', { name: 'Save and run', exact: true }).click()
  await page.getByRole('link', { name: 'Follow active execution' }).waitFor()
  assert.equal(plans.length, 4)
  assert.equal(plans[3].state, 'draft')
  await page.getByRole('link', { name: 'Follow active execution' }).click()
  const executionId = active.id
  await page.goto(`${server.url}#/ext/harness-e2e/execution/${executionId}`)
  await page.getByRole('link', { name: 'back to plan', exact: true }).waitFor()
  assert.equal(
    await page
      .getByRole('heading', { name: 'Plan execution', exact: true })
      .count(),
    0,
  )
  assert.equal(await page.locator('[data-live-state]').count(), 0)
  assert.equal(await page.locator('progress').count(), 1)
  await page
    .getByText('Execution in progress · results are provisional', {
      exact: true,
    })
    .waitFor()
  await page.getByRole('button', { name: /^cancel execution$/i }).click()
  await page.locator('.primary-metrics').waitFor()
  await page.getByRole('link', { name: 'back to plan', exact: true }).click()
  await page
    .locator('[data-plan-run-history]')
    .getByText('cancelled', { exact: true })
    .first()
    .waitFor()
  assert.equal(active, null)
  const seededBaseline = startPlan(plans[0], 'baseline')
  seededBaseline.state = 'completed'
  plans[0].state = 'baseline_ready'
  plans[0].baseline_execution_id = seededBaseline.id
  active = null
  await page.goto(`${server.url}#/ext/harness-e2e/plans/${plans[0].id}`)
  await page.getByRole('button', { name: 'Re-run plan', exact: true }).click()
  const candidateDialog = page.getByRole('dialog', {
    name: 'Run candidate #1?',
  })
  await candidateDialog.waitFor()
  assert.equal(active, null)
  const startsBeforeCandidate = runStarts.length
  await candidateDialog
    .getByRole('button', { name: 'Cancel', exact: true })
    .click()
  assert.equal(runStarts.length, startsBeforeCandidate)
  await page.getByRole('button', { name: 'Re-run plan', exact: true }).click()
  await candidateDialog
    .getByRole('button', { name: 'Run candidate #1', exact: true })
    .click()
  await page.getByRole('button', { name: /^cancel execution$/i }).waitFor()
  assert.deepEqual(runStarts.at(-1), { planId: plans[0].id, role: 'candidate' })
  await page.getByRole('button', { name: /^cancel execution$/i }).click()
  await page
    .locator('[data-plan-run-history]')
    .getByText('cancelled', { exact: true })
    .first()
    .waitFor()
  assert.equal(active, null)
  await page.goto(`${server.url}#/ext/harness-e2e/plans/${plans[2].id}`)
  await page
    .locator('[data-plan-run-history]')
    .getByText('cancelled', { exact: true })
    .first()
    .waitFor()
  await page.setViewportSize({ width: 390, height: 844 })
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
    true,
  )
  plans[2].compatible = false
  await page.reload()
  await page.getByText('Saved scope unavailable', { exact: true }).waitFor()
  assert.deepEqual(errors, [])
  console.log(
    'Unified plan browser flow passed: create, evaluator, keyboard search, save, run confirmation and blocked requirements, baseline and candidate, duplicate, busy draft, cancel, incompatible and narrow viewport in a functional Console host double.',
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
