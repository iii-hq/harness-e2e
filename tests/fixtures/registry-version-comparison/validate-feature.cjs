/* Deterministic executor probe. Run with: docker compose exec -T web node - < validate-feature.cjs */
const fs = require('node:fs/promises');
const crypto = require('node:crypto');
const { chromium } = require('@playwright/test');
const postgres = require('/app/api/node_modules/postgres');

const api = process.env.API_URL || process.env.TEST_API_URL;
const app = process.env.E2E_APP_URL;
const output = '/tmp/registry-validation';
const observations = [];
const cases = {};
const timeout = 15000;

function observe(id, value, expected, observed) {
  observations.push({ id, status: 'measured', value: value ? 1 : 0, evidence: ['feature.json'] });
  cases[id] = { expected, observed };
}
function unavailable(id, expected, reason) {
  observations.push({ id, status: 'unavailable', reason, evidence: ['feature.json'] });
  cases[id] = { expected, observed: `Unavailable: ${reason}` };
}
async function request(path) {
  const response = await fetch(`${api}${path}`, { signal: AbortSignal.timeout(timeout) });
  const text = await response.text();
  let body;
  try { body = JSON.parse(text); } catch { body = text; }
  return { status: response.status, body };
}
function changes(result) { return Array.isArray(result.body?.changes) ? result.body.changes : []; }
function change(result, predicate) { return changes(result).find(predicate); }
function endpoint(from, to, worker = 'orders-worker') { return `/w/${worker}/compare/${from}...${to}`; }

async function temporaryOrderingProbes() {
  const ids = ['object_order', 'required_order', 'enum_order', 'config_array_order'];
  const versions = ['9.90.0', '9.90.1'];
  const sql = postgres(process.env.DATABASE_URL, { max: 1 });
  try {
    await sql`delete from worker_version where worker_id = '10000000-0000-4000-8000-000000000001' and version in (${versions[0]}, ${versions[1]})`;
    const fromFunctions = [{
      name: 'probe::ordered',
      request_schema: { type: 'object', properties: { customerId: { type: 'string', enum: ['one', 'two'] }, currency: { type: 'string' } }, required: ['customerId', 'currency'] },
      response_schema: { type: 'object', properties: { ok: { type: 'boolean' } }, required: ['ok'] },
    }];
    const baseConfig = { probeSequence: ['alpha', 'beta'], required: ['one', 'two'] };
    for (const version of versions) {
      await sql`
        insert into worker_version
          (id, worker_id, version, license, config, readme, binaries, image_tag,
           dependencies, functions, triggers, promoted_at, tag, created_at)
        select gen_random_uuid(), worker_id, ${version}, license, ${JSON.stringify(baseConfig)}::jsonb,
          '# temporary validation probe', binaries, image_tag, dependencies,
          ${JSON.stringify(fromFunctions)}::jsonb, triggers, null, null, now()
        from worker_version
        where worker_id = '10000000-0000-4000-8000-000000000001' and version = '1.0.0'`;
    }
    // Change only one property of the comparison per observation.
    for (const id of ids) {
      const functions = structuredClone(fromFunctions);
      const config = structuredClone(baseConfig);
      if (id === 'object_order') {
        functions[0].request_schema.properties = { currency: { type: 'string' }, customerId: { enum: ['one', 'two'], type: 'string' } };
      } else if (id === 'required_order') {
        functions[0].request_schema.required.reverse();
      } else if (id === 'enum_order') {
        functions[0].request_schema.properties.customerId.enum.reverse();
      } else {
        config.required.reverse();
      }
      await sql`update worker_version set config = ${JSON.stringify(config)}::jsonb,
        functions = ${JSON.stringify(functions)}::jsonb
        where worker_id = '10000000-0000-4000-8000-000000000001' and version = ${versions[1]}`;
      const result = await request(endpoint(...versions));
      const expected = id === 'config_array_order' ? 'Configuration required-array order produces a change' : `${id} alone creates no change`;
      const correct = id === 'config_array_order'
        ? changes(result).some(c => c.area === 'config' && c.path === '/required' && c.kind === 'changed'
          && JSON.stringify(c.before) === '["one","two"]' && JSON.stringify(c.after) === '["two","one"]')
        : Array.isArray(result.body?.changes) && result.body.changes.length === 0;
      observe(`implementation.${id}`, result.status === 200 && correct, expected, result);
    }
  } catch (error) {
    for (const id of ids) unavailable(`implementation.${id}`, 'Temporary isolated comparison observation', `Temporary probe failed: ${error}`);
  } finally {
    try { await sql`delete from worker_version where worker_id = '10000000-0000-4000-8000-000000000001' and version in (${versions[0]}, ${versions[1]})`; }
    finally { await sql.end({ timeout: 5 }); }
  }
}

