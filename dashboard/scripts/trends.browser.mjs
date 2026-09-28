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

// An execution that lands while the page is open.
const regression = fixture.series.find((item) => item.key === 'regression')
const newer = {
  ...structuredClone(regression.points.at(-1)),
  execution_id: 'github-36400000001-1',
  started_at: '2026-09-28T14:17:00-03:00',
}
let landed = false
// Another series has the newest execution: the default view would move.
let elsewhere = false
const se = {
  suite: 'software-engineering',
  provider: 'deepseek',
  model: 'deepseek-flash',
  profile: 'ade-worker-builder',
}
// The default period, the last 30 days, on Sep 28, 2026 in UTC−3.
const last30 = {
  since: '2026-08-30T03:00:00.000Z',
  until: '2026-09-29T02:59:59.999Z',
}
const pinned = {
  suite: 'regression',
  provider: 'deepseek',
  model: 'deepseek-flash',
  profile: null,
  stack: 'any',
  ...last30,
}
// trends-get answers that fail before one succeeds again.
let failing = 0

const calls = []
const requests = (id) =>
  calls.filter((call) => call.id === id).map((call) => call.request)
const trigger = (name, request = {}) => {
  const id = name.replace('e2e::dashboard::', '')
  calls.push({ id, request })
  if (id === 'trends-get') {
    if (failing > 0) {
      failing -= 1
      throw new Error('engine unavailable')
    }
    const answer = trendsAnswer(elsewhere && !request.suite ? se : request)
    if (
      landed &&
      answer.selected.provider === 'deepseek' &&
      answer.stack !== 'not_recorded' &&
      answer.selected.suite === 'regression'
    )
      answer.points = [...answer.points, newer]
    return answer
  }
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
  // The fixture's executions ran in UTC−3 in 2026: read them there and then.
  const page = await browser.newPage({
    viewport: { width: 1440, height: 1000 },
    timezoneId: 'America/Sao_Paulo',
  })
  await page.clock.setFixedTime(new Date('2026-09-28T15:00:00-03:00'))
  await server.install(page, trigger)
  page.setDefaultTimeout(10_000)
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))
  const summary = page.locator('[data-trend-summary]')
  const hash = () => page.evaluate(() => location.hash)

  // The series with the latest execution, on every stack it ran on.
  // A first load that fails says so on the host's StatusPanel and retries.
  failing = 1
  await page.goto(`${server.url}#/ext/harness-e2e/trends`)
  await page.getByRole('heading', { name: 'Trends', level: 1 }).waitFor()
  await page.getByText('Couldn’t load the trend').waitFor()
  await page.getByText('engine unavailable').waitFor()
  await page.getByRole('button', { name: 'Retry' }).click()
  assert.equal(
    await page
      .getByRole('link', { name: 'Trends', exact: true })
      .getAttribute('aria-current'),
    'page',
  )
  assert.deepEqual(requests('trends-get')[0], last30)
  await summary
    .getByText(
      '14 executions in the last 30 days · 11 with counted runs, 3 without · 99 counted runs',
      { exact: false },
    )
    .waitFor()
  const suite = page.locator('[data-picker="suite"]')
  const model = page.locator('[data-picker="model"]')
  const profile = page.locator('[data-picker="profile"]')
  assert.match(await suite.innerText(), /Regression/)
  assert.match(await model.innerText(), /deepseek\/deepseek-flash/)
  assert.match(await profile.innerText(), /none/)
  assert.match(await page.locator('[data-stack-picker]').innerText(), /any/)
  await page
    .getByText('4 of these ran before the Console recorded stacks', {
      exact: false,
    })
    .waitFor()

  // The Sep 26 diamond: no counted run, iii moved; the commits between are
  // asked only now.
  assert.equal(requests('version-compare').length, 0)
  await page.getByRole('button', { name: /^What changed · .+: iii$/ }).click()
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
  assert.deepEqual(requests('version-compare')[0], {
    name: 'iii',
    base: '0.24.2-rc.2',
    head: '0.24.3-rc.1',
  })
  assert.equal(
    await failed.getByRole('link', { name: /^Compare with/ }).count(),
    0,
  )

  // The Harness moved on Sep 24: its tags in iii-hq/workers.
  await page
    .getByRole('button', {
      name: /^What changed · .+: ade, harness, iii-directory, shell_coder_sandbox$/,
    })
    .click()
  assert.equal(
    await page
      .getByRole('link', {
        name: '19 commits between the tags in iii-hq/workers',
      })
      .getAttribute('href'),
    'https://github.com/iii-hq/workers/compare/harness/v1.8.31...harness/v1.8.34',
  )

  // The latest point: its measures against the previous counted execution
  // and Compare with that one.
  await page
    .getByRole('button', { name: /· Score 93\.0$/ })
    .last()
    .click()
  const latest = page.locator('[data-trend-panel="github-36381232467-1"]')
  await latest.getByText('−4.4', { exact: false }).first().waitFor()
  await latest.getByText('Nothing recorded changed', { exact: false }).waitFor()
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
  // The default view is pinned to what it showed, in the hash too.
  assert.match(
    await hash(),
    /\/trends\?suite=regression&provider=deepseek&model=deepseek-flash&profile=&stack=any&range=30d$/,
  )

  // A run lands, and another series has the newest execution: the trend
  // reloads quietly on the same series and stack and keeps the pick.
  const asked = requests('trends-get').length
  landed = true
  elsewhere = true
  const change = () =>
    page.evaluate(() => {
      for (const handler of window.__changeHandlers ?? []) handler({})
    })
  await change()
  await summary.getByText('15 executions ', { exact: false }).waitFor()
  assert.deepEqual(requests('trends-get').at(asked), pinned)
  await latest.getByRole('button', { name: 'Close' }).waitFor()
  // A reload that fails keeps the trend and says so over it.
  failing = 1
  await change()
  await page.getByText('Couldn’t reload the trend').waitFor()
  await summary.getByText('15 executions ', { exact: false }).waitFor()
  await page.getByRole('button', { name: 'Retry' }).click()
  await page
    .getByText('Couldn’t reload the trend')
    .waitFor({ state: 'detached' })
  landed = false
  elsewhere = false
  await latest.getByRole('button', { name: 'Close' }).click()
  assert.equal(await page.locator('[data-trend-panel]').count(), 0)

  // A small chart takes the large one's place.
  await page.getByRole('button', { name: 'Run duration', exact: true }).click()
  await page
    .locator('[data-trend-chart="duration"]')
    .getByRole('heading', { name: 'Run duration' })
    .waitFor()
  assert.equal(await page.locator('[data-trend-mini="score"]').count(), 1)

  // One stack of the series, kept in the hash.
  await page.locator('[data-stack-picker]').click()
  await page.getByRole('menuitemradio', { name: /^default/ }).click()
  await summary.getByText('11 executions ', { exact: false }).waitFor()
  assert.equal(requests('trends-get').at(-1).stack, 'default')
  assert.equal(requests('trends-get').at(-1).suite, 'regression')
  assert.match(
    await hash(),
    /\/trends\?suite=regression&.*stack=default&range=30d$/,
  )
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
  await summary.getByText('11 executions ', { exact: false }).waitFor()
  assert.equal(await hash(), here)

  // Another suite: its latest series (two executions, the stack not
  // recorded, two planned tests that did not run); its one profile.
  await suite.click()
  await page
    .getByRole('menuitemradio', {
      name: /^Software engineering\s*2 executions$/,
    })
    .click()
  await summary.getByText('2 executions ', { exact: false }).waitFor()
  assert.deepEqual(requests('trends-get').at(-1), {
    suite: 'software-engineering',
    ...last30,
  })
  assert.match(await profile.innerText(), /ade-worker-builder/)
  await profile.click()
  assert.equal(
    await page
      .getByRole('menu', { name: 'Profile' })
      .getByRole('menuitemradio')
      .count(),
    1,
  )
  await page.getByRole('menuitemradio', { name: /^ade-worker-builder/ }).click()
  await page.waitForFunction(
    () => window.calls.at(-1)?.payload?.profile === 'ade-worker-builder',
  )
  assert.deepEqual(requests('trends-get').at(-1), {
    suite: 'software-engineering',
    provider: 'deepseek',
    model: 'deepseek-flash',
    profile: 'ade-worker-builder',
    ...last30,
  })
  const stack = page.locator('[data-stack-picker]')
  assert.equal(await stack.isDisabled(), true)
  assert.match(await stack.innerText(), /any/)
  assert.equal(
    await page.locator('[data-by-test] [data-kind="not_run"]').count(),
    2,
  )

  // The Trends tab starts over on the default view, the hash with it.
  await page.getByRole('link', { name: 'Trends', exact: true }).click()
  await summary.getByText('14 executions ', { exact: false }).waitFor()
  assert.match(await suite.innerText(), /Regression/)
  assert.match(
    await hash(),
    /\/trends\?suite=regression&.*stack=any&range=30d$/,
  )

  // A link to a stack the series never ran on: the worker applies its
  // default, and the page says so.
  await page.goto(
    `${server.url}#/ext/harness-e2e/trends?suite=regression&provider=deepseek&model=deepseek-flash&profile=&stack=lean`,
  )
  await page
    .locator('[data-stack-notice]')
    .getByText(
      'No execution of this series ran on lean, so this shows every stack',
      {
        exact: false,
      },
    )
    .waitFor()
  assert.equal(requests('trends-get').at(-1).stack, 'lean')
  assert.match(await stack.innerText(), /any/)
  assert.match(await hash(), /stack=any&range=30d$/)

  // The period: all time, a custom one, From after To said and not asked,
  // then one with nothing in it and the way back to all time.
  const period = page.locator('[data-picker="period"]')
  await period.click()
  await page.getByRole('menuitemradio', { name: 'All time' }).click()
  await summary
    .getByText('14 executions over all time ·', { exact: false })
    .waitFor()
  assert.equal(requests('trends-get').at(-1).since, undefined)
  assert.match(await hash(), /stack=any&range=all$/)
  await period.click()
  await page.getByRole('menuitemradio', { name: 'Custom' }).click()
  const from = page.getByLabel('From', { exact: true })
  const to = page.getByLabel('To', { exact: true })
  assert.equal(await from.inputValue(), '2026-08-30')
  assert.equal(await to.inputValue(), '2026-09-28')
  await from.fill('2026-09-26')
  await summary
    .getByText('5 executions from Sep 26 to Sep 28 ·', { exact: false })
    .waitFor()
  const { since, until } = requests('trends-get').at(-1)
  assert.deepEqual(
    { since, until },
    { since: '2026-09-26T03:00:00.000Z', until: '2026-09-29T02:59:59.999Z' },
  )
  assert.match(await hash(), /stack=any&since=2026-09-26&until=2026-09-28$/)
  const before = requests('trends-get').length
  await from.fill('2026-09-29')
  await page.getByText('From must be on or before To.').waitFor()
  assert.equal(await from.getAttribute('aria-invalid'), 'true')
  assert.equal(requests('trends-get').length, before)
  await to.fill('2026-09-10')
  await from.fill('2026-09-01')
  await page
    .getByRole('heading', {
      name: 'No execution of this series from Sep 1 to Sep 10',
    })
    .waitFor()
  await page.getByRole('button', { name: 'Show all time' }).click()
  await summary
    .getByText('14 executions over all time ·', { exact: false })
    .waitFor()
  assert.equal(await from.count(), 0)

  // This harness: nothing changed between its executions, no diamond.
  await suite.click()
  await page.getByRole('menuitemradio', { name: /^2 tests, unsaved/ }).click()
  await summary.getByText('5 executions ', { exact: false }).waitFor()
  assert.match(await stack.innerText(), /any/)
  assert.equal(await page.locator('.tr-diamond').count(), 0)

  // Nothing counted: say why, open the execution or run it again.
  // Regression again (its latest series is deepseek's), then the model that
  // ran it once.
  await suite.click()
  await page.getByRole('menuitemradio', { name: /^Regression/ }).click()
  await summary.getByText('14 executions ', { exact: false }).waitFor()
  await model.click()
  const models = page.getByRole('menu', { name: 'Model' })
  await models
    .getByRole('menuitemradio', {
      name: /^deepseek\/deepseek-flash\s*14 executions$/,
    })
    .waitFor()
  await models
    .getByRole('menuitemradio', {
      name: /^anthropic\/claude-opus-5-5\s*1 execution$/,
    })
    .click()
  assert.deepEqual(requests('trends-get').at(-1), {
    suite: 'regression',
    provider: 'anthropic',
    model: 'claude-opus-5-5',
  })
  // The period picked stays across series: all time asks for no bounds.
  await page.getByRole('heading', { name: 'Nothing to draw yet' }).waitFor()
  // One action; the execution is a link in the sentence.
  const empty = page.locator('.tr-empty')
  assert.match(
    await empty.getByRole('link').getAttribute('href'),
    new RegExp(`/execution/${opusDetail.id}$`),
  )
  assert.equal(await empty.getByRole('button').count(), 1)
  await page.getByRole('button', { name: 'Run again' }).click()
  await page.getByRole('dialog', { name: 'Run again' }).waitFor()
  assert.equal(requests('execution-get').at(-1).execution_id, opusDetail.id)

  // A narrow pane: the panel under the chart, the table's short form.
  await page.setViewportSize({ width: 640, height: 1000 })
  await page.goto(`${server.url}#/ext/harness-e2e/trends`)
  await page.reload()
  await page.locator('[data-trend-executions][data-narrow]').waitFor()
  assert.equal(
    await page.getByRole('columnheader', { name: 'Stack' }).count(),
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
    'Trends browser flow passed: a failed first load retried from the StatusPanel, the latest series on every stack with the Trends tab current, the Sep 26 diamond with the commits asked when it opened, the latest point against the previous counted one, the default view pinned to its series and stack, a run landing reloaded quietly on it (another series newer) with the pick kept and a failed reload said over the trend, a small chart in the large one’s place, one stack kept in the hash, Compare with and back to the same view, a suite, then its one profile, with planned tests not run, the Trends tab starting over, a stack the series never ran on said, the period (all time, custom, From after To refused, none in it and back to all time), the Harness’s tags, a model of a suite and the empty state’s Run again, narrow pane.',
  )
} finally {
  await browser.close()
  await server.close()
}
