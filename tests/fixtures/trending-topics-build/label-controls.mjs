import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const modulePath = process.env.PLAYWRIGHT_MODULE_PATH
  ?? new URL('../../../dashboard/node_modules/playwright/index.mjs', import.meta.url).href;
const { chromium } = await import(modulePath.startsWith('file:') ? modulePath : pathToFileURL(modulePath).href);
const source = readFileSync(new URL('./acceptance.spec.mjs', import.meta.url), 'utf8');
const code = source.slice(source.indexOf('function renderedText('), source.indexOf('\nasync function visibleLabel('));
const renderedText = new Function(`return (${code})`)();
const normalize = text => text.replace(/\s+/g, ' ').trim();
const browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
const page = await browser.newPage();
const results = {};
try {
  for (const [name, [label, html]] of Object.entries({
    // 0.41.0 r02: required names with a decorative arrow hidden from assistive technology.
    arrowSpan: ['All topics', '<span aria-hidden="true">←</span> All topics'],
    sourceArrow: ['Source', 'Source <span aria-hidden="true">↗</span>'],
    svgArrow: ['All topics', '<svg aria-hidden="true" width="10" height="10"><path d="M0 5h10"/></svg> All topics'],
    blockGlyph: ['All topics', 'All<span aria-hidden="true" style="display:block">—</span>topics'],
    visibleArrow: ['All topics', '← All topics'],
    spanArrow: ['All topics', '<span>←</span> All topics'],
    hiddenCount: ['All topics', 'All topics<span aria-hidden="true"> (12)</span>'],
    hiddenWords: ['All topics', '<span aria-hidden="true">Home</span>', 'All topics'],
    hiddenWrapper: ['All topics', '<span aria-hidden="true">← All topics</span>'],
    iconOnly: ['All topics', '<span aria-hidden="true">←</span>', 'All topics'],
    falseVisible: ['All topics', 'Wrong visible title', 'All topics'],
  }).map(([name, [label, html, ariaLabel]]) => [name, [label, ariaLabel ? `<a href="/" aria-label="${ariaLabel}">${html}</a>` : `<a href="/">${html}</a>`]])) {
    await page.setContent(html);
    // visibleLabel passes true: decorative glyphs inside a link are not part of its label.
    results[name] = normalize(await page.locator('a').evaluate(renderedText, true)) === label;
  }
  assert.deepEqual(results, {
    arrowSpan: true, sourceArrow: true, svgArrow: true, blockGlyph: true,
    visibleArrow: false, spanArrow: false, hiddenCount: false, hiddenWords: false, hiddenWrapper: false, iconOnly: false, falseVisible: false,
  });
  console.log(JSON.stringify(results));
} finally {
  await browser.close();
}
