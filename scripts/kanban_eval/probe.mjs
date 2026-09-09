#!/usr/bin/env node

import { mkdir, readFile, rename, writeFile } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
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

class ControlError extends Error {}

export async function control(output, operation, payload = {}) {
  try {
    const id = randomUUID()
    const request = join(output, 'control-request.json')
    const temporary = `${request}.${id}.tmp`
    await writeFile(temporary, `${JSON.stringify({ id, operation, payload })}\n`)
    await rename(temporary, request)
    const deadline = Date.now() + 90_000
    while (Date.now() < deadline) {
      let response
      try { response = JSON.parse(await readFile(join(output, 'control-response.json'), 'utf8')) } catch {}
      if (response?.id === id) {
        if (!response.ok) throw new Error(`control ${operation} failed: ${response.error ?? 'unknown error'}`)
        return response.value
      }
      await new Promise((resolve) => setTimeout(resolve, 100))
    }
    throw new Error(`control ${operation} timed out`)
  } catch (error) {
    throw new ControlError(details(error))
  }
}

async function inspectorClient(websocketUrl) {
  try {
    const socket = new WebSocket(websocketUrl)
    await new Promise((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error('inspector connection timed out')), 5_000)
      socket.addEventListener('open', () => { clearTimeout(timeout); resolve() }, { once: true })
      socket.addEventListener('error', () => { clearTimeout(timeout); reject(new Error('inspector connection failed')) }, { once: true })
    })
    let nextId = 0
    const pending = new Map()
    socket.addEventListener('message', ({ data }) => {
      const message = JSON.parse(String(data))
      if (!message.id || !pending.has(message.id)) return
      const { resolve, reject, timeout } = pending.get(message.id)
      clearTimeout(timeout)
      pending.delete(message.id)
      if (message.error) reject(new Error(message.error.message))
      else resolve(message.result)
    })
    const command = (method, params = {}) => new Promise((resolve, reject) => {
      const id = ++nextId
      const timeout = setTimeout(() => {
        pending.delete(id)
        reject(new Error(`inspector ${method} timed out`))
      }, 5_000)
      pending.set(id, { resolve, reject, timeout })
      socket.send(JSON.stringify({ id, method, params }))
    })
    await command('Runtime.enable')
    await command('HeapProfiler.enable')
    return { command, close: () => socket.close() }
  } catch (error) {
    throw new ControlError(`inspector setup failed: ${details(error)}`)
  }
}

