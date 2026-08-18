const assert = require('node:assert/strict')
const { readFileSync } = require('node:fs')
const { resolve } = require('node:path')
const test = require('node:test')

const repository = resolve(__dirname, '../..')
const read = (path) => readFileSync(resolve(repository, path), 'utf8')

test('design system demo stays isolated from production routing', () => {
  const dashboardMain = read('dashboard/src/main.tsx')
  const demoHtml = read('dashboard/design-system.html')
  const vite = read('dashboard/vite.config.ts')

  assert.doesNotMatch(dashboardMain, /design-system/)
  assert.match(demoHtml, /src\/design-system\/main\.tsx/)
  assert.match(vite, /designSystem: path\.resolve\(root, 'design-system\.html'\)/)
})

test('demo enforces dense layout and selected GSAP paradigms', () => {
  const page = read('dashboard/src/design-system/DesignSystemPage.tsx')
  const css = read('dashboard/src/design-system/demo.css')

  assert.match(css, /grid-template-columns: repeat\(12, minmax\(0, 1fr\)\)/)
  assert.match(css, /grid-auto-flow: dense/)
  assert.match(page, /ScrollTrigger\.create/)
  assert.match(page, /scale: 0\.8/)
  assert.match(page, /opacity: 0\.2/)
  assert.match(css, /prefers-reduced-motion: reduce/)
})

test('demo avoids banned meta labels and keeps status meanings explicit', () => {
  const page = read('dashboard/src/design-system/DesignSystemPage.tsx')

  assert.doesNotMatch(page, /SECTION \d|QUESTION \d|ABOUT US/)
  for (const status of [
    'passed',
    'failed',
    'inconclusive',
    'unavailable',
    'hard_gate',
    'recommendation',
  ]) {
    assert.match(page, new RegExp(`status="${status}"`))
  }
})
