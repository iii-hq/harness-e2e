// Deterministic browser coverage for comparing two executions. No models run.
import assert from 'node:assert/strict'
import { chromium } from 'playwright'
import { createConsoleTestHost } from './console-test-host.mjs'

const png = (color) =>
  color === 'a'
    ? 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=='
    : 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPj/HwADBwIAMCbHYQAAAABJRU5ErkJggg=='

function run(
  id,
  { score = 80, technical = 'valid', screenshot = false, failure = null } = {},
) {
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
    deliverables: screenshot
      ? [
          {
            id: 'board',
            artifact: { path: `deliverables/${id}/board.json` },
            screenshots: [
              {
                pointer: '/attachments/board.png',
                caption: `board ${id}`,
                media_type: 'image/png',
              },
            ],
          },
        ]
      : [],
  }
}

function execution(id, label, source, parameters, runs, stack) {
  const reports = Object.entries(runs).map(([scenario, runValue], index) => ({
    subject_id: 'flash',
    scenario_id: scenario,
    native_execution_id: `${id.slice(5, 36)}${index}`,
    round: 1,
    available: true,
    report: {
      scenarios: [
        {
          scenario_id: scenario,
          case: { seed: 1, inputs_sha256: `inputs-${scenario}` },
          aggregate: { planned_runs: 1, observed_runs: 1, deferred_runs: 0 },
          runs: [runValue],
        },
      ],
    },
  }))
  return {
    id,
    label,
    status: 'passed',
    state: 'completed',
    completed_at: '2026-09-22T11:00:00Z',
    subjects: [
      { id: 'flash', model: 'flash', provider: 'deepseek', scenarios: [] },
    ],
    totals: {},
    parameters,
    source,
    stack,
    reports,
    plan_execution: {
      id,
      plan_id: null,
      role: null,
      label,
      parameters,
      source,
      stack,
      warnings: [],
      state: 'completed',
      slots: [],
    },
  }
}

