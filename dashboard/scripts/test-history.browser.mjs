// Deterministic browser coverage for a test's history and the A × B of two
// of its runs. No models run: the worker's answers come from the fixture.
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { chromium } from 'playwright'
import { createConsoleTestHost } from './console-test-host.mjs'

const fixture = JSON.parse(
  readFileSync(
    new URL('../src/test-fixtures/test-history.json', import.meta.url),
  ),
)
const formFlow = fixture.history
const row = fixture.catalog_row
const at = (hour) =>
  formFlow.observations.find((item) => item.completed_at.includes(`T${hour}`))
const key = (item) => `${item.execution_id}:${item.case_id}`
const tree = at('12:13') // 9:13 AM in the canvas: a Tech Lead and two more
const solo = at('13:01') // 10:01 AM: one session

// A second test whose current definition never ran.
const moved = `sha256:${'5e'.repeat(32)}`
const stale = {
  ...formFlow,
  test_id: 'shell_coder_sandbox',
  current_version: moved,
  available_versions: [
    ...formFlow.available_versions,
    { version: moved, execution_count: 0, run_count: 0, last_seen: null },
  ],
}

const requests = []
const trigger = (name, request = {}) => {
  const id = name.replace('e2e::dashboard::', '')
  if (id === 'tests-list')
    return {
      revision: 'r1',
      rows: [
        { ...row, test_id: 'fanout_ladder', spec: null },
        row,
        { ...row, test_id: 'shell_coder_sandbox', current_version: moved },
      ],
      total: 3,
      next_cursor: null,
    }
  if (id === 'test-history-get') {
    requests.push(request)
    const source = request.test_id === 'shell_coder_sandbox' ? stale : formFlow
    if (
      request.test_version !== 'all' &&
      !source.available_versions.some(
        (item) => item.version === request.test_version,
      )
    )
      throw new Error(
        `unknown test '${request.test_id}' version ${request.test_version}`,
      )
    const observations = source.observations.filter(
      (item) =>
        (request.test_version === 'all' ||
          item.behavior_sha256 === request.test_version) &&
        (!request.subject_model ||
          item.subject_model === request.subject_model) &&
        (!request.executions || request.executions.includes(item.execution_id)),
    )
    return {
      ...source,
      test_version: request.test_version,
      observations,
      total: observations.length,
    }
  }
  throw new Error(`Unexpected RPC ${name}`)
}

