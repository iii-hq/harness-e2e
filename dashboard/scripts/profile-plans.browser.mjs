// Deterministic browser coverage for the executable-plan journey. No models run.
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..')
const master = JSON.parse(
  await readFile(path.join(root, 'config/test-plan-profiles.json'), 'utf8'),
)
const plans = []
const executions = new Map()
let active = null
const configuration = {
  url: 'ws://localhost:49134',
  model: '',
  provider: '',
  judge_model: '',
  judge_provider: '',
}
const requirements = () => ({
  ready: !active,
  checks: [],
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
    schema_version: 3,
    scenarios: request.scenarios.map((scenario_id) => ({
      scenario_id,
      scenario_version: 1,
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
    plan.state = 'draft'
    plan.incomplete_execution_ids.push(execution.id)
    active = null
    return execution
  }
  throw Error(`Unexpected operation ${request.action}`)
}
const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url, 'http://localhost')
    let value
    if (url.pathname === '/api/dashboard')
      value = {
        mode: 'local',
        transport: 'static',
        page_size: 25,
        functions: {},
      }
    else if (url.pathname === '/api/dashboard/plans/control') {
      let body = ''
      for await (const chunk of req) body += chunk
      value = operation(JSON.parse(body))
    } else if (url.pathname === '/api/dashboard/plans') {
      if (req.method === 'POST') {
        let body = ''
        for await (const chunk of req) body += chunk
        value = createPlan(JSON.parse(body))
      } else value = { plans, master_plan: master }
    } else if (url.pathname.startsWith('/api/dashboard/plans/')) {
      const [, id, action] =
        url.pathname.match(/\/plans\/([^/]+)(?:\/(runs))?$/) ?? []
      const plan = plans.find((p) => p.id === id)
      if (req.method === 'POST') {
        let body = ''
        for await (const chunk of req) body += chunk
        const input = JSON.parse(body)
        value =
          action === 'runs'
            ? startPlan(plan, input.role)
            : Object.assign(plan, input)
      } else if (req.method === 'PATCH') {
        let body = ''
        for await (const chunk of req) body += chunk
        value = Object.assign(plan, JSON.parse(body))
      } else value = plan
    } else if (url.pathname === '/api/local/catalog')
      value = {
        url: configuration.url,
        scenarios: [...new Set(master.profiles.flatMap((p) => p.scenario_ids))],
        models: [
          { provider: 'deepseek', model: 'deepseek-v4-flash' },
          { provider: 'openai-codex', model: 'codex/gpt-5.6-terra' },
        ],
      }
    else if (url.pathname === '/api/local/run')
      value = { job: null, defaults: configuration }
    else if (url.pathname.startsWith('/api/'))
      value = {
        executions: [],
        total: 0,
        next_cursor: null,
        tests: [],
        versions: [],
      }
    if (value !== undefined) {
      res.setHeader('Content-Type', 'application/json')
      res.end(JSON.stringify(value))
      return
    }
    const filename = url.pathname === '/' ? 'index.html' : url.pathname.slice(1)
    const content = await readFile(path.join(root, 'dashboard/dist', filename))
    res.setHeader(
      'Content-Type',
      filename.endsWith('.js')
        ? 'text/javascript'
        : filename.endsWith('.css')
          ? 'text/css'
          : filename.endsWith('.html')
            ? 'text/html'
            : 'application/octet-stream',
    )
    res.end(content)
  } catch (error) {
    res.statusCode = 500
    res.end(String(error))
  }
})
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
const browser = await chromium.launch({ headless: true })
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } })
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))
  await page.goto(`http://127.0.0.1:${server.address().port}/#/plans`)
  await page.getByRole('button', { name: 'My plans', exact: true }).waitFor()
  assert.equal(
    await page.getByRole('link', { name: 'Create Smoke plan' }).count(),
    0,
  )
  await page
    .getByRole('link', { name: 'new plan', exact: true })
    .first()
    .click()
  await page
    .getByRole('combobox', { name: 'Start from a template' })
    .selectOption('smoke')
  await page.getByRole('button', { name: 'Save and run', exact: true }).click()
  await page
    .getByText('Choose a judge model for the selected tests.', { exact: true })
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
  await select('Judge model', 'codex/gpt-5.6-terra')
  await page.getByRole('button', { name: 'Save draft', exact: true }).click()
  await page.locator('[data-plan-lifecycle]').waitFor()
  assert.equal(plans.length, 2)
  assert.equal(plans[1].scenario_ids.length, 5)
  assert.equal(plans[1].baseline_execution_id, null)
  // Template and manual plans use the same table, detail and lifecycle.
  await page.goto(`http://127.0.0.1:${server.address().port}/#/plans`)
  await page.getByText('Smoke', { exact: true }).first().waitFor()
  assert.equal(await page.getByRole('table').count(), 1)
  await page.getByText('Existing manual plan', { exact: true }).first().click()
  await page.locator('[data-plan-lifecycle]').waitFor()
  await page.goto(
    `http://127.0.0.1:${server.address().port}/#/plans/${plans[1].id}`,
  )
  await page.getByRole('link', { name: 'Duplicate plan', exact: true }).click()
  await page
    .getByRole('button', { name: 'Execution model', exact: true })
    .waitFor()
  await page
    .getByRole('button', { name: 'Judge model', exact: true })
    .getByText('codex/gpt-5.6-terra', { exact: true })
    .waitFor()
  assert.match(
    await page
      .getByRole('button', { name: 'Execution model', exact: true })
      .innerText(),
    /Choose a model/,
  )
  assert.match(
    await page
      .getByRole('button', { name: 'Judge model', exact: true })
      .innerText(),
    /codex\/gpt-5.6-terra/,
  )
  await select('Execution model', 'codex/gpt-5.6-terra')
  await page.getByRole('button', { name: 'Save and run', exact: true }).click()
  await page
    .getByRole('button', { name: 'Cancel execution', exact: true })
    .waitFor()
  assert.equal(plans.length, 3)
  assert.equal(active.role, 'baseline')
  assert.equal(active.slots.length, 5)
  assert.equal(await page.locator('progress').getAttribute('max'), '5')
  await page.goto(
    `http://127.0.0.1:${server.address().port}/#/plans/new/profile/smoke`,
  )
  await page
    .getByRole('button', { name: 'Execution model', exact: true })
    .waitFor()
  await select('Execution model', 'deepseek-v4-flash')
  await select('Judge model', 'codex/gpt-5.6-terra')
  await page.getByRole('button', { name: 'Save and run', exact: true }).click()
  await page.getByRole('link', { name: 'Follow active execution' }).waitFor()
  assert.equal(plans.length, 4)
  assert.equal(plans[3].state, 'draft')
  await page.getByRole('link', { name: 'Follow active execution' }).click()
  await page
    .getByRole('button', { name: 'Cancel execution', exact: true })
    .click()
  await page
    .getByText('baseline retry available · scope locked', { exact: true })
    .waitFor()
  assert.equal(active, null)
  await page.setViewportSize({ width: 390, height: 844 })
  for (const theme of ['light', 'dark']) {
    await page.evaluate((theme) => {
      document.documentElement.dataset.theme = theme
    }, theme)
    assert.equal(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
      true,
    )
  }
  plans[2].compatible = false
  await page.reload()
  await page.getByText('Saved scope unavailable', { exact: true }).waitFor()
  assert.deepEqual(errors, [])
  console.log(
    'Unified plan browser flow passed: create, evaluator, keyboard search, save, run, duplicate, busy draft, cancel, incompatible, mobile and themes.',
  )
} catch (error) {
  console.error(
    await browser.contexts()[0]?.pages()[0]?.locator('body').innerText(),
  )
  throw error
} finally {
  await browser.close()
  await new Promise((resolve) => server.close(resolve))
}