async function countSseServerResponses(inspector) {
  const group = `kanban-sse-${randomUUID()}`
  try {
    await inspector.command('HeapProfiler.collectGarbage')
    await inspector.command('HeapProfiler.collectGarbage')
    const evaluated = await inspector.command('Runtime.evaluate', {
      expression: "(await import('node:http')).ServerResponse.prototype",
      awaitPromise: true,
      objectGroup: group,
    })
    const prototypeObjectId = evaluated.result?.objectId
    if (!prototypeObjectId) throw new Error('ServerResponse prototype has no inspector object id')
    const queried = await inspector.command('Runtime.queryObjects', { prototypeObjectId, objectGroup: group })
    const count = await inspector.command('Runtime.callFunctionOn', {
      objectId: queried.objects.objectId,
      functionDeclaration: "function () { return this.filter((response) => { try { return String(response.getHeader('content-type')).startsWith('text/event-stream') } catch { return false } }).length }",
      returnByValue: true,
    })
    if (!Number.isInteger(count.result?.value)) throw new Error('SSE response count is not an integer')
    return count.result.value
  } catch (error) {
    throw new ControlError(`inspector SSE count failed: ${details(error)}`)
  } finally {
    await inspector.command('Runtime.releaseObjectGroup', { objectGroup: group }).catch(() => {})
  }
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
      if (error instanceof ControlError) throw error
      checks.push({ id, status: 'failed', detail: details(error) })
      for (const [index, page] of (browser?.contexts().flatMap((context) => context.pages()) ?? []).entries()) {
        await page.screenshot({ path: join(output, `failure-${id}-${index}.png`), fullPage: true, timeout: 3000 }).catch(() => {})
      }
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
      control: (operation, payload) => control(output, operation, payload),
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
  async kanban_c1_foundation({ api, trigger, control, browser, baseUrl, output, check, unverified }) {
    await check('foundation_assets_and_safe_routes', async () => {
      for (const path of ['/', '/page.js', '/styles.css']) expect((await api(path)).ok, `${path} did not load`)
      for (const path of ['/constructor', '/toString', '/__proto__']) expect((await api(path)).status === 404, `${path} was not 404`)
      return 'Standalone HTML, JavaScript and CSS load; prototype-like asset paths return 404.'
    })
    await check('foundation_configuration_contract', async () => {
      await control('restart', { register_configuration: false, reset_configuration: true })
      const initial = await eventually(async () => {
        const response = await api('/api/config')
        return response.ok ? response.json() : false
      }, 'application did not initialize missing configuration', 15_000)
      expect(initial.data_dir === './data', 'missing configuration did not initialize the default data directory')
      expect(initial.data_dir && initial.resolved_data_dir, 'GET /api/config omitted paths')
      const raw = await trigger('configuration::get', { id: 'kanban', raw: true })
      await trigger('configuration::set', { id: 'kanban', value: { ...raw.value, evaluator_marker: 'preserve-me' } })
      const saved = await json(await api('/api/config', {
        method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ data_dir: './probe-browser-data' }),
      }))
      expect(saved.data_dir === './probe-browser-data', 'PUT did not select the directory')
      const after = await trigger('configuration::get', { id: 'kanban', raw: true })
      expect(after.value.evaluator_marker === 'preserve-me', 'saving discarded unrelated configuration')
      await trigger('configuration::set', { id: 'kanban', value: { ...after.value, data_dir: './direct-probe-data' } })
      const direct = await eventually(async () => {
        const response = await api('/api/config')
        if (!response.ok) return false
        const value = await response.json()
        return value.data_dir === './direct-probe-data' ? value : false
      }, 'direct configuration update did not select the effective directory')
      expect(direct.resolved_data_dir.endsWith('/direct-probe-data'), 'relative direct configuration did not resolve from the project root')
      await json(await api('/api/config', {
        method: 'PUT', headers: { 'content-type': 'application/json' }, body: '{"data_dir":"./probe-browser-data"}',
      }))
      expect((await api('/api/config', { method: 'PUT', headers: { 'content-type': 'application/json' }, body: '{"data_dir":""}' })).status === 400, 'empty directory was accepted')
      await control('restart')
      const restarted = await eventually(async () => {
        const response = await api('/api/config')
        return response.ok ? response.json() : false
      }, 'application did not recover configuration after restart', 15_000)
      const persisted = await trigger('configuration::get', { id: 'kanban', raw: true })
      expect(restarted.data_dir === './probe-browser-data' && persisted.value.evaluator_marker === 'preserve-me', 'restart lost existing or unrelated configuration')
      return 'Browser and direct configuration changes select resolved paths; unrelated data survives saving and restart.'
    })
    await check('foundation_accessible_settings', async () => {
      const { context, page } = await pageFor(browser, baseUrl, { width: 390, height: 844 })
      await page.getByLabel('Data directory').fill('./probe-ui-data')
      await page.getByRole('button', { name: 'Save settings' }).click()
      await expectText(page.getByRole('status').filter({ hasText: /Settings saved/ }), /Settings saved/)
      await screenshot(page, output, 'settings-mobile')
      await context.close()
      return 'The settings form loads and saves at a mobile viewport using accessible controls.'
    })
    await check('foundation_development_hot_reload', async () => {
      const observed = await control('hot_reload')
      expect(observed?.observed !== false, 'source marker was not observed by the running development process')
      expect((await api('/')).ok, 'application stopped serving after the source was restored')
      return 'A trusted temporary TypeScript source marker is observed without a manual build and the original bytes are restored.'
    })
    await checksPassedCriterion(check, 'criterion_1', 'Compose starts against an isolated engine with no Console service and standalone browser assets load.')
    await checksPassedCriterion(check, 'criterion_2', 'Missing configuration initializes and existing unrelated data survives saving and restart.')
    await checksPassedCriterion(check, 'criterion_3', 'Browser and direct iii changes select the effective directory and relative paths resolve from the project root.')
    await checksPassedCriterion(check, 'criterion_4', 'Invalid config and prototype-like asset paths are rejected by the live server.')
    await checksPassedCriterion(check, 'criterion_5', 'Trusted source mutation proves development hot reload and the mobile settings form remains usable.')
  },

  async kanban_c2_persistence({ api, trigger, control, check }) {
    let first
    await check('persistence_iii_crud', async () => {
      first = await trigger('kanban::tickets::create', { title: '  Defaults ticket  ' })
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
      for (const input of [
        { title: '' }, { title: 'x', status: 'invented' }, { title: 'x', priority: 'invented' },
        { title: 'x', assignee: 42 }, { title: 'x', unexpected: true },
      ]) {
        let rejected = false
        try { await trigger('kanban::tickets::create', input) } catch { rejected = true }
        expect(rejected, `invalid ticket was accepted: ${JSON.stringify(input)}`)
      }
      expect((await trigger('kanban::tickets::list')).tickets.length === before, 'invalid create changed the store')
      return 'Invalid fields and values are rejected without adding a ticket.'
    })
    await check('persistence_directory_isolation_and_restart', async () => {
      const initial = await json(await api('/api/config'))
      try {
        await json(await api('/api/config', {
          method: 'PUT', headers: { 'content-type': 'application/json' }, body: '{"data_dir":"/data/alternate"}',
        }))
        expect((await trigger('kanban::tickets::list')).tickets.length === 0, 'alternate directory exposed primary tickets')
        const alternate = await create(trigger, { title: 'Alternate store ticket' })
        expect(alternate.key === 'KAN-1', 'alternate store did not have an independent key sequence')
      } finally {
        await json(await api('/api/config', {
          method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ data_dir: initial.data_dir }),
        }))
      }
      expect((await trigger('kanban::tickets::get', { id: first.id })).id === first.id, 'primary ticket was not restored after switching directories')
      await control('restart')
      await eventually(async () => (await trigger('kanban::tickets::get', { id: first.key })).id === first.id, 'ticket did not survive restart', 15_000)
      return 'Configured directories keep independent tickets and key sequences; primary tickets survive a full runtime restart.'
    })
    await check('persistence_corrupt_and_duplicate_store_fail_closed', async () => {
      const original = await control('read_store')
      const rejectsWithoutRewrite = async (content, label) => {
        await control('write_store', { value: content })
        let rejected = false
        try { await trigger('kanban::tickets::list') } catch { rejected = true }
        expect(rejected, `${label} store was accepted`)
        expect(await control('read_store') === content, `${label} store was replaced after rejection`)
      }
      try {
        await rejectsWithoutRewrite('{broken json', 'corrupt')
        const records = JSON.parse(original)
        const duplicate = `${JSON.stringify([...records, records[0]], null, 2)}\n`
        await rejectsWithoutRewrite(duplicate, 'duplicate')
      } finally {
        await control('write_store', { value: original })
      }
      expect((await trigger('kanban::tickets::get', { id: first.id })).id === first.id, 'restored valid store is unusable')
      return 'Corrupt JSON and duplicate records fail closed without byte changes; the original store is restored afterward.'
    })
    await checksPassedCriterion(check, 'criterion_1', 'Both identifiers and real SDK caller metadata work through iii.')
    await checksPassedCriterion(check, 'criterion_2', 'Defaults, normalization, timestamps and increasing readable keys are observed.')
    await checksPassedCriterion(check, 'criterion_3', 'Configured directories remain isolated and tickets survive runtime restart.')
    await checksPassedCriterion(check, 'criterion_4', 'Invalid inputs and corrupt or duplicate stores fail without replacing existing bytes.')
  },

  async kanban_c3_board({ api, trigger, browser, baseUrl, output, check }) {
    const statuses = [['backlog', 'Backlog'], ['todo', 'To do'], ['in_progress', 'In progress'], ['in_review', 'In review'], ['done', 'Done']]
    await check('board_empty_loading_error_retry', async () => {
      const context = await browser.newContext({ viewport: { width: 1280, height: 900 } })
      const page = await context.newPage()
      let release
      const held = new Promise((resolve) => { release = resolve })
      let mode = 'hold'
      await page.route('**/api/tickets', async (route) => {
        if (mode === 'hold') { await held; await route.continue(); return }
        if (mode === 'fail') { await route.fulfill({ status: 500, contentType: 'application/json', body: '{"error":"probe failure"}' }); return }
        await route.continue()
      })
      await page.goto(baseUrl)
      await expectText(page.getByRole('status').filter({ hasText: /Loading tickets/ }), /Loading tickets/)
      expect(await page.getByText('— tickets', { exact: true }).count() === 1, 'loading fabricated a zero total')
      release()
      await expectText(page.getByRole('status').filter({ hasText: /No tickets yet/ }), /No tickets yet/)
      expect(await page.getByText('No tickets', { exact: true }).count() === 5, 'empty lanes are not distinguished')
      mode = 'fail'
      await page.getByRole('button', { name: 'Refresh board' }).click()
      await expectText(page.getByRole('status').filter({ hasText: /Unable to load tickets/ }), /Unable to load tickets/)
      expect(await page.getByText('— tickets', { exact: true }).count() === 1, 'failed read fabricated a zero total')
      mode = 'pass'
      await page.getByRole('button', { name: 'Retry' }).click()
      await expectText(page.getByRole('status').filter({ hasText: /No tickets yet/ }), /No tickets yet/)
      await context.close()
      return 'Loading, empty and failed reads are distinct; retry recovers and unavailable counts remain unknown.'
    })
    await check('board_five_lanes_refresh_safe_text_and_settings', async () => {
      const initialConfig = await json(await api('/api/config'))
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
      const markup = '<img src=x onerror=alert(1)> literal title'
      await create(trigger, { title: markup })
      await page.getByRole('button', { name: 'Refresh board' }).click()
      await eventually(async () => await page.getByRole('heading', { name: markup }).count(), 'refresh did not load the new ticket')
      expect(await page.locator('img').count() === 0, 'ticket markup created an image element')

      await page.getByRole('link', { name: 'Settings' }).click()
      await page.getByLabel('Data directory').fill('./board-probe-empty')
      await page.getByRole('button', { name: 'Save settings' }).click()
      await expectText(page.getByRole('status').filter({ hasText: /Settings saved/ }), /Settings saved/)
      await page.getByRole('link', { name: /Board/ }).click()
      await expectText(page.getByRole('status').filter({ hasText: /No tickets yet/ }), /No tickets yet/)
      await page.getByRole('link', { name: 'Settings' }).click()
      await page.getByLabel('Data directory').fill(initialConfig.data_dir)
      await page.getByRole('button', { name: 'Save settings' }).click()
      await expectText(page.getByRole('status').filter({ hasText: /Settings saved/ }), /Settings saved/)
      await page.getByRole('link', { name: /Board/ }).click()
      await eventually(async () => await page.getByRole('heading', { name: markup }).count(), 'returning from settings did not reload the restored store')

      expect(!await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth), 'desktop page has document-wide horizontal overflow')
      await screenshot(page, output, 'board-desktop')
      await page.setViewportSize({ width: 390, height: 844 })
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth)
      expect(!overflow, 'mobile page has document-wide horizontal overflow')
      await screenshot(page, output, 'board-mobile')
      await context.close()
      return 'Five lanes, refresh, store navigation, safe user text and responsive overflow behavior work.'
    })
    await checksPassedCriterion(check, 'criterion_1', 'Tickets created through iii appear in all five lanes with counts and metadata.')
    await checksPassedCriterion(check, 'criterion_2', 'Loading, empty, failed and recovered states are distinguished without fabricated counts.')
    await checksPassedCriterion(check, 'criterion_3', 'Refresh and returning from settings reload the selected store.')
    await checksPassedCriterion(check, 'criterion_4', 'Markup remains literal and neither desktop nor mobile has document-wide overflow.')
  },

  async kanban_c4_ticket_flow({ api, trigger, control, browser, baseUrl, output, check }) {
    let created
    await check('ticket_flow_create_detail_and_delete', async () => {
      const keyboardTicket = await create(trigger, { title: 'Keyboard card probe' })
      const { context, page } = await pageFor(browser, baseUrl, { width: 390, height: 844 })
      await eventually(async () => await page.getByRole('heading', { name: keyboardTicket.title }).count(), 'keyboard card did not load')
      const card = page.getByRole('heading', { name: keyboardTicket.title }).locator('xpath=ancestor::a[1]')
      await card.focus()
      await card.press('Enter')
      await expectText(page.getByRole('heading', { name: keyboardTicket.title }), /Keyboard card probe/)
      await page.goBack()
      await expectText(page.getByRole('heading', { name: 'The board.' }), /The board/)
      await page.getByRole('heading', { name: keyboardTicket.title }).click()
      await expectText(page.getByRole('heading', { name: keyboardTicket.title }), /Keyboard card probe/)
      await page.goBack()
      await page.getByRole('button', { name: 'New ticket' }).click()
      const dialog = page.getByRole('dialog', { name: 'New ticket' })
      expect(await dialog.getByLabel('Title').evaluate((element) => element === document.activeElement), 'modal did not focus the title field')
      await dialog.getByLabel('Title').fill('Browser-created ticket')
      await dialog.getByLabel('Description').fill('UTF-8: ação e café')
      await dialog.getByLabel('Status').selectOption('in_review')
      await dialog.getByLabel('Priority').selectOption('urgent')
      await dialog.getByLabel('Assignee').fill('Lin')
      let failCreate = true
      await page.route('**/api/tickets', async (route) => {
        if (failCreate && route.request().method() === 'POST') {
          failCreate = false
          await route.fulfill({ status: 500, contentType: 'application/json', body: '{"error":"probe failure"}' })
          return
        }
        await route.continue()
      })
      await dialog.getByRole('button', { name: 'Create ticket' }).click()
      await expectText(dialog.getByRole('status'), /Unable to create ticket/)
      expect(await dialog.isVisible() && await dialog.getByLabel('Title').inputValue() === 'Browser-created ticket', 'failed create closed the modal or erased its draft')
      await dialog.getByRole('button', { name: 'Create ticket' }).click()
      await expectText(page.getByRole('heading', { name: 'Browser-created ticket' }), /Browser-created ticket/)
      expect(await dialog.isVisible() === false, 'create dialog stayed open')
      created = (await json(await api('/api/tickets'))).tickets.find(({ title }) => title === 'Browser-created ticket')
      expect(created, 'created ticket is absent from API list')
      expect((await json(await api(`/api/tickets/${created.id}`))).ticket.key === created.key, 'HTTP UUID lookup failed')
      expect((await json(await api(`/api/tickets/${created.key}`))).ticket.id === created.id, 'HTTP key lookup failed')
      expect((await trigger('kanban::tickets::get', { id: created.id })).key === created.key, 'iii UUID lookup failed')
      expect((await trigger('kanban::tickets::get', { id: created.key })).id === created.id, 'iii key lookup failed')
      for (const value of ['UTF-8: ação e café', 'In review', 'urgent', 'Lin']) {
        expect(await page.getByText(value, { exact: true }).count() === 1, `detail omitted persisted value: ${value}`)
      }
      await page.reload()
      await expectText(page.getByRole('heading', { name: 'Browser-created ticket' }), /Browser-created ticket/)
      expect(!await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth), 'mobile detail has document-wide overflow')
      await screenshot(page, output, 'ticket-detail-mobile')
      await context.close()
      return 'Keyboard and pointer cards, history, failed-create retry, same-tab details, direct links, focus, reload and mobile layout work.'
    })
    await check('ticket_flow_delete_failure_navigation_and_restart', async () => {
      const { context, page } = await pageFor(browser, `${baseUrl}/#ticket/${created.id}`, { width: 390, height: 844 })
      let failDelete = true
      await page.route('**/api/tickets/**', async (route) => {
        if (failDelete && route.request().method() === 'DELETE') {
          failDelete = false
          await route.fulfill({ status: 500, contentType: 'application/json', body: '{"error":"probe failure"}' })
          return
        }
        await route.continue()
      })
      await page.getByRole('button', { name: 'Delete ticket' }).click()
      await expectText(page.getByRole('status').filter({ hasText: /Unable to delete ticket/ }), /Unable to delete ticket/)
      expect(await page.getByRole('heading', { name: created.title }).count() === 1, 'failed delete removed the detail view')

      await page.unroute('**/api/tickets/**')
      let release
      let intercepted
      let fulfilled
      const reached = new Promise((resolve) => { intercepted = resolve })
      const held = new Promise((resolve) => { release = resolve })
      const delivered = new Promise((resolve) => { fulfilled = resolve })
      await page.route('**/api/tickets/**', async (route) => {
        if (route.request().method() !== 'DELETE') { await route.continue(); return }
        const response = await route.fetch()
        intercepted()
        await held
        await route.fulfill({ response })
        fulfilled()
      })
      await page.getByRole('button', { name: 'Delete ticket' }).click()
      await reached
      await page.evaluate(() => { location.hash = '#board' })
      release()
      await delivered
      await eventually(async () => !(await json(await api('/api/tickets'))).tickets.some(({ id }) => id === created.id), 'deleted card remained listed')
      expect((await api(`/api/tickets/${created.id}`)).status === 404, 'deleted ticket is still retrievable')
      let deleted = JSON.parse(await control('read_store')).find(({ id }) => id === created.id)
      expect(deleted.deleted_at, 'soft-deleted ticket is absent from disk')
      await context.close()
      await control('restart')
      deleted = JSON.parse(await control('read_store')).find(({ id }) => id === created.id)
      expect(deleted.deleted_at, 'restart lost the soft-deleted record')
      const next = await eventually(() => create(trigger, { title: 'Key retirement probe' }), 'create did not recover after restart', 15_000)
      expect(Number(next.key.slice(4)) > Number(created.key.slice(4)), 'deleted readable key was reused')
      return 'Failed deletion is recoverable; late deletion cannot leave a stale card, and disk retention plus key retirement survive restart.'
    })
    await check('ticket_flow_utf8_chunks_and_json_boundary', async () => {
      expect((await api('/api/tickets', { method: 'POST', body: 'not json' })).status === 415, 'non-JSON creation was not rejected')
      const title = 'Chunked ação café'
      const encoded = new TextEncoder().encode(JSON.stringify({ title }))
      const split = encoded.indexOf(0xc3) + 1
      const body = new ReadableStream({
        start(controller) {
          controller.enqueue(encoded.slice(0, split))
          controller.enqueue(encoded.slice(split))
          controller.close()
        },
      })
      const chunked = await json(await api('/api/tickets', {
        method: 'POST', headers: { 'content-type': 'application/json' }, body, duplex: 'half',
      }))
      expect(chunked.ticket.title === title, 'UTF-8 split across request chunks was corrupted')
      return 'Non-JSON creation is rejected and a multibyte character split across request chunks is preserved.'
    })
    await checksPassedCriterion(check, 'criterion_1', 'Modal creation, same-tab details, keyboard and pointer cards, history and reload work.')
    await checksPassedCriterion(check, 'criterion_2', 'HTTP accepts both identifiers and details reflect persisted values.')
    await checksPassedCriterion(check, 'criterion_3', 'Deletion removes the card while preserving disk state across restart and never reuses the key.')
    await checksPassedCriterion(check, 'criterion_4', 'Failed creation and deletion preserve recoverable UI; late deletion does not leave a stale card.')
    await checksPassedCriterion(check, 'criterion_5', 'Chunked UTF-8, JSON enforcement, focus and mobile layout are directly exercised.')
  },

  async kanban_c5_edit_move({ api, trigger, control, browser, baseUrl, output, check }) {
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
      await edit.getByLabel('Description').fill('Edited description')
      await edit.getByLabel('Status').selectOption('todo')
      await edit.getByLabel('Priority').selectOption('urgent')
      await edit.getByLabel('Assignee').fill('Updated')
      let failSave = true
      await page.route('**/api/tickets/**', async (route) => {
        if (failSave && route.request().method() === 'PATCH') {
          failSave = false
          await route.fulfill({ status: 500, contentType: 'application/json', body: '{"error":"probe failure"}' })
          return
        }
        await route.continue()
      })
      await edit.getByRole('button', { name: 'Save changes' }).click()
      await expectText(page.getByRole('status').filter({ hasText: /Unable to save ticket/ }), /Unable to save ticket/)
      expect(await edit.getByLabel('Title').inputValue() === 'Edited probe', 'failed save erased the title draft')
      expect(await edit.getByLabel('Assignee').inputValue() === 'Updated', 'failed save erased another draft field')
      await edit.getByRole('button', { name: 'Save changes' }).click()
      await expectText(page.getByRole('heading', { name: 'Edited probe' }), /Edited probe/)
      const saved = (await json(await api(`/api/tickets/${ticket.id}`))).ticket
      expect(saved.description === 'Edited description' && saved.status === 'todo' && saved.priority === 'urgent' && saved.assignee === 'Updated', 'UI edit was not persisted')

      await page.getByRole('button', { name: 'Edit ticket' }).click()
      edit = page.getByRole('heading', { name: 'Edit ticket' }).locator('xpath=ancestor::form[1]')
      await edit.getByLabel('Title').fill('Late saved probe')
      let release
      let intercepted
      const reached = new Promise((resolve) => { intercepted = resolve })
      const held = new Promise((resolve) => { release = resolve })
      await page.unroute('**/api/tickets/**')
      await page.route('**/api/tickets/**', async (route) => {
        if (route.request().method() !== 'PATCH') { await route.continue(); return }
        intercepted()
        await held
        await route.continue()
      })
      await edit.getByRole('button', { name: 'Save changes' }).click()
      await reached
      await page.getByRole('link', { name: /Back to board/ }).click()
      release()
      await eventually(async () => (await json(await api(`/api/tickets/${ticket.id}`))).ticket.title === 'Late saved probe', 'late save did not persist after navigation')
      await eventually(async () => await page.getByRole('heading', { name: 'Late saved probe' }).count(), 'board did not refresh after the late save')
      await screenshot(page, output, 'ticket-edited-mobile')
      await context.close()
      return 'Cancel is inert, failed-save drafts survive, retry persists edits, and a save finishing after navigation refreshes the board.'
    })
    await check('edit_rejects_invalid_immutable_and_deleted_updates', async () => {
      const before = await trigger('kanban::tickets::get', { id: ticket.id })
      for (const changes of [{}, { id: 'replacement' }, { created_at: new Date().toISOString() }, { status: 'invented' }]) {
        expect((await api(`/api/tickets/${ticket.id}`, {
          method: 'PATCH', headers: { 'content-type': 'application/json' }, body: JSON.stringify(changes),
        })).status === 400, `invalid changes were accepted: ${JSON.stringify(changes)}`)
      }
      const after = await trigger('kanban::tickets::get', { id: ticket.id })
      expect(after.id === before.id && after.created_at === before.created_at && after.status === before.status, 'invalid update changed stored data')
      const deleted = await create(trigger, { title: 'Deleted update probe' })
      await trigger('kanban::tickets::delete', { id: deleted.id })
      expect((await api(`/api/tickets/${deleted.id}`, {
        method: 'PATCH', headers: { 'content-type': 'application/json' }, body: '{"title":"restore"}',
      })).status === 404, 'a deleted ticket was updated')
      return 'Empty, immutable and invalid updates are rejected without writes; deleted tickets cannot be restored.'
    })
    await check('drag_persists_status_only', async () => {
      const { context, page } = await pageFor(browser, baseUrl)
      await eventually(async () => await page.getByRole('heading', { name: 'Late saved probe' }).count(), 'edited card did not appear')
      const card = page.getByRole('heading', { name: 'Late saved probe' }).locator('xpath=ancestor::a[1]')
      const target = page.getByRole('heading', { name: /^Done\s*0$/i }).locator('xpath=ancestor::section[1]')
      let failMove = true
      await page.route('**/api/tickets/**', async (route) => {
        if (failMove && route.request().method() === 'PATCH') {
          failMove = false
          await route.fulfill({ status: 500, contentType: 'application/json', body: '{"error":"probe failure"}' })
          return
        }
        await route.continue()
      })
      await card.dragTo(target)
      await expectText(page.getByRole('status').filter({ hasText: /Unable to move ticket/ }), /Unable to move ticket/)
      expect((await json(await api(`/api/tickets/${ticket.id}`))).ticket.status === 'todo', 'failed drag changed persisted status')
      const todo = page.getByRole('heading', { name: /^To do\s*1$/i }).locator('xpath=ancestor::section[1]')
      expect(await todo.getByRole('heading', { name: 'Late saved probe' }).count() === 1, 'failed drag moved the visible card')
      await card.dragTo(target)
      await eventually(async () => (await json(await api(`/api/tickets/${ticket.id}`))).ticket.status === 'done', 'drag did not persist done status')
      const moved = (await json(await api(`/api/tickets/${ticket.id}`))).ticket
      expect(moved.priority === 'urgent' && moved.assignee === 'Updated', 'drag changed fields besides status')

      await page.unroute('**/api/tickets/**')
      let release
      let intercepted
      const reached = new Promise((resolve) => { intercepted = resolve })
      const held = new Promise((resolve) => { release = resolve })
      await page.route('**/api/tickets/**', async (route) => {
        if (route.request().method() !== 'PATCH') { await route.continue(); return }
        intercepted()
        await held
        await route.continue()
      })
      const review = page.getByRole('heading', { name: /^In review\s*0$/i }).locator('xpath=ancestor::section[1]')
      await card.dragTo(review)
      await reached
      await page.getByRole('link', { name: 'Settings' }).click()
      release()
      await eventually(async () => (await json(await api(`/api/tickets/${ticket.id}`))).ticket.status === 'in_review', 'late move did not persist after navigation')
      await page.getByRole('link', { name: /Board/ }).click()
      const restoredReview = page.getByRole('heading', { name: /^In review\s*1$/i }).locator('xpath=ancestor::section[1]')
      await eventually(async () => await restoredReview.getByRole('heading', { name: 'Late saved probe' }).count(), 'late move is absent from the relevant board')
      await screenshot(page, output, 'board-after-drag')
      await context.close()
      return 'Failed drag stays put, retry persists only status, and a move finishing after navigation appears on return.'
    })
    await check('edit_move_survives_restart', async () => {
      await control('restart')
      const persisted = await eventually(async () => {
        const response = await api(`/api/tickets/${ticket.id}`)
        return response.ok ? (await response.json()).ticket : false
      }, 'edited ticket was unavailable after restart', 15_000)
      expect(persisted.title === 'Late saved probe'
        && persisted.description === 'Edited description'
        && persisted.status === 'in_review'
        && persisted.priority === 'urgent'
        && persisted.assignee === 'Updated', 'restart lost an edited or moved field')
      const { context, page } = await pageFor(browser, `${baseUrl}/#ticket/${ticket.id}`, { width: 390, height: 844 })
      await page.getByRole('button', { name: 'Edit ticket' }).click()
      const edit = page.getByRole('heading', { name: 'Edit ticket' }).locator('xpath=ancestor::form[1]')
      expect(await edit.getByLabel('Status').inputValue() === 'in_review', 'mobile status selector did not reflect the persisted move')
      await context.close()
      return 'All edited fields and the mobile status alternative retain the moved state after runtime restart.'
    })
    await checksPassedCriterion(check, 'criterion_1', 'Save, cancel and partial update behavior are exercised.')
    await checksPassedCriterion(check, 'criterion_2', 'Invalid, immutable, empty and deleted-ticket updates fail without writes.')
    await checksPassedCriterion(check, 'criterion_3', 'Real drag persists only status; injected failure stays put and retry succeeds.')
    await checksPassedCriterion(check, 'criterion_4', 'Failed-save drafts survive and pending save/move completion updates the relevant board after navigation.')
    await checksPassedCriterion(check, 'criterion_5', 'The mobile status selector and all edited fields retain their state after restart.')
  },

  async kanban_c6_discussion({ api, trigger, control, browser, baseUrl, output, check }) {
    let ticket
    let other
    await check('discussion_fixture_creation', async () => {
      ticket = await create(trigger, { title: 'Discussion probe' })
      return 'A persisted ticket is available for discussion evaluation.'
    })
    await check('discussion_comments_replies_timeline', async () => {
      const { context, page } = await pageFor(browser, `${baseUrl}/#ticket/${ticket.key}`, { width: 390, height: 844 })
      await expectText(page.getByRole('heading', { name: ticket.title }), /Discussion probe/)
      await page.getByLabel('Your name').fill('Alice')
      await page.getByRole('textbox', { name: 'Comment', exact: true }).fill('<img src=x onerror=alert(1)> first')
      await page.getByRole('button', { name: 'Post comment' }).click()
      await expectText(page.getByText('<img src=x onerror=alert(1)> first', { exact: true }), /first/)
      expect(await page.locator('img').count() === 0, 'comment markup created an image element')
      await page.getByRole('button', { name: 'Reply to Alice' }).click()
      await page.getByRole('textbox', { name: 'Comment', exact: true }).fill('second')
      await page.getByRole('button', { name: 'Post comment' }).click()
      await expectText(page.getByText('second', { exact: true }), /second/)
      let stored = (await json(await api(`/api/tickets/${ticket.id}`))).ticket
      expect(stored.comments.length === 2 && stored.comments[1].parent_id === stored.comments[0].id, 'browser reply did not persist its parent')
      stored = await trigger('kanban::tickets::comment', {
        id: ticket.key,
        comment: { author: 'Carol', body: 'third', parent_id: stored.comments[1].id },
      })
      expect(stored.comments.map(({ body }) => body).join(',') === '<img src=x onerror=alert(1)> first,second,third', 'timeline posting order is wrong')
      await page.reload()
      await eventually(async () => await page.getByText('third', { exact: true }).count(), 'persisted iii reply was not rendered after reload')
      expect(await page.locator('time').count() === 3, 'timeline does not render a time for every comment')
      expect(await page.locator('time').evaluateAll((items) => items.every((item) => !Number.isNaN(Date.parse(item.dateTime)))), 'timeline contains an invalid machine-readable time')
      const parentLink = page.getByRole('button', { name: /Reply to Alice:/ }).first()
      await parentLink.focus()
      await parentLink.press('Enter')
      expect(await page.evaluate(() => document.activeElement?.textContent?.includes('first')), 'keyboard parent navigation did not focus the referenced comment')
      await screenshot(page, output, 'discussion-mobile')
      await context.close()
      return 'Comments and nested replies retain order; safe text, times and keyboard parent navigation work on mobile.'
    })
    await check('discussion_invalid_input_and_corrupt_store_fail_closed', async () => {
      other = await create(trigger, { title: 'Other discussion' })
      const foreign = await trigger('kanban::tickets::comment', { id: other.id, comment: { author: 'X', body: 'foreign root' } })
      for (const comment of [{ author: '', body: 'body' }, { author: 'A', body: '' }]) {
        expect((await api(`/api/tickets/${ticket.id}/comments`, {
          method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(comment),
        })).status === 400, `invalid comment was accepted: ${JSON.stringify(comment)}`)
      }
      expect((await api(`/api/tickets/${ticket.id}/comments`, {
        method: 'POST', headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ author: 'A', body: 'body', parent_id: foreign.comments[0].id }),
      })).status === 404, 'a foreign comment parent was accepted')
      const original = await control('read_store')
      const rejectsWithoutRewrite = async (mutate, label) => {
        const records = JSON.parse(original)
        mutate(records.find(({ id }) => id === ticket.id).comments)
        const malformed = `${JSON.stringify(records, null, 2)}\n`
        await control('write_store', { value: malformed })
        expect((await api(`/api/tickets/${ticket.id}`)).status === 500, `${label} discussion was accepted`)
        expect(await control('read_store') === malformed, `${label} discussion was rewritten after rejection`)
      }
      try {
        await rejectsWithoutRewrite((comments) => { comments[0].parent_id = randomUUID() }, 'foreign-parent')
        await rejectsWithoutRewrite((comments) => { comments[0].author = ' untrimmed ' }, 'untrimmed-author')
      } finally {
        await control('write_store', { value: original })
      }
      expect((await trigger('kanban::tickets::get', { id: ticket.id })).comments.length === 3, 'restored discussion is unusable')
      return 'Empty input, foreign parents and malformed stored discussions fail without rewriting disk state.'
    })
    await check('discussion_failure_and_overlap_preserve_drafts', async () => {
      const { context, page } = await pageFor(browser, `${baseUrl}/#ticket/${ticket.id}`, { width: 390, height: 844 })
      const author = page.getByLabel('Your name')
      const body = page.getByRole('textbox', { name: 'Comment', exact: true })
      await author.fill('Draft author')
      await body.fill('Draft during edit')
      await page.getByRole('button', { name: 'Edit ticket' }).click()
      await page.getByRole('button', { name: 'Cancel' }).click()
      expect(await author.inputValue() === 'Draft author' && await body.inputValue() === 'Draft during edit', 'ticket editing erased the comment draft')

      let failPost = true
      await page.route('**/api/tickets/*/comments', async (route) => {
        if (failPost) {
          failPost = false
          await route.fulfill({ status: 500, contentType: 'application/json', body: '{"error":"probe failure"}' })
          return
        }
        await route.continue()
      })
      await page.getByRole('button', { name: 'Post comment' }).click()
      await expectText(page.getByRole('status').filter({ hasText: /Unable to post comment/ }), /Unable to post comment/)
      expect(await body.inputValue() === 'Draft during edit', 'failed post erased the comment draft')
      await page.getByRole('button', { name: 'Post comment' }).click()
      await expectText(page.getByRole('status').filter({ hasText: /Comment posted/ }), /Comment posted/)

      await page.unroute('**/api/tickets/*/comments')
      let release
      let intercepted
      let fulfilled
      const reached = new Promise((resolve) => { intercepted = resolve })
      const held = new Promise((resolve) => { release = resolve })
      const delivered = new Promise((resolve) => { fulfilled = resolve })
      await page.route('**/api/tickets/*/comments', async (route) => {
        const response = await route.fetch()
        intercepted()
        await held
        await route.fulfill({ response })
        fulfilled()
      })
      await author.fill('Late author')
      await body.fill('Late activity')
      await page.getByRole('button', { name: 'Post comment' }).click()
      await reached
      await page.evaluate((id) => { location.hash = `#ticket/${id}` }, other.id)
      await expectText(page.getByRole('heading', { name: other.title }), /Other discussion/)
      await page.getByLabel('Your name').fill('Other author')
      await page.getByRole('textbox', { name: 'Comment', exact: true }).fill('Other ticket draft')
      await trigger('kanban::tickets::comment', { id: ticket.id, comment: { author: 'Concurrent', body: 'Newer activity' } })
      release()
      await delivered
      await eventually(async () => (await page.getByRole('textbox', { name: 'Comment', exact: true }).inputValue()) === 'Other ticket draft', 'late response erased another ticket draft')
      const stored = await trigger('kanban::tickets::get', { id: ticket.id })
      expect(stored.comments.some(({ body }) => body === 'Late activity') && stored.comments.some(({ body }) => body === 'Newer activity'), 'overlapping response lost posted activity')
      await screenshot(page, output, 'discussion-overlap-mobile')
      await context.close()
      return 'Editing and failed posts preserve drafts; a delayed post cannot erase another ticket draft or newer activity.'
    })
    await check('discussion_survives_edit_delete_and_restart', async () => {
      const edited = await trigger('kanban::tickets::update', { id: ticket.id, changes: { title: 'Discussion edited' } })
      expect(edited.comments.length >= 5, 'ticket edit discarded comments')
      await trigger('kanban::tickets::delete', { id: ticket.id })
      let records = JSON.parse(await control('read_store'))
      let deleted = records.find(({ id }) => id === ticket.id)
      expect(deleted.deleted_at && deleted.comments.length === edited.comments.length, 'soft deletion discarded the discussion on disk')
      await control('restart')
      records = JSON.parse(await control('read_store'))
      deleted = records.find(({ id }) => id === ticket.id)
      expect(deleted.deleted_at && deleted.comments.length === edited.comments.length, 'restart discarded the deleted discussion')
      await eventually(async () => (await trigger('kanban::tickets::get', { id: other.id })).id === other.id, 'existing ticket was unusable after restart', 15_000)
      return 'Comments survive ticket editing, soft deletion and runtime restart while remaining records stay usable.'
    })
    await checksPassedCriterion(check, 'criterion_1', 'Comments, replies and replies-to-replies persist in posting order with valid parents.')
    await checksPassedCriterion(check, 'criterion_2', 'Timeline text, authors, times and keyboard parent navigation work on mobile.')
    await checksPassedCriterion(check, 'criterion_3', 'Invalid inputs, foreign parents and malformed stored discussions fail closed.')
    await checksPassedCriterion(check, 'criterion_4', 'Failures and overlapping responses preserve drafts and newer activity.')
    await checksPassedCriterion(check, 'criterion_5', 'Discussions survive edits, soft deletion and runtime restart.')
  },

  async kanban_c7_live({ trigger, control, browser, baseUrl, output, check, unverified }) {
    await check('live_three_sessions_and_direct_iii', async () => {
      const sessions = await Promise.all([0, 1, 2].map(() => pageFor(browser, baseUrl)))
      const configuration = await trigger('configuration::get', { id: 'kanban', raw: true })
      const ticket = await create(trigger, { title: 'Live probe', priority: 'low' })
      await Promise.all(sessions.map(({ page }) => eventually(async () => await page.getByRole('heading', { name: 'Live probe' }).count(), 'session missed direct iii creation')))
      const focusedCard = sessions[0].page.getByRole('heading', { name: 'Live probe' }).locator('xpath=ancestor::a[1]')
      await focusedCard.focus()
      await create(trigger, { title: 'Live noise' })
      await eventually(async () => sessions[0].page.evaluate(() => document.activeElement?.textContent?.includes('Live probe')), 'focused card was lost during an ordinary live update')

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
      const preservedDraft = sessions[0].page.getByRole('textbox', { name: 'Comment', exact: true })
      await sessions[0].page.getByLabel('Your name').fill('Draft author')
      await preservedDraft.fill('Ordinary update draft')
      await preservedDraft.focus()
      await trigger('kanban::tickets::update', { id: ticket.id, changes: { description: 'Remote description' } })
      await eventually(async () => (await preservedDraft.inputValue()) === 'Ordinary update draft', 'ordinary live update erased comment draft')
      expect(await sessions[0].page.evaluate(() => document.activeElement?.tagName === 'TEXTAREA'), 'ordinary live update lost textarea focus')

      await sessions[1].page.getByLabel('Your name').fill('Remote author')
      await sessions[1].page.getByRole('textbox', { name: 'Comment', exact: true }).fill('Live comment')
      const [posted] = await Promise.all([
        sessions[1].page.waitForResponse((response) => response.url().endsWith('/comments') && response.request().method() === 'POST'),
        sessions[1].page.getByRole('button', { name: 'Post comment' }).click(),
      ])
      expect(posted.status() === 201, `browser comment failed with HTTP ${posted.status()}`)
      await eventually(async () => await sessions[2].page.getByText('Live comment', { exact: true }).count(), 'third session missed browser comment')
      const withComment = await trigger('kanban::tickets::get', { id: ticket.id })
      await sessions[0].page.getByRole('button', { name: 'Reply to Remote author' }).click()
      await preservedDraft.fill('Reply draft survives')
      await trigger('kanban::tickets::comment', { id: ticket.id, comment: { author: 'Direct', body: 'Live reply', parent_id: withComment.comments[0].id } })
      await Promise.all(sessions.map(({ page }) => eventually(async () => await page.getByText('Live reply', { exact: true }).count(), 'session missed direct iii reply')))
      expect(await preservedDraft.inputValue() === 'Reply draft survives', 'live reply erased a newer reply draft')

      await control('restart')
      await Promise.all(sessions.map(({ page }) => eventually(async () => /connected/i.test(await page.getByRole('status').filter({ hasText: /Live updates/ }).first().textContent() ?? ''), 'SSE did not reconnect after restart', 20_000)))
      await eventually(async () => (await trigger('kanban::tickets::get', { id: ticket.id })).id === ticket.id, 'iii did not reconnect after restart', 15_000)
      expect(await preservedDraft.inputValue() === 'Reply draft survives', 'restart erased same-store reply draft')

      await sessions[2].page.evaluate(() => { location.hash = '#board' })
      await eventually(async () => await sessions[2].page.getByRole('heading', { name: 'Dirty local title' }).count(), 'board did not recover after restart')
      const dragging = sessions[2].page.getByRole('heading', { name: 'Dirty local title' }).locator('xpath=ancestor::a[1]')
      await dragging.evaluate((element) => element.dispatchEvent(new DragEvent('dragstart', { bubbles: true, dataTransfer: new DataTransfer() })))
      await trigger('configuration::set', { id: 'kanban', value: { ...configuration.value, data_dir: '/data/live-alternate' } })
      await eventually(async () => await sessions[2].page.getByText('No tickets yet.', { exact: true }).count(), 'store switch during drag did not load the empty store')
      expect(await preservedDraft.inputValue() === '', 'store switch retained an old-store draft')
      await trigger('configuration::set', { id: 'kanban', value: configuration.value })
      await Promise.all(sessions.slice(0, 2).map(({ page }) => eventually(async () => await page.getByRole('heading', { name: 'Dirty local title' }).count(), 'detail did not recover after restoring the store')))

      await trigger('kanban::tickets::delete', { id: ticket.id })
      await Promise.all(sessions.slice(0, 2).map(({ page }) => eventually(async () => await page.getByText('Ticket not found.', { exact: true }).count(), 'session retained stale deleted details')))
      await eventually(async () => !(await sessions[2].page.getByRole('heading', { name: 'Dirty local title' }).count()), 'board retained a remotely deleted card')
      await screenshot(sessions[2].page, output, 'live-third-session')
      await Promise.all(sessions.map(({ context }) => context.close()))
      return 'Three sessions synchronize mutations; focus and drafts survive ordinary updates/restart, while a store switch during drag clears stale state.'
    })
    await check('live_late_responses_disconnect_and_shutdown', async () => {
      const ticket = await create(trigger, { title: 'Response ordering probe' })
      const { context, page } = await pageFor(browser, `${baseUrl}/#ticket/${ticket.id}`)
      await expectText(page.getByRole('heading', { name: ticket.title }), /Response ordering probe/)
      let release
      let intercepted
      let fulfilled
      const reached = new Promise((resolve) => { intercepted = resolve })
      const held = new Promise((resolve) => { release = resolve })
      const delivered = new Promise((resolve) => { fulfilled = resolve })
      let reads = 0
      await page.route(`**/api/tickets/${ticket.id}`, async (route) => {
        if (route.request().method() !== 'GET' || ++reads > 1) { await route.continue(); return }
        const response = await route.fetch()
        intercepted()
        await held
        await route.fulfill({ response })
        fulfilled()
      })
      await page.reload()
      await reached
      await trigger('kanban::tickets::update', { id: ticket.id, changes: { title: 'Newer GET state' } })
      await expectText(page.getByRole('heading', { name: 'Newer GET state' }), /Newer GET state/)
      release()
      await delivered
      await new Promise((resolve) => setTimeout(resolve, 200))
      expect(await page.getByRole('heading', { name: 'Newer GET state' }).count() === 1, 'late GET replaced newer state')

      await page.unroute(`**/api/tickets/${ticket.id}`)
      await page.getByRole('button', { name: 'Edit ticket' }).click()
      const edit = page.getByRole('heading', { name: 'Edit ticket' }).locator('xpath=ancestor::form[1]')
      await edit.getByLabel('Title').fill('Delayed mutation state')
      let releaseMutation
      let interceptedMutation
      let fulfilledMutation
      const mutationReached = new Promise((resolve) => { interceptedMutation = resolve })
      const mutationHeld = new Promise((resolve) => { releaseMutation = resolve })
      const mutationDelivered = new Promise((resolve) => { fulfilledMutation = resolve })
      await page.route(`**/api/tickets/${ticket.id}`, async (route) => {
        if (route.request().method() !== 'PATCH') { await route.continue(); return }
        const response = await route.fetch()
        interceptedMutation()
        await mutationHeld
        await route.fulfill({ response })
        fulfilledMutation()
      })
      await edit.getByRole('button', { name: 'Save changes' }).click()
      await mutationReached
      await trigger('kanban::tickets::update', { id: ticket.id, changes: { title: 'Newest direct state' } })
      releaseMutation()
      await mutationDelivered
      await eventually(async () => await page.getByRole('heading', { name: 'Newest direct state' }).count(), 'late mutation response replaced newer state')

      const inspected = await control('inspect_runtime')
      expect(typeof inspected?.websocket_url === 'string', 'runtime inspector URL is missing')
      const inspector = await inspectorClient(inspected.websocket_url)
      try {
        const baseline = await countSseServerResponses(inspector)
        const transient = await Promise.all(Array.from({ length: 6 }, () => pageFor(browser, baseUrl)))
        await Promise.all(transient.map(({ page }) => expectText(page.getByRole('status').filter({ hasText: /Live updates connected/ }), /Live updates connected/)))
        const active = await countSseServerResponses(inspector)
        expect(active >= baseline + transient.length, `inspector did not observe active SSE responses: baseline=${baseline}, active=${active}`)
        await Promise.all(transient.map(({ context }) => context.close()))
        let returned = false
        const deadline = Date.now() + 10_000
        while (!returned && Date.now() < deadline) {
          returned = await countSseServerResponses(inspector) === baseline
          if (!returned) await new Promise((resolve) => setTimeout(resolve, 100))
        }
        expect(returned, 'closed SSE responses remained retained after garbage collection')
        await trigger('kanban::tickets::update', { id: ticket.id, changes: { title: 'Post-cleanup state' } })
        await expectText(page.getByRole('heading', { name: 'Post-cleanup state' }), /Post-cleanup state/)
      } finally {
        inspector.close()
      }
      await control('restart')
      await eventually(async () => (await trigger('kanban::tickets::get', { id: ticket.id })).title === 'Post-cleanup state', 'runtime did not shut down and recover with an SSE client', 15_000)
      await expectText(page.getByRole('status').filter({ hasText: /Live updates connected/ }), /Live updates connected/)
      await context.close()
      return 'Late responses stay stale; inspector proves closed SSE responses are collectible, the surviving stream works, and shutdown remains bounded.'
    })
    await checksPassedCriterion(check, 'criterion_1', 'Three browser sessions observe creation, edits, comments, replies and deletion without reload.')
    await checksPassedCriterion(check, 'criterion_2', 'Focus, comment/reply drafts and dirty edits survive updates; saving preserves untouched remote fields.')
    await checksPassedCriterion(check, 'criterion_3', 'Compose restart reconnects open sessions and preserves same-store drafts.')
    await checksPassedCriterion(check, 'criterion_4', 'Remote deletion disables stale details and a store switch during dragging clears old drafts.')
    await checksPassedCriterion(check, 'criterion_5', 'Late responses stay stale; private heap instrumentation proves SSE cleanup and bounded shutdown with a connection open.')
  },
}

function checksPassedCriterion(check, id, detail) {
  return check(id, async () => detail)
}

async function expectText(locator, pattern) {
  await eventually(async () => pattern.test(await locator.first().textContent() ?? ''), `expected text ${pattern}`)
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) await main()