async function apiProbes() {
  let health;
  try { health = await request('/health'); } catch (error) {
    const reason = `API unreachable: ${error}`;
    for (const id of [
      'object_order','required_order','enum_order','config_array_order','same_version','function_removal','required_impact','missing_metadata','worker_lookup',
      'reverse_kinds','reverse_values','reverse_impact','invalid_version','missing_worker','missing_version'
    ]) unavailable(`implementation.${id}`, 'Live API observation', reason);
    return false;
  }
  if (health.status !== 200) throw new Error(`API health returned ${health.status}`);

  const [same, forward, reverse, oldMetadata, scoped, invalid, missingWorker, missingVersion] = await Promise.all([
    request(endpoint('1.0.0','1.0.0')), request(endpoint('1.0.0','2.0.0')),
    request(endpoint('2.0.0','1.0.0')), request(endpoint('0.9.0','0.9.0')),
    request(endpoint('1.0.0','9.9.9')), request(endpoint('invalid','2.0.0')),
    request(endpoint('1.0.0','2.0.0','absent-worker')), request(endpoint('8.8.8','7.7.7'))
  ]);
  observe('implementation.same_version', same.status === 200 && Array.isArray(same.body?.changes) && same.body.changes.length === 0, '200 with changes=[]', same);
  const removedGet = change(forward, c => c.area === 'functions' && c.name === 'orders::get' && c.kind === 'removed');
  observe('implementation.function_removal', !!removedGet, 'orders::get removed', removedGet || forward);
  const currency = changes(forward).filter(c => c.area === 'functions' && c.name === 'orders::create' && (String(c.path).includes('currency') || c.path === '/request_schema/required'));
  observe('implementation.required_impact', currency.length === 2 && currency.every(c => c.impact === 'potentially_breaking'), 'Two potentially_breaking currency entries', currency);
  const missing = oldMetadata.body?.unavailable || [];
  observe('implementation.missing_metadata', oldMetadata.status === 200 && missing.some(v => v.area === 'config' && v.reason === 'metadata_missing'), 'config metadata_missing notice', oldMetadata);
  observe('implementation.worker_lookup', scoped.status === 404 && scoped.body?.error?.code === 'version_not_found', 'Scoped version_not_found', scoped);
  const listReverse = change(reverse, c => c.area === 'functions' && c.name === 'orders::list');
  observe('implementation.reverse_kinds', listReverse?.kind === 'removed', 'orders::list removed in reverse', listReverse || reverse);
  const timeoutForward = change(forward, c => c.area === 'config' && String(c.path).includes('timeoutMs'));
  const timeoutReverse = change(reverse, c => c.area === 'config' && String(c.path).includes('timeoutMs'));
  observe('implementation.reverse_values', timeoutForward && timeoutReverse && JSON.stringify(timeoutForward.before) === JSON.stringify(timeoutReverse.after) && JSON.stringify(timeoutForward.after) === JSON.stringify(timeoutReverse.before), 'timeout values swapped', { timeoutForward, timeoutReverse });
  const reverseRequired = change(reverse, c => c.name === 'orders::create' && c.path === '/request_schema/required');
  observe('implementation.reverse_impact', reverseRequired?.impact === 'additive', 'reduced required set additive', reverseRequired || reverse);
  observe('implementation.invalid_version', invalid.status === 400 && invalid.body?.error?.code === 'invalid_version', '400 invalid_version', invalid);
  observe('implementation.missing_worker', missingWorker.status === 404 && missingWorker.body?.error?.code === 'worker_not_found', '404 worker_not_found', missingWorker);
  observe('implementation.missing_version', missingVersion.status === 404 && missingVersion.body?.error?.code === 'version_not_found' && missingVersion.body?.error?.version === '8.8.8', '404 source-first version_not_found', missingVersion);

  await temporaryOrderingProbes();
  return true;
}

