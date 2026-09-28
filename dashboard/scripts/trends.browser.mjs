// Deterministic browser coverage for Trends: a series over time, what
// changed at a diamond with the commits between, a point against the
// previous counted one, Compare with it and back, the stack and series
// pickers, a small chart in the large one's place, the empty state's Run
// again and the narrow pane. No models run and no worker answers: the
// series are the canvas's, from src/test-fixtures/trends.json.
import assert from 'node:assert/strict'
import { chromium } from 'playwright'
import { createConsoleTestHost } from './console-test-host.mjs'
import { compareAnswer, fixture, trendsAnswer } from './trends-fixture.mjs'

const opus = fixture.series.find((item) => item.key === 'opus')
const opusDetail = {
  id: opus.points[0].execution_id,
  label: null,
  status: 'completed',
  started_at: opus.points[0].started_at,
  subjects: [],
  reports: [],
  totals: {},
  parameters: {
    suite: { id: 'regression', label: 'Regression' },
    scenarios: opus.points[0].planned,
    runs: 1,
    technical_retries: 1,
    model: 'claude-opus-5-5',
    provider: 'anthropic',
    agent: null,
    where: 'harness',
  },
}

const calls = []
const requests = (id) =>
  calls.filter((call) => call.id === id).map((call) => call.request)
const trigger = (name, request = {}) => {
  const id = name.replace('e2e::dashboard::', '')
  calls.push({ id, request })
  if (id === 'trends-get') return trendsAnswer(request)
  if (id === 'version-compare') return compareAnswer(request)
  if (id === 'execution-get' && request.execution_id === opusDetail.id)
    return { detail: opusDetail }
  if (id === 'catalog-get')
    return {
      scenarios: opusDetail.parameters.scenarios,
      scenario_groups: [],
      models: [{ provider: 'anthropic', model: 'claude-opus-5-5' }],
    }
  if (id === 'suites-list') return { suites: [] }
  throw new Error(`Unexpected RPC ${name}`)
}

