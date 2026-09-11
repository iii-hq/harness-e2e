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

const CRITERION_DEPENDENCIES = {
  kanban_c1_foundation: {
    criterion_1: ['foundation_assets_and_safe_routes'],
    criterion_2: ['foundation_configuration_contract'],
    criterion_3: ['foundation_configuration_contract', 'foundation_accessible_settings'],
    criterion_4: ['foundation_assets_and_safe_routes', 'foundation_configuration_contract'],
    criterion_5: ['foundation_development_hot_reload', 'foundation_accessible_settings'],
  },
  kanban_c2_persistence: {
    criterion_1: ['persistence_iii_crud'],
    criterion_2: ['persistence_iii_crud'],
    criterion_3: ['persistence_directory_isolation_and_restart'],
    criterion_4: ['persistence_invalid_input_no_write', 'persistence_corrupt_and_duplicate_store_fail_closed'],
  },
  kanban_c3_board: {
    criterion_1: ['board_lanes_counts_and_metadata'],
    criterion_2: ['board_loading_empty_error_and_retry'],
    criterion_3: ['board_refresh_and_settings_reload'],
    criterion_4: ['board_safe_text_and_responsive_layout'],
  },
  kanban_c4_ticket_flow: {
    criterion_1: ['ticket_flow_create_detail_and_delete'],
    criterion_2: ['ticket_flow_create_detail_and_delete'],
    criterion_3: ['ticket_flow_delete_failure_navigation_and_restart'],
    criterion_4: ['ticket_flow_create_detail_and_delete', 'ticket_flow_delete_failure_navigation_and_restart'],
    criterion_5: ['ticket_flow_create_detail_and_delete', 'ticket_flow_utf8_chunks_and_json_boundary'],
  },
  kanban_c5_edit_move: {
    criterion_1: ['edit_save_cancel_and_partial_update'],
    criterion_2: ['edit_rejects_invalid_immutable_and_deleted_updates'],
    criterion_3: ['drag_persists_status_only'],
    criterion_4: ['edit_save_cancel_and_partial_update', 'drag_persists_status_only'],
    criterion_5: ['edit_save_cancel_and_partial_update', 'edit_move_survives_restart'],
  },
  kanban_c6_discussion: {
    criterion_1: ['discussion_comments_replies_timeline'],
    criterion_2: ['discussion_comments_replies_timeline'],
    criterion_3: ['discussion_invalid_input_and_corrupt_store_fail_closed'],
    criterion_4: ['discussion_failure_and_overlap_preserve_drafts'],
    criterion_5: ['discussion_survives_edit_delete_and_restart'],
  },
  kanban_c7_live: {
    criterion_1: ['live_sse_protocol_contract', 'live_three_sessions_and_direct_iii'],
    criterion_2: ['live_three_sessions_and_direct_iii'],
    criterion_3: ['live_three_sessions_and_direct_iii'],
    criterion_4: ['live_three_sessions_and_direct_iii'],
    criterion_5: ['live_sse_protocol_contract', 'live_late_responses_disconnect_and_shutdown'],
  },
}

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

