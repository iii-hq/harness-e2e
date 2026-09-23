// Deterministic browser coverage for Run tests and Run again. No models run.
import assert from 'node:assert/strict'
import { chromium } from 'playwright'
import { createConsoleTestHost } from './console-test-host.mjs'

const seed = '18446744073709551615'
const imported = {
  id: 'plan-0123456789abcdef0123456789abcdef',
  label: 'Software engineering',
  plan_id: null,
  status: 'passed',
  state: 'completed',
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
    plan_id: null,
    role: null,
    label: 'Software engineering',
    parameters: {
      scenarios: ['minimal_path', 'retired_scenario'],
      runs: 2,
      technical_retries: 0,
      seed,
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
    },
    stack: [],
    warnings: [],
    state: 'completed',
    started_at: '2026-09-20T10:00:00Z',
    finished_at: '2026-09-20T11:00:00Z',
    error: null,
    baseline_eligible: false,
    slots: [],
    measurements: null,
  },
}
const started = []
const deleted = []
let busy = true
let catalogDown = false
const server = await createConsoleTestHost()
const trigger = (name, request = {}) => {
  const id = name.replace('e2e::dashboard::', '')
  if (id === 'executions-list') return { executions: [imported], total: 1 }
  if (id === 'execution-get') return { detail: imported }
  if (id === 'catalog-get') {
    if (catalogDown) throw new Error('catalog unavailable: harness restarting')
    return {
      url: 'ws://localhost:49134',
      scenarios: ['minimal_path', 'context_pressure'],
      models: [{ provider: 'deepseek', model: 'deepseek-v4-flash' }],
    }
  }
  if (id === 'execution-start') {
    if (busy) {
      busy = false
      throw new Error('plan execution plan-busy is active')
    }
    started.push(request)
    return { execution_id: `plan-${String(started.length).padStart(32, 'f')}` }
  }
  if (id === 'execution-delete') {
    deleted.push(request.execution_id)
    return {}
  }
  throw new Error(`Unexpected RPC ${name}`)
}
const browser = await chromium.launch({ headless: true })
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } })
  await server.install(page, trigger)
  page.setDefaultTimeout(10_000)
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))

  // Run tests: a busy runner is named in the footer and nothing moves;
  // the next submit starts an execution and follows it on its page.
  await page.goto(`${server.url}#/ext/harness-e2e/executions`)
  await page.getByRole('button', { name: 'Run tests', exact: true }).click()
  const runTests = page.getByRole('dialog', { name: 'Run suite' })
  await runTests.waitFor()
  await runTests.getByText('catalog ready').waitFor()
  assert.equal(await runTests.getByText('Harness endpoint').count(), 0)
  await runTests.getByRole('checkbox', { name: 'context_pressure' }).check()
  await runTests.getByText('Advanced · sampling, retries and seed').click()
  await runTests.locator('#quick-execution-seed').fill('1e5')
  await runTests
    .getByRole('button', { name: 'run 1 test', exact: true })
    .click()
  await runTests
    .getByText(/The seed is a whole number/)
    .first()
    .waitFor()
  assert.equal(started.length, 0)
  await runTests.locator('#quick-execution-seed').fill('')
  await runTests
    .getByRole('button', { name: 'run 1 test', exact: true })
    .click()
  await runTests.getByText('plan execution plan-busy is active').waitFor()
  assert.equal(started.length, 0)
  assert.ok(await runTests.isVisible())
  await runTests
    .getByRole('button', { name: 'run 1 test', exact: true })
    .click()
  await page.waitForFunction(() => location.hash.includes('/execution/plan-'))
  assert.deepEqual(started[0], {
    label: '',
    parameters: {
      scenarios: ['context_pressure'],
      runs: 1,
      technical_retries: 1,
      seed: null,
      model: 'deepseek-v4-flash',
      provider: 'deepseek',
      agent: null,
    },
  })
  assert.match(
    await page.evaluate(() => location.hash),
    /\/execution\/plan-f{31}1$/,
  )

  // Run again: the form holds the execution's parameters, the seed exact,
  // and sends them unchanged even when the catalog cannot be read.
  catalogDown = true
  await page.goto(`${server.url}#/ext/harness-e2e/execution/${imported.id}`)
  await page.getByRole('button', { name: 'run again', exact: true }).click()
  const again = page.getByRole('dialog', { name: 'Run again' })
  await again.waitFor()
  await again.getByText('catalog unavailable: harness restarting').waitFor()
  await again.getByText('Advanced · sampling, retries and seed').click()
  assert.equal(await again.locator('#quick-execution-runs').inputValue(), '2')
  assert.equal(
    await again.locator('#quick-execution-retries').inputValue(),
    '0',
  )
  assert.equal(await again.locator('#quick-execution-seed').inputValue(), seed)
  assert.equal(
    await again.locator('#quick-execution-agent').inputValue(),
    'tech-lead',
  )
  for (const scenario of ['minimal_path', 'retired_scenario'])
    assert.ok(await again.getByRole('checkbox', { name: scenario }).isChecked())
  await again.getByRole('button', { name: 'run 2 tests', exact: true }).click()
  await page.waitForFunction(() => !location.hash.includes('0123456789abcdef'))
  assert.deepEqual(started[1], {
    label: '',
    parameters: imported.plan_execution.parameters,
  })

  // A finished execution without a plan can be deleted.
  catalogDown = false
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
    'Run tests and Run again browser flow passed: busy runner, seed check, start and follow, prefill with an exact seed and no catalog, delete.',
  )
} finally {
  await browser.close()
  await server.close()
}
