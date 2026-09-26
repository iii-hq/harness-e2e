// Deterministic browser coverage for stacks: list them, copy one, edit its
// YAML (a warning shown, a parse error refused), save and delete it; then the
// provider credentials below them: import, set, add and delete one, by name
// only. The
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
const calls = { create: [], update: [], remove: [], credentials: [] }

// The worker's credentials: names only ever leave it.
const known = {
  ANTHROPIC_API_KEY: ['anthropic'],
  DEEPSEEK_API_KEY: ['deepseek'],
  OPENAI_API_KEY: ['openai'],
  ZAI_API_KEY: ['zai'],
}
const stored = new Map()
const fromFile = new Set(['ZAI_API_KEY'])
const credentials = () => ({
  credentials: [...new Set([...Object.keys(known), ...stored.keys()])]
    .sort()
    .map((name) => {
      const source = stored.has(name)
        ? 'console'
        : fromFile.has(name)
          ? 'provider_env_file'
          : undefined
      return {
        name,
        set: source !== undefined,
        ...(source ? { source } : {}),
        providers: known[name] ?? [],
      }
    }),
})

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
  if (id === 'credentials-list') return credentials()
  if (id === 'credential-set') {
    calls.credentials.push(['set', request])
    stored.set(request.name, request.value)
    return credentials()
  }
  if (id === 'credential-delete') {
    calls.credentials.push(['delete', request])
    stored.delete(request.name)
    return credentials()
  }
  if (id === 'credentials-import') {
    calls.credentials.push(['import', request])
    stored.set('DEEPSEEK_API_KEY', 'sk-imported-4d2e')
    return {
      found: ['DEEPSEEK_API_KEY'],
      not_found: ['ANTHROPIC_API_KEY', 'OPENAI_API_KEY', 'ZAI_API_KEY'],
      ...credentials(),
    }
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

  // Provider credentials, below the stacks: each by name, set or not.
  const section = page.locator('[data-credentials]')
  const status = (name) =>
    section.locator(`[data-credential="${name}"] td[data-label="Status"]`)
  await status('OPENAI_API_KEY').getByText('not set', { exact: true }).waitFor()
  await status('ZAI_API_KEY')
    .getByText('set by the worker’s provider_env_file', { exact: true })
    .waitFor()
  assert.equal(
    await section
      .locator('[data-credential="ZAI_API_KEY"]')
      .getByRole('button', { name: 'Delete ZAI_API_KEY' })
      .count(),
    0,
  )

  // Imported from the worker's environment: it says what it found.
  await section
    .getByRole('button', { name: 'import from this machine', exact: true })
    .click()
  await section
    .getByText(
      "Set from the worker's environment: DEEPSEEK_API_KEY. Not found there: ANTHROPIC_API_KEY, OPENAI_API_KEY, ZAI_API_KEY.",
      { exact: true },
    )
    .waitFor()
  await status('DEEPSEEK_API_KEY')
    .getByText('set here', { exact: true })
    .waitFor()

  // Set one: the value goes once, masked, and is never shown.
  await section
    .getByRole('button', { name: 'Set OPENAI_API_KEY', exact: true })
    .click()
  const set = page.getByRole('dialog', { name: 'Set credential' })
  assert.equal(
    await set.locator('#credential-name').inputValue(),
    'OPENAI_API_KEY',
  )
  assert.equal(await set.locator('#credential-name').isEditable(), false)
  const value = set.locator('#credential-value')
  assert.equal(await value.getAttribute('type'), 'password')
  await value.fill('sk-typed-7c1b')
  await set
    .getByRole('button', { name: 'save credential', exact: true })
    .click()
  await set.waitFor({ state: 'hidden' })
  await status('OPENAI_API_KEY')
    .getByText('set here', { exact: true })
    .waitFor()
  assert.deepEqual(calls.credentials.at(-1), [
    'set',
    { name: 'OPENAI_API_KEY', value: 'sk-typed-7c1b' },
  ])

  // Add one by name: a name that is not an environment variable is refused
  // before anything is sent.
  await section
    .getByRole('button', { name: 'add credential', exact: true })
    .click()
  const add = page.getByRole('dialog', { name: 'Add a credential' })
  await add.locator('#credential-name').fill('my-token')
  await add.locator('#credential-value').fill('gw-secret-5a9f')
  await add
    .getByText('Capital letters, digits and _, starting with a letter.', {
      exact: true,
    })
    .waitFor()
  assert.equal(
    await add
      .getByRole('button', { name: 'save credential', exact: true })
      .isDisabled(),
    true,
  )
  await add.locator('#credential-name').fill('MY_GATEWAY_TOKEN')
  await add
    .getByRole('button', { name: 'save credential', exact: true })
    .click()
  await add.waitFor({ state: 'hidden' })
  await status('MY_GATEWAY_TOKEN')
    .getByText('set here', { exact: true })
    .waitFor()

  // Deleted after a confirmation.
  await section
    .getByRole('button', { name: 'Delete MY_GATEWAY_TOKEN', exact: true })
    .click()
  await page
    .getByRole('dialog', { name: 'Delete MY_GATEWAY_TOKEN?' })
    .getByRole('button', { name: 'delete credential', exact: true })
    .click()
  await section
    .locator('[data-credential="MY_GATEWAY_TOKEN"]')
    .waitFor({ state: 'detached' })
  assert.deepEqual(calls.credentials.at(-1), [
    'delete',
    { name: 'MY_GATEWAY_TOKEN' },
  ])
  const visible = await page.locator('body').innerText()
  for (const secret of ['sk-imported-4d2e', 'sk-typed-7c1b', 'gw-secret-5a9f'])
    assert.equal(visible.includes(secret), false, `${secret} is shown`)

  // Narrow: the stacks and credentials tables fit.
  await page.setViewportSize({ width: 390, height: 844 })
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
    true,
  )
  assert.deepEqual(errors, [])
  console.log(
    'Stacks browser flow passed: repository stacks listed and viewed read-only, copy one with its YAML as written, a path worker saved with a warning next to the editor and on the stack, YAML that does not parse refused next to the editor, edit again, delete; provider credentials listed by name, imported from the worker, set masked, added by a valid name only, deleted, no value shown; narrow viewport.',
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
