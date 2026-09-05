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
const snapshot = (profile) => ({
  profile,
  ...profile,
  version: master.version,
  definition_sha256: master.definition_sha256,
})
const requirements = () => ({
  ready: !active,
  checks: [
    {
      id: 'fixture',
      status: 'pending',
      message: 'Fixture checked by native setup.',
    },
  ],
  active_execution: active && {
    id: active.id,
    kind: 'plan',
    plan_id: active.plan_id,
  },
})
function operation(request) {
  const plan = plans.find((p) => p.id === request.plan_id)
  switch (request.action) {
    case 'requirements':
      return requirements()
    case 'get':
      return plan
    case 'create': {
      const p = {
        id: `profile-${plans.length + 1}`,
        schema_version: 2,
        configuration: request.configuration,
        snapshot: snapshot(
          master.profiles.find(
            (p) => p.id === request.configuration.profile_id,
          ),
        ),
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
        locked: false,
        compatible: true,
        state: 'draft',
        history: [],
        last_execution: null,
        baseline_execution_id: null,
      }
      plans.push(p)
      return p
    }
    case 'update':
      Object.assign(plan.configuration, request.configuration)
      return plan
    case 'duplicate':
      return operation({
        action: 'create',
        configuration: {
          ...plan.configuration,
          model: request.model,
          provider: request.provider,
          label: request.label,
        },
      })
    case 'export':
      return { schema: 'harness-e2e-profile-campaigns/v1', saved_plan: plan }
    case 'start': {
      if (active) return { blocked: true, requirements: requirements() }
      const id = `plan-${executions.size + 1}`
      active = {
        id,
        plan_id: plan.id,
        state: 'running',
        role: request.role,
        started_at: new Date().toISOString(),
        slots: plan.snapshot.scenario_ids.map((scenario_id, i) => ({
          scenario_id,
          round: 1,
          group_id: `group-${i}`,
          execution_id: `native-${i}`,
          state: i ? 'pending' : 'running',
          observed: 0,
          completed: 0,
          passed: 0,
          technical_valid: 0,
        })),
        measurements: null,
      }
      executions.set(id, active)
      plan.locked = true
      plan.state = 'running'
      plan.last_execution = {
        ...active,
        planned: active.slots.length,
        finished: 0,
      }
      plan.history.unshift(plan.last_execution)
      return { execution_id: id }
    }
    case 'execution':
      return executions.get(request.execution_id)
    case 'cancel': {
      const execution = executions.get(request.execution_id)
      execution.state = 'cancelled'
      execution.slots.forEach((s) => {
        s.state = 'not_run'
      })
      const p = plans.find((p) => p.id === execution.plan_id)
      p.state = 'cancelled'
      p.last_execution.state = 'cancelled'
      active = null
      return execution
    }
    default:
      throw Error(`Unexpected profile action ${request.action}`)
  }
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
    else if (url.pathname === '/api/dashboard/plans')
      value = { plans: [], master_plan: master, profile_plans: plans }
    else if (url.pathname === '/api/local/catalog')
      value = {
        url: configuration.url,
        scenarios: [],
        models: [
          { provider: 'deepseek', model: 'deepseek-v4-flash' },
          { provider: 'openai-codex', model: 'codex/gpt-5.6-terra' },
        ],
      }
    else if (url.pathname === '/api/local/run')
      value = { job: null, defaults: configuration }
    else if (url.pathname === '/api/dashboard/profile-plans') {
      let body = ''
      for await (const chunk of req) body += chunk
      value = operation(JSON.parse(body))
    } else if (url.pathname.startsWith('/api/'))
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
  await page.getByRole('link', { name: 'Create Smoke plan' }).click()
  await page.getByRole('button', { name: 'Save and run', exact: true }).click()
  await page
    .getByText('This profile requires an evaluator.', { exact: true })
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
  await select('Evaluator', 'codex/gpt-5.6-terra')
  await page.getByRole('button', { name: 'Save draft', exact: true }).click()
  await page.getByRole('button', { name: 'Run plan', exact: true }).waitFor()
  assert.equal(plans.length, 1)
  assert.equal(plans[0].history.length, 0)
  await page.getByRole('link', { name: 'Duplicate plan', exact: true }).click()
  await page
    .getByRole('button', { name: 'Execution model', exact: true })
    .waitFor()
  assert.match(
    await page
      .getByRole('button', { name: 'Execution model', exact: true })
      .innerText(),
    /Select an execution model/,
  )
  assert.equal(
    await page
      .getByRole('button', { name: 'Evaluator', exact: true })
      .isDisabled(),
    true,
  )
  await select('Execution model', 'codex/gpt-5.6-terra')
  await page.getByRole('button', { name: 'Save and run', exact: true }).click()
  await page
    .getByRole('button', { name: 'Cancel execution', exact: true })
    .waitFor()
  assert.equal(plans.length, 2)
  assert.equal(active.slots.length, 5)
  assert.equal(await page.locator('progress').getAttribute('max'), '5')
  // Busy admission preserves a newly saved draft and offers a follow link.
  await page.goto(
    `http://127.0.0.1:${server.address().port}/#/plans/new/profile/smoke`,
  )
  await page
    .getByRole('button', { name: 'Execution model', exact: true })
    .waitFor()
  await select('Execution model', 'deepseek-v4-flash')
  await select('Evaluator', 'codex/gpt-5.6-terra')
  await page.getByRole('button', { name: 'Save and run', exact: true }).click()
  await page.getByRole('link', { name: 'Follow active execution' }).waitFor()
  assert.equal(plans.length, 3)
  assert.equal(plans[2].state, 'draft')
  await page.getByRole('link', { name: 'Follow active execution' }).click()
  await page
    .getByRole('button', { name: 'Cancel execution', exact: true })
    .click()
  await page.getByRole('button', { name: 'Run again', exact: true }).waitFor()
  assert.equal(active, null)
  // Narrow layout and both themes retain keyboard reachable controls.
  await page.setViewportSize({ width: 390, height: 844 })
  for (const theme of ['light', 'dark']) {
    await page.evaluate((theme) => {
      document.documentElement.dataset.theme = theme
    }, theme)
    assert.equal(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
      true,
    )
  }
  plans[1].compatible = false
  await page.reload()
  await page.getByText('Pinned revision unavailable', { exact: true }).waitFor()
  assert.equal(
    await page
      .getByRole('button', { name: 'Run again', exact: true })
      .isDisabled(),
    true,
  )
  assert.deepEqual(errors, [])
  console.log(
    'Profile browser flow passed: create, evaluator, keyboard search, save, run, duplicate, busy draft, cancel, incompatible, mobile and themes.',
  )
} finally {
  await browser.close()
  await new Promise((resolve) => server.close(resolve))
}