const server = await createConsoleTestHost()
const browser = await chromium.launch({
  headless: true,
  args: ['--disable-dev-shm-usage'],
})
try {
  const page = await browser.newPage({
    viewport: { width: 1440, height: 1000 },
  })
  await server.install(page, trigger)
  page.setDefaultTimeout(10_000)
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))
  const summary = page.locator('[data-trend-summary]')
  const hash = () => page.evaluate(() => location.hash)

  // The series with the latest execution, on its latest execution's stack.
  await page.goto(`${server.url}#/ext/harness-e2e/trends`)
  await page.getByRole('heading', { name: 'Trends', level: 1 }).waitFor()
  assert.equal(
    await page
      .getByRole('link', { name: 'Trends', exact: true })
      .getAttribute('aria-current'),
    'page',
  )
  assert.deepEqual(requests('trends-get')[0], {})
  await summary
    .getByText(
      '11 executions · 9 with counted runs, 2 without · 81 counted runs',
      { exact: false },
    )
    .waitFor()
  const series = page.locator('[data-series-picker]')
  assert.match(
    await series.innerText(),
    /Regression[\s\S]*deepseek\/deepseek-flash[\s\S]*none/,
  )
  assert.match(await page.locator('[data-stack-picker]').innerText(), /default/)
  await page
    .getByText('4 of these ran before the Console recorded stacks', {
      exact: false,
    })
    .waitFor()

  // The Sep 26 diamond: no counted run, iii and the runner moved; the
  // commits between are asked only now.
  assert.equal(requests('version-compare').length, 0)
  await page
    .getByRole('button', { name: /^What changed · .+: iii, harness-e2e$/ })
    .click()
  const failed = page.locator('[data-trend-panel="github-36220337119-1"]')
  await failed
    .getByText('did not register e2e::scenarios-list within 300 s', {
      exact: false,
    })
    .waitFor()
  const iii = failed.getByRole('link', { name: '9 commits between the tags' })
  assert.equal(
    await iii.getAttribute('href'),
    'https://github.com/iii-hq/iii/compare/iii/v0.24.2-rc.2...iii/v0.24.3-rc.1',
  )
  await failed
    .getByRole('link', { name: '6 commits between the tags' })
    .waitFor()
  assert.deepEqual(requests('version-compare')[0], {
    name: 'iii',
    base: '0.24.2-rc.2',
    head: '0.24.3-rc.1',
  })
  assert.equal(
    await failed.getByRole('link', { name: /^Compare with/ }).count(),
    0,
  )

  // The latest point: its measures against the previous counted execution
  // and Compare with that one.
  await page
    .getByRole('button', { name: /· Score 93\.0$/ })
    .last()
    .click()
  const latest = page.locator('[data-trend-panel="github-36381232467-1"]')
  await latest.getByText('−4.4', { exact: false }).first().waitFor()
  await latest
    .getByRole('link', { name: '2 commits between the tags' })
    .waitFor()
  assert.match(
    await latest
      .getByRole('link', { name: /^Compare with/ })
      .getAttribute('href'),
    /\/compare\/github-36296751640-1\/github-36381232467-1\?from=/,
  )
  assert.equal(
    await page.locator('[data-by-test] [aria-pressed="true"]').count(),
    1,
  )
  await latest.getByRole('button', { name: 'Close' }).click()
  assert.equal(await page.locator('[data-trend-panel]').count(), 0)

  // A small chart takes the large one's place.
  await page.getByRole('button', { name: 'Run duration', exact: true }).click()
  await page
    .locator('[data-trend-chart="duration"]')
    .getByRole('heading', { name: 'Run duration' })
    .waitFor()
  assert.equal(await page.locator('[data-trend-mini="score"]').count(), 1)

  // Every stack of the series, kept in the hash.
  await page.locator('[data-stack-picker]').click()
  await page.getByRole('menuitemradio', { name: /^any/ }).click()
  await summary.getByText('14 executions ·', { exact: false }).waitFor()
  assert.equal(requests('trends-get').at(-1).stack, 'any')
  assert.equal(requests('trends-get').at(-1).suite, 'regression')
  assert.match(await hash(), /\/trends\?suite=regression&.*stack=any$/)
  const here = await hash()

  // Compare with the previous counted execution, and back to this view.
  await page
    .getByRole('button', { name: /Sep 28, .+ · Run duration 29s$/ })
    .last()
    .click()
  await page.getByRole('link', { name: /^Compare with/ }).click()
  const back = page.getByRole('link', { name: 'Back to Trends' })
  await back.waitFor()
  assert.equal(await back.getAttribute('href'), here)
  await back.click()
  await summary.getByText('14 executions ·', { exact: false }).waitFor()
  assert.equal(await hash(), here)

  // Another series: two executions, the stack not recorded, two planned
  // tests that did not run.
  await series.click()
  await page
    .getByRole('menuitemradio', { name: /^Software engineering/ })
    .click()
  await summary.getByText('2 executions ·', { exact: false }).waitFor()
  assert.deepEqual(requests('trends-get').at(-1), {
    suite: 'software-engineering',
    provider: 'deepseek',
    model: 'deepseek-flash',
    profile: 'ade-worker-builder',
  })
  const stack = page.locator('[data-stack-picker]')
  assert.equal(await stack.isDisabled(), true)
  assert.match(await stack.innerText(), /not recorded/)
  assert.equal(
    await page.locator('[data-by-test] [data-kind="not_run"]').count(),
    2,
  )

  // This harness: the runner moved between two checkouts.
  await series.click()
  await page.getByRole('menuitemradio', { name: /^2 tests, unsaved/ }).click()
  await summary.getByText('5 executions ·', { exact: false }).waitFor()
  assert.match(await stack.innerText(), /this harness/)
  await page
    .getByRole('button', { name: /^What changed · .+: harness-e2e$/ })
    .click()
  assert.equal(
    await page
      .getByRole('link', { name: '1 commit between them on GitHub' })
      .getAttribute('href'),
    'https://github.com/iii-hq/harness-e2e/compare/7a16130...2f8826a',
  )
  await page.getByText('* the checkout also had uncommitted edits').waitFor()

  // Nothing counted: say why, open the execution or run it again.
  await series.click()
  await page
    .getByRole('menuitemradio', { name: /^Regression · anthropic/ })
    .click()
  await page.getByRole('heading', { name: 'Nothing to draw yet' }).waitFor()
  assert.match(
    await page
      .getByRole('link', { name: 'Open the execution' })
      .getAttribute('href'),
    new RegExp(`/execution/${opusDetail.id}$`),
  )
  await page.getByRole('button', { name: 'Run again' }).click()
  await page.getByRole('dialog', { name: 'Run again' }).waitFor()
  assert.equal(requests('execution-get').at(-1).execution_id, opusDetail.id)

  // A narrow pane: the panel under the chart, the table's short form.
  await page.setViewportSize({ width: 640, height: 1000 })
  await page.goto(`${server.url}#/ext/harness-e2e/trends`)
  await page.reload()
  await page.locator('[data-trend-executions][data-narrow]').waitFor()
  assert.equal(
    await page.getByRole('columnheader', { name: 'Runner' }).count(),
    0,
  )
  await page
    .getByRole('button', { name: /· Score 93\.0$/ })
    .last()
    .click()
  await page.locator('[data-trend-panel]').waitFor()
  assert.equal(await page.locator('.tr-top[data-panel]').count(), 0)

  assert.deepEqual(errors, [])
  console.log(
    'Trends browser flow passed: the latest series on its stack with the Trends tab current, the Sep 26 diamond with the commits asked when it opened, the latest point against the previous counted one, a small chart in the large one’s place, every stack kept in the hash, Compare with and back to the same view, a series with planned tests not run, commits between two checkouts, the empty state’s Run again, narrow pane.',
  )
} finally {
  await browser.close()
  await server.close()
}
