#!/usr/bin/env node
// Captures the live Console with this checkout's dashboard bundle: the
// Console's requests for the extension's page.js and styles.css are answered
// from a local dist-console, so a branch shows on real data without
// rebuilding or restarting the harness-e2e worker. Each route is captured in
// light, dark and narrow (640px wide, light).
//
// It opens the worker's standalone page (#/worker/harness-e2e), never the
// e2e tab, and sets the dashboard's route before the bundle mounts, so the
// Console's saved layout is left alone. Only the reads the pages make go
// through (READS below); any other function is refused.
//
//   node scripts/preview-bundle.mjs executions tests suites stacks
//   node scripts/preview-bundle.mjs --dist dist-console --out /tmp/shots executions
//
// Routes are what follows #/ext/harness-e2e/ (default: executions). Without
// --dist the bundle is built first. Options: --base (the Console, default
// http://127.0.0.1:3113/), --out (default dashboard/.screenshots/preview),
// --width (default 1440), --narrow (default 640), --height (default 1000). --dist and --out are read
// from the current directory. Where /tmp is small, point TMPDIR at a
// directory on disk or Chromium runs out of room.
import { spawnSync } from 'node:child_process'
import { mkdirSync, readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright'

const root = fileURLToPath(new URL('..', import.meta.url))
const { options, routes } = parseArgs(process.argv.slice(2))
const base = options.base ?? 'http://127.0.0.1:3113/'
const outDir = options.out
  ? path.resolve(options.out)
  : path.join(root, '.screenshots/preview')
const width = Number(options.width ?? 1440)
const narrow = Number(options.narrow ?? 640)
// The Console scrolls inside its pane: a taller window shows more of a page.
const height = Number(options.height ?? 1000)

let distDir = options.dist ? path.resolve(options.dist) : null
if (!distDir) {
  const build = spawnSync(
    'pnpm',
    ['exec', 'vite', 'build', '--config', 'vite.console.config.ts'],
    { cwd: root, stdio: 'inherit' },
  )
  if (build.status !== 0) process.exit(build.status ?? 1)
  distDir = path.join(root, 'dist-console')
}
const bundle = {
  'page-real.js': readFileSync(path.join(distDir, 'page.js')),
  'styles.css': readFileSync(path.join(distDir, 'styles.css')),
}
// The functions the dashboard only reads through (console-entry.tsx):
// lists, gets, evidence, GitHub contracts, iii-hq/templates and the registry. Anything else, and anything
// outside e2e::dashboard::, is refused.
const READS = [
  'executions-list',
  'execution-get',
  'evidence-read',
  'github-runs-list',
  'github-run-contracts',
  'github-status-get',
  'evaluated-versions-list',
  'tests-list',
  'test-version-get',
  'test-history-get',
  'catalog-get',
  'suites-list',
  'stacks-list',
  'stack-templates-list',
  'worker-resolve',
  'stack-preview',
  'iii-releases-list',
  'credentials-list',
  'trends-get',
  'version-compare',
].map((name) => `e2e::dashboard::${name}`)
// The Console loads page.js; this one sets the route and guards the calls,
// then hands over to the bundle.
const wrapper = `import setup from './page-real.js'
const READS = new Set(${JSON.stringify(READS)})
export default function (host) {
  if (window.__previewRoute) history.replaceState(null, '', window.__previewRoute)
  const trigger = (id, payload, options) =>
    READS.has(String(id))
      ? host.iii.trigger(id, payload, options)
      : Promise.reject(new Error('preview refuses ' + id))
  // The host's client is frozen: a copy of its own members, trigger guarded.
  return setup({ ...host, iii: { ...host.iii, trigger } })
}`

const variants = [
  { name: 'light', theme: 'light', width },
  { name: 'dark', theme: 'dark', width },
  { name: 'narrow', theme: 'light', width: narrow },
]

mkdirSync(outDir, { recursive: true })
const browser = await chromium.launch({ args: ['--disable-dev-shm-usage'] })
let failed = 0
try {
  captures: for (const route of routes.length ? routes : ['executions']) {
    for (const variant of variants) {
      const file = path.join(
        outDir,
        // A route with many ids stays under the file name limit.
        `${route.replace(/[/?&=]/g, '_').slice(0, 120)}-${variant.name}.png`,
      )
      const result = await capture(route, variant, file)
      if (!result.ok) failed += 1
      console.log(`${route} ${variant.name}: ${result.message}`)
      if (result.fatal) break captures
    }
  }
} finally {
  await browser.close()
}
process.exit(failed ? 1 : 0)

async function capture(route, { theme, width: viewportWidth }, file) {
  // A service worker would answer page.js from its cache, past the routes.
  const context = await browser.newContext({
    viewport: { width: viewportWidth, height },
    colorScheme: theme,
    serviceWorkers: 'block',
  })
  // The Console keeps its theme in localStorage and html[data-theme].
  await context.addInitScript(
    ({ value, hash }) => {
      localStorage.setItem('iii-theme', value)
      window.__previewRoute = hash
    },
    { value: theme, hash: `#/ext/harness-e2e/${route}` },
  )
  const page = await context.newPage()
  const served = { 'page-real.js': 0, 'styles.css': 0 }
  await page.route('**/ui/harness-e2e/page.js*', (request) =>
    request.fulfill({ contentType: 'text/javascript', body: wrapper }),
  )
  for (const name of Object.keys(bundle)) {
    await page.route(`**/ui/harness-e2e/${name}*`, (request) => {
      served[name] += 1
      return request.fulfill({
        contentType: name.endsWith('.js') ? 'text/javascript' : 'text/css',
        body: bundle[name],
      })
    })
  }
  const errors = []
  page.on('pageerror', (error) => errors.push(String(error).slice(0, 160)))
  try {
    await page.goto(new URL('#/worker/harness-e2e', base).href, {
      waitUntil: 'load',
      timeout: 30_000,
    })
    await page.evaluate((value) => {
      document.documentElement.dataset.theme = value
    }, theme)
    const found = await page
      .waitForSelector('[data-harness-e2e-dashboard]', { timeout: 20_000 })
      .then(() => true)
      .catch(() => false)
    if (!found)
      return {
        ok: false,
        fatal: true,
        message: `the Console at ${base} shows no Harness E2E page at #/worker/harness-e2e. Check that its harness-e2e worker is running, then run this again.`,
      }
    // Pages mark their loading placeholders busy; wait them out.
    await page
      .waitForFunction(
        () =>
          !document.querySelector(
            '[data-harness-e2e-dashboard] [role="status"][aria-busy="true"]',
          ),
        null,
        { timeout: 15_000 },
      )
      .catch(() => errors.push('still loading after 15s'))
    await page.waitForTimeout(1000)
    await page.screenshot({ path: file })
    if (!served['page-real.js'] || !served['styles.css'])
      return {
        ok: false,
        message: `not the local bundle (served ${JSON.stringify(served)})`,
      }
    return {
      ok: true,
      message: `${file}${errors.length ? ` errors=${errors.join(' | ')}` : ''}`,
    }
  } catch (error) {
    return { ok: false, message: `FAILED ${String(error).split('\n')[0]}` }
  } finally {
    await context.close()
  }
}

function parseArgs(argv) {
  const parsed = { options: {}, routes: [] }
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index]
    if (arg === '--') continue
    if (arg.startsWith('--')) parsed.options[arg.slice(2)] = argv[++index]
    else parsed.routes.push(arg)
  }
  return parsed
}
