#!/usr/bin/env node

import { mkdir, writeFile } from 'node:fs/promises'
import { pathToFileURL } from 'node:url'
import { join } from 'node:path'

const CASES = [
  'kanban_c1_foundation',
  'kanban_c2_persistence',
  'kanban_c3_board',
  'kanban_c4_ticket_flow',
  'kanban_c5_edit_move',
  'kanban_c6_discussion',
  'kanban_c7_live',
]

const usage = `Usage: probe.mjs --case <id> --base-url <url> --engine-url <ws-url> --output <directory>

Environment:
  III_SDK_MODULE       Absolute path to the trusted iii-sdk module
  PLAYWRIGHT_MODULE    Absolute path to the trusted Playwright module

Other:
  --list-cases         Print the supported case ids as JSON
  --help               Show this help`

function argumentsOf(argv) {
  const values = {}
  for (let index = 0; index < argv.length; index += 2) {
    const name = argv[index]
    if (!name?.startsWith('--') || argv[index + 1] === undefined) throw new Error(`Invalid argument: ${name ?? ''}`)
    values[name.slice(2)] = argv[index + 1]
  }
  return values
}

function moduleUrl(name) {
  const value = process.env[name]
  if (!value?.startsWith('/')) throw new Error(`${name} must be an absolute path`)
  return pathToFileURL(value).href
}

function details(error) {
  return error instanceof Error ? error.message : String(error)
}

function expect(value, message) {
  if (!value) throw new Error(message)
}

async function eventually(action, message, timeout = 8_000) {
  const deadline = Date.now() + timeout
  let last
  while (Date.now() < deadline) {
    try {
      const value = await action()
      if (value) return value
    } catch (error) { last = error }
    await new Promise((resolve) => setTimeout(resolve, 100))
  }
  throw new Error(`${message}${last ? `: ${details(last)}` : ''}`)
}

async function main() {
  if (process.argv.includes('--help')) { console.log(usage); return }
  if (process.argv.includes('--list-cases')) { console.log(JSON.stringify(CASES)); return }

  const started = Date.now()
  let args
  let output
  let caseId = 'unknown'
  try {
    args = argumentsOf(process.argv.slice(2))
    caseId = args.case
    output = args.output
    if (!CASES.includes(caseId)) throw new Error(`Unknown case: ${caseId}`)
    if (!args['base-url'] || !args['engine-url'] || !output) throw new Error('Missing required arguments')
    await mkdir(output, { recursive: true })
  } catch (error) {
    const result = resultFor(caseId, [], Date.now() - started, 'evaluation_failed', details(error))
    if (output) {
      await mkdir(output, { recursive: true })
      await writeFile(join(output, 'result.json'), `${JSON.stringify(result, null, 2)}\n`)
    }
    console.log(JSON.stringify(result))
    process.exitCode = 2
    return
  }

  const checks = []
  let browser
  let iii
  let infrastructureError
  const check = async (id, action) => {
    if (id.startsWith('criterion_') && checks.some(({ id, status }) => !id.startsWith('criterion_') && status === 'failed')) {
      checks.push({ id, status: 'failed', detail: 'Associated functional evidence failed.' })
      return false
    }
    try {
      const detail = await action()
      checks.push({ id, status: 'passed', detail: detail || 'Observed through the running application.' })
      return true
    } catch (error) {
      checks.push({ id, status: 'failed', detail: details(error) })
      return false
    }
  }
  const unverified = (id, detail) => checks.push({ id, status: 'unverified', detail })

  try {
    const sdk = await import(moduleUrl('III_SDK_MODULE'))
    const playwright = await import(moduleUrl('PLAYWRIGHT_MODULE'))
    const registerWorker = sdk.registerWorker ?? sdk.default?.registerWorker
    const chromium = playwright.chromium ?? playwright.default?.chromium
    if (!registerWorker || !chromium) throw new Error('Trusted evaluator dependencies have unexpected exports')
    iii = registerWorker(args['engine-url'], {
      workerName: `kanban-evaluator-${process.pid}`,
      workerDescription: 'Trusted Harness Kanban evaluator',
    })
    browser = await chromium.launch({ headless: true })

    const context = {
      baseUrl: args['base-url'].replace(/\/$/, ''),
      output,
      browser,
      api: (path, init = {}) => fetch(`${args['base-url'].replace(/\/$/, '')}${path}`, {
        ...init,
        signal: AbortSignal.timeout(5_000),
      }),
      trigger: (functionId, payload = {}) => iii.trigger({
        function_id: functionId,
        namespace: 'default',
        payload,
        timeoutMs: 5_000,
      }),
      check,
      unverified,
    }
    await PROBES[caseId](context)
  } catch (error) {
    infrastructureError = details(error)
  } finally {
    await browser?.close().catch(() => {})
    await iii?.shutdown?.().catch(() => {})
  }

  const functionalStatus = infrastructureError
    ? null
    : checks.some(({ status }) => status === 'failed') ? 'failed' : 'passed'
  const status = infrastructureError
    ? 'evaluation_failed'
    : functionalStatus === 'failed' ? 'failed'
      : checks.some(({ status }) => status === 'unverified') ? 'incomplete' : 'passed'
  const result = resultFor(caseId, checks, Date.now() - started, status, infrastructureError, functionalStatus)
  const coverage = {
    schema: 'kanban-evaluation-coverage/v1',
    case_id: caseId,
    complete: !infrastructureError && checks.length > 0 && !checks.some(({ status }) => status === 'unverified'),
    criteria: checks.filter(({ id }) => id.startsWith('criterion_')),
  }
  await writeFile(join(output, 'result.json'), `${JSON.stringify(result, null, 2)}\n`)
  await writeFile(join(output, 'coverage.json'), `${JSON.stringify(coverage, null, 2)}\n`)
  console.log(JSON.stringify(result))
}

