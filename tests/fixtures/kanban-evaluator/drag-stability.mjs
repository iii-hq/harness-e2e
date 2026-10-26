import { pathToFileURL } from 'node:url'

const probe = await import(pathToFileURL(process.argv[2]).href)
const playwright = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE_PATH).href)
const browser = await playwright.chromium.launch({
  headless: true,
  executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH,
})

async function replayShard(style, payload, attempts) {
  const page = await browser.newPage({ viewport: { width: 900, height: 720 } })
  await page.setContent(`<style>
    #board { display:grid; grid-template-columns:repeat(5,150px); gap:${style === 'baseline' ? '16px' : '24px'} }
    .lane { height:693px; box-sizing:border-box; padding:${style === 'baseline' ? '12px' : '20px'}; border:1px solid }
    .card { display:block; height:96px; margin:8px 0 }
  </style><div style="height:140px"></div><main><div id="board">
    <section class="lane">${'<div class="card"></div>'.repeat(5)}</section>
    <section class="lane"><a id="source" class="card" draggable="true">Drag probe</a><div class="card"></div></section>
    <section class="lane"></section><section class="lane"></section>
    <section id="target" class="lane">${'<div class="card"></div>'.repeat(5)}</section>
  </div></main>`)
  const requests = []
  await page.route('http://kanban.test/api/tickets/id', async (route) => {
    requests.push(route.request().postDataJSON())
    await route.fulfill({ status: 200, contentType: 'application/json', body: '{}' })
  })
  await page.locator('#target').evaluate((target, body) => {
    target.addEventListener('dragover', (event) => event.preventDefault())
    target.addEventListener('drop', (event) => {
      event.preventDefault()
      fetch('http://kanban.test/api/tickets/id', {
        method: 'PATCH', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body),
      })
    })
  }, payload)
  const clipped = await page.locator('#target').evaluate((target) => target.getBoundingClientRect().bottom > innerHeight)
  for (let attempt = 0; attempt < attempts; attempt++) {
    await probe.dragToVisibleLane(page.locator('#source'), page.locator('#target'), page)
    const expected = attempt + 1
    for (let wait = 0; requests.length < expected && wait < 100; wait++) await new Promise((resolve) => setTimeout(resolve, 10))
  }
  const result = {
    attempts, patches: requests.length,
    valid: requests.filter((body) => probe.sameJson(body, { status: 'done' })).length,
    height: (await page.locator('#target').boundingBox()).height, clipped,
  }
  await page.close()
  return result
}

async function replay(style, payload = { status: 'done' }, attempts = 100) {
  const shards = Math.min(5, attempts)
  const base = Math.floor(attempts / shards)
  const results = await Promise.all(Array.from({ length: shards }, (_, index) =>
    replayShard(style, payload, base + (index < attempts % shards ? 1 : 0))))
  return {
    attempts: results.reduce((sum, result) => sum + result.attempts, 0),
    patches: results.reduce((sum, result) => sum + result.patches, 0),
    valid: results.reduce((sum, result) => sum + result.valid, 0),
    height: results[0].height,
    clipped: results.every((result) => result.clipped),
  }
}

async function missingDrop() {
  const page = await browser.newPage()
  await page.setContent('<a id="source" draggable="true">card</a><section id="target" style="height:693px"></section>')
  let patches = 0
  await page.route('http://kanban.test/api/tickets/id', async (route) => { patches++; await route.abort() })
  await probe.dragToVisibleLane(page.locator('#source'), page.locator('#target'), page)
  await page.waitForTimeout(50)
  await page.close()
  return patches
}

async function malformedPayload() {
  const page = await browser.newPage()
  await page.route('http://kanban.test/**', route => route.fulfill({ status: 200, body: '' }))
  await page.goto('http://kanban.test/')
  let request
  page.on('request', value => { if (value.method() === 'PATCH') request = value })
  const checks = []
  const recorder = probe.createCheckRecorder(checks)
  await recorder.check('malformed_json', async () => {
    await page.evaluate(() => fetch('/api/tickets/id', { method: 'PATCH', body: '{', headers: { 'content-type': 'application/json' } }))
    request.postDataJSON()
  })
  await recorder.check('after_malformed_json', async () => 'Independent check reached')
  await page.close()
  return checks.map(({ id, status }) => ({ id, status }))
}

const checks = []
const recorder = probe.createCheckRecorder(checks)
await recorder.check('drag_failure', async () => { throw new Error('injected failed move') })
await recorder.check('drag_success', async () => 'status-only PATCH observed')
await recorder.check('drag_navigation', async () => 'late move observed')
const rubric = [
  { id: 'criterion_drag_failure', description: 'failure', checks: ['drag_failure'] },
  { id: 'criterion_drag', description: 'success', checks: ['drag_success'] },
  { id: 'criterion_move_navigation', description: 'navigation', checks: ['drag_navigation'] },
]

try {
  console.log(JSON.stringify({
    baseline: await replay('baseline'),
    deepwork: await replay('deepwork'),
    wrongPayload: await replay('baseline', { status: 'done', title: 'changed' }, 1),
    missingDrop: await missingDrop(),
    malformedPayload: await malformedPayload(),
    independent: probe.scoreCriteria(checks, rubric).map(({ id, status }) => ({ id, status })),
  }))
} finally {
  await browser.close()
}
