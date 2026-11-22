import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const modulePath = process.env.PLAYWRIGHT_MODULE_PATH
  ?? new URL('../../../dashboard/node_modules/playwright/index.mjs', import.meta.url).href;
const { chromium } = await import(modulePath.startsWith('file:') ? modulePath : pathToFileURL(modulePath).href);
const source = readFileSync(process.env.TRENDING_FOCUS_SOURCE ?? new URL('./acceptance.spec.mjs', import.meta.url), 'utf8');
const code = source.slice(source.indexOf('async function focusStyle('), source.indexOf('\nasync function keyboardFocus('));
const focusStyle = new Function(`return (${code})`)();
const browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
const page = await browser.newPage();
const results = {};
try {
  for (const [name, css] of Object.entries({
    outline: 'outline: 3px solid #d14d2c',
    background: 'background: #111; color: white',
    translucent: 'outline: 3px solid rgba(0,0,0,.65)',
    faded: 'outline: 3px solid #111; opacity: .1',
    transparent: 'outline: 4px solid transparent; transform: translateX(.0001px)',
    lowContrast: 'outline: 4px solid #f4f1e8',
    imperceptible: 'transform: translateX(.0001px)',
    // A dark pill: a ring outside it sits on the page, an inset or pseudo-element ring on the pill.
    pillOutline: 'outline: 3px solid #1d4ed8; outline-offset: 3px',
    pillShadow: 'box-shadow: 0 0 0 3px #1d4ed8',
    pillInset: 'outline: 3px solid #f4f1e8; outline-offset: -6px',
    pillPageColor: 'outline: 3px solid #f4f1e8; outline-offset: 3px',
    pseudoInsideRing: "content: ''; position: absolute; inset: 3px; box-shadow: 0 0 0 2px #f4f1e8",
    pillInsetShadow: 'box-shadow: inset 0 0 0 3px #f4f1e8',
    pseudoInsideOutline: "content: ''; position: absolute; inset: 4px; outline: 2px solid #f4f1e8",
  })) {
    const pill = /^(pill|pseudo)/.test(name) ? 'background:#17130e;color:#fffdf8;padding:4px;position:relative' : '';
    await page.setContent(`<style>body{background:#f4f1e8}a{color:#185d37;outline:none;${pill}}a:focus-visible${name.startsWith('pseudo') ? '::after' : ''}{${css}}</style><a href="#article">Article</a>`);
    const link = page.getByRole('link', { name: 'Article' });
    const before = await focusStyle(link);
    await page.keyboard.press('Tab');
    const after = await focusStyle(link);
    results[name] = after.some(indicator => !before.some(previous => JSON.stringify(previous) === JSON.stringify(indicator)));
  }
  if (!process.env.TRENDING_FOCUS_SOURCE) assert.deepEqual(results, {
    outline: true, background: true, translucent: true, faded: false, transparent: false, lowContrast: false, imperceptible: false,
    pillOutline: true, pillShadow: true, pillInset: true, pillPageColor: false, pseudoInsideRing: true,
    pillInsetShadow: true, pseudoInsideOutline: true,
  });
  console.log(JSON.stringify(results));
} finally {
  await browser.close();
}
