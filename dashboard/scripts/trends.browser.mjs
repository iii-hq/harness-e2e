// Deterministic browser coverage for Trends: the release overview it opens
// on (its verdict, the local runs beside the releases, the CI runs outside
// Release Control, CI sync's status line) and, under More options, a series
// over time, what changed at a diamond with the commits between, a point
// against the previous counted one, Compare with it and back, the stack and
// series pickers, a small chart in the large one's place, the empty state's
// Run again and the narrow pane. No models run and no worker answers: the
// series and the overview are the canvas's, from src/test-fixtures.
import assert from 'node:assert/strict'
import { chromium } from 'playwright'
import { createConsoleTestHost } from './console-test-host.mjs'
import {
  ciSyncAnswer,
  compareAnswer,
  fixture,
  overviewAnswer,
  trendsAnswer,
} from './trends-fixture.mjs'

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
// trends-get answers that fail before one succeeds again, then the same for
// release-overview-get.
let failing = 0
let overviewFails = 0
// CI sync's status: Download turns it on, Turn off sync back off.
let sync = ciSyncAnswer('off')

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
  if (id === 'release-overview-get') {
    if (overviewFails > 0) {
      overviewFails -= 1
      throw new Error('engine unavailable')
    }
    return overviewAnswer()
  }
  if (id === 'ci-sync-status-get' || id === 'ci-sync-check') return sync
  if (id === 'ci-sync-set') {
    sync = ciSyncAnswer(request.enabled ? 'quick' : 'off')
    return sync
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
  // The page pins the hash after it renders: wait for it, then check it.
  const hashMatches = async (pattern) => {
    await page
      .waitForFunction(
        (source) => new RegExp(source).test(location.hash),
        pattern.source,
        { timeout: 5000 },
      )
      .catch(() => undefined)
    assert.match(await hash(), pattern)
  }

  // The release overview first. One that fails to load says so under its
  // heading, and the options stay; Retry asks again.
  const verdictLine = page.locator('[data-verdict-line]')
  const verdictMatches = async () =>
    assert.match(
      await verdictLine.innerText(),
      /^v1\.8\.36 \(Today\) vs v1\.8\.35:/,
    )
  const moreOptions = page.locator('summary', { hasText: 'More options' })
  // The last request to `id`, once more than `before` of them were made.
  const askedAfter = async (id, before) => {
    for (let wait = 0; wait < 100 && requests(id).length <= before; wait++)
      await page.waitForTimeout(50)
    assert.ok(requests(id).length > before, `${id} was not asked again`)
    return requests(id).at(-1)
  }
  overviewFails = 1
  await page.goto(`${server.url}#/ext/harness-e2e`)
  await page.getByRole('heading', { name: 'Trends', level: 1 }).waitFor()
  await page.getByText('Couldn’t load the release overview').waitFor()
  await page.getByRole('heading', { name: 'Harness evolution' }).waitFor()
  await moreOptions.waitFor()
  await page.getByRole('button', { name: 'Retry' }).click()
  await verdictLine.waitFor()
  assert.equal(
    await page
      .getByRole('link', { name: 'Trends', exact: true })
      .getAttribute('aria-current'),
    'page',
  )
  assert.deepEqual(requests('trends-get')[0], {})
  assert.ok(requests('release-overview-get').length >= 2)
  await verdictMatches()

  // Five local runs as rings beside the releases, none as a filled dot.
  const chart = page.locator('[data-trend-chart]')
  assert.equal(await chart.locator('.tr-point[data-source="local"]').count(), 5)
  assert.equal(
    await page.locator('circle.tr-dot[data-source="local"]').count(),
    0,
  )
  // What changed at the latest release, and the panel closed.
  await page.getByRole('button', { name: 'What changed', exact: true }).click()
  await page.locator('[data-trend-panel]').waitFor()
  await page
    .locator('[data-trend-panel]')
    .getByRole('button', { name: 'Close' })
    .click()
  assert.equal(await page.locator('[data-trend-panel]').count(), 0)
  // The CI runs outside Release Control, and the executions that are them.
  const outside = page.locator('[data-outside]')
  await outside
    .getByText('CI runs outside Release Control · 3', {
      exact: false,
    })
    .waitFor()
  assert.equal(
    await outside.locator('a').getAttribute('href'),
    '#/ext/harness-e2e/executions?origin=outside-release-control',
  )

  // The measure is in the hash and kept by a reload; the score is the default.
  const metrics = page.getByRole('radiogroup', { name: 'Metric' })
  await metrics.getByRole('radio', { name: 'Turns' }).click()
  await hashMatches(/harness-e2e\?metric=turns$/)
  await page.reload()
  await verdictLine.waitFor()
  assert.equal(
    await metrics.getByRole('radio', { name: 'Turns' }).isChecked(),
    true,
  )
  await hashMatches(/harness-e2e\?metric=turns$/)
  await metrics.getByRole('radio', { name: 'Score' }).click()
  await hashMatches(/harness-e2e$/)

  // CI sync: Download turns it on and the line says where it is; Check now
  // asks once; a worker without `gh` signed in says how to fix it; Turn off
  // sync brings the offer back.
  await page.getByRole('button', { name: 'Download CI results' }).click()
  assert.deepEqual(requests('ci-sync-set').at(-1), { enabled: true })
  await page
    .getByText('Importing the newest runs first: 3 of 12', { exact: false })
    .waitFor()
  sync = ciSyncAnswer('keeping_up')
  await page.getByRole('button', { name: 'Check now' }).click()
  await page
    .getByText('Synced from CI at 2:55 PM · 4 imports failed', { exact: false })
    .waitFor()
  assert.equal(requests('ci-sync-check').length, 1)
  sync = ciSyncAnswer('signed_out')
  await page.getByRole('button', { name: 'Check now' }).click()
  await page.locator('code', { hasText: 'gh auth login' }).waitFor()
  await page.getByRole('button', { name: 'Sync options' }).click()
  await page.getByRole('menuitem', { name: /^Turn off sync/ }).click()
  assert.deepEqual(requests('ci-sync-set').at(-1), { enabled: false })
  await page.getByRole('button', { name: 'Download CI results' }).waitFor()

  // More options holds the series pickers; opening them names no series.
  await moreOptions.click()
  await page.locator('[data-picker="suite"]').waitFor()
  assert.doesNotMatch(await hash(), /suite=/)
  // A suite opens its series under the same verdict. This one recorded no
  // Harness release, so each execution is drawn, and said.
  const suite = page.locator('[data-picker="suite"]')
  await suite.click()
  await page
    .getByRole('menuitemradio', {
      name: /^Software engineering\s*2 executions$/,
    })
    .click()
  await page
    .locator('[data-group-note]')
    .getByText(
      'No Harness release is recorded in this series, so each execution is drawn.',
    )
    .waitFor()
  assert.deepEqual(requests('trends-get').at(-1), {
    suite: 'software-engineering',
  })
  await hashMatches(/\?suite=software-engineering&.*&stack=any&range=all$/)
  await page
    .locator('summary', { hasText: 'Details' })
    .getByText('by test · executions')
    .waitFor()
  await verdictMatches()
  assert.ok((await chart.locator('.tr-point').count()) > 0)
  // Back to the release overview, every series control where it started.
  const beforeBack = requests('trends-get').length
  await page.getByRole('button', { name: 'Back to release overview' }).click()
  await hashMatches(/harness-e2e$/)
  assert.deepEqual(await askedAfter('trends-get', beforeBack), {})
  await verdictMatches()

  // The series with the latest execution, on every stack it ran on, in the
  // last 30 days, one point per execution. A first load that fails says so
  // on the host's StatusPanel and retries. The overview's own panel asked
  // for commits before: count from here.
  const compared = requests('version-compare').length
  await page.goto(
    `${server.url}#/ext/harness-e2e/trends?range=30d&group=execution`,
  )
  await summary.getByText('14 executions in the last 30 days').waitFor()
  failing = 1
  const mark = requests('trends-get').length
  await page.reload()
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
  assert.deepEqual(requests('trends-get').at(mark), last30)
  await summary
    .getByText(
      '14 executions in the last 30 days · 11 with counted runs, 3 without · 99 counted runs',
      { exact: false },
    )
    .waitFor()
  const model = page.locator('[data-picker="model"]')
  const profile = page.locator('[data-picker="profile"]')
  assert.match(await suite.innerText(), /Regression/)
  assert.match(await model.innerText(), /deepseek\/deepseek-flash/)
  assert.match(await profile.innerText(), /none/)
  assert.match(await page.locator('[data-stack-picker]').innerText(), /any/)
  // Every stack shown: no note about executions matched by their workers.
  assert.equal(
    await page
      .getByText('ran before the Console recorded stacks', { exact: false })
      .count(),
    0,
  )

  // The Sep 26 diamond: no counted run, iii moved; the commits between are
  // asked only now, not while the series loads.
  assert.equal(requests('version-compare').length, compared)
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
  assert.deepEqual(requests('version-compare').at(compared), {
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
  await hashMatches(
    /\/trends\?suite=regression&provider=deepseek&model=deepseek-flash&profile=&stack=any&range=30d&group=execution$/,
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
  // The measure in front is in the hash after the period and grouping.
  await hashMatches(
    /\/trends\?suite=regression&.*stack=default&range=30d&group=execution&metric=duration$/,
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
  // Its one profile, and any.
  assert.equal(
    await page
      .getByRole('menu', { name: 'Profile' })
      .getByRole('menuitemradio')
      .count(),
    2,
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

  // The Trends tab starts over on the release overview, the hash with it.
  await page.getByRole('link', { name: 'Trends', exact: true }).click()
  await page.getByRole('heading', { name: 'Harness evolution' }).waitFor()
  await hashMatches(/\/trends$/)
  await page.goto(
    `${server.url}#/ext/harness-e2e/trends?range=30d&group=execution`,
  )
  await summary.getByText('14 executions ', { exact: false }).waitFor()
  assert.match(await suite.innerText(), /Regression/)
  await hashMatches(
    /\/trends\?suite=regression&.*stack=any&range=30d&group=execution$/,
  )

  // A link to a stack the series never ran on: the worker applies its
  // default, and the page says so.
  await page.goto(
    `${server.url}#/ext/harness-e2e/trends?suite=regression&provider=deepseek&model=deepseek-flash&profile=&stack=lean&range=30d&group=execution`,
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
  await hashMatches(/stack=any&range=30d&group=execution$/)

  // The period: all time, a custom one, From after To said and not asked,
  // then one with nothing in it and the way back to all time.
  const period = page.locator('[data-picker="period"]')
  await period.click()
  await page.getByRole('menuitemradio', { name: 'All time' }).click()
  await summary
    .getByText('14 executions over all time ·', { exact: false })
    .waitFor()
  assert.equal(requests('trends-get').at(-1).since, undefined)
  await hashMatches(/stack=any&range=all&group=execution$/)
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
  await hashMatches(
    /stack=any&since=2026-09-26&until=2026-09-28&group=execution$/,
  )
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

  // A baseline: pin Sep 23, read the others against it, keep it in the URL.
  // The Baseline picker: "previous execution" until one is picked.
  const baselinePicker = page.locator('[data-picker="baseline"]')
  const baselineIs = (text) =>
    baselinePicker.getByText(text, { exact: true }).waitFor()
  const pickBaseline = async (name) => {
    await baselinePicker.click()
    await page
      .getByRole('menu', { name: 'Baseline' })
      .getByRole('menuitemradio', { name })
      .click()
  }
  const scoreCard = page.locator('[data-trend-chart="score"]')
  const scoreRef = scoreCard.locator('.tr-reference')
  const scoreAt = (label) => scoreCard.getByRole('button', { name: label })
  await baselineIs('previous execution')
  await scoreAt(/^Sep 23, 2:17 AM · Score 90\.8$/).click()
  const sep23 = page.locator('[data-trend-panel="github-35821773226-2"]')
  await sep23.getByRole('button', { name: 'Set as baseline' }).click()
  await baselineIs('Sep 23, 2:17 AM')
  await hashMatches(/&range=all&group=execution&base=github-35821773226-2$/)
  // Focus stays in the panel, on the button that now clears it.
  assert.equal(
    await page.evaluate(() => document.activeElement?.textContent),
    'Clear baseline',
  )
  // Its own panel is tagged, and the cards read it against the execution before.
  assert.equal(await sep23.getByText('baseline', { exact: true }).count(), 1)
  assert.equal(
    await scoreRef.innerText(),
    'Sep 23, 2:17 AM (baseline) against Sep 22, 3:27 PM',
  )
  await sep23.getByRole('button', { name: 'Close' }).click()
  assert.equal(
    await scoreRef.innerText(),
    'Sep 28, 2:17 AM against Sep 23, 2:17 AM (baseline)',
  )
  assert.match(await scoreCard.locator('.tr-delta').innerText(), /2\.2/)
  // A rule on every chart (the six main measures), labelled only on the
  // large one.
  assert.equal(await page.locator('.tr-baseline').count(), 6)
  assert.equal(await page.locator('.tr-baseline-label').count(), 1)

  // Sep 25 against it: the commits from the earlier version to the later,
  // the minor changes folded past five, and the pair to compare.
  await scoreAt(/^Sep 25, 2:17 AM · Score 96\.9$/).click()
  const sep25 = page.locator('[data-trend-panel="github-36097908502-1"]')
  await sep25
    .getByRole('heading', {
      name: 'What changed between Sep 23, 2:17 AM (baseline) and Sep 25, 2:17 AM',
    })
    .waitFor()
  await sep25.getByText('1.8.31 → 1.8.35').waitFor()
  await sep25
    .getByRole('link', {
      name: '26 commits between the tags in iii-hq/workers',
    })
    .waitFor()
  assert.ok(
    requests('version-compare').some(
      (call) =>
        call.name === 'harness' &&
        call.base === '1.8.31' &&
        call.head === '1.8.35',
    ),
  )
  assert.equal(await sep25.locator('li[data-change]').count(), 7)
  await sep25.getByRole('button', { name: 'Show 3 more' }).click()
  assert.equal(await sep25.locator('li[data-change]').count(), 10)
  await sep25.getByRole('button', { name: 'Show fewer' }).click()
  assert.equal(await sep25.locator('li[data-change]').count(), 7)
  // Clearing the baseline with the list unfolded leaves nothing to fold back.
  await sep25.getByRole('button', { name: 'Show 3 more' }).click()
  await pickBaseline(/^Previous execution/)
  await sep25.getByRole('heading', { name: /^What changed since / }).waitFor()
  assert.equal(await sep25.getByRole('button', { name: /^Show/ }).count(), 0)
  await sep25.getByRole('button', { name: 'Set as baseline' }).waitFor()
  await scoreAt(/^Sep 23, 2:17 AM · Score 90\.8$/).click()
  await sep23.getByRole('button', { name: 'Set as baseline' }).click()
  await baselineIs('Sep 23, 2:17 AM')
  await scoreAt(/^Sep 25, 2:17 AM · Score 96\.9$/).click()
  await sep25.getByRole('button', { name: 'Show 3 more' }).waitFor()
  await sep25.getByText('Against the baseline, Sep 23, 2:17 AM').waitFor()
  assert.equal(
    await scoreRef.innerText(),
    'Sep 25, 2:17 AM against Sep 23, 2:17 AM (baseline)',
  )
  assert.match(await scoreCard.locator('.tr-delta').innerText(), /6\.1/)
  assert.match(
    await sep25
      .getByRole('link', { name: 'Compare with the baseline' })
      .getAttribute('href'),
    /\/compare\/github-35821773226-2\/github-36097908502-1\?from=/,
  )
  // Before the baseline it reads how far it was from it, still earlier as A.
  await scoreAt(/^Sep 22, 11:45 AM · Score 89\.1$/).click()
  const sep22 = page.locator('[data-trend-panel="github-35742568444-1"]')
  assert.match(await scoreCard.locator('.tr-delta').innerText(), /1\.7/)
  assert.match(
    await sep22
      .getByRole('link', { name: 'Compare with the baseline' })
      .getAttribute('href'),
    /\/compare\/github-35742568444-1\/github-35821773226-2\?from=/,
  )
  await sep22.getByRole('button', { name: 'Close' }).click()

  // It survives a reload; a period that leaves it out says so and reads
  // against the execution before; all time brings it back.
  await page.reload()
  await baselineIs('Sep 23, 2:17 AM')
  assert.equal(
    await scoreRef.innerText(),
    'Sep 28, 2:17 AM against Sep 23, 2:17 AM (baseline)',
  )
  await period.click()
  await page.getByRole('menuitemradio', { name: 'Custom' }).click()
  await from.fill('2026-09-26')
  await baselineIs('not in this view')
  assert.equal(
    await scoreRef.innerText(),
    'Sep 28, 2:17 AM against Sep 27, 2:17 AM',
  )
  assert.equal(await page.locator('.tr-baseline').count(), 0)
  await hashMatches(
    /since=2026-09-26&until=2026-09-28&group=execution&base=github-35821773226-2$/,
  )
  await period.click()
  await page.getByRole('menuitemradio', { name: 'All time' }).click()
  await baselineIs('Sep 23, 2:17 AM')
  assert.equal(await page.locator('.tr-baseline').count(), 6)

  // Another stack keeps it while its execution is there.
  await page.locator('[data-stack-picker]').click()
  await page.getByRole('menuitemradio', { name: /^default/ }).click()
  await summary.getByText('11 executions ', { exact: false }).waitFor()
  await hashMatches(
    /stack=default&range=all&group=execution&base=github-35821773226-2$/,
  )
  await baselineIs('Sep 23, 2:17 AM')
  await page.locator('[data-stack-picker]').click()
  await page.getByRole('menuitemradio', { name: /^any/ }).click()
  await summary.getByText('14 executions ', { exact: false }).waitFor()
  await hashMatches(
    /stack=any&range=all&group=execution&base=github-35821773226-2$/,
  )

  // Previous execution in the picker clears it; the picker sets one too,
  // each execution listed newest first with its score.
  await pickBaseline(/^Previous execution/)
  await baselineIs('previous execution')
  assert.equal(await page.locator('.tr-baseline').count(), 0)
  assert.doesNotMatch(await hash(), /base=/)
  assert.equal(
    await scoreRef.innerText(),
    'Sep 28, 2:17 AM against Sep 27, 2:17 AM',
  )
  await baselinePicker.click()
  const listed = page
    .getByRole('menu', { name: 'Baseline' })
    .getByRole('menuitemradio')
  assert.equal(await listed.count(), 15)
  assert.match(
    await listed.nth(1).innerText(),
    /^Sep 28, 2:17 AM\s+score 93\.0/,
  )
  // An execution without a counted run cannot be one.
  assert.equal(
    await listed
      .filter({ hasText: 'no counted run' })
      .first()
      .getAttribute('aria-disabled'),
    'true',
  )
  await listed.filter({ hasText: /^Sep 23, 2:17 AM/ }).click()
  await baselineIs('Sep 23, 2:17 AM')
  await hashMatches(/&base=github-35821773226-2$/)
  // Its own panel clears it as well.
  await scoreAt(/^Sep 23, 2:17 AM · Score 90\.8$/).click()
  await sep23.getByRole('button', { name: 'Clear baseline' }).click()
  await baselineIs('previous execution')

  // Another series, and the Trends tab, start without one. Sep 23's panel
  // is still open.
  await sep23.getByRole('button', { name: 'Set as baseline' }).click()
  await baselineIs('Sep 23, 2:17 AM')
  await suite.click()
  await page.getByRole('menuitemradio', { name: /^Regression/ }).click()
  await page
    .getByRole('menuitemradio', { name: /^Regression/ })
    .waitFor({ state: 'detached' })
  await baselineIs('previous execution')
  assert.doesNotMatch(await hash(), /base=/)
  await scoreAt(/^Sep 23, 2:17 AM · Score 90\.8$/).click()
  await sep23.getByRole('button', { name: 'Set as baseline' }).click()
  await baselineIs('Sep 23, 2:17 AM')
  await page.getByRole('link', { name: 'Trends', exact: true }).click()
  await page.getByRole('heading', { name: 'Harness evolution' }).waitFor()
  await hashMatches(/\/trends$/)
  await page.goto(
    `${server.url}#/ext/harness-e2e/trends?range=30d&group=execution`,
  )
  await summary.getByText('in the last 30 days', { exact: false }).waitFor()
  await hashMatches(/stack=any&range=30d&group=execution$/)
  await baselineIs('previous execution')
  // Back to all time, where the flow goes on.
  await period.click()
  await page.getByRole('menuitemradio', { name: 'All time' }).click()
  await summary.getByText('over all time', { exact: false }).waitFor()

  // This harness: nothing changed between its executions, no diamond.
  await suite.click()
  await page.getByRole('menuitemradio', { name: /^2 tests, unsaved/ }).click()
  await summary.getByText('5 executions ', { exact: false }).waitFor()
  assert.match(await stack.innerText(), /any/)
  assert.equal(await page.locator('.tr-diamond').count(), 0)
  assert.equal(
    await page
      .locator('[data-trend-chart="score"] .tr-point[data-source="local"]')
      .count(),
    5,
  )

  // Any model: Regression on deepseek and on Opus, one line, a diamond
  // where the model changed; any profile on top of it; back to one model.
  await suite.click()
  await page.getByRole('menuitemradio', { name: /^Regression/ }).click()
  await summary.getByText('14 executions ', { exact: false }).waitFor()
  await model.click()
  await page
    .getByRole('menu', { name: 'Model' })
    .getByRole('menuitemradio', {
      name: /^any\s*every model, on one line\s*15 executions$/,
    })
    .click()
  await summary.getByText('15 executions ', { exact: false }).waitFor()
  assert.deepEqual(requests('trends-get').at(-1), {
    suite: 'regression',
    model: 'any',
    profile: null,
  })
  assert.match(await model.innerText(), /any/)
  await hashMatches(
    /provider=any&model=any&profile=&stack=any&range=all&group=execution$/,
  )
  // Two model changes: to Opus on Sep 27 and back on Sep 28.
  const modelDiamonds = page.getByRole('button', {
    name: /^What changed · .+: model/,
  })
  assert.equal(await modelDiamonds.count(), 2)
  await modelDiamonds.first().click()
  const opusPanel = page.locator(`[data-trend-panel="${opusDetail.id}"]`)
  await opusPanel
    .getByText('deepseek/deepseek-flash → anthropic/claude-opus-5-5')
    .waitFor()
  await opusPanel
    .getByText('anthropic/claude-opus-5-5 · profile none', { exact: false })
    .waitFor()
  await opusPanel.getByRole('button', { name: 'Close' }).click()
  await page
    .getByText('model, profile, iii, Harness, tests or stack changed')
    .waitFor()
  await profile.click()
  await page
    .getByRole('menu', { name: 'Profile' })
    .getByRole('menuitemradio', { name: /^any/ })
    .click()
  await hashMatches(
    /model=any&profile=any&stack=any&range=all&group=execution$/,
  )
  assert.deepEqual(requests('trends-get').at(-1), {
    suite: 'regression',
    provider: 'any',
    model: 'any',
    profile: 'any',
  })
  // A model picked keeps the profile at any.
  await model.click()
  await page
    .getByRole('menu', { name: 'Model' })
    .getByRole('menuitemradio', { name: /^deepseek\/deepseek-flash/ })
    .click()
  await summary.getByText('14 executions ', { exact: false }).waitFor()
  assert.deepEqual(requests('trends-get').at(-1), {
    suite: 'regression',
    provider: 'deepseek',
    model: 'deepseek-flash',
    profile: 'any',
  })
  await profile.click()
  await page
    .getByRole('menu', { name: 'Profile' })
    .getByRole('menuitemradio', { name: /^none/ })
    .click()
  await hashMatches(
    /model=deepseek-flash&profile=&stack=any&range=all&group=execution$/,
  )

  // Group by day: a point per day, its panel on its executions; a day set as
  // the baseline pins its last execution, kept when back to executions.
  const groupPicker = page.locator('[data-picker="group"]')
  assert.match(await groupPicker.innerText(), /Group by\s*None/)
  const groupBy = async (name) => {
    await groupPicker.click()
    await page
      .getByRole('menu', { name: 'Group by' })
      .getByRole('menuitemradio', { name })
      .click()
  }
  await groupBy(/^Day/)
  await hashMatches(/range=all&group=day$/)
  const scorePoints = page.locator('[data-trend-chart="score"] button.tr-point')
  assert.equal(await scorePoints.count(), 7)
  await page
    .locator('[data-trend-chart="score"]')
    .getByRole('button', { name: 'Sep 24 · Score 95.2' })
    .click()
  const sep24 = page.locator('[data-trend-panel="day:2026-09-24"]')
  await sep24.getByText('2 executions · 18 counted runs').waitFor()
  await sep24
    .getByText('did not all run the same versions', { exact: false })
    .waitFor()
  assert.equal(await sep24.locator('.tr-members a').count(), 2)
  await sep24.getByRole('button', { name: 'Set as baseline' }).click()
  await page
    .locator('[data-picker="baseline"]')
    .getByText('Sep 24', { exact: true })
    .waitFor()
  await hashMatches(/group=day&base=github-36010839787-2$/)
  await groupBy(/^None/)
  await hashMatches(/range=all&group=execution&base=github-36010839787-2$/)
  await page
    .locator('[data-picker="baseline"]')
    .getByText('Sep 24, 11:10 AM', { exact: true })
    .waitFor()
  await pickBaseline(/^Previous execution/)
  // Group by Harness release: its versions under the plot, the executions
  // with no recorded version said and left out.
  await groupBy(/^Harness release/)
  await hashMatches(/range=all&group=release$/)
  assert.equal(await scorePoints.count(), 4)
  await page
    .getByText('5 executions recorded no single Harness release', {
      exact: false,
    })
    .waitFor()
  await page
    .locator('[data-trend-chart="score"]')
    .getByText('v1.8.31', { exact: true })
    .waitFor()
  await groupBy(/^None/)
  assert.equal(await scorePoints.count(), 11)

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
  await page.goto(
    `${server.url}#/ext/harness-e2e/trends?range=30d&group=execution`,
  )
  await page.reload()
  await page.locator('summary', { hasText: 'Details' }).click()
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
  // The Baseline picker wraps under the others instead of running off the
  // pane.
  await page.getByRole('button', { name: 'Set as baseline' }).click()
  await baselineIs('Sep 28, 2:17 AM')
  const box = await baselinePicker.boundingBox()
  assert.ok(box && box.x >= 0 && box.x + box.width <= 640, 'the picker fits')
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
    true,
  )

  assert.deepEqual(errors, [])
  console.log(
    'Trends browser flow passed: the release overview first (a failed load retried, the verdict line, five local runs as rings, What changed at the latest release, the CI runs outside Release Control linking to the executions, the measure kept in the hash and across a reload, Download CI results through Check now, a sign-in to fix and Turn off sync, a suite opened under More options drawn per execution with its note, Back to release overview), a failed first load retried from the StatusPanel, the latest series on every stack with the Trends tab current, the Sep 26 diamond with the commits asked when it opened, the latest point against the previous counted one, the default view pinned to its series and stack, a run landing reloaded quietly on it (another series newer) with the pick kept and a failed reload said over the trend, a small chart in the large one’s place, one stack kept in the hash, Compare with and back to the same view, a suite, then its one profile, with planned tests not run, the Trends tab starting over on the release overview, a stack the series never ran on said, the period (all time, custom, From after To refused, none in it and back to all time), the Harness’s tags, a baseline pinned, read against by the cards and the panel with the commits earlier to later and the minor changes folded, kept across a reload and out of a period that leaves it out, cleared, and dropped by another series and the Trends tab, any model on one line with the model change as a diamond, any profile kept across a model, grouped by day (a day’s panel on its executions, a day as the baseline kept on its last execution) and by Harness release (unrecorded ones said and left out, the releases in version order), a model of a suite and the empty state’s Run again, narrow pane with Details open and the Baseline picker, the five local runs of this harness as rings.',
  )
} finally {
  await browser.close()
  await server.close()
}
