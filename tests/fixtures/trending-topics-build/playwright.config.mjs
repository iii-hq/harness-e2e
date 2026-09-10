import { defineConfig } from '@playwright/test';
import { isAbsolute, join } from 'node:path';

for (const name of ['TT_BASE_URL', 'TT_FEED_PATH', 'TT_OUTPUT_DIR', 'TT_DATASET', 'TT_COMMIT_SHA']) {
  if (!process.env[name]) throw new Error(`${name} is required`);
}
for (const name of ['TT_FEED_PATH', 'TT_OUTPUT_DIR']) {
  if (!isAbsolute(process.env[name])) throw new Error(`${name} must be absolute`);
}

export default defineConfig({
  testDir: '.',
  testMatch: 'acceptance.spec.mjs',
  workers: 1,
  retries: 0,
  timeout: 30_000,
  expect: { timeout: 3_000 },
  outputDir: join(process.env.TT_OUTPUT_DIR, 'test-results'),
  reporter: [
    ['list'],
    ['json', { outputFile: join(process.env.TT_OUTPUT_DIR, 'results.json') }],
    ['html', { outputFolder: join(process.env.TT_OUTPUT_DIR, 'html'), open: 'never' }],
  ],
  use: {
    browserName: 'chromium',
    baseURL: process.env.TT_BASE_URL,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    actionTimeout: 3_000,
  },
  projects: [
    { name: 'desktop', use: { viewport: { width: 1440, height: 900 } } },
    { name: 'mobile', use: { viewport: { width: 390, height: 844 } } },
  ],
});
