#!/usr/bin/env node
// Captures every dashboard route at three widths and two themes, plus a
// typography census per capture, so a pull request can show before/after
// evidence. Needs a running dashboard (console at 127.0.0.1:3113 by default,
// or the standalone server) and the Chromium that ships with Playwright.
//
//   pnpm screenshots                       # console, all routes
//   pnpm screenshots -- --base standalone  # http://127.0.0.1:4173/#/
//   pnpm screenshots -- --only overview,tests --widths 1440 --themes light
//   pnpm screenshots -- --out .screenshots/before
import { mkdir, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright'

const BASES = {
  console: 'http://127.0.0.1:3113/#/ext/harness-e2e/',
  standalone: 'http://127.0.0.1:4173/#/',
}

// Detail routes are discovered from the lists so the script keeps working
// as executions, tests and plans change.
const ROUTES = [
  { name: 'overview', route: 'overview' },
  { name: 'tests', route: 'tests' },
  { name: 'executions', route: 'executions' },
  { name: 'plans', route: 'plans' },
  { name: 'plan-new', route: 'plans/new' },
  { name: 'compare', route: 'compare' },
  { name: 'coverage', route: 'coverage' },
  { name: 'execution', discover: 'execution/' },
  { name: 'test-history', discover: 'tests/' },
  { name: 'plan-detail', discover: 'plans/plan-' },
]

const args = parseArgs(process.argv.slice(2))
const base = BASES[args.base] ?? args.base ?? BASES.console
const widths = (args.widths ?? '1440,720,390').split(',').map(Number)
const themes = (args.themes ?? 'light,dark').split(',')
const only = args.only ? new Set(args.only.split(',')) : null
const root = fileURLToPath(new URL('..', import.meta.url))
const outDir = path.resolve(root, args.out ?? '.screenshots')
const maxHeight = Number(args['max-height'] ?? 6000)

await mkdir(outDir, { recursive: true })
const browser = await chromium.launch()
const census = []

try {
  const discovered = await discoverRoutes(browser)
  const targets = ROUTES.flatMap((entry) => {
    if (only && !only.has(entry.name)) return []
    if (entry.route) return [{ name: entry.name, route: entry.route }]
    const found = discovered.find((href) => href.startsWith(entry.discover))
    return found ? [{ name: entry.name, route: found }] : []
  })

  for (const target of targets) {
    for (const theme of themes) {
      for (const width of widths) {
        const record = await capture(browser, target, theme, width)
        census.push(record)
        process.stdout.write(`${record.file}\n`)
      }
    }
  }
} finally {
  await browser.close()
}

await writeFile(
  path.join(outDir, 'census.json'),
  `${JSON.stringify(summarize(census), null, 2)}\n`,
)
process.stdout.write(`${census.length} captures → ${outDir}\n`)

function parseArgs(argv) {
  const out = {}
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index]
    if (!arg.startsWith('--')) continue
    const key = arg.slice(2)
    const next = argv[index + 1]
    if (next && !next.startsWith('--')) {
      out[key] = next
      index += 1
    } else {
      out[key] = 'true'
    }
  }
  return out
}

async function newPage(browser, theme, width) {
  const context = await browser.newContext({
    viewport: { width, height: 900 },
    deviceScaleFactor: 1,
  })
  await context.addInitScript((value) => {
    try {
      // The console and the standalone shell store the theme under
      // different keys; setting both keeps one script for both hosts.
      localStorage.setItem('iii-theme', value)
      localStorage.setItem('harness-e2e-theme', value)
    } catch {
      // storage can be unavailable in some contexts; the default theme wins
    }
  }, theme)
  const page = await context.newPage()
  return { context, page }
}

async function discoverRoutes(browser) {
  const hrefs = new Set()
  for (const route of ['executions', 'tests', 'plans']) {
    const { context, page } = await newPage(browser, 'light', 1440)
    try {
      await page.goto(`${base}${route}`, {
        waitUntil: 'networkidle',
        timeout: 30_000,
      })
      await page.waitForTimeout(1500)
      const links = await page.evaluate(() =>
        [...document.querySelectorAll('a[href*="#/"]')].map((anchor) =>
          anchor.getAttribute('href'),
        ),
      )
      for (const href of links) {
        const tail = href?.split('#/').pop() ?? ''
        const relative = tail.replace(/^ext\/harness-e2e\//, '')
        if (relative) hrefs.add(relative)
      }
    } catch (error) {
      process.stderr.write(`discover ${route}: ${error.message}\n`)
    } finally {
      await context.close()
    }
  }
  return [...hrefs]
}

async function capture(browser, target, theme, width) {
  const { context, page } = await newPage(browser, theme, width)
  const errors = []
  page.on('pageerror', (error) => errors.push(String(error).slice(0, 200)))
  try {
    await page.goto(`${base}${target.route}`, {
      waitUntil: 'networkidle',
      timeout: 30_000,
    })
    await page.waitForTimeout(1500)
    // The console keeps its own scroll container, so a full-page shot needs
    // the viewport to grow to the tallest scrollable element.
    const height = await page.evaluate(() => {
      let bottom = document.documentElement.scrollHeight
      for (const element of document.querySelectorAll('*')) {
        const style = getComputedStyle(element)
        if (
          /(auto|scroll)/.test(style.overflowY) &&
          element.scrollHeight > element.clientHeight + 4
        ) {
          bottom = Math.max(
            bottom,
            element.getBoundingClientRect().top + element.scrollHeight,
          )
        }
      }
      return Math.ceil(bottom + 8)
    })
    await page.setViewportSize({
      width,
      height: Math.min(maxHeight, Math.max(900, height)),
    })
    await page.waitForTimeout(400)
    const file = `${target.name}-${theme}-${width}.png`
    await page.screenshot({ path: path.join(outDir, file) })
    const typography = await page.evaluate(() => {
      const root =
        document.querySelector('.harness-e2e-dashboard') ?? document.body
      const families = new Set()
      const sizes = new Set()
      for (const element of root.querySelectorAll('*')) {
        const hasText = [...element.childNodes].some(
          (node) => node.nodeType === 3 && node.textContent.trim(),
        )
        if (!hasText) continue
        const style = getComputedStyle(element)
        families.add(style.fontFamily.split(',')[0].replace(/"/g, ''))
        sizes.add(Number.parseFloat(style.fontSize))
      }
      return {
        families: [...families],
        sizes: [...sizes].sort((a, b) => a - b),
      }
    })
    return { file, route: target.route, theme, width, errors, ...typography }
  } finally {
    await context.close()
  }
}

function summarize(records) {
  const sizes = new Set()
  const families = new Set()
  for (const record of records) {
    for (const size of record.sizes) sizes.add(size)
    for (const family of record.families) families.add(family)
  }
  return {
    generatedAt: new Date().toISOString(),
    base,
    distinctFontSizes: [...sizes].sort((a, b) => a - b),
    fontSizesBelow11px: [...sizes].filter((size) => size < 11).length,
    fontFamilies: [...families],
    captures: records,
  }
}
