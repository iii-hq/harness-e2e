import { test, expect } from '@playwright/test';
import { readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';

const feedBytes = readFileSync(process.env.TT_FEED_PATH);
const feed = JSON.parse(feedBytes);
const topics = [...feed.topics].sort((a, b) => a.rank - b.rank);
const normalize = text => text.replace(/\s+/g, ' ').trim();
const titleLink = (page, topic) => page.getByRole('link', { name: topic.title, exact: true });

// Rendered text, including split inline text; hidden or commonly clipped blocks do not count.
function renderedText(node) {
  if (node.nodeType === Node.TEXT_NODE) return Number.parseFloat(getComputedStyle(node.parentElement).fontSize) === 0 ? '' : node.textContent;
  if (!(node instanceof Element)) return '';
  const css = getComputedStyle(node);
  if (css.display === 'none' || css.visibility !== 'visible' || Number(css.opacity) === 0) return '';
  for (let parent = node.parentElement; parent; parent = parent.parentElement) {
    const ancestor = getComputedStyle(parent);
    if (ancestor.display === 'none' || ancestor.visibility !== 'visible' || Number(ancestor.opacity) === 0) return '';
  }
  if (css.clip !== 'auto' && node.clientHeight <= 1) return '';
  if (node.tagName === 'BR') return ' ';
  if ((['hidden', 'clip'].includes(css.overflowY) || Number(css.webkitLineClamp) > 0)
      && node.scrollHeight > node.clientHeight + 1) return '';
  const text = [...node.childNodes].map(renderedText).join('');
  return css.display.startsWith('inline') ? text : ` ${text} `;
}

async function visibleLabel(locator, text) {
  await expect(locator).toBeVisible();
  await expect.poll(async () => normalize(await locator.evaluate(renderedText))).toBe(normalize(text));
}

async function article(page, topic) {
  await visibleLabel(page.getByRole('heading', { level: 1, name: topic.title, exact: true }), topic.title);
  await expect.poll(async () => normalize(await page.getByRole('main').evaluate(renderedText)))
    .toContain(normalize(topic.source_body));
}

async function semantics(page, title) {
  await expect(page).toHaveTitle(new RegExp(title.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
  await expect(page.getByRole('main')).toHaveCount(1);
  await expect(page.getByRole('heading', { level: 1 })).toHaveCount(1);
  await visibleLabel(page.getByRole('heading', { level: 1, name: title, exact: true }), title);
}

async function focusStyle(link) {
  return link.evaluate(node => {
    const properties = ['outline', 'outlineOffset', 'boxShadow', 'backgroundColor', 'backgroundImage',
      'borderColor', 'borderWidth', 'color', 'textDecoration', 'opacity', 'transform', 'content'];
    const styles = [];
    for (let current = node; current; current = current.parentElement) {
      if (current !== node && current.querySelectorAll('a[href],button,input,select,textarea').length > 1) break;
      for (const pseudo of [null, '::before', '::after']) {
        const css = getComputedStyle(current, pseudo);
        styles.push(properties.map(property => css[property]));
      }
    }
    return styles;
  });
}

async function keyboardFocus(page, link, name) {
  await expect(link).toHaveCount(1);
  await visibleLabel(link, name);
  const before = await focusStyle(link);
  const limit = await page.locator('a[href],button,input,select,textarea,[tabindex],[contenteditable]').count() + 2;
  for (let step = 0; step < limit; step++) {
    await page.keyboard.press('Tab');
    if (await link.evaluate(node => document.activeElement === node)) break;
  }
  await expect(link).toBeFocused();
  // ponytail: recognizes common CSS focus indicators, not contrast or arbitrary visual effects.
  await expect.poll(() => focusStyle(link)).not.toEqual(before);
}

async function layout(page, links) {
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth))
    .toBeLessThanOrEqual(1);
  for (const link of links) {
    await expect(link).toBeVisible();
    await link.scrollIntoViewIfNeeded();
    const box = await link.boundingBox();
    expect(box.x).toBeGreaterThanOrEqual(-1);
    expect(box.x + box.width).toBeLessThanOrEqual(page.viewportSize().width + 1);
    await link.click({ trial: true });
  }
}

test.beforeEach(async ({ context }) => {
  // Deterministic link checks; this is not the model/runtime security boundary.
  await context.route('**/*', route => new URL(route.request().url()).origin === new URL(process.env.TT_BASE_URL).origin
    ? route.continue() : route.abort());
});

test('B03 home contains the edition and exactly six associated titles and ranks', async ({ page }) => {
  await page.goto('/');
  await visibleLabel(page.getByRole('heading', { level: 1, name: 'Trending topics', exact: true }), 'Trending topics');
  await expect.poll(async () => normalize(await page.getByRole('main').evaluate(renderedText))).toContain(feed.edition);
  const paths = await page.getByRole('link').evaluateAll(nodes => nodes.map(node => new URL(node.href).pathname)
    .filter(path => path.startsWith('/posts/')));
  expect(paths.sort()).toEqual(topics.map(topic => `/posts/${topic.id}`).sort());
  for (const topic of topics) {
    const link = titleLink(page, topic);
    await expect(link).toHaveCount(1);
    await visibleLabel(link, topic.title);
    expect(await link.evaluate(node => node.tagName)).toBe('A');
    expect(await link.evaluate(node => node.href)).toBe(new URL(`/posts/${topic.id}`, process.env.TT_BASE_URL).href);
    const rankVisible = await link.evaluate((anchor, rank) => {
      for (let node = anchor.parentElement; node; node = node.parentElement) {
        const links = [...node.querySelectorAll('a[href]')].filter(link => new URL(link.href).pathname.startsWith('/posts/'));
        if (links.length !== 1) break;
        const walker = document.createTreeWalker(node, NodeFilter.SHOW_TEXT);
        const parts = [];
        while (walker.nextNode()) {
          const text = walker.currentNode;
          if (anchor.contains(text)) continue;
          let visible = Number.parseFloat(getComputedStyle(text.parentElement).fontSize) > 0;
          for (let parent = text.parentElement; parent; parent = parent.parentElement) {
            const css = getComputedStyle(parent);
            if (css.display === 'none' || css.visibility !== 'visible' || Number(css.opacity) === 0) visible = false;
            if (css.clip !== 'auto' && parent.clientHeight <= 1) visible = false;
            if (['hidden', 'clip'].includes(css.overflowY) && parent.scrollHeight > parent.clientHeight + 1) visible = false;
          }
          if (visible) parts.push(text.textContent);
        }
        if (new RegExp(`(^|[^\\d])0*${rank}([^\\d]|$)`).test(parts.join(' '))) return true;
        const marker = getComputedStyle(node, '::marker');
        if (node.tagName === 'LI' && node.parentElement.tagName === 'OL'
            && getComputedStyle(node).display === 'list-item' && getComputedStyle(node).listStyleType !== 'none'
            && marker.content === 'normal' && Number.parseFloat(marker.fontSize) > 0
            && marker.color !== 'rgba(0, 0, 0, 0)' && marker.visibility === 'visible') {
          const list = node.parentElement;
          const items = [...list.children].filter(item => item.tagName === 'LI');
          let value = list.hasAttribute('start') ? list.start : list.reversed ? items.length : 1;
          for (const item of items) {
            if (item.hasAttribute('value')) value = item.value;
            if (item === node) return value === rank;
            value += list.reversed ? -1 : 1;
          }
        }
      }
      return false;
    }, topic.rank);
    expect(rankVisible, `visible rank ${topic.rank} associated with ${topic.id}`).toBe(true);
  }
});

test('B04 home follows ascending rank in DOM and visual reading order', async ({ page }) => {
  await page.goto('/');
  const paths = await page.getByRole('link').evaluateAll(nodes => nodes.map(node => new URL(node.href).pathname)
    .filter(path => path.startsWith('/posts/')));
  expect(paths).toEqual(topics.map(topic => `/posts/${topic.id}`));
  const boxes = [];
  for (const topic of topics) {
    await expect(titleLink(page, topic)).toBeVisible();
    boxes.push(await titleLink(page, topic).boundingBox());
  }
  for (let index = 1; index < boxes.length; index++) {
    const previous = boxes[index - 1];
    const current = boxes[index];
    const sameRow = current.y < previous.y + previous.height && previous.y < current.y + current.height;
    if (sameRow) expect(current.x, 'visual columns ascend within a row').toBeGreaterThan(previous.x);
    else expect(current.y, 'visual rows ascend').toBeGreaterThan(previous.y);
  }
});

test('B09 home semantics', async ({ page }) => {
  await page.goto('/');
  await semantics(page, 'Trending topics');
});

test('B10 home has no horizontal overflow or obstructed topic links', async ({ page }) => {
  await page.goto('/');
  await layout(page, topics.map(topic => titleLink(page, topic)));
});

for (const topic of topics) {
  test(`B05 ${topic.id} has its full visible article`, async ({ page }) => {
    await page.goto('/');
    await titleLink(page, topic).click();
    await article(page, topic);
  });

  test(`B06 ${topic.id} supports links and browser Back`, async ({ page }) => {
    await page.goto('/');
    await titleLink(page, topic).click();
    await expect(page).toHaveURL(`/posts/${topic.id}`);
    await visibleLabel(page.getByRole('heading', { level: 1, name: topic.title, exact: true }), topic.title);
    const back = page.getByRole('link', { name: 'All topics', exact: true });
    await visibleLabel(back, 'All topics');
    await back.click();
    await expect(page).toHaveURL('/');
    await titleLink(page, topic).click();
    await page.goBack();
    await expect(page).toHaveURL('/');
    await expect(titleLink(page, topic)).toBeVisible();
  });

  test(`B07 ${topic.id} supports direct entry and reload`, async ({ page }) => {
    const response = await page.goto(`/posts/${topic.id}`);
    expect(response.ok()).toBe(true);
    await article(page, topic);
    const reloaded = await page.reload();
    expect(reloaded.ok()).toBe(true);
    await article(page, topic);
  });

  test(`B08 ${topic.id} exposes the supplied source link`, async ({ page }) => {
    await page.goto(`/posts/${topic.id}`);
    const source = page.getByRole('link', { name: 'Source', exact: true });
    await expect(source).toHaveCount(1);
    await visibleLabel(source, 'Source');
    expect(await source.evaluate(node => node.tagName)).toBe('A');
    await expect(source).toHaveAttribute('href', topic.url);
  });

  test(`B09 ${topic.id} semantics and keyboard navigation with visible focus`, async ({ page, context }) => {
    await page.goto('/');
    await keyboardFocus(page, titleLink(page, topic), topic.title);
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(`/posts/${topic.id}`);
    await semantics(page, topic.title);
    const back = page.getByRole('link', { name: 'All topics', exact: true });
    await expect(back).toHaveAttribute('href', '/');
    await keyboardFocus(page, back, 'All topics');
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL('/');
    await page.goto(`/posts/${topic.id}`);
    await keyboardFocus(page, page.getByRole('link', { name: 'Source', exact: true }), 'Source');
    const sourceUrl = new URL(topic.url);
    sourceUrl.hash = '';
    const request = context.waitForEvent('request', request => request.url() === sourceUrl.href);
    await page.keyboard.press('Enter');
    await request;
  });

  test(`B10 ${topic.id} has no overflow or obstructed article links`, async ({ page }) => {
    await page.goto(`/posts/${topic.id}`);
    await layout(page, ['All topics', 'Source'].map(name => page.getByRole('link', { name, exact: true })));
  });
}

test('evidence captures home and first article through a real click', async ({ page, browser }, info) => {
  const records = [];
  await page.goto('/');
  for (const view of ['home', 'article']) {
    if (view === 'article') {
      await titleLink(page, topics[0]).click();
      await expect(page).toHaveURL(`/posts/${topics[0].id}`);
      await article(page, topics[0]);
    }
    const path = info.outputPath(`${process.env.TT_DATASET}-${view}.png`);
    const bytes = await page.screenshot({ path, fullPage: true });
    await info.attach(view, { path, contentType: 'image/png' });
    records.push({ view, dataset: process.env.TT_DATASET, commit: process.env.TT_COMMIT_SHA,
      feed_sha256: createHash('sha256').update(feedBytes).digest('hex'), url: page.url(),
      viewport: page.viewportSize(), browser: browser.version(),
      screenshot: path, screenshot_sha256: createHash('sha256').update(bytes).digest('hex') });
  }
  const path = info.outputPath('evidence.json');
  writeFileSync(path, `${JSON.stringify(records, null, 2)}\n`);
  await info.attach('evidence', { path, contentType: 'application/json' });
});
