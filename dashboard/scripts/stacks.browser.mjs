// Deterministic browser coverage for Stacks as the redesign canvas draws it:
// the repository's stacks apart from this Console's (empty, then a copy), a
// repository stack viewed read-only with its YAML as written, Copy to edit,
// an edit whose warnings show beside the editor, a save the runner refuses
// said beside it without clearing what was typed, Discard and the question
// before closing unsaved changes; New stack, the stack builder: iii-hq/templates
// failing then read again, the harness template with a worker it would ignore
// refused, a pin to a commit, a copy carrying a worker the template ignores
// (blocked, removed) and Create; a copy created as written when untouched,
// and with only its changed lines when touched; without a template a worker another brings
// (blocked, by the registry), a name the registry does not know, and Edit as
// YAML saved as a new stack; Delete behind the host's confirmation (run once
// when confirmed twice), a sheet kept when its stack is deleted elsewhere;
// then the provider credentials: set, add and delete one, by name only. The
// repository's stacks are the files of stacks/; the runner's answers (what a
// stack declares, its warnings and the parse error), iii-hq/templates and the
// registry are stood in for here and covered by the Rust tests.
import assert from 'node:assert/strict'
import { readdirSync, readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright'
import { createConsoleTestHost } from './console-test-host.mjs'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..')

/** What the runner lists for a stack's YAML: its containers and warnings. */
function view(id, label, source, yaml) {
  if (!/^containers:/m.test(yaml))
    throw new Error('A stack needs a `containers` mapping.')
  const block = yaml.slice(yaml.search(/^containers:/m)).split(/\n(?=\S)/)[0]
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
const calls = {
  create: [],
  update: [],
  remove: [],
  credentials: [],
  resolve: [],
}
// iii-hq/templates, as the worker lists them: the first read fails.
let failTemplates = true
const templateWorkers = (names) =>
  names.map((name) => ({
    name,
    worker: `package://${name}`,
    version: 'latest',
  }))
const templates = {
  repository: 'iii-hq/templates',
  ref: 'main',
  revision: '4077e670ee4c503f760f2a72c9275b2e00ac6437',
  templates: [
    {
      id: 'starter',
      name: 'Starter',
      description: 'A basic iii project with TypeScript.',
      workers: [],
      note: 'Ships no worker-compose.yaml, so a group would start nothing.',
    },
    {
      id: 'harness',
      name: 'Harness',
      description: 'Build with agents in the ADE, the iii agent workspace',
      workers: templateWorkers([
        'queue',
        'state',
        'session-manager',
        'llm-router',
        'provider-anthropic',
        'provider-openai',
        'provider-deepseek',
        'context-manager',
        'iii-directory',
        'cron',
        'ade',
        'ide',
        'harness',
        'browser',
      ]),
    },
  ],
}
// The iii registry: harness brings browser; browser answers once let go.
const registry = {
  harness: {
    name: 'harness',
    version: '1.8.36',
    dependencies: ['browser', 'llm-router', 'session-manager', 'state'],
  },
  fp: { name: 'fp', version: '0.2.20', dependencies: [] },
  browser: { name: 'browser', version: '0.3.1', dependencies: [] },
}
// Creating Mine waits until the flow lets it go.
let letMineGo
const mineCreated = new Promise((resolve) => {
  letMineGo = resolve
})
let letBrowserGo
const browserAnswer = new Promise((resolve) => {
  letBrowserGo = resolve
})
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
const fromEnvironment = new Set(['DEEPSEEK_API_KEY'])
const credentials = () => ({
  credentials: [...new Set([...Object.keys(known), ...stored.keys()])]
    .sort()
    .map((name) => {
      const source = stored.has(name)
        ? 'console'
        : fromFile.has(name)
          ? 'provider_env_file'
          : fromEnvironment.has(name)
            ? 'environment'
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
      request.yaml ?? from.yaml,
    )
    local.push(stack)
    return request.label === 'Mine' ? mineCreated.then(() => stack) : stack
  }
  if (id === 'stack-templates-list') {
    if (failTemplates) {
      failTemplates = false
      throw new Error(
        'GitHub answered 503 Service Unavailable for https://api.github.com/repos/iii-hq/templates/commits/main.',
      )
    }
    return templates
  }
  if (id === 'worker-resolve') {
    calls.resolve.push(request.worker)
    if (request.worker === 'browser') return browserAnswer
    // The registry does not answer about flaky-worker the first time.
    if (request.worker === 'flaky-worker')
      if (
        calls.resolve.filter((worker) => worker === 'flaky-worker').length === 1
      )
        throw new Error(
          'The iii registry answered 503: The registry is restarting.',
        )
      else return { name: 'flaky-worker', version: '0.1.0', dependencies: [] }
    return (
      registry[request.worker] ?? {
        error: {
          code: 'worker_not_found',
          message: `Worker '${request.worker}' was not found in the registry.`,
        },
      }
    )
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

  // A stack of this Console with its own environment and comments, made
  // elsewhere: the next reload lists it.
  const envYaml = [
    '# A stack with its own environment.',
    'iii: latest',
    '',
    'containers:',
    '  harness:',
    '    worker: package://harness',
    '    version: latest',
    '    environment:',
    '      RUST_LOG: debug # louder',
    '',
    '  fp:',
    '    worker: package://fp',
    '    version: latest',
    '',
  ].join('\n')
  local.push(view('stack-env', 'With environment', 'local', envYaml))

  // New stack is the stack builder. iii-hq/templates could not be read: it
  // says so, the rest still answers, and Try again reads it.
  await page
    .getByRole('button', { name: 'New stack', exact: true })
    .first()
    .click()
  const builder = page.getByRole('dialog', { name: 'New stack' })
  const create = builder.getByRole('button', {
    name: 'Create stack',
    exact: true,
  })
  const trouble = builder.getByRole('alert')
  await trouble.waitFor()
  assert.match(
    await trouble.innerText(),
    /^iii-hq\/templates couldn’t be read\. GitHub answered 503 Service Unavailable/,
  )
  await builder.getByText('Pick a template.', { exact: true }).waitFor()
  assert.equal(await create.getAttribute('aria-disabled'), 'true')
  assert.equal(await create.getAttribute('aria-describedby'), 'sb-status')
  await trouble.getByRole('button', { name: 'Try again', exact: true }).click()
  await trouble.waitFor({ state: 'detached' })

  // A template without workers can't be picked, and says why.
  const starter = builder.locator('[data-template="starter"]')
  await starter
    .getByText(
      'Ships no worker-compose.yaml, so a group would start nothing.',
      {
        exact: true,
      },
    )
    .waitFor()
  assert.equal(
    await starter.getByRole('radio').getAttribute('aria-disabled'),
    'true',
  )
  await starter.click({ force: true })
  assert.equal(await starter.getByRole('radio').isChecked(), false)

  // The harness template: the stack is named after it, and runs every
  // template worker at the template's version.
  await builder.locator('[data-template="harness"]').click()
  const name = builder.getByLabel('Name', { exact: true })
  assert.equal(await name.inputValue(), 'harness')
  await builder
    .getByText('harness · main @ 4077e670', { exact: true })
    .waitFor()
  await builder
    .getByText(
      'Runs the harness project with every worker at the template’s version.',
      { exact: true },
    )
    .waitFor()
  assert.equal(
    await builder
      .getByRole('group', { name: 'From the harness template · 14' })
      .getByRole('listitem')
      .count(),
    14,
  )

  // Add worker opens the picker in the YAML's place; fp, which the template
  // would ignore, is not added.
  await builder.getByRole('button', { name: 'Add worker', exact: true }).click()
  const picker = builder.getByRole('complementary', { name: 'Add a worker' })
  const fpPick = picker.locator('[data-pick="fp"]')
  await fpPick
    .getByText('not in the template, ignored', { exact: true })
    .waitFor()
  assert.equal(await fpPick.getAttribute('aria-disabled'), 'true')
  await fpPick.click({ force: true })
  await builder
    .getByText(
      'Nothing pinned: every template worker runs at the template’s version. Add one of its workers to pin it.',
      { exact: true },
    )
    .waitFor()
  await picker
    .getByRole('button', { name: 'Done, back to the YAML', exact: true })
    .click()
  await builder.getByRole('complementary', { name: 'YAML' }).waitFor()

  // A copy of harness-template brings fp: blocked, its lines marked, and
  // Create held back until it is removed.
  await builder.getByRole('radio', { name: /^Copy of a stack/ }).check()
  await builder
    .getByRole('radio', { name: /^harness-template stacks\/harness-template/ })
    .check()
  assert.equal(await name.inputValue(), 'harness-template · copy')
  const fpRow = builder.locator('[data-worker="fp"]')
  assert.equal(await fpRow.getAttribute('data-tone'), 'block')
  assert.equal(
    await fpRow.locator('.sb-note strong').innerText(),
    'Would be ignored.',
  )
  await builder
    .getByText(
      'Can’t create it yet: fp would be ignored by the harness template.',
      { exact: true },
    )
    .waitFor()
  assert.deepEqual(
    await builder.locator('.sk-line[data-blocked] .sk-text').allTextContents(),
    ['  fp:', '    worker: package://fp', '    version: latest'],
  )
  const made = calls.create.length
  await create.click({ force: true })
  assert.equal(calls.create.length, made)
  await fpRow.getByRole('button', { name: 'Remove fp', exact: true }).click()
  await fpRow.waitFor({ state: 'detached' })
  await builder
    .getByText('Runs the harness project with 4 pinned.', { exact: true })
    .waitFor()

  // harness pinned to a commit: the menu writes the YAML as it is picked,
  // and Escape closes the menu, not the builder.
  const harnessRow = builder.locator('[data-worker="harness"]')
  const version = harnessRow.getByRole('button', { name: 'Version of harness' })
  await version.click()
  const versionMenu = builder.getByRole('dialog', {
    name: 'Version of harness',
  })
  await versionMenu.getByRole('radio', { name: /^A commit/ }).check()
  await builder
    .getByText('Type the commit for harness.', { exact: true })
    .waitFor()
  assert.equal(await create.getAttribute('aria-disabled'), 'true')
  const commit = versionMenu.getByRole('textbox', { name: 'A commit' })
  await commit.fill('8c02f93a1d4e')
  await version.getByText('commit 8c02f93', { exact: true }).waitFor()
  assert.match(
    await harnessRow.locator('.sb-note').innerText(),
    /^Pins the template’s harness\. The template keeps it, built from commit 8c02f93/,
  )
  await builder
    .locator('.sb-chips li[data-pinned]')
    .getByText('commit 8c02f93', { exact: true })
    .waitFor()
  await commit.press('Escape')
  await versionMenu.waitFor({ state: 'detached' })
  assert.equal(
    await page.evaluate(() =>
      document.activeElement?.getAttribute('aria-label'),
    ),
    'Version of harness',
  )

  // Named and created from its YAML; it opens as a copy does.
  await name.fill('Mine')
  await create.click()
  // While it is created, Edit as YAML waits too.
  await builder
    .getByRole('button', { name: 'Creating…', exact: true })
    .waitFor()
  assert.equal(
    await builder
      .getByRole('button', { name: 'Edit as YAML', exact: true })
      .getAttribute('aria-disabled'),
    'true',
  )
  letMineGo()
  const mineSheet = page.getByRole('dialog', { name: 'Edit Mine' })
  await mineSheet.waitFor()
  const builtYaml = `${[
    'iii: latest',
    'template: harness',
    '',
    'containers:',
    '  harness:',
    '    worker: package://harness',
    '    commit: "8c02f93a1d4e"',
    '  harness-e2e:',
    '    worker: package://harness-e2e',
    '    version: latest',
    '  provider-deepseek:',
    '    worker: package://provider-deepseek',
    '    version: latest',
    '  provider-zai:',
    '    worker: package://provider-zai',
    '    version: latest',
    '',
    'startup_timeout: 5m',
    'stop_timeout: 30s',
  ].join('\n')}\n`
  assert.deepEqual(calls.create.at(-1), { label: 'Mine', yaml: builtYaml })
  assert.equal(await mineSheet.locator('#sk-yaml').inputValue(), builtYaml)
  await mineSheet
    .getByText('Created. Change what you need, then save.', { exact: true })
    .waitFor()
  await mineSheet.getByRole('button', { name: 'Close', exact: true }).click()
  await mineSheet.waitFor({ state: 'detached' })

  // A copy the form did not touch is the stack as written: Create copies it,
  // comments, environment and all.
  await page
    .getByRole('button', { name: 'New stack', exact: true })
    .first()
    .click()
  await builder.getByRole('radio', { name: /^Copy of a stack/ }).check()
  await builder
    .getByRole('radio', { name: /^With environment stack-env/ })
    .check()
  assert.equal(await name.inputValue(), 'With environment · copy')
  await builder
    .getByText('copied as written from stack-env', { exact: true })
    .waitFor()
  assert.equal(
    await builder
      .getByRole('region', { name: 'Stack YAML, copied as written' })
      .locator('.sk-text')
      .evaluateAll((lines) =>
        lines.map((line) => line.textContent.trimEnd()).join('\n'),
      ),
    envYaml.replace(/\n$/, ''),
  )
  await create.click()
  const envSheet = page.getByRole('dialog', {
    name: 'Edit With environment · copy',
  })
  await envSheet.waitFor()
  assert.deepEqual(calls.create.at(-1), {
    from: 'stack-env',
    label: 'With environment · copy',
  })
  assert.equal(await envSheet.locator('#sk-yaml').inputValue(), envYaml)
  await envSheet.getByRole('button', { name: 'Close', exact: true }).click()
  await envSheet.waitFor({ state: 'detached' })

  // Touched, it says what the form leaves out; Edit as YAML keeps it, with
  // the change made to that container's lines only.
  await page
    .getByRole('button', { name: 'New stack', exact: true })
    .first()
    .click()
  await builder.getByRole('radio', { name: /^Copy of a stack/ }).check()
  await builder
    .getByRole('radio', { name: /^With environment stack-env/ })
    .check()
  await builder
    .locator('[data-worker="fp"]')
    .getByRole('button', { name: 'Remove fp', exact: true })
    .click()
  await builder
    .getByText(
      'Creating from the form keeps each worker and its pin; comments and other keys of With environment are left out. Edit as YAML keeps them.',
      { exact: true },
    )
    .waitFor()
  await builder
    .getByRole('button', { name: 'Edit as YAML', exact: true })
    .click()
  const envDraft = page.locator('[data-stack-sheet="edit"]')
  assert.equal(
    await envDraft.locator('#sk-yaml').inputValue(),
    envYaml.replace(
      '  fp:\n    worker: package://fp\n    version: latest\n',
      '',
    ),
  )
  await envDraft.getByRole('button', { name: 'Cancel', exact: true }).click()
  await page
    .getByRole('alertdialog', { name: 'Discard this new stack?' })
    .getByRole('button', { name: 'Discard changes', exact: true })
    .click()
  await envDraft.waitFor({ state: 'detached' })

  // Without a template, what a worker brings comes from the registry:
  // browser, added first, is blocked once harness brings it, even before
  // its own answer.
  await page
    .getByRole('button', { name: 'New stack', exact: true })
    .first()
    .click()
  await builder.locator('[data-template="harness"]').waitFor()
  await builder.getByRole('radio', { name: /^No template/ }).check()
  await builder.getByText('Add at least one worker.', { exact: true }).waitFor()
  await builder.getByRole('button', { name: 'Add worker', exact: true }).click()
  const search = picker.getByRole('searchbox', { name: 'Search workers' })
  await search.fill('brow')
  await picker.locator('[data-pick="browser"]').click()
  const browserRow = builder.locator('[data-worker="browser"]')
  await browserRow
    .getByText('Checking the registry…', { exact: true })
    .waitFor()
  await search.fill('harness')
  await picker.locator('[data-pick="harness"]').click()
  const harnessDeclared = builder.locator('[data-worker="harness"]')
  await harnessDeclared
    .locator('.sb-note strong')
    .getByText('Brings browser, llm-router, session-manager, state with it.', {
      exact: true,
    })
    .waitFor()
  assert.equal(await browserRow.getAttribute('data-tone'), 'block')
  assert.equal(
    await browserRow.locator('.sb-note strong').innerText(),
    'Already arrives with harness.',
  )
  await builder
    .getByText(
      'Can’t create it yet: browser already arrives with another worker.',
      { exact: true },
    )
    .waitFor()
  letBrowserGo(registry.browser)

  // A name the registry does not know says so and can't be added.
  await search.fill('package://nope-worker')
  const typed = picker.locator('[data-pick="nope-worker"]')
  await typed
    .getByText(
      "package://nope-worker · Worker 'nope-worker' was not found in the registry.",
      { exact: true },
    )
    .waitFor()
  assert.equal(await typed.getAttribute('aria-disabled'), 'true')
  await typed.click({ force: true })
  assert.equal(await builder.locator('[data-worker="nope-worker"]').count(), 0)
  assert.deepEqual(
    calls.resolve.filter((worker) => worker === 'nope-worker'),
    ['nope-worker'],
  )
  // A registry that does not answer is not a worker that does not exist:
  // it warns, blocks nothing, and Try again asks again.
  await search.fill('flaky-worker')
  const flaky = picker.locator('[data-pick="flaky-worker"]')
  await flaky
    .getByText(
      'package://flaky-worker · couldn’t reach the registry: The iii registry answered 503: The registry is restarting.',
      { exact: true },
    )
    .waitFor()
  assert.equal(await flaky.getAttribute('aria-disabled'), null)
  await picker.getByRole('button', { name: 'Try again', exact: true }).click()
  await flaky
    .getByText('package://flaky-worker · found, 0.1.0', { exact: true })
    .waitFor()

  // Escape closes the picker, not the builder.
  await search.press('Escape')
  await picker.waitFor({ state: 'detached' })
  await builder.getByRole('complementary', { name: 'YAML' }).waitFor()

  // Edit as YAML hands what the form wrote to the editor, unsaved; Save
  // stack creates it.
  await browserRow
    .getByRole('button', { name: 'Remove browser', exact: true })
    .click()
  // Without a template nothing names it: Create waits for a name.
  await builder.getByText('Name the stack.', { exact: true }).waitFor()
  await name.fill('Harness only')
  await builder
    .getByText('Runs the 1 worker declared, plus what they depend on.', {
      exact: true,
    })
    .waitFor()
  await builder
    .getByRole('button', { name: 'Edit as YAML', exact: true })
    .click()
  await page.locator('[data-stack-builder]').waitFor({ state: 'detached' })
  const draft = page.locator('[data-stack-sheet="edit"]')
  const bareYaml =
    'iii: latest\n\ncontainers:\n  harness:\n    worker: package://harness\n    version: latest\n\nstartup_timeout: 5m\nstop_timeout: 30s\n'
  assert.equal(await draft.locator('#sk-yaml').inputValue(), bareYaml)
  assert.equal(
    await draft.getByLabel('Stack name', { exact: true }).inputValue(),
    'Harness only',
  )
  await draft
    .getByText('Not saved yet. Save stack creates it in this Console.', {
      exact: true,
    })
    .waitFor()
  await draft.getByRole('button', { name: 'Save stack', exact: true }).click()
  const savedDraft = page.getByRole('dialog', { name: 'Edit Harness only' })
  await savedDraft.waitFor()
  assert.deepEqual(calls.create.at(-1), {
    label: 'Harness only',
    yaml: bareYaml,
  })
  await savedDraft.getByRole('button', { name: 'Close', exact: true }).click()
  await savedDraft.waitFor({ state: 'detached' })

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
  vanish = `stack-${calls.create.length}`
  await kept.getByLabel('Stack name', { exact: true }).fill('Vanishing')
  await kept.getByRole('button', { name: 'Save stack', exact: true }).click()
  await kept
    .getByText(
      'This stack is no longer in this Console. Copy the YAML before closing.',
      { exact: true },
    )
    .waitFor()
  await mine.locator(`[data-stack="${vanish}"]`).waitFor({ state: 'detached' })
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

  // Inherited from the worker's environment: nothing to import or delete.
  await status('DEEPSEEK_API_KEY')
    .getByText('Set by the worker’s environment', { exact: true })
    .waitFor()
  assert.equal(
    await section.getByRole('button', { name: /^Import/ }).count(),
    0,
  )

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
  // Drawn as dots, never offered to the browser's password manager, and
  // never written to an attribute.
  const value = set.locator('#credential-value')
  assert.equal(await value.getAttribute('type'), 'text')
  assert.equal(await value.getAttribute('autocomplete'), 'off')
  assert.equal(await value.getAttribute('spellcheck'), 'false')
  assert.equal(
    await value.evaluate((input) =>
      getComputedStyle(input).getPropertyValue('-webkit-text-security'),
    ),
    'disc',
  )
  await value.fill('sk-typed-7c1b')
  assert.equal(await value.getAttribute('value'), null)
  assert.equal(
    await page.evaluate(() =>
      document.documentElement.outerHTML.includes('sk-typed-7c1b'),
    ),
    false,
  )
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

  // Narrow: each stack and credential reflows, and nothing scrolls sideways;
  // New stack is one column, its YAML below the form.
  await page.setViewportSize({ width: 390, height: 844 })
  await page.locator('.sk-page[data-narrow]').waitFor()
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
    true,
  )
  await page.getByRole('button', { name: 'More actions', exact: true }).click()
  await page.getByRole('menuitem', { name: 'New stack', exact: true }).click()
  const narrowBuilder = page.locator('.sb[data-narrow]')
  await narrowBuilder.locator('[data-template="harness"]').click()
  const [form, yamlSide] = await Promise.all([
    narrowBuilder.locator('.sb-form').boundingBox(),
    narrowBuilder.getByRole('complementary', { name: 'YAML' }).boundingBox(),
  ])
  assert.equal(yamlSide.y >= form.y + form.height - 1, true)
  assert.equal(
    await narrowBuilder.evaluate(
      (dialog) => dialog.scrollWidth <= dialog.clientWidth,
    ),
    true,
  )
  await narrowBuilder
    .getByRole('button', { name: 'Cancel', exact: true })
    .click()
  await narrowBuilder.waitFor({ state: 'detached' })
  assert.deepEqual(errors, [])
  console.log(
    'Stacks browser flow passed: a failed first read tried again; repository stacks apart and read-only; one viewed with its YAML as written, its workers and Copy YAML; Copy to edit; a path worker pinning a commit saved with both warnings beside the editor and on the stack; YAML the runner refuses said beside the editor while typing goes on; Discard; closing unsaved changes asks first; New stack built from the harness template (templates tried again, fp refused in the picker and blocked from a copy, removed, harness pinned to a commit, created), a copy the form did not touch created as written and a touched one warned and handed to the editor with only its lines changed, without a template (browser blocked as harness brings it, a name the registry does not know, Edit as YAML saved as a new stack); delete behind the host confirmation, confirmed twice and run once; a stack deleted elsewhere keeps its sheet and says so; the error of a failed copy left behind; provider credentials listed by name, inherited from the worker environment, set masked, added by a valid name only, deleted, no value shown; narrow viewport, the builder in one column.',
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