async function browserProbes(apiHealthy) {
  const ids = ['shared_url','stale_results','history','expanded_detail','keyboard_selectors','versions_regression','readme_regression','api_reference_regression'];
  if (!apiHealthy) { for (const id of ids) unavailable(`implementation.${id}`, 'Live browser observation', 'API prerequisite unavailable'); return; }
  let browser;
  try { browser = await chromium.launch({ headless: true }); } catch (error) { for (const id of ids) unavailable(`implementation.${id}`, 'Live browser observation', `Browser unavailable: ${error}`); return; }
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
  try {
    const comparisonUrl = `${app}/workers/orders-worker?tab=changelog&from=1.0.0&to=2.0.0`;
    await page.goto(comparisonUrl, { waitUntil: 'networkidle', timeout });
    const selected = await page.locator('select').evaluateAll(nodes => nodes.map(n => n.value));
    const historyText = await page.locator('body').innerText();
    const hasChangelog = /changelog/i.test(historyText);
    observe('implementation.shared_url', hasChangelog && selected.includes('1.0.0') && selected.includes('2.0.0'), 'Selected pair restored', { url: page.url(), selected, hasChangelog });
    observe('implementation.history', hasChangelog && ['0.9.0','1.0.0','1.1.0','2.0.0'].every(v => historyText.includes(v)), 'Four seeded releases visible in Changelog', historyText.slice(0,2000));
    const detail = page.getByRole('button', { name: /timeout/i }).or(page.locator('summary').filter({ hasText: /timeout/i })).first();
    let expanded = false;
    if (await detail.count()) { await detail.click(); const text = await page.locator('body').innerText(); expanded = /3000|3,000/.test(text) && /5000|5,000/.test(text); }
    observe('implementation.expanded_detail', expanded, 'Expanded timeout shows 3000 and 5000', { controlFound: !!(await detail.count()), expanded });
    const selects = page.locator('select');
    let keyboard = false;
    let staleCleared = false;
    if (await selects.count() >= 2) {
      const originalValue = await selects.nth(1).inputValue();
      const selectedIndex = await selects.nth(1).evaluate(node => node.selectedIndex);
      const optionCount = await selects.nth(1).locator('option').count();
      await selects.nth(1).focus();
      await page.keyboard.press(selectedIndex < optionCount - 1 ? 'ArrowDown' : 'ArrowUp');
      await page.keyboard.press('Enter');
      await page.waitForLoadState('networkidle');
      keyboard = (await selects.nth(1).inputValue()) !== originalValue;
      // Restore a successful pair before inducing a failure in the same document.
      await page.goto(comparisonUrl, { waitUntil: 'networkidle', timeout });
      const beforeFailure = await page.locator('body').innerText();
      const compare = page.getByRole('button', { name: /compare/i }).first();
      const submitsComparison = await compare.count() > 0 && await compare.isVisible();
      const sql = postgres(process.env.DATABASE_URL, { max: 1 });
      try {
        const renamed = await sql`update worker_version set version = '9.90.9'
          where worker_id = '10000000-0000-4000-8000-000000000001' and version = '0.9.0'`;
        if (renamed.count !== 1) throw new Error('Expected one temporary stale-state probe version');
        await selects.nth(1).selectOption('0.9.0');
        if (submitsComparison) await compare.click();
        await page.waitForLoadState('networkidle');
        const afterFailure = await page.locator('body').innerText();
        staleCleared = beforeFailure.includes('orders::get') && !afterFailure.includes('orders::get')
          && /error|failed|unable|unavailable|not[ _]found|retry/i.test(afterFailure);
      } finally {
        try {
          await sql`update worker_version set version = '0.9.0'
            where worker_id = '10000000-0000-4000-8000-000000000001' and version = '9.90.9'`;
        } finally { await sql.end({ timeout: 5 }); }
      }
    }
    observe('implementation.keyboard_selectors', keyboard, 'Keyboard input changes the selected version', { selectCount: await selects.count(), keyboard });
    observe('implementation.stale_results', staleCleared, 'Successful comparison is cleared after changing the pair to a missing version', { staleCleared });

    await page.goto(`${app}/workers/orders-worker?tab=versions`, { waitUntil: 'networkidle', timeout });
    const versionsText = await page.locator('body').innerText();
    observe('implementation.versions_regression', ['0.9.0','1.0.0','1.1.0','2.0.0'].every(v => versionsText.includes(v)), 'Versions tab shows seeded releases', versionsText.slice(0,1200));
    await page.goto(`${app}/workers/orders-worker?version=1.0.0`, { waitUntil: 'networkidle', timeout });
    const readmeText = await page.locator('body').innerText();
    observe('implementation.readme_regression', readmeText.includes('orders-worker 1.0.0'), 'README content visible', readmeText.slice(0,1200));
    await page.goto(`${app}/workers/orders-worker?version=1.0.0&tab=api`, { waitUntil: 'networkidle', timeout });
    const apiText = await page.locator('body').innerText();
    observe('implementation.api_reference_regression', apiText.includes('orders::get'), 'API reference shows orders::get', apiText.slice(0,1200));
  } catch (error) {
    for (const id of ids) if (!cases[`implementation.${id}`]) unavailable(`implementation.${id}`, 'Live browser observation', String(error));
  } finally { await browser.close(); }
}