const worker = (name, observed) => ({
  name,
  source: 'package',
  requested: null,
  observed,
  commit: null,
  dirty: null,
})
const parameters = (runs) => ({
  scenarios: ['minimal_path', 'persistent_state', 'timer_wake'],
  runs,
  technical_retries: 2,
  seed: '18446744073709551615',
  model: 'flash',
  provider: 'deepseek',
  agent: 'tech-lead',
})
const a = execution(
  'plan-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
  'smoke',
  {
    kind: 'github',
    repository: 'iii-hq/harness-e2e',
    run_id: 42,
    run_attempt: 1,
    url: 'https://github.com/iii-hq/harness-e2e/actions/runs/42',
    release_control_execution_id: null,
  },
  parameters(1),
  {
    minimal_path: run('a1', { score: 82 }),
    persistent_state: run('a2', { score: 100, screenshot: true }),
    timer_wake: run('a3', {
      technical: 'technical_invalid',
      score: null,
      failure: 'scenario setup failed: UNKNOWN_DB primary',
    }),
  },
  [worker('harness-e2e', '0.11.24'), worker('state', '0.22.3')],
)
const b = execution(
  'plan-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
  'smoke rerun',
  { kind: 'local' },
  parameters(3),
  {
    minimal_path: run('b1', { score: 94 }),
    persistent_state: run('b2', { score: 62, screenshot: true }),
    timer_wake: run('b3', { score: 40 }),
  },
  [
    worker('harness-e2e', '0.11.27'),
    worker('state', '0.22.3'),
    {
      ...worker('llm-router', '1.3.0'),
      source: 'path',
      commit: '852b87e0',
      dirty: true,
    },
  ],
)
const details = { [a.id]: a, [b.id]: b }
const started = []
const read = []
const trigger = (name, request = {}) => {
  const id = name.replace('e2e::dashboard::', '')
  if (id === 'executions-list')
    return {
      executions: [b, a].map(({ reports, ...summary }) => summary),
      total: 2,
    }
  if (id === 'execution-get') return { detail: details[request.execution_id] }
  if (id === 'evidence-read') {
    read.push(request)
    return {
      media_type: 'image/png',
      base64: png(request.execution_id.startsWith('a') ? 'a' : 'b'),
    }
  }
  if (id === 'catalog-get')
    return {
      scenarios: ['minimal_path', 'persistent_state', 'timer_wake'],
      models: [{ provider: 'deepseek', model: 'flash' }],
    }
  if (id === 'execution-start') {
    started.push(request)
    return { execution_id: 'plan-cccccccccccccccccccccccccccccccc' }
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

  // Tick A first, then B, and compare.
  await page.goto(`${server.url}#/ext/harness-e2e/executions`)
  await page
    .getByRole('checkbox', { name: 'Compare smoke', exact: true })
    .check()
  await page.getByRole('checkbox', { name: 'Compare smoke rerun' }).check()
  await page.getByRole('button', { name: 'compare', exact: true }).click()
  await page.locator('[data-comparison-scenarios]').waitFor()
  assert.match(
    await page.evaluate(() => location.hash),
    new RegExp(`/compare/${a.id}/${b.id}$`),
  )
  // Why a scenario is out, in the run's words; its state where a score would be.
  await page
    .getByText(
      'technical_invalid in A: infrastructure_error — scenario setup failed: UNKNOWN_DB primary',
    )
    .first()
    .waitFor()
  assert.equal(
    await page.locator('[data-scenario="timer_wake"] td').nth(1).innerText(),
    'infrastructure_error',
  )
  assert.equal(await page.getByText(/Not reported|Not comparable/).count(), 0)
  await page
    .getByText(
      'Different runners: 0.11.24 → 0.11.27 — scenario definitions and scoring may differ.',
    )
    .waitFor()
  assert.equal(
    await page.locator('[data-stack-summary]').innerText(),
    'stack · 1 worker from your code @852b87e (uncommitted changes) · 1 only in B',
  )

  // Both sides' screenshots, read on demand from their native runs.
  await page.getByRole('button', { name: 'persistent_state' }).click()
  await page.locator('[data-comparison-evidence="b"] img').waitFor()
  assert.equal(await page.locator('[data-comparison-evidence] img').count(), 2)
  assert.deepEqual(
    read.map((request) => [request.execution_id, request.pointer]).sort(),
    [a, b].map((side) => [
      `${side.id.slice(5, 36)}1`,
      '/attachments/board.png',
    ]),
  )

  // Rerun selected: Run again with B's parameters and only the ticked tests.
  for (const scenario of ['minimal_path', 'timer_wake'])
    await page
      .getByRole('checkbox', { name: `Select ${scenario} to run again` })
      .check()
  await page.getByRole('button', { name: 'rerun selected (2)' }).click()
  const again = page.getByRole('dialog', { name: 'Run again' })
  await again.waitFor()
  await again.getByText('Advanced · sampling, retries and seed').click()
  assert.equal(await again.locator('#quick-execution-runs').inputValue(), '3')
  assert.equal(
    await again.locator('#quick-execution-retries').inputValue(),
    '2',
  )
  assert.equal(
    await again.locator('#quick-execution-seed').inputValue(),
    '18446744073709551615',
  )
  assert.equal(
    await again.locator('#quick-execution-agent').inputValue(),
    'tech-lead',
  )
  for (const [scenario, checked] of [
    ['minimal_path', true],
    ['persistent_state', false],
    ['timer_wake', true],
  ])
    assert.equal(
      await again
        .getByRole('checkbox', { name: scenario, exact: true })
        .isChecked(),
      checked,
      scenario,
    )
  await again.getByRole('button', { name: 'run 2 tests', exact: true }).click()
  await page.waitForFunction(() => location.hash.includes('/execution/plan-c'))
  assert.deepEqual(started, [
    {
      label: '',
      parameters: {
        ...b.parameters,
        scenarios: ['minimal_path', 'timer_wake'],
      },
    },
  ])
  assert.deepEqual(errors, [])
  console.log(
    'Compare browser flow passed: tick A then B, exclusions, side-by-side screenshots, rerun selected with B parameters.',
  )
} finally {
  await browser.close()
  await server.close()
}
