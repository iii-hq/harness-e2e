import fs from 'node:fs'

const playwright = (await import(process.env.PLAYWRIGHT_MODULE_PATH)).default

const source = fs.readFileSync(new URL('../../../src/scenarios/visual_worker_build.rs', import.meta.url), 'utf8')
const rawStrings = [...source.matchAll(/r#"([\s\S]*?)"#/g)].map(match => match[1])
const themeProbe = rawStrings.find(code => code.includes('const settle=async()=>'))
const graphProbe = rawStrings.find(code => code.includes('const labels=graph?'))
const trackerProbe = rawStrings.find(code => code.includes('window.__harnessRpc={{requests:[],results:[],invalid:0}}'))
const browser = await playwright.chromium.launch({ ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH } : {}), headless: true })
const page = await browser.newPage({ viewport: { width: 480, height: 900 } })

try {
  const markup = `<!doctype html><html data-theme="light"><head><style>
  :root{color-scheme:light;--panel:#fafafa;--surface:rgba(24,24,24,.055);--ink:#0a0a0a;--faint:#6b6b6b}
  [data-theme="dark"]{color-scheme:dark;--panel:#171717;--surface:rgba(255,255,255,.055);--ink:#ededed;--faint:#a6a6a6}
  .pane{background:var(--panel);color:var(--ink);transition:background-color 160ms cubic-bezier(.2,0,0,1);width:440px}
  [data-testid="domain-result"]{background:var(--surface);color:var(--ink);padding:10px}
  [data-testid="reset"]{background:var(--panel);color:var(--faint);transition:background-color 160ms cubic-bezier(.2,0,0,1),color 160ms cubic-bezier(.2,0,0,1)}
  </style></head><body><main class="pane" data-workspace-pane-id="one"><pre data-testid="domain-result">domain</pre><button data-testid="reset">Reset</button></main></body></html>`
  const runTheme = async code => {
    await page.setContent(markup)
    await page.waitForTimeout(200)
    return page.evaluate(`(async()=>{${code}})()`)
  }
  const oldTheme = await runTheme(themeProbe.replace('await settle();', 'await new Promise(requestAnimationFrame);await new Promise(requestAnimationFrame);'))
  const stableTheme = await runTheme(themeProbe)

  const graphCode = graphProbe
    .replace('const label={};', 'const label="Environment";')
    .replace('for(let i=0;i<200;i++){{', 'for(let i=0;i<1;i++){{')
    .replaceAll('{{', '{').replaceAll('}}', '}')
  const graphResult = async mode => {
    const fill = mode === 'sameColor' ? 'white' : '#111'
    const opacity = mode === 'transparentAncestor' ? 'opacity:0' : ''
    await page.setContent(`<div data-iii-ui="canvas"><div aria-label="diagram preview"><svg width="300" height="100" style="background:white;${opacity}"><text x="20" y="40" fill="${fill}" style="${mode === 'hidden' ? 'display:none' : ''}">Environment</text>${mode === 'covered' ? '<rect x="0" y="0" width="200" height="80" fill="white" />' : ''}</svg></div></div>`)
    return page.evaluate(`(async()=>{${graphCode}})()`)
  }
  await page.setContent('<main>rpc</main>')
  await page.evaluate(() => { window.WebSocket = class extends EventTarget { send() {} } })
  const functionId = 'visual-worker::preview'
  const trackerCode = trackerProbe.replace('const expected={},', `const expected=${JSON.stringify(functionId)},`).replaceAll('{{', '{').replaceAll('}}', '}')
  await page.evaluate(`(()=>{${trackerCode}})()`)
  const rpc = await page.evaluate(functionId => {
    const socket = new WebSocket()
    socket.addEventListener('message', event => event.data)
    socket.send('not json')
    socket.send(JSON.stringify({ type: 'invokefunction', invocation_id: 'one', function_id: functionId, data: { edit: 'add_environment' } }))
    socket.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ type: 'invocationresult', invocation_id: 'one', function_id: functionId, result: { visible_fields: ['environment'] } }) }))
    return window.__harnessRpc
  }, functionId)
  console.log(JSON.stringify({ oldTheme, stableTheme, hiddenLabel: await graphResult('hidden'), coveredLabel: await graphResult('covered'), sameColorLabel: await graphResult('sameColor'), transparentAncestorLabel: await graphResult('transparentAncestor'), visibleLabel: await graphResult('visible'), rpc }))
} finally {
  await browser.close()
}