function resultFor(caseId, checks, durationMs, status, error, functionalStatus = null) {
  return {
    schema: 'kanban-evaluation/v1',
    case_id: caseId,
    checks,
    status,
    functional_status: functionalStatus,
    duration_ms: durationMs,
    ...(error ? { error } : {}),
  }
}

async function json(response) {
  const body = await response.json().catch(() => ({}))
  expect(response.ok, `HTTP ${response.status}: ${body.error ?? 'request failed'}`)
  return body
}

async function pageFor(browser, baseUrl, viewport = { width: 1280, height: 900 }) {
  const context = await browser.newContext({ viewport })
  const page = await context.newPage()
  await page.goto(baseUrl)
  return { context, page }
}

async function screenshot(page, output, name) {
  await page.screenshot({ path: join(output, `${name}.png`), fullPage: true })
}

async function create(trigger, fields = {}) {
  return trigger('kanban::tickets::create', { title: `Probe ${Date.now()}-${Math.random()}`, ...fields })
}

const PROBES = {
  async kanban_c1_foundation({ api, trigger, browser, baseUrl, output, check, unverified }) {
    await check('foundation_assets_and_safe_routes', async () => {
      for (const path of ['/', '/page.js', '/styles.css']) expect((await api(path)).ok, `${path} did not load`)
      for (const path of ['/constructor', '/toString', '/__proto__']) expect((await api(path)).status === 404, `${path} was not 404`)
      return 'Standalone HTML, JavaScript and CSS load; prototype-like asset paths return 404.'
    })
    await check('foundation_configuration_contract', async () => {
      const initial = await json(await api('/api/config'))
      expect(initial.data_dir && initial.resolved_data_dir, 'GET /api/config omitted paths')
      const raw = await trigger('configuration::get', { id: 'kanban', raw: true })
      await trigger('configuration::set', { id: 'kanban', value: { ...raw.value, evaluator_marker: 'preserve-me' } })
      const saved = await json(await api('/api/config', {
        method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ data_dir: './probe-browser-data' }),
      }))
      expect(saved.data_dir === './probe-browser-data', 'PUT did not select the directory')
      const after = await trigger('configuration::get', { id: 'kanban', raw: true })
      expect(after.value.evaluator_marker === 'preserve-me', 'saving discarded unrelated configuration')
      expect((await api('/api/config', { method: 'PUT', headers: { 'content-type': 'application/json' }, body: '{"data_dir":""}' })).status === 400, 'empty directory was accepted')
      return 'GET/PUT work through the real configuration worker and unrelated data survives saving.'
    })
    await check('foundation_accessible_settings', async () => {
      const { context, page } = await pageFor(browser, baseUrl, { width: 390, height: 844 })
      await page.getByRole('link', { name: 'Settings' }).click()
      await page.getByLabel('Data directory').fill('./probe-ui-data')
      await page.getByRole('button', { name: 'Save settings' }).click()
      await expectText(page.getByRole('status'), /Settings saved/)
      await screenshot(page, output, 'settings-mobile')
      await context.close()
      return 'The settings form loads and saves at a mobile viewport using accessible controls.'
    })
    unverified('criterion_1', 'The probe observes a standalone app, but Compose dependency declarations are assessed by the runner.')
    unverified('criterion_2', 'Initialization and save preservation are exercised; preservation across application startup is not restarted here.')
    unverified('criterion_3', 'Browser saving is exercised; asynchronous direct configuration selection is not fully exercised.')
    await checksPassedCriterion(check, 'criterion_4', 'Invalid config and prototype-like asset paths are rejected by the live server.')
    unverified('criterion_5', 'Mobile usability is captured; development hot reload is outside this trusted probe.')
  },

  async kanban_c2_persistence({ api, trigger, check, unverified }) {
    await check('persistence_iii_crud', async () => {
      const first = await trigger('kanban::tickets::create', { title: '  Defaults ticket  ' })
      expect(first.title === 'Defaults ticket' && first.status === 'backlog' && first.priority === 'medium', 'defaults or title normalization are wrong')
      expect(first.description === '' && first.assignee === null, 'nullable defaults are wrong')
      expect(/^KAN-[1-9]\d*$/.test(first.key) && !Number.isNaN(Date.parse(first.created_at)), 'identity or timestamps are invalid')
      const second = await create(trigger, { status: 'done', priority: 'urgent', assignee: 'Ada' })
      const listed = await trigger('kanban::tickets::list')
      expect(listed.tickets.at(-2).id === first.id && listed.tickets.at(-1).id === second.id, 'list is not in creation order')
      expect((await trigger('kanban::tickets::get', { id: first.id })).key === first.key, 'UUID lookup failed')
      expect((await trigger('kanban::tickets::get', { id: first.key })).id === first.id, 'key lookup failed')
      return 'Real iii create/list/get calls prove defaults, normalized titles, identifiers, timestamps and ordering.'
    })
    await check('persistence_invalid_input_no_write', async () => {
      const before = (await trigger('kanban::tickets::list')).tickets.length
      let rejected = false
      try { await trigger('kanban::tickets::create', { title: '', status: 'invented' }) } catch { rejected = true }
      expect(rejected, 'invalid ticket was accepted')
      expect((await trigger('kanban::tickets::list')).tickets.length === before, 'invalid create changed the store')
      return 'Invalid creation is rejected without adding a ticket.'
    })
    await checksPassedCriterion(check, 'criterion_1', 'Both identifiers and real SDK caller metadata work through iii.')
    await checksPassedCriterion(check, 'criterion_2', 'Defaults, normalization, timestamps and increasing readable keys are observed.')
    unverified('criterion_3', 'Directory isolation and process restart durability are runner-level checks and are not claimed here.')
    unverified('criterion_4', 'Invalid input is covered; corrupt and duplicate on-disk stores are not modified by this black-box probe.')
  },

  async kanban_c3_board({ trigger, browser, baseUrl, output, check, unverified }) {
    const statuses = [['backlog', 'Backlog'], ['todo', 'To do'], ['in_progress', 'In progress'], ['in_review', 'In review'], ['done', 'Done']]
    await check('board_five_lanes_and_cards', async () => {
      const seeded = []
      for (const [status, label] of statuses) seeded.push(await create(trigger, { title: `${label} probe`, status, priority: 'high', assignee: 'Grace' }))
      const { context, page } = await pageFor(browser, baseUrl)
      await eventually(async () => (await page.getByText(`${seeded.length} tickets`, { exact: true }).count()) === 1, 'board total did not load')
      for (let index = 0; index < statuses.length; index++) {
        const [, label] = statuses[index]
        const heading = page.getByRole('heading', { name: new RegExp(`^${label}\\s*1$`, 'i') })
        expect(await heading.count() === 1, `${label} lane count is wrong`)
        const lane = heading.locator('xpath=ancestor::section[1]')
        expect(await lane.getByRole('heading', { name: seeded[index].title }).count() === 1, `${label} card is missing`)
        expect(await lane.getByText('Grace', { exact: true }).count() === 1, `${label} assignee is missing`)
      }
      await screenshot(page, output, 'board-desktop')
      await page.setViewportSize({ width: 390, height: 844 })
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth)
      expect(!overflow, 'mobile page has document-wide horizontal overflow')
      await screenshot(page, output, 'board-mobile')
      await context.close()
      return 'Five accessible lanes show accurate counts and seeded card metadata at desktop and mobile widths.'
    })
    await checksPassedCriterion(check, 'criterion_1', 'Tickets created through iii appear in all five lanes with counts and metadata.')
    unverified('criterion_2', 'The populated state is covered; injected failed-read and recovery states are not exercised.')
    unverified('criterion_3', 'Initial loading is covered; refresh and settings store switching are not fully exercised.')
    unverified('criterion_4', 'Mobile overflow is covered; adversarial markup rendering is not exercised in this run.')
  },

  async kanban_c4_ticket_flow({ api, trigger, browser, baseUrl, output, check, unverified }) {
    let created
    await check('ticket_flow_create_detail_and_delete', async () => {
      const { context, page } = await pageFor(browser, baseUrl, { width: 390, height: 844 })
      await page.getByRole('button', { name: 'New ticket' }).click()
      const dialog = page.getByRole('dialog', { name: 'New ticket' })
      await dialog.getByLabel('Title').fill('Browser-created ticket')
      await dialog.getByLabel('Description').fill('UTF-8: ação e café')
      await dialog.getByLabel('Status').selectOption('in_review')
      await dialog.getByLabel('Priority').selectOption('urgent')
      await dialog.getByLabel('Assignee').fill('Lin')
      await dialog.getByRole('button', { name: 'Create ticket' }).click()
      await expectText(page.getByRole('heading', { name: 'Browser-created ticket' }), /Browser-created ticket/)
      expect(await dialog.isVisible() === false, 'create dialog stayed open')
      created = (await json(await api('/api/tickets'))).tickets.find(({ title }) => title === 'Browser-created ticket')
      expect(created, 'created ticket is absent from API list')
      expect((await json(await api(`/api/tickets/${created.id}`))).ticket.key === created.key, 'HTTP UUID lookup failed')
      expect((await json(await api(`/api/tickets/${created.key}`))).ticket.id === created.id, 'HTTP key lookup failed')
      await page.reload()
      await expectText(page.getByRole('heading', { name: 'Browser-created ticket' }), /Browser-created ticket/)
      await screenshot(page, output, 'ticket-detail-mobile')
      await page.getByRole('button', { name: 'Delete ticket' }).click()
      await expectText(page.getByRole('heading', { name: 'The board.' }), /The board/)
      await eventually(async () => !(await json(await api('/api/tickets'))).tickets.some(({ id }) => id === created.id), 'deleted card remained listed')
      expect((await api(`/api/tickets/${created.id}`)).status === 404, 'deleted ticket is still retrievable')
      await context.close()
      return 'The mobile browser creates UTF-8 data in a modal, opens and reloads details, then soft-deletes the ticket.'
    })
    await check('ticket_flow_json_boundary', async () => {
      expect((await api('/api/tickets', { method: 'POST', body: 'not json' })).status === 415, 'non-JSON creation was not rejected')
      return 'Creation rejects a request without the JSON content type.'
    })
    unverified('criterion_1', 'Creation, same-tab details and reload are covered; pointer/keyboard card navigation and full history traversal are not all exercised.')
    await checksPassedCriterion(check, 'criterion_2', 'HTTP accepts both identifiers and details reflect persisted values.')
    unverified('criterion_3', 'Deletion and key retirement are observed, but on-disk retention across restart is runner-level.')
    unverified('criterion_4', 'Successful deletion is covered; injected create/delete failures and navigation races are not.')
    unverified('criterion_5', 'UTF-8 JSON, media-type rejection and mobile rendering are covered; focus behavior is not exhaustively assessed.')
  },

  async kanban_c5_edit_move({ api, trigger, browser, baseUrl, output, check, unverified }) {
    let ticket
    await check('edit_fixture_creation', async () => {
      ticket = await create(trigger, { title: 'Editable probe', description: 'Keep me', assignee: 'Initial' })
      return 'A persisted ticket is available for edit and move evaluation.'
    })
    await check('edit_save_cancel_and_partial_update', async () => {
      const partial = await trigger('kanban::tickets::update', { id: ticket.key, changes: { priority: 'high' } })
      expect(partial.description === 'Keep me' && partial.title === ticket.title, 'partial update erased unrelated fields')
      const { context, page } = await pageFor(browser, `${baseUrl}/#ticket/${ticket.id}`, { width: 390, height: 844 })
      await expectText(page.getByRole('heading', { name: ticket.title }), /Editable probe/)
      await page.getByRole('button', { name: 'Edit ticket' }).click()
      let edit = page.getByRole('heading', { name: 'Edit ticket' }).locator('xpath=ancestor::form[1]')
      await edit.getByLabel('Title').fill('Discard this')
      await edit.getByRole('button', { name: 'Cancel' }).click()
      await expectText(page.getByRole('heading', { name: ticket.title }), /Editable probe/)
      await page.getByRole('button', { name: 'Edit ticket' }).click()
      edit = page.getByRole('heading', { name: 'Edit ticket' }).locator('xpath=ancestor::form[1]')
      await edit.getByLabel('Title').fill('Edited probe')
      await edit.getByLabel('Status').selectOption('todo')
      await edit.getByLabel('Priority').selectOption('urgent')
      await edit.getByLabel('Assignee').fill('Updated')
      await edit.getByRole('button', { name: 'Save changes' }).click()
      await expectText(page.getByRole('heading', { name: 'Edited probe' }), /Edited probe/)
      const saved = (await json(await api(`/api/tickets/${ticket.id}`))).ticket
      expect(saved.status === 'todo' && saved.priority === 'urgent' && saved.assignee === 'Updated', 'UI edit was not persisted')
      await screenshot(page, output, 'ticket-edited-mobile')
      await context.close()
      return 'Partial iii updates preserve fields; browser cancel discards a draft and save persists editable fields on mobile.'
    })
    await check('drag_persists_status_only', async () => {
      const { context, page } = await pageFor(browser, baseUrl)
      await eventually(async () => await page.getByRole('heading', { name: 'Edited probe' }).count(), 'edited card did not appear')
      const card = page.getByRole('heading', { name: 'Edited probe' }).locator('xpath=ancestor::a[1]')
      const target = page.getByRole('heading', { name: /^Done\s*0$/i }).locator('xpath=ancestor::section[1]')
      await card.dragTo(target)
      await eventually(async () => (await json(await api(`/api/tickets/${ticket.id}`))).ticket.status === 'done', 'drag did not persist done status')
      const moved = (await json(await api(`/api/tickets/${ticket.id}`))).ticket
      expect(moved.priority === 'urgent' && moved.assignee === 'Updated', 'drag changed fields besides status')
      await screenshot(page, output, 'board-after-drag')
      await context.close()
      return 'A real pointer drag persists only status and updates the board.'
    })
    await checksPassedCriterion(check, 'criterion_1', 'Save, cancel and partial update behavior are exercised.')
    unverified('criterion_2', 'Field preservation is covered; every invalid/immutable/deleted update boundary is not exercised.')
    unverified('criterion_3', 'A real successful pointer drag is covered; injected failure and retry are not.')
    unverified('criterion_4', 'Normal completion is covered; pending-operation navigation races and failed drafts are not.')
    unverified('criterion_5', 'Mobile status editing is covered; process restart durability is runner-level.')
  },

  async kanban_c6_discussion({ api, trigger, browser, baseUrl, output, check, unverified }) {
    let ticket
    await check('discussion_fixture_creation', async () => {
      ticket = await create(trigger, { title: 'Discussion probe' })
      return 'A persisted ticket is available for discussion evaluation.'
    })
    await check('discussion_comments_replies_timeline', async () => {
      const { context, page } = await pageFor(browser, `${baseUrl}/#ticket/${ticket.key}`, { width: 390, height: 844 })
      await expectText(page.getByRole('heading', { name: ticket.title }), /Discussion probe/)
      await page.getByLabel('Your name').fill('Alice')
      await page.getByLabel('Comment').fill('<img src=x onerror=alert(1)> first')
      await page.getByRole('button', { name: 'Post comment' }).click()
      await expectText(page.getByText('<img src=x onerror=alert(1)> first', { exact: true }), /first/)
      expect(await page.locator('img').count() === 0, 'comment markup created an image element')
      await page.getByRole('button', { name: 'Reply to Alice' }).click()
      await page.getByLabel('Comment').fill('second')
      await page.getByRole('button', { name: 'Post comment' }).click()
      await expectText(page.getByText('second', { exact: true }), /second/)
      let stored = (await json(await api(`/api/tickets/${ticket.id}`))).ticket
      expect(stored.comments.length === 2 && stored.comments[1].parent_id === stored.comments[0].id, 'browser reply did not persist its parent')
      stored = await trigger('kanban::tickets::comment', {
        id: ticket.key,
        comment: { author: 'Carol', body: 'third', parent_id: stored.comments[1].id },
      })
      expect(stored.comments.map(({ body }) => body).join(',') === '<img src=x onerror=alert(1)> first,second,third', 'timeline posting order is wrong')
      await eventually(async () => await page.getByText('third', { exact: true }).count(), 'direct iii reply was not rendered live')
      await screenshot(page, output, 'discussion-mobile')
      await context.close()
      return 'Browser comments and replies plus a direct iii reply-to-reply persist in chronological order and render text safely.'
    })
    await check('discussion_foreign_parent_rejected', async () => {
      const other = await create(trigger, { title: 'Other discussion' })
      const foreign = await trigger('kanban::tickets::comment', { id: other.id, comment: { author: 'X', body: 'foreign root' } })
      let rejected = false
      try {
        await trigger('kanban::tickets::comment', { id: ticket.id, comment: { author: 'X', body: 'bad', parent_id: foreign.comments[0].id } })
      } catch { rejected = true }
      expect(rejected, 'a foreign parent was accepted')
      return 'A comment cannot reference a parent from another ticket.'
    })
    await checksPassedCriterion(check, 'criterion_1', 'Comments, replies and replies-to-replies persist in posting order with valid parents.')
    unverified('criterion_2', 'Safe rendering and mobile use are covered; keyboard parent navigation is not explicitly exercised.')
    unverified('criterion_3', 'Foreign parents are rejected; all malformed-store cases are not exercised.')
    unverified('criterion_4', 'Successful overlap with a live iii response is covered; injected failures and all draft races are not.')
    unverified('criterion_5', 'Existing records and edits remain usable; deletion and restart durability are not exercised here.')
  },

  async kanban_c7_live({ api, trigger, browser, baseUrl, output, check, unverified }) {
    await check('live_three_sessions_and_direct_iii', async () => {
      const sessions = await Promise.all([0, 1, 2].map(() => pageFor(browser, baseUrl)))
      const ticket = await create(trigger, { title: 'Live probe', priority: 'low' })
      await Promise.all(sessions.map(({ page }) => eventually(async () => await page.getByRole('heading', { name: 'Live probe' }).count(), 'session missed direct iii creation')))

      await sessions[0].page.getByRole('heading', { name: 'Live probe' }).click()
      await sessions[1].page.getByRole('heading', { name: 'Live probe' }).click()
      await sessions[2].page.getByRole('heading', { name: 'Live probe' }).click()
      await sessions[0].page.getByRole('button', { name: 'Edit ticket' }).click()
      const edit = sessions[0].page.getByRole('heading', { name: 'Edit ticket' }).locator('xpath=ancestor::form[1]')
      await edit.getByLabel('Title').fill('Dirty local title')
      await trigger('kanban::tickets::update', { id: ticket.id, changes: { priority: 'urgent', status: 'in_progress' } })
      await eventually(async () => (await edit.getByLabel('Title').inputValue()) === 'Dirty local title', 'dirty edit draft was lost')
      await edit.getByRole('button', { name: 'Save changes' }).click()
      await eventually(async () => (await trigger('kanban::tickets::get', { id: ticket.id })).title === 'Dirty local title', 'local edit did not save')
      expect((await trigger('kanban::tickets::get', { id: ticket.id })).priority === 'urgent', 'saving overwrote untouched remote priority')

      await sessions[1].page.getByLabel('Your name').fill('Remote author')
      await sessions[1].page.getByLabel('Comment').fill('Live comment')
      await sessions[1].page.getByRole('button', { name: 'Post comment' }).click()
      await eventually(async () => await sessions[2].page.getByText('Live comment', { exact: true }).count(), 'third session missed browser comment')
      const withComment = await trigger('kanban::tickets::get', { id: ticket.id })
      await trigger('kanban::tickets::comment', { id: ticket.id, comment: { author: 'Direct', body: 'Live reply', parent_id: withComment.comments[0].id } })
      await Promise.all(sessions.map(({ page }) => eventually(async () => await page.getByText('Live reply', { exact: true }).count(), 'session missed direct iii reply')))

      await trigger('kanban::tickets::delete', { id: ticket.id })
      await Promise.all(sessions.map(({ page }) => eventually(async () => await page.getByText('Ticket not found.', { exact: true }).count(), 'session retained stale deleted details')))
      await screenshot(sessions[2].page, output, 'live-third-session')
      await Promise.all(sessions.map(({ context }) => context.close()))
      return 'Three isolated browser contexts receive direct iii create/update/reply/delete changes without reload; a dirty edit preserves an untouched remote field.'
    })
    await checksPassedCriterion(check, 'criterion_1', 'Three browser sessions observe creation, edits, comments, replies and deletion without reload.')
    unverified('criterion_2', 'A dirty edit and untouched remote field are covered; every focus/comment/reply draft combination is not.')
    unverified('criterion_3', 'Offline reconnect and Compose restart are runner-level and are not claimed by this probe.')
    unverified('criterion_4', 'Remote deletion is covered; store switching during active drag is not.')
    unverified('criterion_5', 'Normal live ordering is covered; delayed-response injection, listener accounting and shutdown timing are not.')
  },
}

function checksPassedCriterion(check, id, detail) {
  return check(id, async () => detail)
}

async function expectText(locator, pattern) {
  await eventually(async () => pattern.test(await locator.first().textContent() ?? ''), `expected text ${pattern}`)
}

await main()
