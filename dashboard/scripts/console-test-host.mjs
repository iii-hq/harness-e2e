// Functional Console host double. It does not validate the real Console's visuals.

import tailwindcss from '@tailwindcss/vite'
import { createServer } from 'vite'

export async function createConsoleTestHost() {
  const server = await createServer({
    plugins: [tailwindcss()],
    optimizeDeps: { include: ['react', 'react-dom/client', 'lucide-react'] },
    server: { host: '127.0.0.1', port: 0 },
  })
  await server.listen()
  const url = `${server.resolvedUrls.local[0]}__console-test`
  return {
    url,
    close: () => server.close(),
    async install(page, trigger) {
      await page.exposeBinding('__consoleTrigger', (_source, id, payload) =>
        trigger(id, payload),
      )
      const html = `<!doctype html><html lang="en"><head><meta name="viewport" content="width=device-width, initial-scale=1"><title>Functional Console host double</title></head><body><div data-iii-ui="harness-e2e" id="root"></div><script type="module">
        import React from 'react';
        import {createRoot} from 'react-dom/client';
        import setup from '/src/console-entry.tsx';
        window.calls=[];
        const host={
          iii:{browserId:'console-test',on:()=>()=>{},registerTrigger:()=>()=>{},trigger(id,payload){window.calls.push({id,payload});return window.__consoleTrigger(id,payload)}},
          useTheme:()=> window.__consoleTheme ?? 'light',
          pages:{register(page){createRoot(document.getElementById('root')).render(page.render({tabId:'test',panelSide:'left'}));return ()=>{}}}
        };
        setup(host);
      </script></body></html>`
      await page.route('**/__console-test', async (route) =>
        route.fulfill({
          contentType: 'text/html',
          body: await server.transformIndexHtml('/__console-test', html),
        }),
      )
    },
  }
}
