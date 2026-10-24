import crypto from 'node:crypto'
import http from 'node:http'

const frame = text => {
  const payload = Buffer.from(text)
  const header = payload.length < 126
    ? Buffer.from([0x81, payload.length])
    : Buffer.from([0x81, 126, payload.length >> 8, payload.length & 0xff])
  return Buffer.concat([header, payload])
}

export const startWebSocketServer = async reply => {
  const server = http.createServer()
  const sockets = new Set()
  server.on('connection', socket => {
    sockets.add(socket)
    socket.on('close', () => sockets.delete(socket))
  })
  server.on('upgrade', (request, socket) => {
    const accept = crypto.createHash('sha1')
      .update(`${request.headers['sec-websocket-key']}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`)
      .digest('base64')
    socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`)
    let pending = Buffer.alloc(0)
    socket.on('data', chunk => {
      pending = Buffer.concat([pending, chunk])
      while (pending.length >= 6) {
        const lengthByte = pending[1] & 0x7f
        const offset = lengthByte === 126 ? 4 : 2
        const length = lengthByte === 126 ? pending.readUInt16BE(2) : lengthByte
        if (pending.length < offset + 4 + length) break
        const mask = pending.subarray(offset, offset + 4)
        const payload = pending.subarray(offset + 4, offset + 4 + length)
        const text = Buffer.from(payload.map((byte, index) => byte ^ mask[index % 4])).toString()
        pending = pending.subarray(offset + 4 + length)
        try {
          const response = reply(JSON.parse(text))
          if (response) socket.write(frame(JSON.stringify(response)))
        } catch {}
      }
    })
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  return {
    url: `ws://127.0.0.1:${server.address().port}`,
    close: () => new Promise(resolve => {
      server.close(resolve)
      sockets.forEach(socket => socket.destroy())
      server.closeAllConnections()
    }),
  }
}
