/*
 * Executor-owned browser evidence for the Registry version comparison feature.
 *
 * Run this file through `docker compose exec -T web node - < capture.cjs`.
 * `@playwright/test` must be installed in /app/app and E2E_APP_URL must name
 * the reachable Registry frontend. This is a capture helper, not a test oracle:
 * it records unavailable states instead of inventing screenshots or scores.
 *
 * The routes and labels below are deliberately limited to the public scenario
 * contract. If the implemented UI uses different accessible names, the capture
 * is unavailable and its reason is retained in captures.json for review.
 */

const fs = require('node:fs/promises');
const path = require('node:path');
const { chromium } = require('@playwright/test');

const outputDirectory = '/tmp/registry-comparison-evidence';
const appUrl = process.env.E2E_APP_URL;
const workerPath = '/workers/orders-worker';
const viewport = { width: 1440, height: 1000 };
const timeoutMs = 20_000;

if (!appUrl) {
  throw new Error('E2E_APP_URL is required');
}

const origin = new URL(appUrl).origin;
const captures = [];

function scenarioUrl(from, to) {
  const url = new URL(workerPath, appUrl);
  url.searchParams.set('tab', 'changelog');
  if (from) url.searchParams.set('from', from);
  if (to) url.searchParams.set('to', to);
  return url.toString();
}

async function firstVisible(locators) {
  for (const locator of locators) {
    if (await locator.count() && await locator.first().isVisible()) return locator.first();
  }
  return null;
}

async function assertChangelogAvailable(page) {
  const control = await firstVisible([
    page.getByRole('tab', { name: /changelog/i }),
    page.getByRole('button', { name: /changelog/i }),
    page.getByRole('link', { name: /changelog/i }),
  ]);
  const content = await firstVisible([
    page.getByRole('heading', { name: /changelog/i }),
    page.getByText(/^changelog$/i),
  ]);

  if (!control && !content) {
    throw new Error('Changelog control or content is not visible');
  }
}

async function open(page, url) {
  await page.goto(url, { waitUntil: 'domcontentloaded', timeout: timeoutMs });
  await page.waitForLoadState('networkidle', { timeout: timeoutMs });
  await assertChangelogAvailable(page);
}

async function capture(page, { id, caption, from, to, beforeCapture }) {
  const url = scenarioUrl(from, to);
  const record = { id, caption, url, status: 'unavailable' };
  try {
    await open(page, url);
    if (beforeCapture) await beforeCapture(page);
    const filename = `${id}.png`;
    await page.screenshot({ path: path.join(outputDirectory, filename), fullPage: true });
    record.status = 'captured';
    record.screenshot = filename;
  } catch (error) {
    record.reason = error instanceof Error ? error.message : String(error);
  }
  captures.push(record);
}

async function expandTimeoutDetail(page) {
  const timeout = await firstVisible([
    page.getByRole('button', { name: /timeout/i }),
    page.locator('summary').filter({ hasText: /timeout/i }),
  ]);
  if (!timeout) throw new Error('Timeout change detail is not visible');

  const expanded = await timeout.getAttribute('aria-expanded');
  const value = page.getByText(/5[,.]?000/).first();
  const valueWasVisible = await value.count() && await value.isVisible();
  await timeout.click({ timeout: timeoutMs });
  if (expanded !== null) {
    await timeout.waitFor({ state: 'visible', timeout: timeoutMs });
    if (await timeout.getAttribute('aria-expanded') !== 'true') {
      throw new Error('Timeout change detail did not expand');
    }
    return;
  }
  if (valueWasVisible) {
    throw new Error('Timeout detail has no observable expanded state');
  }
  await value.waitFor({ state: 'visible', timeout: timeoutMs });
}

async function main() {
  await fs.mkdir(outputDirectory, { recursive: true });
  const browser = await chromium.launch({ headless: true });
  try {
    const context = await browser.newContext({ viewport });
    await context.route('**/*', async route => {
      const requestUrl = new URL(route.request().url());
      if (requestUrl.origin === origin) return route.continue();
      return route.abort('blockedbyclient');
    });
    const page = await context.newPage();

    await capture(page, {
      id: '01-history',
      caption: 'Changelog history for orders-worker',
    });
    await capture(page, {
      id: '02-1.0.0-to-1.1.0',
      caption: 'Comparison from 1.0.0 to 1.1.0',
      from: '1.0.0',
      to: '1.1.0',
    });
    await capture(page, {
      id: '03-1.0.0-to-2.0.0',
      caption: 'Comparison from 1.0.0 to 2.0.0',
      from: '1.0.0',
      to: '2.0.0',
    });
    await capture(page, {
      id: '04-timeout-detail',
      caption: 'Expanded timeout change detail from 1.0.0 to 2.0.0',
      from: '1.0.0',
      to: '2.0.0',
      beforeCapture: expandTimeoutDetail,
    });

    await context.close();
  } finally {
    await browser.close();
    await fs.writeFile(
      path.join(outputDirectory, 'captures.json'),
      `${JSON.stringify({ app_url: appUrl, viewport, captures }, null, 2)}\n`,
    );
  }
}

main().catch(async error => {
  // Startup failures are executor infrastructure failures. Per-capture UI
  // absence is handled above as evidence and intentionally leaves exit code 0.
  await fs.mkdir(outputDirectory, { recursive: true });
  await fs.writeFile(
    path.join(outputDirectory, 'captures.json'),
    `${JSON.stringify({ app_url: appUrl, viewport, captures, infrastructure_error: String(error) }, null, 2)}\n`,
  );
  console.error(error);
  process.exitCode = 1;
});