export async function inspectorClient(websocketUrl) {
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

export async function countSseServerResponses(inspector) {
  const group = `kanban-sse-${randomUUID()}`
  try {
    await inspector.command('HeapProfiler.collectGarbage')
    await inspector.command('HeapProfiler.collectGarbage')
    const evaluated = await inspector.command('Runtime.evaluate', {
      expression: "process.getBuiltinModule('node:http').ServerResponse.prototype",
      objectGroup: group,
    })
    const prototypeObjectId = evaluated.result?.objectId
    if (!prototypeObjectId) throw new Error('ServerResponse prototype has no inspector object id')
    const queried = await inspector.command('Runtime.queryObjects', { prototypeObjectId, objectGroup: group })
    const count = await inspector.command('Runtime.callFunctionOn', {
      objectId: queried.objects.objectId,
      functionDeclaration: "function () { return this.filter((response) => { try { return String(response.getHeader?.('content-type') ?? response._header).includes('text/event-stream') } catch { return false } }).length }",
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
  const criterion = async (id, detail) => {
    const dependencies = CRITERION_DEPENDENCIES[caseId]?.[id]
    if (!dependencies?.length) {
      unverified(id, `Missing dependency mapping for ${caseId}/${id}`)
      return false
    }
    const status = criterionDependencyStatus(checks, dependencies)
    if (status === 'unverified') {
      unverified(id, `Required functional evidence is unavailable: ${dependencies.join(', ')}`)
      return false
    }
    return check(id, async () => {
      expect(status === 'passed', `Required functional evidence failed: ${dependencies.join(', ')}`)
      return detail
    })
  }

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
      criterion,
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

export function boardLane(page, label) {
  return page.getByRole('heading', { name: new RegExp(`^${label}(?:\\s*\\d+)?$`, 'i') }).locator('xpath=ancestor::section[1]')
}

export async function boardLaneWithCount(page, label, count) {
  const lane = boardLane(page, label)
  await eventually(async () => await lane.count() === 1
    && await lane.getByText(new RegExp(`^${count}$`)).filter({ visible: true }).count() === 1,
  `${label} lane count did not become ${count}`)
  return lane
}

export function boardTicketTotal(page, count) {
  return page.getByText(new RegExp(`^(?:${count}\\s+tickets?|total\\s+tickets?\\s*:?\\s*${count})$`, 'i')).filter({ visible: true })
}

export async function ticketEditor(page, expectedTitle) {
  const forms = page.locator('form')
    .filter({ has: page.getByRole('textbox', { name: 'Title', exact: true }) })
    .filter({ has: page.getByRole('button', { name: 'Save changes', exact: true }) })
    .filter({ visible: true })
  const open = page.getByRole('button', { name: /^(?:edit|edit ticket)$/i }).filter({ visible: true })
  await eventually(async () => await forms.count() === 1 || await open.count() === 1,
    'ticket edit form or edit action is unavailable')
  if (await forms.count() !== 1) await open.click()
  await eventually(async () => await forms.count() === 1, 'ticket edit action did not open one edit form')
  if (expectedTitle !== undefined) {
    expect(await forms.getByLabel('Title', { exact: true }).inputValue() === expectedTitle,
      'Cancel did not restore the persisted title in the edit form')
  }
  return forms
}

export function commentParentAction(entry) {
  const name = /first|parent|reference|in reply|show.*comment|comment.*reply answers/i
  return entry.getByRole('button', { name }).or(entry.getByRole('link', { name })).first()
}

export async function assertAccessibleFormControls(form) {
  const invalid = await form.evaluate(element => [...element.querySelectorAll('label[for]')]
    .filter(label => !label.control || [...document.querySelectorAll('[id]')]
      .filter(target => target.id === label.htmlFor).length !== 1)
    .map(label => label.htmlFor))
  expect(invalid.length === 0, `Form controls have duplicate or unresolved label targets: ${invalid.join(', ')}`)
}

export async function rejectCommentWithoutWrite(api, control, path, comment, label) {
  const before = await control('read_store')
  const response = await api(path, {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(comment),
  })
  expect([400, 404].includes(response.status), `${label}: expected rejection (HTTP 400 or 404), received HTTP ${response.status}`)
  expect(await control('read_store') === before, `${label}: rejected comment changed the store`)
}

export function boardRefreshButton(page) {
  return page.getByRole('button', { name: /refresh/i })
}

export function duplicateStoredTicket(value, id) {
  if (Array.isArray(value)) {
    const ticket = value.find((item) => item?.id === id)
    if (ticket) { value.push(ticket); return true }
  }
  return value !== null && typeof value === 'object'
    && Object.values(value).some((item) => duplicateStoredTicket(item, id))
}

export function standaloneCompose(status) {
  if (!Array.isArray(status?.containers)) throw new ControlError('Compose status omitted declared containers')
  return status.containers.length > 0 && status.containers.every(({ container }) => !/console/i.test(container))
}

export function reachedCommentParent({ parent, link }) {
  const insideParent = (element) => element !== link && (element === parent
    || element?.closest('li, article, [role="listitem"]') === parent)
  return insideParent(document.activeElement)
    || insideParent(document.getElementById(decodeURIComponent(location.hash.slice(1))))
}

async function create(trigger, fields = {}) {
  return trigger('kanban::tickets::create', { title: `Probe ${Date.now()}-${Math.random()}`, ...fields })
}

const PROBES = {
  async kanban_c1_foundation({ api, trigger, control, browser, baseUrl, output, check, criterion, unverified }) {
    await check('foundation_assets_and_safe_routes', async () => {
      expect(standaloneCompose(await trigger('compose::status', { file: '/workspace/worker-compose.yaml' })), 'Compose declares Console or no application')
      const home = await api('/')
      expect(home.ok && home.headers.get('content-type')?.startsWith('text/html'), 'standalone HTML did not load')
      for (const path of ['/constructor', '/toString', '/__proto__']) expect((await api(path)).status === 404, `${path} was not 404`)
      return 'Standalone HTML loads and prototype-like asset paths return 404.'
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
        return value.data_dir === './direct-probe-data'
          && value.resolved_data_dir.endsWith('/direct-probe-data') ? value : false
      }, 'direct configuration update did not select the effective directory')
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
      const input = page.getByRole('textbox')
      await eventually(async () => await input.inputValue() === (await json(await api('/api/config'))).data_dir, 'settings form did not load the current directory')
      await input.fill('./probe-ui-data')
      await page.getByRole('button', { name: /save|apply|update/i }).click()
      await eventually(async () => (await json(await api('/api/config'))).data_dir === './probe-ui-data', 'settings form did not persist the directory')
      expect(await input.inputValue() === './probe-ui-data', 'saving replaced the selected directory')
      expect(!await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth), 'mobile settings have document-wide overflow')
      await screenshot(page, output, 'settings-mobile')
      await page.setViewportSize({ width: 1280, height: 900 })
      expect(await input.isVisible() && !await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth), 'desktop settings are not usable')
      await screenshot(page, output, 'settings-desktop')
      await context.close()
      return 'The settings form loads and saves at a mobile viewport using accessible controls.'
    })
    await check('foundation_development_hot_reload', async () => {
      const observed = await control('hot_reload')
      expect(observed?.observed !== false, 'source marker was not observed by the running development process')
      expect((await api('/')).ok, 'application stopped serving after the source was restored')
      return 'A trusted temporary TypeScript source marker is observed without a manual build and the original bytes are restored.'
    })
    await criterion('criterion_1', 'Compose starts against an isolated engine with no Console service and standalone browser assets load.')
    await criterion('criterion_2', 'Missing configuration initializes and existing unrelated data survives saving and restart.')
    await criterion('criterion_3', 'Browser and direct iii changes select the effective directory and relative paths resolve from the project root.')
    await criterion('criterion_4', 'Invalid config and prototype-like asset paths are rejected by the live server.')
    await criterion('criterion_5', 'Trusted source mutation proves development hot reload and the mobile settings form remains usable.')
  },

  async kanban_c2_persistence({ api, trigger, control, check, criterion }) {
    let first
    const crudAvailable = await check('persistence_iii_crud', async () => {
      first = await trigger('kanban::tickets::create', { title: '  Defaults ticket  ' })
      expect(first.title === 'Defaults ticket' && first.status === 'backlog' && first.priority === 'medium', 'defaults or title normalization are wrong')
      expect(first.description === '' && first.assignee === null, 'nullable defaults are wrong')
      expect(/^KAN-[1-9]\d*$/.test(first.key) && !Number.isNaN(Date.parse(first.created_at)), 'identity or timestamps are invalid')
      expect(/^[\da-f]{8}(?:-[\da-f]{4}){3}-[\da-f]{12}$/i.test(first.id)
        && Date.parse(first.updated_at) >= Date.parse(first.created_at), 'UUID or updated timestamp is invalid')
      const second = await create(trigger, { status: 'done', priority: 'urgent', assignee: 'Ada' })
      expect(second.id !== first.id && /^KAN-[1-9]\d*$/.test(second.key)
        && Number(second.key.slice(4)) > Number(first.key.slice(4)), 'ticket identifiers are not unique and increasing')
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
      expect(crudAvailable, 'Ticket persistence is unavailable; corrupt-store checks require working create/list/get functions.')
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
        if (!duplicateStoredTicket(records, first.id)) throw new ControlError('Cannot locate the ticket collection for a duplicate-store probe')
        const duplicate = `${JSON.stringify(records, null, 2)}\n`
        await rejectsWithoutRewrite(duplicate, 'duplicate')
      } finally {
        await control('write_store', { value: original })
      }
      expect((await trigger('kanban::tickets::get', { id: first.id })).id === first.id, 'restored valid store is unusable')
      return 'Corrupt JSON and duplicate records fail closed without byte changes; the original store is restored afterward.'
    })
    await criterion('criterion_1', 'Both identifiers and real SDK caller metadata work through iii.')
    await criterion('criterion_2', 'Defaults, normalization, timestamps and increasing readable keys are observed.')
    await criterion('criterion_3', 'Configured directories remain isolated and tickets survive runtime restart.')
    await criterion('criterion_4', 'Invalid inputs and corrupt or duplicate stores fail without replacing existing bytes.')
  },

  async kanban_c3_board({ api, trigger, browser, baseUrl, output, check, criterion }) {
    const statuses = [['backlog', 'Backlog'], ['todo', 'To do'], ['in_progress', 'In progress'], ['in_review', 'In review'], ['done', 'Done']]

    await check('board_loading_empty_error_and_retry', async () => {
      const context = await browser.newContext({ viewport: { width: 1280, height: 900 } })
      const page = await context.newPage()
      let release
      const held = new Promise((resolve) => { release = resolve })
      let mode = 'hold'
      try {
        await context.route('**/api/tickets', async (route) => {
          if (mode === 'hold') { await held; await route.continue(); return }
          if (mode === 'fail') { await route.fulfill({ status: 500, contentType: 'application/json', body: '{"error":"probe failure"}' }); return }
          await route.continue()
        })
        await page.goto(baseUrl)
        await expectText(page.getByRole('status').filter({ hasText: /load/i }), /load/i)
        expect(await boardTicketTotal(page, 0).count() === 0, 'loading fabricated a zero total')
        expect(await page.getByText(/^0$/).filter({ visible: true }).count() === 0, 'loading fabricated zero lane counts')
        release()
        await expectText(page.getByRole('status').filter({ hasText: /no tickets|empty/i }), /no tickets|empty/i)
        expect(await boardTicketTotal(page, 0).count() === 1, 'empty store total is wrong')
        for (const [, label] of statuses) await boardLaneWithCount(page, label, 0)
        mode = 'fail'
        const failurePage = await context.newPage()
        await failurePage.goto(baseUrl)
        await expectText(failurePage.locator('[role="alert"], [role="status"]').filter({ hasText: /unable|error|failed/i }), /unable|error|failed/i)
        expect(await boardTicketTotal(failurePage, 0).count() === 0, 'failed first read fabricated a zero total')
        expect(await failurePage.getByText(/^0$/).filter({ visible: true }).count() === 0, 'failed first read fabricated zero lane counts')
        mode = 'pass'
        await failurePage.getByRole('button', { name: /retry|try again/i }).click()
        await expectText(failurePage.getByRole('status').filter({ hasText: /no tickets|empty/i }), /no tickets|empty/i)
        for (const [, label] of statuses) await boardLaneWithCount(failurePage, label, 0)
      } finally {
        mode = 'pass'
        release?.()
        await context.unroute('**/api/tickets').catch(() => {})
        await context.close().catch(() => {})
      }
      return 'Loading, empty and failed reads are distinct; retry recovers and unavailable counts remain unknown.'
    })

    await check('board_lanes_counts_and_metadata', async () => {
      const seeded = []
      for (const [status, label] of statuses) seeded.push(await create(trigger, { title: `${label} probe`, status, priority: 'high', assignee: 'Grace' }))
      const { context, page } = await pageFor(browser, baseUrl)
      await eventually(async () => (await boardTicketTotal(page, seeded.length).count()) === 1, 'board total did not load')
      for (let index = 0; index < statuses.length; index++) {
        const [, label] = statuses[index]
        const lane = await boardLaneWithCount(page, label, 1)
        expect(await lane.getByText(seeded[index].key, { exact: true }).count() === 1, `${label} readable key is missing`)
        expect(await lane.getByRole('heading', { name: seeded[index].title }).count() === 1, `${label} card is missing`)
        expect(await lane.getByText(/^high$/i).count() === 1, `${label} priority is missing`)
        expect(await lane.getByText('Grace', { exact: true }).count() === 1, `${label} assignee is missing`)
      }
      await context.close()
      return 'Five lanes contain the correct cards, counts, priorities and assignees.'
    })

    await check('board_refresh_and_settings_reload', async () => {
      const initialConfig = await json(await api('/api/config'))
      const { context, page } = await pageFor(browser, baseUrl)
      const initialTickets = (await json(await api('/api/tickets'))).tickets
      await eventually(async () => await boardTicketTotal(page, initialTickets.length).count(), 'initial board total did not load')
      let ticketReads = 0
      page.on('request', (request) => { if (new URL(request.url()).pathname === '/api/tickets') ticketReads++ })
      const refreshed = await create(trigger, { title: 'Explicit refresh probe' })
      const readsBeforeRefresh = ticketReads
      await boardRefreshButton(page).click()
      await eventually(async () => ticketReads > readsBeforeRefresh, 'refresh did not request current tickets')
      await eventually(async () => await page.getByRole('heading', { name: refreshed.title }).count(), 'refresh did not load the new ticket')
      try {
        await page.getByRole('link', { name: 'Settings' }).click()
        await page.getByLabel('Data directory').fill('./board-probe-empty')
        await page.getByRole('button', { name: 'Save settings' }).click()
        await expectText(page.getByRole('status').filter({ hasText: /saved/i }), /saved/i)
        await page.getByRole('link', { name: /Board/ }).click()
        await expectText(page.getByRole('status').filter({ hasText: /no tickets|empty/i }), /no tickets|empty/i)
        await eventually(async () => await boardTicketTotal(page, 0).count() === 1, 'selected empty store total is wrong')
        await page.getByRole('link', { name: 'Settings' }).click()
        await page.getByLabel('Data directory').fill(initialConfig.data_dir)
        await page.getByRole('button', { name: 'Save settings' }).click()
        await expectText(page.getByRole('status').filter({ hasText: /saved/i }), /saved/i)
        await page.getByRole('link', { name: /Board/ }).click()
        await eventually(async () => await page.getByRole('heading', { name: refreshed.title }).count(), 'returning from settings did not reload the restored store')
      } finally {
        await json(await api('/api/config', {
          method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ data_dir: initialConfig.data_dir }),
        }))
      }
      await context.close()
      return 'Refresh requests current data and navigation reloads each selected store.'
    })

    await check('board_safe_text_and_responsive_layout', async () => {
      const markup = '<img src=x onerror=alert(1)> literal title'
      await create(trigger, { title: markup })
      const { context, page } = await pageFor(browser, baseUrl)
      await eventually(async () => await page.getByRole('heading', { name: markup }).count(), 'literal ticket title did not load')
      expect(await page.locator('img[src="x"]').count() === 0, 'ticket markup created an image element')
      expect(!await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth), 'desktop page has document-wide horizontal overflow')
      await screenshot(page, output, 'board-desktop')
      await page.setViewportSize({ width: 390, height: 844 })
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth)
      expect(!overflow, 'mobile page has document-wide horizontal overflow')
      await screenshot(page, output, 'board-mobile')
      await context.close()
      return 'User markup stays literal and desktop and mobile avoid document-wide overflow.'
    })
    await criterion('criterion_1', 'Tickets created through iii appear in all five lanes with counts and metadata.')
    await criterion('criterion_2', 'Loading, empty, failed and recovered states are distinguished without fabricated counts.')
    await criterion('criterion_3', 'Refresh and returning from settings reload the selected store.')
    await criterion('criterion_4', 'Markup remains literal and neither desktop nor mobile has document-wide overflow.')
  },

  async kanban_c4_ticket_flow({ api, trigger, control, browser, baseUrl, output, check, criterion }) {
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
      await assertAccessibleFormControls(dialog)
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
      expect(await page.getByRole('heading', { name: 'Browser-created ticket' }).locator('xpath=ancestor::*[self::dialog or @role="dialog" or @aria-modal="true"]').count() === 0, 'ticket details opened in a modal')
      expect(context.pages().length === 1, 'ticket creation opened another tab')
      created = (await json(await api('/api/tickets'))).tickets.find(({ title }) => title === 'Browser-created ticket')
      expect(created, 'created ticket is absent from API list')
      expect((await json(await api(`/api/tickets/${created.id}`))).ticket.key === created.key, 'HTTP UUID lookup failed')
      expect((await json(await api(`/api/tickets/${created.key}`))).ticket.id === created.id, 'HTTP key lookup failed')
      expect((await trigger('kanban::tickets::get', { id: created.id })).key === created.key, 'iii UUID lookup failed')
      expect((await trigger('kanban::tickets::get', { id: created.key })).id === created.id, 'iii key lookup failed')
      for (const value of ['UTF-8: ação e café', 'In review', 'urgent', 'Lin']) {
        expect(await page.getByText(value, { exact: true }).filter({ visible: true }).count() === 1, `detail omitted persisted value: ${value}`)
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
      await eventually(async () => await page.getByRole('heading', { name: 'Keyboard card probe' }).count(), 'board did not load after navigating during deletion')
      await eventually(async () => await page.getByRole('heading', { name: created.title, exact: true }).count() === 0, 'deleted card remained visible after the late response')
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
      expect([400, 415].includes((await api('/api/tickets', { method: 'POST', body: 'not json' })).status), 'non-JSON creation was not rejected')
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
    await criterion('criterion_1', 'Modal creation, same-tab details, keyboard and pointer cards, history and reload work.')
    await criterion('criterion_2', 'HTTP accepts both identifiers and details reflect persisted values.')
    await criterion('criterion_3', 'Deletion removes the card while preserving disk state across restart and never reuses the key.')
    await criterion('criterion_4', 'Failed creation and deletion preserve recoverable UI; late deletion does not leave a stale card.')
    await criterion('criterion_5', 'Chunked UTF-8, JSON enforcement, focus and mobile layout are directly exercised.')
  },

  async kanban_c5_edit_move({ api, trigger, control, browser, baseUrl, output, check, criterion }) {
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
      let edit = await ticketEditor(page)
      await edit.getByLabel('Title').fill('Discard this')
      await edit.getByRole('button', { name: 'Cancel' }).click()
      await expectText(page.getByRole('heading', { name: ticket.title }), /Editable probe/)
      edit = await ticketEditor(page, ticket.title)
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
      await screenshot(page, output, 'ticket-edited-mobile')

      edit = await ticketEditor(page)
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
      const beforeMove = (await json(await api(`/api/tickets/${ticket.id}`))).ticket
      const { context, page } = await pageFor(browser, baseUrl)
      await eventually(async () => await page.getByRole('heading', { name: 'Late saved probe' }).count(), 'edited card did not appear')
      const card = page.getByRole('heading', { name: 'Late saved probe' }).locator('xpath=ancestor::a[1]')
      const target = await boardLaneWithCount(page, 'Done', 0)
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
      const todo = await boardLaneWithCount(page, 'To do', 1)
      expect(await todo.getByRole('heading', { name: 'Late saved probe' }).count() === 1, 'failed drag moved the visible card')
      await card.dragTo(target)
      await eventually(async () => (await json(await api(`/api/tickets/${ticket.id}`))).ticket.status === 'done', 'drag did not persist done status')
      const moved = (await json(await api(`/api/tickets/${ticket.id}`))).ticket
      expect(Object.entries(beforeMove).every(([field, value]) => ['status', 'updated_at'].includes(field)
        || JSON.stringify(moved[field]) === JSON.stringify(value)), 'drag changed fields besides status')
      expect(Date.parse(moved.updated_at) > Date.parse(beforeMove.updated_at), 'drag did not advance updated_at')

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
      const review = await boardLaneWithCount(page, 'In review', 0)
      await card.dragTo(review)
      await reached
      await page.getByRole('link', { name: 'Settings' }).click()
      release()
      await eventually(async () => (await json(await api(`/api/tickets/${ticket.id}`))).ticket.status === 'in_review', 'late move did not persist after navigation')
      await page.getByRole('link', { name: /Board/ }).click()
      const restoredReview = await boardLaneWithCount(page, 'In review', 1)
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
      const edit = await ticketEditor(page)
      expect(await edit.getByLabel('Status').inputValue() === 'in_review', 'mobile status selector did not reflect the persisted move')
      await context.close()
      return 'All edited fields and the mobile status alternative retain the moved state after runtime restart.'
    })
    await criterion('criterion_1', 'Save, cancel and partial update behavior are exercised.')
    await criterion('criterion_2', 'Invalid, immutable, empty and deleted-ticket updates fail without writes.')
    await criterion('criterion_3', 'Real drag persists only status; injected failure stays put and retry succeeds.')
    await criterion('criterion_4', 'Failed-save drafts survive and pending save/move completion updates the relevant board after navigation.')
    await criterion('criterion_5', 'The mobile status selector and all edited fields retain their state after restart.')
  },

  async kanban_c6_discussion({ api, trigger, control, browser, baseUrl, output, check, criterion }) {
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
      const safeComment = page.getByText('<img src=x onerror=alert(1)> first', { exact: true })
      await expectText(safeComment, /first/)
      expect(await safeComment.isVisible() && await page.locator('img[src="x"]').count() === 0, 'comment body did not render markup as visible literal text')
      const entry = (body) => page.getByText(body, { exact: true }).locator('xpath=ancestor::*[self::li or self::article or @role="listitem"][1]')
      await entry('<img src=x onerror=alert(1)> first').getByRole('button', { name: /^reply/i }).click()
      await page.getByLabel('Your name').fill('Bob')
      await page.getByRole('textbox', { name: 'Comment', exact: true }).fill('second')
      await page.getByRole('button', { name: 'Post comment' }).click()
      await expectText(page.getByText('second', { exact: true }), /second/)
      let stored = (await json(await api(`/api/tickets/${ticket.id}`))).ticket
      expect(stored.comments.length === 2 && stored.comments[1].parent_id === stored.comments[0].id, 'browser reply did not persist its parent')
      stored = await trigger('kanban::tickets::comment', {
        id: ticket.key,
        comment: { author: '  Carol  ', body: '  third  ', parent_id: stored.comments[1].id },
      })
      expect(stored.comments[2].author === 'Carol', 'comment author was not trimmed')
      expect(stored.comments.map(({ body }) => body).join(',') === '<img src=x onerror=alert(1)> first,second,third', 'timeline posting order is wrong')
      expect(stored.comments.every(({ id, created_at }) => /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i.test(id)
        && !Number.isNaN(Date.parse(created_at))), 'comment identity or timestamp is invalid')
      await page.reload()
      await eventually(async () => await page.getByText('third', { exact: true }).count(), 'persisted iii reply was not rendered after reload')
      const entries = []
      for (const { body, author } of stored.comments) {
        const item = entry(body)
        expect(await item.count() === 1, `timeline has no semantic entry for ${body}`)
        const visible = await item.innerText()
        expect(visible.includes(author), `timeline omitted the author for ${body}`)
        entries.push(await item.elementHandle())
        expect((await item.locator('time').filter({ visible: true }).allTextContents()).some((text) => text.trim())
          || /\b(?:\d{1,2}:\d{2}|\d{1,4}[/-]\d{1,2}|jan(?:uary)?|feb(?:ruary)?|mar(?:ch)?|apr(?:il)?|may|jun(?:e)?|jul(?:y)?|aug(?:ust)?|sep(?:tember)?|oct(?:ober)?|nov(?:ember)?|dec(?:ember)?|just now|seconds?|minutes?|hours?|today|yesterday)\b/i.test(visible), `timeline entry has no visible time for ${body}`)
      }
      expect(await page.evaluate((items) => items.slice(1).every((item, index) =>
        items[index].compareDocumentPosition(item) & Node.DOCUMENT_POSITION_FOLLOWING), entries), 'timeline is not in posting order')
      const replyItem = entry('second')
      const parentLink = commentParentAction(replyItem)
      expect(await parentLink.count() === 1, 'reply has no accessible parent navigation action')
      const parent = await entry('<img src=x onerror=alert(1)> first').elementHandle()
      const link = await parentLink.elementHandle()
      await parentLink.focus()
      await parentLink.press('Enter')
      await eventually(() => page.evaluate(reachedCommentParent, { parent, link }), 'keyboard parent navigation did not reach the referenced comment')
      await screenshot(page, output, 'discussion-mobile')
      await context.close()
      return 'Comments and nested replies retain order; safe text, times and keyboard parent navigation work on mobile.'
    })
    await check('discussion_invalid_input_and_corrupt_store_fail_closed', async () => {
      other = await create(trigger, { title: 'Other discussion' })
      const foreign = await trigger('kanban::tickets::comment', { id: other.id, comment: { author: 'X', body: 'foreign root' } })
      for (const comment of [{ author: '', body: 'body' }, { author: 'A', body: '' },
        { author: '   ', body: 'body' }, { author: 'A', body: '  \t ' }]) {
        expect((await api(`/api/tickets/${ticket.id}/comments`, {
          method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(comment),
        })).status === 400, `invalid comment was accepted: ${JSON.stringify(comment)}`)
      }
      await rejectCommentWithoutWrite(api, control, `/api/tickets/${ticket.id}/comments`,
        { author: 'A', body: 'body', parent_id: foreign.comments[0].id }, 'foreign comment parent')
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
      await (await ticketEditor(page)).getByRole('button', { name: 'Cancel' }).click()
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
      await eventually(() => page.getByLabel('Your name').isEnabled(),
        'another ticket comment form stayed disabled while the previous ticket POST was pending')
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
    await criterion('criterion_1', 'Comments, replies and replies-to-replies persist in posting order with valid parents.')
    await criterion('criterion_2', 'Timeline text, authors, times and keyboard parent navigation work on mobile.')
    await criterion('criterion_3', 'Invalid inputs, foreign parents and malformed stored discussions fail closed.')
    await criterion('criterion_4', 'Failures and overlapping responses preserve drafts and newer activity.')
    await criterion('criterion_5', 'Discussions survive edits, soft deletion and runtime restart.')
  },

  async kanban_c7_live({ api, trigger, control, browser, baseUrl, output, check, criterion, unverified }) {
    await check('live_sse_protocol_contract', async () => {
      const configuration = await json(await api('/api/config'))
      let original
      const { context, page } = await pageFor(browser, baseUrl)
      try {
        await page.evaluate(() => {
          globalThis.__kanbanProbeEvents = []
          globalThis.__kanbanProbeSource = new EventSource('/api/events')
          globalThis.__kanbanProbeSource.addEventListener('change', (event) => {
            try { globalThis.__kanbanProbeEvents.push(JSON.parse(event.data)) }
            catch { globalThis.__kanbanProbeEvents.push({ parse_error: true }) }
          })
        })
        const eventAt = (index) => eventually(async () => page.evaluate(
          (position) => globalThis.__kanbanProbeEvents[position] ?? false, index,
        ), `SSE change event ${index + 1} was not received`)
        const exactStore = (event) => event?.store === configuration.resolved_data_dir
        expect(exactStore(await eventAt(0)), 'initial SSE event has the wrong name or store payload')
        await create(trigger, { title: 'SSE protocol probe' })
        expect(exactStore(await eventAt(1)), 'persisted mutation SSE event has the wrong name or store payload')
        await new Promise((resolve) => setTimeout(resolve, 250))
        const count = await page.evaluate(() => globalThis.__kanbanProbeEvents.length)
        original = await control('read_store')
        await control('write_store', { value: '{broken json' })
        let rejected = false
        try { await create(trigger, { title: 'Rejected SSE mutation' }) } catch { rejected = true }
        expect(rejected, 'mutation unexpectedly succeeded against a corrupt store')
        await new Promise((resolve) => setTimeout(resolve, 750))
        expect(await page.evaluate(() => globalThis.__kanbanProbeEvents.length) === count, 'failed persistence emitted a change event')
      } finally {
        try { if (original !== undefined) await control('write_store', { value: original }) } finally {
          await page.evaluate(() => globalThis.__kanbanProbeSource?.close()).catch(() => {})
          await context.close().catch(() => {})
        }
      }
      return 'Named SSE change events carry the exact store, start with an initial event and exclude failed persistence.'
    })
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
      const edit = await ticketEditor(sessions[0].page)
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
      await eventually(async () => (await trigger('kanban::tickets::get', { id: ticket.id })).id === ticket.id, 'iii did not reconnect after restart', 15_000)
      await trigger('kanban::tickets::update', { id: ticket.id, changes: { description: 'Reconnected live description' } })
      await Promise.all(sessions.map(({ page }) => eventually(async () => await page.getByText('Reconnected live description', { exact: true }).count(), 'browser session did not reconnect after restart', 20_000)))
      expect(await preservedDraft.inputValue() === 'Reply draft survives', 'restart erased same-store reply draft')

      await sessions[2].page.evaluate(() => { location.hash = '#board' })
      await eventually(async () => await sessions[2].page.getByRole('heading', { name: 'Dirty local title' }).count(), 'board did not recover after restart')
      await trigger('kanban::tickets::update', { id: ticket.id, changes: { status: 'done' } })
      await Promise.all(sessions.slice(0, 2).map(({ page }) => eventually(async () =>
        await page.getByText('Done', { exact: true }).filter({ visible: true }).count(), 'remote move did not update detail status')))
      await eventually(async () => await boardLane(sessions[2].page, 'Done').getByRole('heading', { name: 'Dirty local title' }).count(), 'remote move did not update the board lane')
      const dragging = sessions[2].page.getByRole('heading', { name: 'Dirty local title' }).locator('xpath=ancestor::a[1]')
      const dragged = await dragging.elementHandle()
      await dragging.evaluate((element) => {
        globalThis.__kanbanDragStarted = false
        globalThis.__kanbanDragEnded = false
        element.addEventListener('dragstart', () => { globalThis.__kanbanDragStarted = true }, { once: true })
        element.addEventListener('dragend', () => { globalThis.__kanbanDragEnded = true }, { once: true })
      })
      const box = await dragging.boundingBox()
      expect(box, 'live drag source is not visible')
      const mouse = sessions[2].page.mouse
      try {
        await mouse.move(box.x + box.width / 2, box.y + box.height / 2)
        await mouse.down()
        await mouse.move(box.x + box.width / 2 - 30, box.y + box.height / 2 + 15, { steps: 5 })
        await eventually(() => sessions[2].page.evaluate(() => globalThis.__kanbanDragStarted), 'real pointer drag did not start')
        await trigger('kanban::tickets::update', { id: ticket.id, changes: { description: 'Updated during drag' } })
        await Promise.all(sessions.slice(0, 2).map(({ page }) => eventually(async () =>
          await page.getByText('Updated during drag', { exact: true }).count(), 'ordinary update did not reach the other sessions during drag')))
        expect(await dragged.evaluate((element) => element.isConnected && !globalThis.__kanbanDragEnded), 'ordinary live update replaced or ended the active drag')
        await trigger('configuration::set', { id: 'kanban', value: { ...configuration.value, data_dir: '/data/live-alternate' } })
        await eventually(async () => await sessions[2].page.getByText('No tickets yet.', { exact: true }).count(), 'store switch during drag did not load the empty store')
      } finally {
        await mouse.up()
      }
      await trigger('configuration::set', { id: 'kanban', value: configuration.value })
      await Promise.all(sessions.slice(0, 2).map(({ page }) => eventually(async () => await page.getByRole('heading', { name: 'Dirty local title' }).count(), 'detail did not recover after restoring the store')))
      expect(await preservedDraft.inputValue() === '', 'store switch retained an old-store draft')

      await trigger('kanban::tickets::delete', { id: ticket.id })
      await Promise.all(sessions.slice(0, 2).map(({ page }) => eventually(async () => {
        const actions = page.getByRole('button', { name: /^(?:Edit ticket|Delete ticket)$/ })
        return await actions.count() === 0 || await actions.evaluateAll(
          (items) => items.every((item) => item.disabled || item.getAttribute('aria-disabled') === 'true'),
        )
      }, 'session retained enabled actions for a remotely deleted ticket')))
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
      const edit = await ticketEditor(page)
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
        const active = await eventually(async () => {
          const count = await countSseServerResponses(inspector)
          return count >= baseline + transient.length ? count : false
        }, 'inspector did not observe all active SSE responses')
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
      await trigger('kanban::tickets::update', { id: ticket.id, changes: { title: 'Restarted stream state' } })
      await expectText(page.getByRole('heading', { name: 'Restarted stream state' }), /Restarted stream state/)
      await context.close()
      return 'Late responses stay stale; inspector proves closed SSE responses are collectible, the surviving stream works, and shutdown remains bounded.'
    })
    await criterion('criterion_1', 'Three browser sessions observe creation, edits, comments, replies and deletion without reload.')
    await criterion('criterion_2', 'Focus, comment/reply drafts and dirty edits survive updates; saving preserves untouched remote fields.')
    await criterion('criterion_3', 'Compose restart reconnects open sessions and preserves same-store drafts.')
    await criterion('criterion_4', 'Remote deletion disables stale details and a store switch during dragging clears old drafts.')
    await criterion('criterion_5', 'Late responses stay stale; private heap instrumentation proves SSE cleanup and bounded shutdown with a connection open.')
  },
}

export function criterionDependencyStatus(checks, dependencies) {
  const statuses = dependencies.map((dependency) => checks.find(({ id }) => id === dependency)?.status)
  if (statuses.some((status) => status === 'failed')) return 'failed'
  return statuses.every((status) => status === 'passed') ? 'passed' : 'unverified'
}

async function expectText(locator, pattern) {
  await eventually(async () => pattern.test(await locator.first().textContent() ?? ''), `expected text ${pattern}`)
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) await main()
