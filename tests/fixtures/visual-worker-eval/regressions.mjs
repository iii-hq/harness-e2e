import fs from 'node:fs'
import { startWebSocketServer } from './websocket-server.mjs'

const playwright = (await import(process.env.PLAYWRIGHT_MODULE_PATH)).default

const source = fs.readFileSync(new URL('../../../src/scenarios/visual_worker_build.rs', import.meta.url), 'utf8')
const rawStrings = [...source.matchAll(/r#"([\s\S]*?)"#/g)].map(match => match[1])
const themeProbe = rawStrings.find(code => code.includes('const settle=async()=>'))
const graphProbe = rawStrings.find(code => code.includes('const labels=graph?'))
const trackerProbe = rawStrings.find(code => code.includes('window.__harnessRpc={{requests:[],results:[],invalid:0,sequence:0}}'))
const rpcServer = await startWebSocketServer(message => {
  if (message.function_id === 'control::inbound-request') return {
    type: 'invokefunction', invocation_id: `inbound-${message.invocation_id}`, function_id: 'visual-worker::preview', data: { edit: 'fake' },
  }
  if (message.type !== 'invokefunction') return null
  const result = message.function_id === 'visual-worker::canvas'
    ? { canvas_id: 'canvas-one', source: 'edited' }
    : { visible_fields: ['environment'] }
  return { type: 'invocationresult', invocation_id: message.invocation_id, function_id: message.function_id, result }
})
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
  const functionId = 'visual-worker::preview'
  const canvasFunction = 'visual-worker::canvas'
  const trackerCode = trackerProbe
    .replace('const expected=new Set([{},{}]),', `const expected=new Set([${JSON.stringify(functionId)},${JSON.stringify(canvasFunction)}]),`)
    .replaceAll('{{', '{').replaceAll('}}', '}')
  await page.evaluate(`(()=>{${trackerCode}})()`)
  const tracked = await page.evaluate(async ({ functionId, canvasFunction, url }) => {
    const socket = new WebSocket(url)
    await new Promise((resolve, reject) => { socket.addEventListener('open', resolve, { once: true }); socket.addEventListener('error', reject, { once: true }) })
    const exchange = message => new Promise((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error(`RPC timeout: ${message.invocation_id}`)), 1000)
      const receive = event => {
        const response = JSON.parse(event.data)
        if (response.invocation_id !== message.invocation_id) return
        clearTimeout(timeout); socket.removeEventListener('message', receive); resolve(response)
      }
      socket.addEventListener('message', receive)
      socket.send(JSON.stringify(message))
    })
    const synthetic = new MessageEvent('message', { data: JSON.stringify({ type: 'invocationresult', invocation_id: 'synthetic', function_id: functionId, result: { visible_fields: ['environment'] } }) })
    socket.dispatchEvent(synthetic); void synthetic.data
    for (let replay = 0; replay < 100; replay++) {
      const beforeId = `before-${replay}`, domainId = `domain-${replay}`, canvasId = `canvas-${replay}`, afterId = `after-${replay}`
      socket.send('not json')
      socket.send(JSON.stringify({ type: 'invocationresult', invocation_id: `outbound-${replay}`, function_id: functionId, result: { visible_fields: ['environment'] } }))
      const inbound = new Promise(resolve => socket.addEventListener('message', event => { void event.data; resolve() }, { once: true }))
      socket.send(JSON.stringify({ type: 'invokefunction', invocation_id: `control-${replay}`, function_id: 'control::inbound-request', data: {} }))
      await inbound
      await exchange({ type: 'invokefunction', invocation_id: beforeId, function_id: functionId, data: { values: { work_type: 'bug' } } })
      await exchange({ type: 'invokefunction', invocation_id: domainId, function_id: functionId, data: { edit: 'add_environment' } })
      await exchange({ type: 'invokefunction', invocation_id: canvasId, function_id: canvasFunction, data: { canvas_id: 'canvas-one', edit: 'add_environment' } })
      await exchange({ type: 'invokefunction', invocation_id: `ignored-${replay}`, function_id: 'other::function', data: {} })
      await exchange({ type: 'invokefunction', invocation_id: afterId, function_id: functionId, data: { values: { work_type: 'bug' } } })
    }
    socket.close()
    return { rpc: window.__harnessRpc, controls: { outboundFakeResults: 100, inboundFakeRequests: 100 } }
  }, { functionId, canvasFunction, url: rpcServer.url })
  const { rpc, controls } = tracked
  console.log(JSON.stringify({ oldTheme, stableTheme, hiddenLabel: await graphResult('hidden'), coveredLabel: await graphResult('covered'), sameColorLabel: await graphResult('sameColor'), transparentAncestorLabel: await graphResult('transparentAncestor'), visibleLabel: await graphResult('visible'), rpc, controls }))
} finally {
  await browser.close()
  await rpcServer.close()
}
