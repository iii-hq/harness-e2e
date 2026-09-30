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
    // Enough of an assessment for the run's own pages to open.
    assessment: {
      run_id: id,
      system_status: technical === 'valid' ? 'passed' : 'infrastructure_error',
      assessments: [],
    },
    failures: failure ? [{ phase: 'setup', message: failure }] : [],
    deliverables: screenshot
      ? [
          {
            id: 'board',
            artifact: { path: `deliverables/${id}/board.json` },
            screenshots: [
              {
                pointer: '/attachments/board.png',
                caption: 'board',
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
// Two registry tests that the runner only runs together, in order.
const group = ['registry_implementation', 'registry_verification']
const parameters = (runs, suite) => ({
  suite,
  scenarios: ['minimal_path', 'persistent_state', 'timer_wake', ...group],
  runs,
  technical_retries: 2,
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
  // A suite of the master plan, as the workflow recorded it.
  parameters(1, {
    id: 'smoke',
    label: 'Smoke',
    sha256: 'sha256:0123456789abcdef0123456789abcdef',
  }),
  {
    minimal_path: run('a1', { score: 82 }),
    persistent_state: run('a2', { score: 100, screenshot: true }),
    timer_wake: run('a3', {
      technical: 'technical_invalid',
      score: null,
      failure: 'scenario setup failed: UNKNOWN_DB primary',
    }),
    registry_implementation: run('a4', { score: 70 }),
    registry_verification: run('a5', { score: 70 }),
  },
  [worker('harness-e2e', '0.11.24'), worker('state', '0.22.3')],
)
const b = execution(
  'plan-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
  'smoke rerun',
  { kind: 'local' },
  // The same tests ticked by hand: an unnamed suite.
  parameters(3, {
    label: '',
    sha256: 'sha256:fedcba9876543210fedcba9876543210',
  }),
  {
    minimal_path: run('b1', { score: 94 }),
    persistent_state: run('b2', { score: 62, screenshot: true }),
    timer_wake: run('b3', { score: 40 }),
    registry_implementation: run('b4', { score: 70 }),
    registry_verification: run('b5', { score: 70 }),
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
const renamed = []
const deleted = []
const reimported = []
// At the end fifty older executions join, to load older ones.
const older = Array.from({ length: 50 }, (_, index) => ({
  ...b,
  id: `plan-${String(index).padStart(32, 'e')}`,
  label: `older ${index}`,
  completed_at: '2026-09-01T11:00:00Z',
}))
let withOlder = false
// Refreshes of B that fail before one succeeds again.
let failB = 0
const trigger = (name, request = {}) => {
  const id = name.replace('e2e::dashboard::', '')
  if (id === 'executions-list') {
    const listed = [b, a, ...(withOlder ? older : [])].filter(
      (side) => !deleted.includes(side.id),
    )
    const start = Number(request.cursor ?? 0)
    const end = start + (request.limit ?? 50)
    return {
      executions: listed
        .slice(start, end)
        .map(({ reports, ...summary }) => summary),
      total: listed.length,
      next_cursor: end < listed.length ? String(end) : null,
    }
  }
  if (id === 'github-run-import') {
    reimported.push(request.run_id)
    return { execution_id: a.id, state: 'importing' }
  }
  if (id === 'execution-rename') {
    renamed.push(request)
    return details[request.execution_id].plan_execution
  }
  if (id === 'execution-delete') {
    // The worker refuses what has not finished, in its own words.
    if (request.execution_id === b.id)
      throw new Error('Only a finished execution can be deleted.')
    deleted.push(request.execution_id)
    return {}
  }
  if (id === 'execution-get') {
    if (request.execution_id === b.id && failB > 0) {
      failB -= 1
      throw new Error('engine unavailable')
    }
    return { detail: details[request.execution_id] }
  }
  if (id === 'evidence-read') {
    read.push(request)
    return {
      media_type: 'image/png',
      base64: png(request.execution_id.startsWith('a') ? 'a' : 'b'),
    }
  }
  if (id === 'catalog-get')
    return {
      scenarios: ['minimal_path', 'persistent_state', 'timer_wake', ...group],
      scenario_groups: [group],
      models: [{ provider: 'deepseek', model: 'flash' }],
    }
  if (id === 'execution-start') {
    started.push(request)
    return { execution_id: 'plan-cccccccccccccccccccccccccccccccc' }
  }
  if (id === 'suites-list') return { suites: [] }
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
    .getByRole('checkbox', { name: 'Select smoke', exact: true })
    .check()
  await page.getByRole('checkbox', { name: 'Select smoke rerun' }).check()
  const selection = page.getByRole('toolbar', { name: 'Selected executions' })
  await selection.getByText('A is the first you ticked.').waitFor()
  assert.equal(
    await page
      .locator('[title="Compared as A"]')
      .evaluate((mark) => mark.closest('tr')?.dataset.executionId),
    a.id,
  )
  await selection
    .getByRole('button', { name: 'Compare A and B', exact: true })
    .click()
  await page.locator('[data-comparison-scenarios]').waitFor()
  assert.match(
    await page.evaluate(() => location.hash),
    new RegExp(`/compare/${a.id}/${b.id}$`),
  )
  // The title is A × B; each side says where it ran and what it ran.
  await page
    .getByRole('heading', { name: 'smoke × smoke rerun', level: 1 })
    .waitFor()
  await page
    .getByRole('article', { name: 'A · Reference' })
    .getByText('deepseek/flash · profile tech-lead · Smoke', { exact: true })
    .waitFor()
  // Why a test is out, in the run's words; its state where a score would be.
  const picker = page.getByRole('region', { name: 'Tests in this comparison' })
  await picker
    .getByText(
      'Out by itself · technical_invalid in A: infrastructure_error — scenario setup failed: UNKNOWN_DB primary',
    )
    .waitFor()
  assert.equal(
    await picker.getByRole('checkbox', { name: /^timer_wake/ }).isChecked(),
    false,
  )
  assert.match(
    await page.locator('[data-scenario="timer_wake"] .cmp-score').innerText(),
    /infrastructure error → 40/,
  )
  assert.equal(await page.getByText(/Not reported|Not comparable/).count(), 0)
  // Bringing it back counts it, and the link says so.
  await picker.getByRole('checkbox', { name: /^timer_wake/ }).check()
  await page.waitForFunction(() =>
    location.hash.endsWith('?include=timer_wake'),
  )
  assert.equal(
    await page
      .locator('[data-scenario="timer_wake"]')
      .getAttribute('data-counted'),
    'true',
  )
  await picker.getByRole('button', { name: 'Automatic', exact: true }).click()
  await page.waitForFunction(() => !location.hash.includes('?'))
  // The suite difference names each side's suite by name and digest.
  assert.equal(
    await page.locator('[data-change="suite"] .ds-fact-value').innerText(),
    'Smoke · 0123456789ab → unnamed suite · fedcba987654',
  )
  await page
    .getByText(
      'Different runners: 0.11.24 → 0.11.27 — scenario definitions and scoring may differ.',
    )
    .waitFor()
  assert.equal(
    await page.locator('[data-change="stack"] .ds-fact-value').innerText(),
    '1 worker changed · 1 only in B',
  )
  // Stack details opens the stack worker by worker, and focus lands on it.
  await page.getByRole('button', { name: 'Stack details' }).click()
  await page.locator('[data-stack-worker="harness-e2e"]').waitFor()
  await page.waitForFunction(
    () => document.activeElement?.closest('#comparison-stack') !== null,
  )
  assert.equal(
    await page.locator('[data-stack-only="b"] dd').innerText(),
    'llm-router',
  )
  // No verdict anywhere.
  assert.doesNotMatch(
    await page.locator('[data-harness-e2e-dashboard]').innerText(),
    /better|worse|improv|regress|winner/i,
  )

  // Both sides' screenshots, paired by caption, read on demand from their
  // native runs; one opens full size.
  await page
    .getByRole('button', { name: 'persistent_state', exact: true })
    .click()
  await page.locator('[data-comparison-evidence="b"] img').waitFor()
  assert.equal(await page.locator('[data-screenshot-pair]').count(), 1)
  assert.equal(await page.locator('[data-comparison-evidence] img').count(), 2)
  assert.deepEqual(
    read.map((request) => [request.execution_id, request.pointer]).sort(),
    [a, b].map((side) => [
      `${side.id.slice(5, 36)}1`,
      '/attachments/board.png',
    ]),
  )
  // Opened without focusing the button, as WebKit clicks: focus still
  // comes back to it.
  await page
    .getByRole('button', { name: 'Open A · board full size' })
    .dispatchEvent('click')
  const viewer = page.getByRole('dialog', { name: 'board' })
  await viewer.getByRole('link', { name: 'Evidence record' }).waitFor()
  await page.keyboard.press('Escape')
  await viewer.waitFor({ state: 'detached' })
  // Focus is back on the screenshot it opened from.
  assert.equal(
    await page.evaluate(() =>
      document.activeElement?.getAttribute('aria-label'),
    ),
    'Open A · board full size',
  )

  // A run's transcript opened from the comparison goes back to it.
  await page
    .locator('[data-run-side="b"]')
    .getByRole('link', { name: 'Transcript', exact: true })
    .first()
    .click()
  await page.getByRole('link', { name: 'Back to comparison' }).click()
  await page.locator('[data-comparison-scenarios]').waitFor()
  assert.match(
    await page.evaluate(() => location.hash),
    /^#\/ext\/harness-e2e\/compare\//,
  )

  // Run again: B's parameters, on the tests B scored lower on.
  await page.getByRole('button', { name: 'Run again · 1 test' }).click()
  const again = page.getByRole('dialog', { name: 'Run again' })
  await again.waitFor()
  await again.getByText('catalog ready').waitFor()
  assert.ok(
    await again
      .getByRole('checkbox', { name: /^persistent_state(\s|$)/ })
      .isChecked(),
  )
  assert.equal(
    await again.getByRole('checkbox', { name: /^minimal_path(\s|$)/ }).count(),
    0,
  )
  await page.keyboard.press('Escape')
  await again.waitFor({ state: 'detached' })

  // Run this test again from an open row, and tick more in the dialog; one
  // test of a sequential group brings the whole group.
  await page.getByRole('button', { name: 'minimal_path', exact: true }).click()
  await page
    .locator('[data-scenario-detail="minimal_path"]')
    .getByRole('button', { name: 'Run this test again' })
    .click()
  await again.waitFor()
  await again.getByText('catalog ready').waitFor()
  assert.equal(await again.locator('#run-dialog-runs-value').innerText(), '3')
  assert.equal(
    await again.locator('#run-dialog-technicalRetries-value').innerText(),
    '2',
  )
  // No seed: the Console always runs the canonical case.
  assert.doesNotMatch(await again.textContent(), /seed/i)
  assert.equal(
    await again.locator('#run-dialog-agent').inputValue(),
    'tech-lead',
  )
  await again
    .getByRole('group', { name: 'Show' })
    .getByRole('button', { name: /^All/ })
    .click()
  for (const scenario of ['timer_wake', 'registry_verification'])
    await again
      .getByRole('checkbox', { name: new RegExp(`^${scenario}(\\s|$)`) })
      .check()
  for (const scenario of ['minimal_path', 'timer_wake', ...group])
    assert.ok(
      await again
        .getByRole('checkbox', { name: new RegExp(`^${scenario}(\\s|$)`) })
        .isChecked(),
    )
  assert.equal(
    await again
      .getByRole('checkbox', { name: /^persistent_state(\s|$)/ })
      .isChecked(),
    false,
  )
  await again.getByRole('button', { name: 'Run 4 tests', exact: true }).click()
  await page.waitForFunction(() => location.hash.includes('/execution/plan-c'))
  assert.equal(started.length, 1)
  assert.deepEqual(
    {
      ...started[0].parameters,
      scenarios: [...started[0].parameters.scenarios].sort(),
    },
    {
      ...b.parameters,
      // A subset ticked by hand is an unnamed suite.
      suite: null,
      scenarios: ['minimal_path', ...group, 'timer_wake'].sort(),
      where: 'harness',
    },
  )
  assert.equal(started[0].label, b.label)

  // B still running is followed; a refresh that fails keeps the comparison
  // on screen, its open row included, says so, and clears once one works.
  b.status = 'running'
  await page.goto(`${server.url}#/ext/harness-e2e/compare/${a.id}/${b.id}`)
  await page.locator('[data-comparison-live]').waitFor()
  await page.getByRole('button', { name: 'minimal_path', exact: true }).click()
  const open = page.locator('[data-scenario-detail="minimal_path"]')
  await open.waitFor()
  failB = 1
  const refresh = page.locator('[data-comparison-refresh-error]')
  await refresh.getByText('engine unavailable').waitFor({ timeout: 15_000 })
  assert.ok(await open.isVisible())
  await refresh.waitFor({ state: 'detached', timeout: 15_000 })
  assert.ok(await open.isVisible())
  b.status = 'passed'

  // From the list: rename one through its menu, then delete both; the one
  // the worker refuses stays, with the refusal said.
  await page.goto(`${server.url}#/ext/harness-e2e/executions`)
  await page
    .getByRole('button', { name: 'Actions for smoke', exact: true })
    .click()
  await page.getByRole('menuitem', { name: 'Rename…' }).click()
  const rename = page
    .getByRole('dialog')
    .filter({ hasText: 'Rename execution' })
  await rename.getByRole('textbox', { name: 'Execution name' }).fill('smoke A')
  await rename.getByRole('button', { name: 'Save', exact: true }).click()
  await rename.waitFor({ state: 'detached' })
  assert.deepEqual(renamed, [{ execution_id: a.id, label: 'smoke A' }])
  // Focus goes back to the menu button the rename came from.
  await page.waitForFunction(
    (id) =>
      document.activeElement?.getAttribute('aria-label') ===
        'Actions for smoke' &&
      document.activeElement.closest('[data-execution-id]')?.dataset
        .executionId === id,
    a.id,
  )

  // Import again sends the run back through the import; Copy says it copied.
  await page
    .getByRole('button', { name: 'Actions for smoke', exact: true })
    .click()
  await page.getByRole('menuitem', { name: /^Import again/ }).click()
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write'])
  await page
    .getByRole('button', { name: 'Actions for smoke', exact: true })
    .click()
  await page.getByRole('menuitem', { name: 'Copy execution id' }).click()
  await page.locator('.ex-flash').getByText(`Copied ${a.id}.`).waitFor()
  assert.deepEqual(reimported, [42])
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), a.id)

  // Run again from the menu opens the form on that execution's parameters.
  await page
    .getByRole('button', { name: 'Actions for smoke rerun', exact: true })
    .click()
  await page.getByRole('menuitem', { name: 'Run again' }).click()
  const rerun = page.getByRole('dialog', { name: 'Run again' })
  assert.equal(await rerun.locator('#run-dialog-runs-value').innerText(), '3')
  await page.keyboard.press('Escape')
  await rerun.waitFor({ state: 'detached' })

  await page
    .getByRole('checkbox', { name: 'Select every execution shown' })
    .check()
  await selection.getByText('2 selected').waitFor()
  await selection.getByRole('button', { name: 'Delete 2', exact: true }).click()
  const confirm = page.getByRole('alertdialog')
  await confirm.getByText('Delete 2 executions?').waitFor()
  await confirm
    .getByText('The run on GitHub is not touched. You can import #42 again.')
    .waitFor()
  await confirm
    .getByRole('button', { name: 'Delete 2 executions', exact: true })
    .click()
  await page
    .locator('.ex-flash')
    .getByText('Deleted “smoke” with its runs and evidence.')
    .waitFor()
  await page.getByText('Couldn’t delete “smoke rerun”').waitFor()
  await page
    .getByText('Only a finished execution can be deleted.', { exact: false })
    .waitFor()
  assert.deepEqual(deleted, [a.id])
  // The refused one stays ticked, and focus is back on the selection bar.
  await page.waitForFunction(() =>
    document.activeElement?.closest('[data-selection-bar]'),
  )
  await page
    .locator(`[data-execution-id="${a.id}"]`)
    .waitFor({ state: 'detached' })
  await page.locator(`[data-execution-id="${b.id}"]`).waitFor()

  // Older executions arrive by cursor, and stay loaded after an action.
  withOlder = true
  await page.reload()
  const last = older.at(-1).id
  await page
    .getByRole('button', { name: 'Load older executions · 50 of 51 loaded' })
    .click()
  await page.locator(`[data-execution-id="${last}"]`).waitFor()
  await page
    .getByRole('button', { name: 'Actions for smoke rerun', exact: true })
    .click()
  await page.getByRole('menuitem', { name: 'Rename…' }).click()
  await rename.getByRole('button', { name: 'Save', exact: true }).click()
  await rename.waitFor({ state: 'detached' })
  await page.waitForTimeout(300)
  assert.equal(await page.locator(`[data-execution-id="${last}"]`).count(), 1)

  assert.deepEqual(errors, [])
  console.log(
    'Compare browser flow passed: tick A then B, A × B with both sides, suite difference by name and digest, an exclusion with its reason brought back and restored through the URL, the stack worker by worker, screenshots paired by caption and opened full size, a run’s transcript that goes back to the comparison, Run again of B on the tests it scored lower on, a test run again with more ticked in the dialog on B parameters, a running side whose refresh fails once keeps the comparison and its open row; rename, import again, copy the id and run again from the row menu, focus back on the row, load older, delete the selection with a refusal said.',
  )
} finally {
  await browser.close()
  await server.close()
}
