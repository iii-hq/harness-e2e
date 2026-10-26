import assert from 'node:assert/strict';
import http from 'node:http';
import { once } from 'node:events';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { comparisonFailure } = require('./validate-feature.cjs');
const { chromium } = (await import(process.env.PLAYWRIGHT_MODULE_PATH)).default;
const requests = [];
const server = http.createServer(async (request, response) => {
  const url = new URL(request.url, 'http://fixture');
  if (url.pathname.startsWith('/w/')) {
    requests.push(url.pathname);
    response.writeHead(503, { 'content-type': 'application/json', 'cache-control': 'no-store' });
    response.end('{"error":{"code":"fixture_comparison_failure"}}');
    return;
  }
  const mode = url.pathname.split('/')[1];
  const negative = url.searchParams.get('negative') || '';
  const from = url.searchParams.get('from') || '1.0.0';
  const to = url.searchParams.get('to') || '2.0.0';
  const failing = from === '1.1.0' || to === '1.1.0';
  let result = '<p>orders::get</p>';
  if (mode === 'ssr' && failing) {
    const fault = await fetch(`http://127.0.0.1:${server.address().port}/w/orders-worker/compare/${from}...${to}`);
    assert.equal(fault.status, 503);
    result = negative === 'missing-alert' ? '<p>Failed</p>' : '<p role="alert">Comparison unavailable</p>';
    if (negative === 'stale') result += '<p>orders::get</p>';
  }
  response.setHeader('content-type', 'text/html');
  response.end(`<!doctype html><select id="from">${['1.0.0','1.1.0','2.0.0'].map(v => `<option ${v === from ? 'selected' : ''}>${v}</option>`).join('')}</select><select id="to">${['1.0.0','1.1.0','2.0.0'].map(v => `<option ${v === to ? 'selected' : ''}>${v}</option>`).join('')}</select><main>${result}</main><script>
    const negative = ${JSON.stringify(negative)};
    const mode = ${JSON.stringify(mode)};
    async function compare() {
      const url = new URL(location.href);
      url.searchParams.set('from', document.querySelector('#from').value);
      url.searchParams.set('to', document.querySelector('#to').value);
      if (mode === 'ssr') { location.href = url; return; }
      history.pushState({}, '', url);
      const response = await fetch('/w/orders-worker/compare/' + url.searchParams.get('from') + '...' + url.searchParams.get('to'));
      if (!response.ok) document.querySelector('main').innerHTML = (negative === 'stale' ? '<p>orders::get</p>' : '') + (negative === 'missing-alert' ? '<p>Failed</p>' : '<p role="alert">Comparison unavailable</p>');
    }
    document.querySelector('#to').onchange = compare;
    if (mode === 'spa' && ${failing}) compare();
  </script>`);
});
server.listen(0, '127.0.0.1');
await once(server, 'listening');
const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH } : {}) });
const summary = {};
try {
  for (const mode of ['ssr', 'spa']) {
    const page = await browser.newPage();
    for (const negative of ['', 'stale', 'missing-alert']) {
      const key = `${mode}:${negative || 'correct'}`;
      summary[key] = 0;
      await page.route('**/workers/orders-worker?*', route => {
        const url = new URL(route.request().url());
        url.pathname = `/${mode}/workers/orders-worker`;
        if (negative) url.searchParams.set('negative', negative);
        return route.continue({ url: url.href });
      });
      for (let replay = 0; replay < 100; replay++) {
        for (const path of ['navigation', 'interaction']) {
          const result = await comparisonFailure(page, `http://127.0.0.1:${server.address().port}`, path, negative ? 150 : 3000);
          assert.equal(result.precondition, true, JSON.stringify(result));
          assert.equal(result.attempted, true, JSON.stringify(result));
          assert.equal(result.passed, !negative, JSON.stringify(result));
        }
        summary[key]++;
      }
      await page.unroute('**/workers/orders-worker?*');
    }
    await page.close();
  }
  assert(requests.some(path => path.endsWith('1.1.0...2.0.0')));
  assert(requests.some(path => path.endsWith('1.0.0...1.1.0')));
  console.log(JSON.stringify({ replays: summary, actual503Responses: requests.length }));
} finally {
  await browser.close();
  server.closeAllConnections();
  await new Promise(resolve => server.close(resolve));
}
