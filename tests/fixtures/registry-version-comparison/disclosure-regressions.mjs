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
// 0.45.0 r03: a native disclosure whose one label span joins the area and the path.
const details = label => `<!doctype html><main><details><summary><span>review</span><span style="min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${label}</span><span>changed</span></summary><div><div><span>before</span><pre>3000</pre></div><div><span>after</span><pre>5000</pre></div></div></details></main>`;
// 0.46.0 r02: the kind label is an inline element right after the path, so the
// label span's text reads '/timeoutMschanged'.
const inlineKind = (path, style = '') => `<!doctype html><main><ul><li><details><summary><span aria-hidden="true">→</span><span style="${style}">${path}<span style="margin-left:8px">changed</span></span><span>~ review</span><span>show values</span></summary><div><div><p>before</p><pre><code>3000</code></pre></div><div><p>after</p><pre><code>5000</code></pre></div></div></details></li></ul></main>`;
const cases = {
  nested: [page({ layout: 'nested' }), true],
  details_combined_label: [details('configuration /timeoutMs'), true],
  details_nested_path: [details('configuration /retry/timeoutMs'), false],
  details_longer_key: [details('configuration /timeoutMsOverride'), false],
  details_inline_kind: [inlineKind('/timeoutMs'), true],
  details_inline_nested_path: [inlineKind('/retry/timeoutMs'), false],
  details_inline_longer_key: [inlineKind('/timeoutMsOverride'), false],
  // React splits '{parent}/{leaf}' into text nodes; the label still reads as one word.
  details_inline_split_path: [inlineKind('/<!-- -->timeoutMs'), true],
  details_inline_split_nested_path: [inlineKind('/retry<!-- -->/<!-- -->timeoutMs'), false],
  details_inline_wbr_nested_path: [inlineKind('/retry<wbr>/timeoutMs'), false],
  details_inline_element_nested_path: [inlineKind('<span>/retry</span>/timeoutMs'), false],
  details_inline_glued_longer_key: [inlineKind('/timeoutMs<b>Override</b>'), false],
  details_split_suffix: [details('/timeoutMs<!-- --> (changed)'), false],
  details_inline_hidden_label: [inlineKind('/timeoutMs', 'display:none'), false],
  details_inline_invisible_label: [inlineKind('/timeoutMs', 'visibility:hidden'), false],
  details_inline_split_word: [inlineKind('/time<!-- -->outMs'), true],
  details_inline_hidden_separator: [inlineKind('/retry<span style="display:none">x</span>/timeoutMs'), false],
  details_inline_nested_owner: [inlineKind('/retry<span>/timeoutMs<span style="margin-left:8px">changed</span></span>'), false],
  // An invisible tooltip laid out right after the path does not glue a word to it.
  details_inline_invisible_neighbour: [inlineKind('/timeoutMs<span style="position:absolute;opacity:0">copy</span>'), true],
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

const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH } : {}) });
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
