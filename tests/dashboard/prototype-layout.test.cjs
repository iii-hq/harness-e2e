const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')

const repositoryRoot = path.join(__dirname, '..', '..')
const prototypeRoot = path.join(
  repositoryRoot,
  'dashboard',
  'public',
  'prototype',
)
const read = (filename) =>
  fs.readFileSync(path.join(prototypeRoot, filename), 'utf8')

const html = read('index.html')
const styles = read('styles.css')
const app = read('app.js')

test('keeps the prototype isolated from the React dashboard entrypoint', () => {
  assert.match(html, /Harness E2E dashboard prototype/)
  assert.match(html, /src="\.\/app\.js"/)
  assert.match(html, /href="\.\/styles\.css"/)
  assert.doesNotMatch(html, /src="\/src\/main\.tsx"/)
})

test('supports the proposed run-centric navigation and interactions', () => {
  for (const view of ['overview', 'runs', 'scenarios', 'compare']) {
    assert.match(html, new RegExp(`data-view-panel="${view}"`))
    assert.match(app, new RegExp(`'${view}'`))
  }
  assert.match(html, /id="run-dialog"/)
  assert.match(app, /selectRun\(id\)/)
  assert.match(app, /renderComparison\(\)/)
  assert.match(app, /toggleTheme\(\)/)
})

test('loads real execution summaries when available and labels the fallback', () => {
  assert.match(app, /fetch\('\.\.\/executions\.json'/)
  assert.match(app, /source: 'sample'/)
  assert.match(app, /Live dashboard data/)
  assert.match(app, /Prototype data/)
  assert.doesNotMatch(app, /api\/local\/run/)
})

test('keeps hard gates separate from advisory quality', () => {
  assert.match(html, /Blocking events/)
  assert.match(html, /Quality score/)
  assert.match(html, /Advisory · out of 100/)
  assert.match(app, /Numeric deltas are intentionally disabled/)
  assert.match(app, /median_quality_score/)
})

test('includes responsive and accessible prototype states', () => {
  assert.match(styles, /@media \(max-width: 1000px\)/)
  assert.match(styles, /@media \(max-width: 700px\)/)
  assert.match(styles, /:root\[data-theme="light"\]/)
  assert.match(html, /class="skip-link"/)
  assert.match(html, /aria-live="polite"/)
  assert.match(html, /aria-labelledby="run-dialog-title"/)
})
