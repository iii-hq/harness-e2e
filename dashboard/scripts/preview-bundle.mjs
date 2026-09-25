#!/usr/bin/env node
// Captures the live Console with this checkout's dashboard bundle: the
// Console's requests for the extension's page.js and styles.css are answered
// from a local dist-console, so a branch shows on real data without
// rebuilding or restarting the harness-e2e worker. Each route is captured in
// light, dark and narrow (640px wide, light).
//
//   node scripts/preview-bundle.mjs executions tests suites stacks
//   node scripts/preview-bundle.mjs --dist dist-console --out /tmp/shots executions
//
// Routes are what follows #/ext/harness-e2e/ (default: executions). Without
// --dist the bundle is built first. Options: --base (the Console, default
// http://127.0.0.1:3113/), --out (default .screenshots/preview), --width
// (default 1440), --narrow (default 640). Where /tmp is small, point TMPDIR
// at a directory on disk or Chromium runs out of room.
import { spawnSync } from 'node:child_process'
import { mkdirSync, readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright'

const root = fileURLToPath(new URL('..', import.meta.url))
const { options, routes } = parseArgs(process.argv.slice(2))
const base = options.base ?? 'http://127.0.0.1:3113/'
const outDir = path.resolve(root, options.out ?? '.screenshots/preview')
const width = Number(options.width ?? 1440)
const narrow = Number(options.narrow ?? 640)

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
  'page.js': readFileSync(path.join(distDir, 'page.js')),
  'styles.css': readFileSync(path.join(distDir, 'styles.css')),
}

const variants = [
  { name: 'light', theme: 'light', width },
  { name: 'dark', theme: 'dark', width },
  { name: 'narrow', theme: 'light', width: narrow },
]

mkdirSync(outDir, { recursive: true })
const browser = await chromium.launch({ args: ['--disable-dev-shm-usage'] })
let failed = 0
try {
  for (const route of routes.length ? routes : ['executions']) {
    for (const variant of variants) {
      const file = path.join(
        outDir,
        `${route.replace(/\//g, '_')}-${variant.name}.png`,
      )
      const result = await capture(route, variant, file)
      if (!result.ok) failed += 1
      console.log(`${route} ${variant.name}: ${result.message}`)
    }
  }
} finally {
  await browser.close()
}
process.exit(failed ? 1 : 0)

async function capture(route, { theme, width: viewportWidth }, file) {
  const context = await browser.newContext({
    viewport: { width: viewportWidth, height: 1000 },
    colorScheme: theme,
  })
  // The Console keeps its theme in localStorage and html[data-theme].
  await context.addInitScript((value) => {
    localStorage.setItem('iii-theme', value)
  }, theme)
  const page = await context.newPage()
  const served = { 'page.js': 0, 'styles.css': 0 }
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
    await page.goto(base, { waitUntil: 'load', timeout: 30_000 })
    await page.evaluate((value) => {
      document.documentElement.dataset.theme = value
    }, theme)
    // Open the extension's tab first; a bare hash change does not switch tabs.
    await page
      .getByRole('tab')
      .filter({ hasText: /^e2e/ })
      .first()
      .click({ timeout: 15_000 })
    await page.evaluate((next) => {
      window.location.hash = next
    }, `#/ext/harness-e2e/${route}`)
    await page.waitForSelector('[data-harness-e2e-dashboard]', {
      timeout: 15_000,
    })
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
    if (!served['page.js'] || !served['styles.css'])
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
