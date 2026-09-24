// Deterministic browser coverage for stacks: list them, copy one, edit its
// YAML (a warning shown, a parse error refused), save and delete it. The
// repository's stacks are the files of stacks/; the runner's answers (what a
// stack declares, its warnings and the parse error) are stood in for here and
// covered by the Rust tests.
import assert from 'node:assert/strict'
import { readdirSync, readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright'
import { createConsoleTestHost } from './console-test-host.mjs'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..')

/** What the runner lists for a stack's YAML: its containers and warnings. */
function view(id, label, source, yaml) {
  const containers = [
    ...yaml.matchAll(
      /^ {2}([\w-]+):\n {4}worker: .*\n(?: {4}version: (\S+))?/gm,
    ),
  ].map(([, name, version]) => ({
    name,
    version: version ?? null,
    commit: null,
  }))
  const warnings = [
    ...yaml.matchAll(/^ {2}([\w-]+):\n {4}worker: (path:\/\/\S+)/gm),
  ].map(
    ([, name, worker]) =>
      `${name} runs ${worker}, a path on this machine; the stack runs it only here.`,
  )
  return {
    id,
    label,
    source,
    yaml,
    iii: /^iii: (\S+)$/m.exec(yaml)?.[1] ?? null,
    template: /^template: (\S+)$/m.exec(yaml)?.[1] ?? null,
    containers,
    warnings,
    updated_at: source === 'local' ? '2026-09-24T09:00:00Z' : null,
  }
}

const repository = readdirSync(path.join(root, 'stacks'))
  .filter((file) => file.endsWith('.yaml'))
  .sort()
  .map((file) => {
    const id = file.replace(/\.yaml$/, '')
    return view(
      id,
      id,
      'repository',
      readFileSync(path.join(root, 'stacks', file), 'utf8'),
    )
  })
const local = []
const calls = { create: [], update: [], remove: [] }

const trigger = (name, request = {}) => {
  const id = name.replace('e2e::dashboard::', '')
  if (id === 'stacks-list') return { stacks: [...repository, ...local] }
  if (id === 'stack-create') {
    calls.create.push(request)
    const from = [...repository, ...local].find(
      (stack) => stack.id === request.from,
    )
    const stack = view(
      `stack-${local.length + 1}`,
      request.label || `${from.label} copy`,
      'local',
      from.yaml,
    )
    local.push(stack)
    return stack
  }
  if (id === 'stack-update') {
    calls.update.push(request)
    if (request.yaml?.startsWith('containers: ['))
      throw new Error(
        'The stack is not YAML: did not find expected node content at line 2 column 1, while parsing a flow node',
      )
    const index = local.findIndex((stack) => stack.id === request.stack_id)
    local[index] = view(
      request.stack_id,
      request.label ?? local[index].label,
      'local',
      request.yaml ?? local[index].yaml,
    )
    return local[index]
  }
  if (id === 'stack-delete') {
    calls.remove.push(request)
    local.splice(
      local.findIndex((stack) => stack.id === request.stack_id),
      1,
    )
    return {}
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

  // The Stacks tab lists the repository's stacks, read-only.
  await page.goto(`${server.url}#/ext/harness-e2e/executions`)
  await page.getByRole('link', { name: 'Stacks', exact: true }).first().click()
  await page.locator('[data-stacks]').waitFor()
  assert.deepEqual(
    repository.map((stack) => stack.id),
    ['default', 'harness-template'],
  )
  for (const stack of repository) {
    const row = page.locator(`[data-stack="${stack.id}"]`)
    await row.getByText(stack.label, { exact: true }).first().waitFor()
    await row.getByText('repository', { exact: true }).waitFor()
    await row
      .getByText(
        `iii latest${stack.template ? ` · template ${stack.template}` : ''} · 5 containers`,
        { exact: true },
      )
      .waitFor()
    assert.equal(await row.getByRole('button', { name: /^Edit / }).count(), 0)
    assert.equal(await row.getByRole('button', { name: /^Delete / }).count(), 0)
  }
  const fallback = repository[0]

  // A repository stack opens read-only, as stacks/ writes it.
  await page.getByRole('button', { name: 'View default', exact: true }).click()
  const view = page.getByRole('dialog', { name: 'View default' })
  const shown = view.locator('#stack-editor-yaml')
  assert.equal(await shown.inputValue(), fallback.yaml)
  assert.equal(await shown.isEditable(), false)
  assert.equal(await view.locator('#stack-editor-label').count(), 0)
  assert.equal(
    await view.getByRole('button', { name: 'save stack' }).count(),
    0,
  )
  await view.getByRole('button', { name: 'close', exact: true }).click()
  await view.waitFor({ state: 'hidden' })

  // A copy of a repository stack is a stack of this Console, opened to edit
  // with its YAML exactly as the repository writes it.
  await page.getByRole('button', { name: 'Copy default', exact: true }).click()
  const editor = page.getByRole('dialog', { name: 'Edit default copy' })
  await editor.waitFor()
  assert.deepEqual(calls.create, [{ from: 'default', label: '' }])
  const text = editor.locator('#stack-editor-yaml')
  assert.equal(await text.inputValue(), fallback.yaml)
  assert.equal(await editor.locator('[data-stack-warnings]').count(), 0)

  // A path worker is a warning: saved, and shown next to the editor.
  const edited = fallback.yaml.replace(
    '    worker: package://harness\n    version: latest\n',
    '    worker: path://../harness # a local build\n',
  )
  assert.notEqual(edited, fallback.yaml)
  await editor.locator('#stack-editor-label').fill('Local harness')
  await text.fill(edited)
  await editor.getByText('unsaved changes', { exact: true }).waitFor()
  await editor.getByRole('button', { name: 'save stack', exact: true }).click()
  await editor.getByText('saved with 1 warning', { exact: true }).waitFor()
  const warning =
    'harness runs path://../harness, a path on this machine; the stack runs it only here.'
  await editor
    .locator('[data-stack-warnings]')
    .getByText(warning, { exact: true })
    .waitFor()
  assert.deepEqual(calls.update, [
    { stack_id: 'stack-1', label: 'Local harness', yaml: edited },
  ])

  // YAML that does not parse is refused next to the editor; nothing changes.
  await text.fill('containers: [\n')
  await editor.getByRole('button', { name: 'save stack', exact: true }).click()
  await editor
    .getByRole('alert')
    .getByText(/^The stack is not YAML: /)
    .waitFor()
  await editor.getByText('unsaved changes', { exact: true }).waitFor()
  assert.equal(local[0].yaml, edited)

  // Written back, it saves again; the stack lists what it declares and warns.
  await text.fill(edited)
  await editor.getByRole('button', { name: 'save stack', exact: true }).click()
  await editor.getByRole('alert').waitFor({ state: 'detached' })
  await editor.getByText('saved with 1 warning', { exact: true }).waitFor()
  await editor.getByRole('button', { name: 'close', exact: true }).click()
  await editor.waitFor({ state: 'hidden' })
  const copy = page.locator('[data-stack="stack-1"]')
  await copy.getByText('Local harness', { exact: true }).waitFor()
  await copy.getByText('this Console', { exact: true }).waitFor()
  assert.equal(calls.update.length, 3)
  await copy.getByText('iii latest · 5 containers', { exact: true }).waitFor()
  await copy.getByText(/^harness, harness-e2e latest, /).waitFor()
  await copy.getByText(warning, { exact: true }).waitFor()

  // Edit opens it again, as saved.
  await page
    .getByRole('button', { name: 'Edit Local harness', exact: true })
    .click()
  const again = page.getByRole('dialog', { name: 'Edit Local harness' })
  assert.equal(await again.locator('#stack-editor-yaml').inputValue(), edited)
  await again.getByRole('button', { name: 'close', exact: true }).click()

  // A stack of this Console is deleted after a confirmation.
  await page
    .getByRole('button', { name: 'Delete Local harness', exact: true })
    .click()
  const confirm = page.getByRole('dialog', { name: 'Delete Local harness?' })
  await confirm.getByRole('button', { name: 'cancel', exact: true }).click()
  assert.deepEqual(calls.remove, [])
  await page
    .getByRole('button', { name: 'Delete Local harness', exact: true })
    .click()
  await confirm
    .getByRole('button', { name: 'delete stack', exact: true })
    .click()
  await copy.waitFor({ state: 'detached' })
  assert.deepEqual(calls.remove, [{ stack_id: 'stack-1' }])

  // Narrow: the stacks table fits.
  await page.setViewportSize({ width: 390, height: 844 })
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
    true,
  )
  assert.deepEqual(errors, [])
  console.log(
    'Stacks browser flow passed: repository stacks listed and viewed read-only, copy one with its YAML as written, a path worker saved with a warning next to the editor and on the stack, YAML that does not parse refused next to the editor, edit again, delete, narrow viewport.',
  )
} catch (error) {
  console.error(
    await browser.contexts()[0]?.pages()[0]?.locator('body').innerText(),
  )
  throw error
} finally {
  await browser.close()
  await server.close()
}
