import fs from 'node:fs'
import { startWebSocketServer } from '../visual-worker-eval/websocket-server.mjs'

const playwright = (await import(process.env.PLAYWRIGHT_MODULE_PATH)).default

const source = fs.readFileSync(new URL('../../../src/scenarios/chess_engine_build.rs', import.meta.url), 'utf8')
const tail = source.slice(source.indexOf('async fn inspect_ui('))
const inspect = tail.match(/r#"([\s\S]*?)"#/)[1]
const trackerProbe = [...source.matchAll(/r#"([\s\S]*?)"#/g)]
  .map(match => match[1])
  .find(code => code.includes('window.__harnessRpc={{requests:[],results:[],invalid:0}}'))
const start = 'rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1'
const rpcServer = await startWebSocketServer(message => {
  if (message.function_id === 'control::inbound-request') return {
    type: 'invokefunction', invocation_id: `inbound-${message.invocation_id}`, function_id: 'chess-engine::play', data: { fen: start, move: 'e2e4' },
  }
  return message.type === 'invokefunction' ? {
    type: 'invocationresult', invocation_id: message.invocation_id, function_id: message.function_id, result: { fen: start },
  } : null
})
const browser = await playwright.chromium.launch({ ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH } : {}), headless: true })
const page = await browser.newPage({ viewport: { width: 1280, height: 900 } })

try {
  await page.setContent('<style>body{margin:0}#board{display:grid;grid-template-columns:repeat(8,40px);width:320px}[data-square]{width:40px;height:40px;border:0;background:#ddd;color:#ddd}</style><div id="board"></div><p data-testid="fen"></p>')
  await page.evaluate(start => {
    const pieces = {}
    start.split(' ')[0].split('/').forEach((rank, index) => {
      let file = 0
      for (const token of rank) {
        if (/\d/.test(token)) file += Number(token)
        else { pieces['abcdefgh'[file] + String(8 - index)] = token; file++ }
      }
    })
    for (const file of 'abcdefgh') for (const rank of '12345678') {
      const square = file + rank
      const element = document.createElement('button')
      element.dataset.square = square
      if (pieces[square]) { element.dataset.piece = pieces[square]; element.textContent = pieces[square] }
      document.querySelector('#board').append(element)
    }
    document.querySelector('[data-testid="fen"]').textContent = start
  }, start)
  const run = () => page.evaluate(`(async()=>{const expected=${JSON.stringify(start)};const afterMove=false;${inspect}})()`)
  const invisibleReplays = []
  for (let replay = 0; replay < 100; replay++) invisibleReplays.push(await run())
  const invisible = invisibleReplays[0]
  await page.locator('[data-piece]').evaluateAll(elements => elements.forEach(element => { element.style.color = '#111' }))
  const visibleReplays = []
  for (let replay = 0; replay < 100; replay++) visibleReplays.push(await run())
  const visible = visibleReplays[0]
  await page.locator('[data-square="e2"]').evaluate(element => { element.style.color = '#555' })
  const variedPieceStyle = await run()
  await page.locator('[data-square="e2"]').evaluate(element => {
    element.style.position = 'relative'; element.style.overflow = 'hidden'; element.textContent = ''
    const glyph = document.createElement('span'); glyph.textContent = 'P'; glyph.style.position = 'absolute'; glyph.style.left = '80px'; element.append(glyph)
  })
  const clipped = await run()
  await page.locator('[data-piece]').evaluateAll(elements => elements.forEach(element => {
    element.textContent = ''; element.style.backgroundImage = 'linear-gradient(#111,#111)'; element.style.overflow = ''; element.style.position = ''
  }))
  const identicalBackgrounds = await run()
  await page.locator('[data-piece]').evaluateAll(elements => {
    const codes = [...new Set(elements.map(element => element.dataset.piece))]
    const colors = ['#000000','#300000','#003000','#000030','#303000','#300030','#003030','#501000','#105000','#001050','#505000','#500050']
    elements.forEach(element => { element.style.backgroundImage = `linear-gradient(transparent,${colors[codes.indexOf(element.dataset.piece)]})` })
  })
  const mixedGradients = await run()
  await page.locator('[data-piece]').evaluateAll(elements => elements.forEach(element => { element.style.backgroundImage = 'linear-gradient(#ddd,#ddd)' }))
  const sameColorGradients = await run()
  await page.locator('[data-piece]').evaluateAll(elements => elements.forEach(element => { element.style.backgroundImage = 'linear-gradient(oklch(0 0 0 / 0),oklch(0 0 0 / 0))' }))
  const transparentGradients = await run()
  await page.locator('[data-piece]').evaluateAll(elements => elements.forEach(element => { element.style.backgroundImage = 'url(/missing-piece-background.png)' }))
  const brokenBackgrounds = await run()
  await page.locator('[data-piece]').evaluateAll(elements => elements.forEach(element => {
    element.style.backgroundImage = ''
    const codes = 'PNBRQKpnbrqk', index = codes.indexOf(element.dataset.piece)
    const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg'); svg.setAttribute('width', '24'); svg.setAttribute('height', '24'); svg.setAttribute('viewBox', '0 0 24 24')
    const path = document.createElementNS('http://www.w3.org/2000/svg', 'path'); path.setAttribute('d', `M2 2h${8 + index}v20H2z`); path.setAttribute('fill', '#111'); svg.append(path); element.append(svg)
  }))
  const svgPieces = await run()
  await page.locator('[data-piece]').evaluateAll(elements => elements.forEach(element => {
    element.textContent = ''; const image = document.createElement('img'); image.src = '/missing-piece.png'; element.append(image)
  }))
  await page.waitForTimeout(50)
  const brokenImages = await run()
  await page.locator('[data-piece]').evaluateAll(async elements => {
    const codes = 'PNBRQKpnbrqk'
    for (const element of elements) {
      element.textContent = ''; const index = codes.indexOf(element.dataset.piece), color = `rgb(${20 + index * 15},0,0)`
      const image = document.createElement('img'); image.src = `data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="${8 + index}" height="20" fill="${color}"/></svg>`)}`; element.append(image); await image.decode()
    }
  })
  const visibleImages = await run()
  await page.locator('[data-piece]').evaluateAll(async elements => {
    for (const element of elements) { element.textContent = ''; const image = document.createElement('img'); image.src = `data:image/svg+xml,${encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"></svg>')}`; element.append(image); await image.decode() }
  })
  const transparentImages = await run()
  const rpcPage = await browser.newPage()
  await rpcPage.setContent('<main>rpc</main>')
  const functionId = 'chess-engine::play'
  const trackerCode = trackerProbe
    .replace('{encoded_play_function}', JSON.stringify(functionId))
    .replaceAll('{{', '{').replaceAll('}}', '}')
  await rpcPage.evaluate(`(()=>{${trackerCode}})()`)
  const tracked = await rpcPage.evaluate(async ({ functionId, url }) => {
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
    for (let replay = 0; replay < 100; replay++) {
      const synthetic = new MessageEvent('message', { data: JSON.stringify({ type: 'invocationresult', invocation_id: `synthetic-${replay}`, function_id: functionId, result: { fen: 'canned' } }) })
      socket.dispatchEvent(synthetic); void synthetic.data
      socket.send(JSON.stringify({ type: 'invocationresult', invocation_id: `outbound-${replay}`, function_id: functionId, result: { fen: 'canned' } }))
      const inbound = new Promise(resolve => socket.addEventListener('message', event => { void event.data; resolve() }, { once: true }))
      socket.send(JSON.stringify({ type: 'invokefunction', invocation_id: `control-${replay}`, function_id: 'control::inbound-request', data: {} }))
      await inbound
      await exchange({ type: 'invokefunction', invocation_id: `play-${replay}`, function_id: functionId, data: { fen: 'before', move: 'e2e4' } })
    }
    socket.close()
    return { rpc: window.__harnessRpc, controls: { outboundFakeResults: 100, inboundFakeRequests: 100 } }
  }, { functionId, url: rpcServer.url })
  console.log(JSON.stringify({ invisible, visible, invisibleReplays: invisibleReplays.length, invisibleStable: invisibleReplays.every(result => !result.passed), visibleReplays: visibleReplays.length, visibleStable: visibleReplays.every(result => result.passed), variedPieceStyle, clipped, identicalBackgrounds, mixedGradients, sameColorGradients, transparentGradients, brokenBackgrounds, svgPieces, brokenImages, visibleImages, transparentImages, rpc: tracked.rpc, controls: tracked.controls }))
} finally {
  await browser.close()
  await rpcServer.close()
}