const server = await createConsoleTestHost()
const browser = await chromium.launch({ headless: true })
try {
  const page = await browser.newPage({
    viewport: { width: 1440, height: 1000 },
  })
  await server.install(page, trigger)
  page.setDefaultTimeout(10_000)
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))

  await page.goto(`${server.url}#/ext/harness-e2e/tests/form_flow_build`)
  await page
    .getByRole('heading', { name: 'form_flow_build', level: 1 })
    .waitFor()
  // Every definition by default, current first; the chips say so.
  assert.equal(requests[0].test_version, 'all')
  await page
    .getByRole('listitem', { name: 'Runs: 6 in 3 definitions' })
    .waitFor()
  const definitions = page.getByRole('radiogroup', { name: 'Definition' })
  await definitions.getByRole('radio', { name: /^All/ }).waitFor()
  const table = page.locator('[data-history-table]')
  assert.equal(await table.locator('[data-run-key]').count(), 6)
  assert.equal(await page.locator('[data-stale-definition]').count(), 0)

  // The chart follows the metric picked.
  await page
    .getByRole('radiogroup', { name: 'Metric' })
    .getByRole('radio', { name: 'Duration' })
    .click()
  await page.getByRole('heading', { name: 'Duration by run' }).waitFor()
  await page
    .getByText('Wall time per run, retries included.', { exact: false })
    .waitFor()

  // A run opens on its session tree.
  const when = await page
    .locator(`[data-run-key="${key(tree)}"] .th-strong`)
    .innerText()
  await page.getByRole('button', { name: `Details of the ${when} run` }).click()
  const detail = page.locator(`[id="th-detail-${key(tree)}"]`)
  await detail
    .getByText(
      '1 root session + 3 sub-agents, 2 levels deep · 196 turns in all',
    )
    .waitFor()
  assert.equal(await detail.locator('[data-session]').count(), 4)
  await detail.getByText('tech-lead · spawned by the root').waitFor()
  await detail.getByText('runtime_contract').waitFor()
  assert.match(
    await detail
      .getByRole('link', { name: 'Evidence record' })
      .getAttribute('href'),
    new RegExp(`/execution/${tree.plan_execution_id}/run/`),
  )

  // Results filter the list only.
  await page
    .getByRole('radiogroup', { name: 'Result' })
    .getByRole('radio', { name: /^Lost points/ })
    .click()
  await page.waitForFunction(
    () => document.querySelectorAll('[data-run-key]').length === 3,
  )
  assert.match(await page.evaluate(() => location.hash), /result=lost/)
  await page
    .getByRole('radiogroup', { name: 'Result' })
    .getByRole('radio', { name: /^All/ })
    .click()

  // Tick A then B; a third cannot be ticked.
  const compare = page.locator('[data-compare-runs]')
  assert.equal(await compare.isDisabled(), true)
  const soloWhen = await page
    .locator(`[data-run-key="${key(solo)}"] .th-strong`)
    .innerText()
  await page.getByRole('checkbox', { name: `Pick ${when} for A/B` }).check()
  await page.getByText('A ticked · tick B').waitFor()
  await page.getByRole('checkbox', { name: `Pick ${soloWhen} for A/B` }).check()
  await page.getByText('A and B ticked').waitFor()
  assert.equal(
    await page
      .locator('[data-run-key] input[type="checkbox"]:disabled')
      .count(),
    4,
  )
  await page.getByRole('link', { name: 'Compare A and B' }).click()

  // A × B: both sides, what changed, the criterion that changed and why.
  await page
    .getByRole('heading', { name: 'Two runs of form_flow_build', level: 1 })
    .waitFor()
  assert.match(
    await page.evaluate(() => location.hash),
    new RegExp(
      `/tests/form_flow_build/compare\\?a=${encodeURIComponent(key(tree))}&b=${encodeURIComponent(key(solo))}$`,
    ),
  )
  // Only the two runs are asked for, wherever they sit in the history.
  assert.deepEqual(
    [...requests.at(-1).executions].sort(),
    [tree.execution_id, solo.execution_id].sort(),
  )
  await page
    .getByRole('article', { name: 'A · Reference' })
    .getByText('profile ade-worker-builder', { exact: false })
    .waitFor()
  await page
    .getByRole('listitem', {
      name: 'profile: ade-worker-builder → ade-solo-builder',
    })
    .waitFor()
  const criterion = page.locator('[data-criterion="runtime_contract"]')
  await criterion
    .getByText('compose_valid=true, worker_ready=true, function_surface=false')
    .waitFor()
  await page
    .getByText('root session only; A also ran 3 child sessions')
    .waitFor()
  await page.locator('[data-worker="form_flow"]').getByText('17').waitFor()
  await page.getByRole('heading', { name: 'A · Sub-agents' }).waitFor()
  await page.getByText('+10 pts').waitFor()
  // Swapping reads every difference the other way round.
  await page.getByRole('button', { name: 'Swap A and B' }).click()
  await page.getByText('−10 pts').waitFor()
  assert.match(
    await page.evaluate(() => location.hash),
    new RegExp(
      `\\?a=${encodeURIComponent(key(solo))}&b=${encodeURIComponent(key(tree))}$`,
    ),
  )
  // Back returns to the order before the swap; a link with the executions
  // alone, as older links carry, opens their first cases.
  await page.goBack()
  await page.getByText('+10 pts').waitFor()
  await page.evaluate(
    ([a, b]) => {
      location.hash = `#/ext/harness-e2e/tests/form_flow_build/compare?a=${a}&b=${b}`
    },
    [solo.execution_id, tree.execution_id],
  )
  await page.getByText('−10 pts').waitFor()
  await page
    .getByRole('article', { name: 'A · Reference' })
    .getByText('profile ade-solo-builder', { exact: false })
    .waitFor()
  await page.getByRole('link', { name: 'Back to form_flow_build' }).click()
  await page
    .getByRole('heading', { name: 'form_flow_build', level: 1 })
    .waitFor()

  // One definition at a time, from the link too.
  const zero = formFlow.available_versions.find(
    (item) => item.execution_count === 4,
  ).version
  await definitions.getByRole('radio', { name: /^0c5c0902/ }).click()
  await page.waitForFunction(
    () => document.querySelectorAll('[data-run-key]').length === 4,
  )
  assert.equal(requests.at(-1).test_version, zero)
  assert.match(
    await page.evaluate(() => location.hash),
    new RegExp(`definition=${encodeURIComponent(zero)}`),
  )

  // An older link: a definition the history no longer holds falls back to
  // every definition and says so; the old result filter still filters.
  await page.goto(
    `${server.url}#/ext/harness-e2e/tests/form_flow_build?definition=sha256:${'de'.repeat(32)}&result=failed`,
  )
  await page.reload()
  await page
    .getByText('That definition is no longer in this test’s history.')
    .waitFor()
  assert.equal(
    await definitions.getByRole('radio', { name: /^All/ }).isChecked(),
    true,
  )
  await page.waitForFunction(
    () => document.querySelectorAll('[data-run-key]').length === 3,
  )
  assert.equal(
    await page
      .getByRole('radiogroup', { name: 'Result' })
      .getByRole('radio', { name: /^Lost points/ })
      .isChecked(),
    true,
  )

  // The current definition has not run: say so, and offer to run it.
  await page.goto(`${server.url}#/ext/harness-e2e/tests/shell_coder_sandbox`)
  const notice = page.locator('[data-stale-definition]')
  await notice
    .getByText('The current definition 5e5e5e5e hasn’t run yet.', {
      exact: false,
    })
    .waitFor()
  await notice
    .getByRole('link', { name: 'Run on the current definition' })
    .waitFor()

  // A narrow pane keeps the run, its score and its actions.
  await page.setViewportSize({ width: 640, height: 1000 })
  await page.goto(`${server.url}#/ext/harness-e2e/tests/form_flow_build`)
  await page.locator('[data-history-table][data-narrow]').waitFor()
  assert.equal(
    await page.getByRole('columnheader', { name: 'Model · profile' }).count(),
    0,
  )

  await page.setViewportSize({ width: 390, height: 1000 })
  const detailToggle = page.getByRole('button', {
    name: `Details of the ${when} run`,
  })
  if ((await detailToggle.getAttribute('aria-expanded')) !== 'true') {
    await detailToggle.click()
  }
  assert.equal(await detailToggle.getAttribute('aria-expanded'), 'true')
  await detail.waitFor({ state: 'visible' })
  const narrowCriteria = detail.locator('.th-criterion-history')
  await narrowCriteria.waitFor({ state: 'visible' })
  assert.equal(
    await narrowCriteria.evaluate(
      (element) =>
        element.clientWidth > 0 && element.scrollWidth <= element.clientWidth,
    ),
    true,
  )

  assert.deepEqual(errors, [])
  console.log(
    'Test history browser flow passed: every definition by default with chips, the chart by metric, a run opened on its session tree and links, the result filter in the URL, A then B ticked and a third refused, the A × B with what changed, the criterion that changed and why, calls by worker, both session trees and swap, back to the history, one definition at a time, the not-yet-run notice, narrow pane.',
  )
} finally {
  await browser.close()
  await server.close()
}
