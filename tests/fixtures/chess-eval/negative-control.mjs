import fs from 'node:fs'

const playwright = (await import(process.env.PLAYWRIGHT_MODULE_PATH)).default

const source = fs.readFileSync(new URL('../../../src/scenarios/chess_engine_build.rs', import.meta.url), 'utf8')
const tail = source.slice(source.indexOf('async fn inspect_ui('))
const inspect = tail.match(/r#"([\s\S]*?)"#/)[1]
const start = 'rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1'
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
  const invisible = await run()
  await page.locator('[data-piece]').evaluateAll(elements => elements.forEach(element => { element.style.color = '#111' }))
  const visible = await run()
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
  console.log(JSON.stringify({ invisible, visible, variedPieceStyle, clipped, identicalBackgrounds, mixedGradients, sameColorGradients, transparentGradients, brokenBackgrounds, svgPieces, brokenImages, visibleImages, transparentImages }))
} finally {
  await browser.close()
}
