import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { expandedDetail } = require('./validate-feature.cjs');
const { chromium } = (await import(process.env.PLAYWRIGHT_MODULE_PATH)).default;

// One timeout change row whose toggle reveals before/after values, in the two
// valid layouts (values inside the row, or in the region the toggle controls)
// and in the ways a disclosure can look done without showing them.
const SR_ONLY = 'position:absolute;width:1px;height:1px;overflow:hidden;clip:rect(0 0 0 0)';
const values = (style = '', before = '3000', after = '5000') =>
  `<div>BEFORE v1.0.0 <span style="${style}">${before}</span></div><div>AFTER v2.0.0 <span style="${style}">${after}</span></div>`;
const page = ({ layout = 'sibling', inner = values(), open = false, flip = true, reveal = true }) => {
  const toggle = `<button type="button" aria-expanded="${open}" aria-controls="detail">show before and after</button>`;
  const region = `<div id="detail" role="region" ${open ? '' : 'hidden'}>${inner}</div>`;
  const row = `<span>configuration</span> <span>CHANGED</span> <span>/timeoutMs</span> ${toggle}`;
  const body = layout === 'nested'
    ? `<div class="row">${row}${region}</div>`
    : `<div class="change"><div class="row">${row}</div>${region}</div>`;
  return `<!doctype html><main>${body}</main><script>
    document.querySelector('button').addEventListener('click', event => {
      ${flip ? "event.currentTarget.setAttribute('aria-expanded', 'true'); event.currentTarget.textContent = 'hide values';" : ''}
      ${reveal ? "document.getElementById('detail').hidden = false;" : ''}
    });
  </script>`;
};
const cases = {
  nested: [page({ layout: 'nested' }), true],
  controlled_sibling: [page({}), true],
  region_stays_hidden: [page({ reveal: false }), false],
  toggle_does_not_flip: [page({ flip: false }), false],
  already_open: [page({ open: true }), false],
  values_missing: [page({ inner: values('', 'n/a', 'n/a') }), false],
  values_display_none: [page({ inner: values('display:none') }), false],
  values_visibility_hidden: [page({ inner: values('visibility:hidden') }), false],
  values_transparent: [page({ inner: values('opacity:0') }), false],
  values_screen_reader_only: [page({ inner: values(SR_ONLY) }), false],
  nested_screen_reader_only: [page({ layout: 'nested', inner: values(SR_ONLY) }), false],
};

const browser = await chromium.launch({ headless: true });
const results = {};
try {
  for (const [name, [html, expected]] of Object.entries(cases)) {
    const tab = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
    await tab.setContent(html);
    const { expanded, observed } = await expandedDetail(tab);
    results[name] = expanded;
    await tab.close();
    assert.equal(expanded, expected, `${name}: ${JSON.stringify(observed)}`);
  }
} finally {
  await browser.close();
  console.log(JSON.stringify({ disclosure: results }));
}