async function downloadProbe(apiHealthy) {
  if (!apiHealthy) return unavailable('implementation.download_regression', 'Artifact bytes match hash', 'API prerequisite unavailable');
  try {
    const response = await request('/download/orders-worker?version=1.0.0');
    const artifacts = Object.values(response.body?.binaries || {});
    let valid = response.status === 200 && artifacts.length > 0;
    const details = [];
    for (const artifact of artifacts) { const bytes = Buffer.from(await (await fetch(artifact.url, { signal: AbortSignal.timeout(timeout) })).arrayBuffer()); const digest = crypto.createHash('sha256').update(bytes).digest('hex'); details.push({ url: artifact.url, expected: artifact.sha256, digest }); valid &&= digest === artifact.sha256; }
    observe('implementation.download_regression', valid, 'Artifact bytes match stored SHA-256', details);
  } catch (error) { unavailable('implementation.download_regression', 'Artifact bytes match hash', String(error)); }
}

(async () => {
  await fs.mkdir(output, { recursive: true });
  const apiHealthy = await apiProbes();
  await browserProbes(apiHealthy);
  await downloadProbe(apiHealthy);
  for (const id of ['implementation.patch_application']) unavailable(id, 'Patch applies to pinned base', 'Measured by the controller before runtime probing');
  await fs.writeFile(`${output}/feature.json`, `${JSON.stringify({ api_url: api, app_url: app, observations, cases }, null, 2)}\n`);
})().catch(async error => {
  await fs.mkdir(output, { recursive: true });
  await fs.writeFile(`${output}/feature.json`, `${JSON.stringify({ api_url: api, app_url: app, infrastructure_error: String(error), observations, cases }, null, 2)}\n`);
  console.error(error); process.exitCode = 1;
});
