// Deterministic browser coverage for Stacks as the redesign canvas draws it:
// the repository's stacks apart from this Console's (empty, then a copy), a
// repository stack viewed read-only with its YAML as written, Copy to edit,
// an edit whose warnings show beside the editor, a save the runner refuses
// said beside it without clearing what was typed, Discard and the question
// before closing unsaved changes, New stack from a stack of this Console,
// Delete behind the host's confirmation (run once when confirmed twice), a
// sheet kept when its stack is deleted elsewhere; then the provider credentials:
// import, set, add and delete one, by name only. The repository's stacks are
// the files of stacks/; the runner's answers (what a stack declares, its
// warnings and the parse error) are stood in for here and covered by the
// Rust tests.
import assert from 'node:assert/strict'
import { readdirSync, readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright'
import { createConsoleTestHost } from './console-test-host.mjs'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..')

/** What the runner lists for a stack's YAML: its containers and warnings. */
function view(id, label, source, yaml) {
  if (!/^containers:$/m.test(yaml))
    throw new Error('A stack needs a `containers` mapping.')
  const block = yaml.slice(yaml.search(/^containers:$/m)).split(/\n(?=\S)/)[0]
  const containers = [
    ...block.matchAll(/^ {2}([\w-]+):\n((?: {4}.*(?:\n|$))*)/gm),
  ].map(([, name, body]) => {
    const key = (field) =>
      new RegExp(`^ {4}${field}: "?([^\\s"#]+)"?`, 'm').exec(body)?.[1] ?? null
    return {
      name,
      worker: key('worker'),
      version: key('version'),
      commit: key('commit'),
    }
  })
  const warnings = containers.flatMap(({ name, worker, commit }) => [
    ...(worker?.startsWith('path://')
      ? [
          `${name} runs ${worker}, a path on this machine; the stack runs it only here.`,
        ]
      : []),
    ...(commit && !worker?.startsWith('package://')
      ? [
          `${name} pins a commit, but only a package:// worker is built from one; the executor refuses it.`,
        ]
      : []),
  ])
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
// The first read fails: the page says so and tries again.
let failList = true
// Set to make the next copy fail.
let failCreate = false
// A stack deleted elsewhere right after it is saved.
let vanish = null

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
  if (id === 'stacks-list') {
    if (failList) {
      failList = false
      throw new Error('The worker did not answer.')
    }
    return { stacks: [...repository, ...local] }
  }
  if (id === 'stack-create') {
    if (failCreate) {
      failCreate = false
      throw new Error('The runner could not copy it.')
    }
    calls.create.push(request)
    const from = [...repository, ...local].find(
      (stack) => stack.id === request.from,
    )
    const stack = view(
      `stack-${calls.create.length}`,
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
    const saved = view(
      request.stack_id,
      request.label ?? local[index].label,
      'local',
      request.yaml ?? local[index].yaml,
    )
    local[index] = saved
    if (request.stack_id === vanish) local.splice(index, 1)
    return saved
  }
  if (id === 'stack-delete') {
    calls.remove.push(request)
    local.splice(
      local.findIndex((stack) => stack.id === request.stack_id),
      1,
    )
    return {}
  }
  if (id === 'credentials-list') return credentials()
  if (id === 'credential-set') {
    calls.credentials.push(['set', request])
    stored.set(request.name, request.secret)
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
  throw new Error(`Unexpected RPC ${name}`)
}

/** Two clicks in one task, before the page renders again: a confirmation
 *  the host does not close at once. */
const twice = (button) => {
  button.click()
  button.click()
}

const server = await createConsoleTestHost()
const browser = await chromium.launch({ headless: true })
try {
  const context = await browser.newContext({
    viewport: { width: 1280, height: 900 },
  })
  await context.grantPermissions(['clipboard-read', 'clipboard-write'])
  const page = await context.newPage()
  await server.install(page, trigger)
  page.setDefaultTimeout(10_000)
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))

  // A first read that fails says so, and tries again.
  await page.goto(`${server.url}#/ext/harness-e2e/executions`)
  await page.getByRole('link', { name: 'Stacks', exact: true }).first().click()
  await page.getByText('Stacks could not be loaded', { exact: true }).waitFor()
  await page.getByRole('button', { name: 'try again', exact: true }).click()

  // The repository's stacks, read-only, apart from this Console's, empty.
  const repo = page.locator('[data-stack-group="repository"]')
  const mine = page.locator('[data-stack-group="local"]')
  await repo.waitFor()
  await page
    .getByText(/^2 in the repository, read-only · none in this Console\. /)
    .waitFor()
  assert.deepEqual(
    repository.map((stack) => stack.id),
    ['default', 'harness-template'],
  )
  for (const stack of repository) {
    const row = repo.locator(`[data-stack="${stack.id}"]`)
    await row.getByText(`stacks/${stack.id}.yaml`, { exact: true }).waitFor()
    await row.getByRole('listitem', { name: 'iii: latest' }).waitFor()
    await row.getByRole('listitem', { name: 'workers: 5' }).waitFor()
    await row
      .getByRole('listitem', { name: `template: ${stack.template ?? 'none'}` })
      .waitFor()
    await row.getByText('No warnings', { exact: true }).waitFor()
    assert.equal(
      await row.getByRole('button', { name: `View ${stack.id}` }).count(),
      1,
    )
    assert.equal(await row.getByRole('button', { name: /^Edit / }).count(), 0)
    assert.equal(await row.getByRole('button', { name: /^Delete / }).count(), 0)
  }
  await repo
    .locator('[data-stack="harness-template"]')
    .getByText('Against default: adds template harness.', { exact: true })
    .waitFor()
  await mine.getByText(/^None yet\. /).waitFor()
  const fallback = repository[0]

  // A copy that fails says so on the page; opening another stack does not
  // carry it along.
  failCreate = true
  await repo
    .getByRole('button', { name: 'Copy harness-template', exact: true })
    .click()
  await page
    .getByText('The runner could not copy it.', { exact: true })
    .waitFor()

  // A repository stack opens read-only: what it installs, its workers with
  // the worker each runs, and its YAML exactly as stacks/ writes it.
  await page.getByRole('button', { name: 'View default', exact: true }).click()
  const viewer = page.getByRole('dialog', { name: 'default', exact: true })
  await viewer.getByText('Repository · read-only', { exact: true }).waitFor()
  await viewer
    .getByText('Read-only. Copies you make appear under This Console.', {
      exact: true,
    })
    .waitFor()
  assert.equal(await page.getByText('The runner could not copy it.').count(), 0)
  const shownYaml = await viewer
    .locator('.sk-line .sk-text')
    .evaluateAll((lines) => lines.map((line) => line.textContent).join('\n'))
  assert.equal(shownYaml, fallback.yaml.replace(/\n$/, ''))
  await viewer
    .getByText(
      `${fallback.yaml.replace(/\n$/, '').split('\n').length} lines · `,
    )
    .waitFor()
  const harness = viewer.locator('[data-worker="harness"]')
  await harness.getByText('package://harness', { exact: true }).waitFor()
  await harness.getByText('latest', { exact: true }).waitFor()
  assert.equal(await viewer.locator('#sk-yaml').count(), 0)
  assert.equal(
    await viewer.getByRole('button', { name: 'Save stack' }).count(),
    0,
  )
  await viewer.getByRole('button', { name: 'Copy YAML', exact: true }).click()
  await viewer.getByRole('button', { name: 'Copied', exact: true }).waitFor()
  assert.equal(
    await page.evaluate(() => navigator.clipboard.readText()),
    fallback.yaml,
  )

  // Copy to edit makes a stack of this Console and opens it to edit, its
  // YAML exactly as the repository writes it.
  await viewer
    .getByRole('button', { name: 'Copy to edit', exact: true })
    .click()
  await page.getByRole('dialog', { name: 'Edit default copy' }).waitFor()
  // Named by the stack it edits, which a save renames.
  const editor = page.locator('[data-stack-sheet="edit"]')
  assert.deepEqual(calls.create, [{ from: 'default', label: '' }])
  await editor
    .getByText('Created. Change what you need, then save.', { exact: true })
    .waitFor()
  const text = editor.locator('#sk-yaml')
  assert.equal(await text.inputValue(), fallback.yaml)
  assert.equal(await editor.locator('[data-stack-warnings]').count(), 0)

  // A path worker pinning a commit is two warnings: saved, and shown beside
  // the editor as last saved.
  const edited = fallback.yaml.replace(
    '    worker: package://harness\n    version: latest\n',
    '    worker: path://../harness # a local build\n    commit: 8c02f93a1d4e\n',
  )
  assert.notEqual(edited, fallback.yaml)
  await editor.getByLabel('Stack name', { exact: true }).fill('Local harness')
  await text.fill(edited)
  await editor
    .getByText(
      'Unsaved changes. The summary and warnings refresh when you save.',
      { exact: true },
    )
    .waitFor()
  await editor.getByRole('button', { name: 'Save stack', exact: true }).click()
  await editor.getByText('Saved with 2 warnings.', { exact: true }).waitFor()
  // Saving never disables what was focused: focus stays on Save stack.
  assert.equal(
    await page.evaluate(() => document.activeElement?.textContent),
    'Save stack',
  )
  const warnings = [
    'harness runs path://../harness, a path on this machine; the stack runs it only here.',
    'harness pins a commit, but only a package:// worker is built from one; the executor refuses it.',
  ]
  const saved = editor.locator('[data-stack-warnings]')
  await saved.getByText('2 warnings as last saved', { exact: true }).waitFor()
  for (const warning of warnings)
    await saved.getByText(warning, { exact: true }).waitFor()
  assert.deepEqual(calls.update, [
    { stack_id: 'stack-1', label: 'Local harness', yaml: edited },
  ])

  // YAML the runner refuses is said beside the editor, which keeps what was
  // typed and goes on taking it.
  await text.fill('containers: [\n')
  await editor.getByRole('button', { name: 'Save stack', exact: true }).click()
  await editor
    .getByRole('alert')
    .getByText(/^Not saved\. The stack is not YAML: /)
    .waitFor()
  await editor.getByRole('alert').getByText('Not saved.').waitFor()
  assert.equal(await text.inputValue(), 'containers: [\n')
  assert.equal(await text.getAttribute('aria-invalid'), 'true')
  assert.equal(local[0].yaml, edited)
  await text.press('End')
  await text.pressSequentially(']')
  assert.equal(await text.inputValue(), 'containers: [\n]')

  // Written back, it saves again and the refusal goes.
  await text.fill(edited)
  await editor.getByRole('button', { name: 'Save stack', exact: true }).click()
  await editor.getByRole('alert').waitFor({ state: 'detached' })
  await editor.getByText('Saved with 2 warnings.', { exact: true }).waitFor()
  assert.equal(calls.update.length, 3)

  // Discard changes puts back what was saved.
  await text.fill(`${edited}# more\n`)
  await editor
    .getByRole('button', { name: 'Discard changes', exact: true })
    .click()
  assert.equal(await text.inputValue(), edited)

  // Closing with unsaved changes asks first.
  await text.fill(`${edited}# more\n`)
  await text.press('Escape')
  const ask = page.getByRole('alertdialog', {
    name: 'Discard changes to Local harness?',
  })
  await ask.getByRole('button', { name: 'Keep editing', exact: true }).click()
  await ask.waitFor({ state: 'detached' })
  assert.equal(await text.inputValue(), `${edited}# more\n`)
  await text.press('Escape')
  await ask
    .getByRole('button', { name: 'Discard changes', exact: true })
    .click()
  await editor.waitFor({ state: 'detached' })
  assert.equal(calls.update.length, 3)

  // The copy lists under This Console: what it pins, how it differs from
  // default, its warnings.
  const copy = mine.locator('[data-stack="stack-1"]')
  await copy.getByText('Local harness', { exact: true }).waitFor()
  await copy.getByText(/^stack-1 · saved /).waitFor()
  await copy.getByText('commit 8c02f93a1d4e', { exact: true }).waitFor()
  await copy
    .getByText('Against default: changes harness.', { exact: true })
    .waitFor()
  await copy.getByText('2 warnings', { exact: true }).waitFor()
  for (const warning of warnings)
    await copy.getByText(warning, { exact: true }).waitFor()
  await page
    .getByText(/^2 in the repository, read-only · 1 in this Console\. /)
    .waitFor()

  // Edit opens it again, as saved.
  await copy.getByRole('button', { name: 'Edit Local harness' }).click()
  const again = page.getByRole('dialog', { name: 'Edit Local harness' })
  assert.equal(await again.locator('#sk-yaml').inputValue(), edited)
  await again.getByRole('button', { name: 'Close', exact: true }).click()
  await again.waitFor({ state: 'detached' })

  // New stack: a copy of a stack of this Console, named, opened to edit.
  await page
    .getByRole('button', { name: 'New stack', exact: true })
    .first()
    .click()
  const creating = page.getByRole('dialog', { name: 'New stack' })
  assert.equal(
    await creating.getByRole('radio', { name: /^default / }).isChecked(),
    true,
  )
  await creating.getByRole('radio', { name: /^Local harness / }).check()
  const name = creating.getByLabel(/^Name/)
  assert.equal(await name.getAttribute('placeholder'), 'Local harness copy')
  await creating
    .getByText('Copies Local harness into this Console.', { exact: true })
    .waitFor()
  await name.fill('Mine')
  await creating
    .getByRole('button', { name: 'Create and edit', exact: true })
    .click()
  const made = page.getByRole('dialog', { name: 'Edit Mine' })
  await made.waitFor()
  assert.deepEqual(calls.create.at(-1), { from: 'stack-1', label: 'Mine' })
  assert.equal(await made.locator('#sk-yaml').inputValue(), edited)
  await made.getByRole('button', { name: 'Close', exact: true }).click()
  await made.waitFor({ state: 'detached' })

  // A stack of this Console is deleted after the host's confirmation.
  const row = mine.locator('[data-stack="stack-2"]')
  await row.getByRole('button', { name: 'Delete Mine' }).click()
  const confirm = page.getByRole('alertdialog', { name: 'Delete “Mine”?' })
  await confirm.getByRole('button', { name: 'Cancel', exact: true }).click()
  assert.deepEqual(calls.remove, [])
  await row.getByRole('button', { name: 'Delete Mine' }).click()
  // Confirmed twice before the dialog goes: deleted once.
  await confirm
    .getByRole('button', { name: 'Delete stack', exact: true })
    .evaluate(twice)
  await row.waitFor({ state: 'detached' })
  assert.deepEqual(calls.remove, [{ stack_id: 'stack-2' }])
  assert.equal(
    await page.evaluate(() => document.activeElement?.id),
    'sk-heading',
  )

  // A stack deleted elsewhere while it is open: the reload no longer lists
  // it, and its sheet stays, saying so, with what it holds.
  await page
    .getByRole('button', { name: 'Copy default', exact: true })
    .first()
    .click()
  const kept = page.locator('[data-stack-sheet="edit"]')
  await page.getByRole('dialog', { name: 'Edit default copy' }).waitFor()
  vanish = 'stack-3'
  await kept.getByLabel('Stack name', { exact: true }).fill('Vanishing')
  await kept.getByRole('button', { name: 'Save stack', exact: true }).click()
  await kept
    .getByText(
      'This stack is no longer in this Console. Copy the YAML before closing.',
      { exact: true },
    )
    .waitFor()
  await mine.locator('[data-stack="stack-3"]').waitFor({ state: 'detached' })
  await kept.locator('#sk-yaml').fill(`${fallback.yaml}# kept\n`)
  assert.equal(
    await kept.locator('#sk-yaml').inputValue(),
    `${fallback.yaml}# kept\n`,
  )
  await kept.locator('#sk-yaml').press('Escape')
  await page
    .getByRole('alertdialog', { name: 'Discard changes to Vanishing?' })
    .getByRole('button', { name: 'Discard changes', exact: true })
    .click()
  await kept.waitFor({ state: 'detached' })

  // Provider credentials, below the stacks: each by name, set or not.
  const section = page.locator('[data-credentials]')
  const status = (name) =>
    section.locator(`[data-credential="${name}"] [data-status]`)
  await status('OPENAI_API_KEY').getByText('Not set', { exact: true }).waitFor()
  await status('ZAI_API_KEY')
    .getByText('Set by the worker’s provider_env_file', { exact: true })
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
    .getByRole('button', { name: 'Import from this machine', exact: true })
    .click()
  await section
    .getByText(
      "Set from the worker's environment: DEEPSEEK_API_KEY. Not found there: ANTHROPIC_API_KEY, OPENAI_API_KEY, ZAI_API_KEY.",
      { exact: true },
    )
    .waitFor()
  await status('DEEPSEEK_API_KEY')
    .getByText('Set here', { exact: true })
    .waitFor()

  // Set one: its name is fixed, the value goes once, masked, never shown.
  await section
    .getByRole('button', { name: 'Set OPENAI_API_KEY', exact: true })
    .click()
  const set = page.getByRole('dialog', { name: 'Set OPENAI_API_KEY' })
  assert.equal(
    await set.locator('#credential-name').inputValue(),
    'OPENAI_API_KEY',
  )
  assert.equal(await set.locator('#credential-name').isEditable(), false)
  const value = set.locator('#credential-value')
  assert.equal(await value.getAttribute('type'), 'password')
  await value.fill('sk-typed-7c1b')
  await set
    .getByRole('button', { name: 'Save credential', exact: true })
    .click()
  await set.waitFor({ state: 'detached' })
  await status('OPENAI_API_KEY')
    .getByText('Set here', { exact: true })
    .waitFor()
  assert.deepEqual(calls.credentials.at(-1), [
    'set',
    { name: 'OPENAI_API_KEY', secret: 'sk-typed-7c1b' },
  ])

  // Add one by name: a name that is not an environment variable is refused
  // before anything is sent.
  await section
    .getByRole('button', { name: 'Add credential', exact: true })
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
      .getByRole('button', { name: 'Save credential', exact: true })
      .isDisabled(),
    true,
  )
  await add.locator('#credential-name').fill('MY_GATEWAY_TOKEN')
  await add
    .getByRole('button', { name: 'Save credential', exact: true })
    .click()
  await add.waitFor({ state: 'detached' })
  await status('MY_GATEWAY_TOKEN')
    .getByText('Set here', { exact: true })
    .waitFor()

  // Deleted after the host's confirmation.
  await section
    .getByRole('button', { name: 'Delete MY_GATEWAY_TOKEN', exact: true })
    .click()
  await page
    .getByRole('alertdialog', { name: 'Delete MY_GATEWAY_TOKEN?' })
    .getByRole('button', { name: 'Delete credential', exact: true })
    .evaluate(twice)
  await section
    .locator('[data-credential="MY_GATEWAY_TOKEN"]')
    .waitFor({ state: 'detached' })
  assert.deepEqual(
    calls.credentials.filter(([kind]) => kind === 'delete'),
    [['delete', { name: 'MY_GATEWAY_TOKEN' }]],
  )
  const visible = await page.locator('body').innerText()
  for (const secret of ['sk-imported-4d2e', 'sk-typed-7c1b', 'gw-secret-5a9f'])
    assert.equal(visible.includes(secret), false, `${secret} is shown`)

  // Narrow: each stack and credential reflows, and nothing scrolls sideways.
  await page.setViewportSize({ width: 390, height: 844 })
  await page.locator('.sk-page[data-narrow]').waitFor()
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
    true,
  )
  assert.deepEqual(errors, [])
  console.log(
    'Stacks browser flow passed: a failed first read tried again; repository stacks apart and read-only; one viewed with its YAML as written, its workers and Copy YAML; Copy to edit; a path worker pinning a commit saved with both warnings beside the editor and on the stack; YAML the runner refuses said beside the editor while typing goes on; Discard; closing unsaved changes asks first; New stack from a copy, named; delete behind the host confirmation, confirmed twice and run once; a stack deleted elsewhere keeps its sheet and says so; a failed copy's error left behind; provider credentials listed by name, imported, set masked, added by a valid name only, deleted, no value shown; narrow viewport.',
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
