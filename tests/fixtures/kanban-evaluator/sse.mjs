import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { chromium } from '../../../dashboard/node_modules/playwright/index.mjs'
import { PROBES } from '../../../scripts/kanban_eval/probe.mjs'

const browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH })
const results = {}
try {
  for (const variant of ['valid', 'beforeBarrier', 'afterBarrier']) {
    const clients = new Set()
    const original = '/workspace/original'
    let directory = original, corrupt = false, writes = 0, failed = false
    const emit = store => {
      for (const response of clients) response.write(`event: change\ndata: ${JSON.stringify({ store })}\n\n`)
    }
    const server = createServer(async (request, response) => {
      if (request.url === '/api/events') {
        response.writeHead(200, { 'content-type': 'text/event-stream' })
        clients.add(response)
        response.write(`event: change\ndata: ${JSON.stringify({ store: directory })}\n\n`)
        request.on('close', () => clients.delete(response))
      } else if (request.url === '/api/config') {
        if (request.method === 'PUT') {
          let body = ''
          for await (const chunk of request) body += chunk
          directory = JSON.parse(body).data_dir
          writes++
          if (failed && ((variant === 'beforeBarrier' && writes === 1)
            || (variant === 'afterBarrier' && writes === 2))) emit(original)
          emit(directory)
        }
        response.writeHead(200, { 'content-type': 'application/json' })
        response.end(JSON.stringify({ data_dir: directory, resolved_data_dir: directory }))
      } else {
        response.end('<!doctype html><title>SSE control</title>')
      }
    })
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
    const baseUrl = `http://127.0.0.1:${server.address().port}`
    try {
      await PROBES.kanban_c7_live({
        browser, baseUrl, stage() {},
        api: (path, options) => fetch(baseUrl + path, options),
        control: async (operation, payload) => {
          if (operation === 'read_store') return '{}'
          corrupt = payload.value === '{broken json'
        },
        trigger: async () => {
          if (corrupt) { failed = true; throw Error('corrupt store') }
          emit(directory)
          return { id: 'one' }
        },
        check: async (id, action) => {
          if (id !== 'live_sse_protocol_contract') return
          try { await action(); results[variant] = true }
          catch (error) {
            assert.match(error.message, /extra change event/)
            results[variant] = false
          }
        },
      })
    } finally {
      for (const response of clients) response.end()
      await new Promise(resolve => server.close(resolve))
    }
  }
  assert.deepEqual(results, { valid: true, beforeBarrier: false, afterBarrier: false })
  console.log(JSON.stringify(results))
} finally {
  await browser.close()
}
