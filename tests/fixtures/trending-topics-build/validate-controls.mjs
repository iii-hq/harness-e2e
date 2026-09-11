#!/usr/bin/env node

import { spawn } from 'node:child_process';
import { constants } from 'node:fs';
import { access, cp, mkdir, readFile, symlink, writeFile } from 'node:fs/promises';
import { dirname, isAbsolute, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { createServer } from 'node:net';

const [fixtureApp, outputRoot] = process.argv.slice(2);
if (!fixtureApp || !outputRoot || !isAbsolute(fixtureApp) || !isAbsolute(outputRoot)) {
  throw new Error('Usage: node validate-controls.mjs <absolute-fixture-app> <absolute-new-output-dir>');
}
await access(fixtureApp, constants.R_OK);
try {
  await access(outputRoot);
  throw new Error(`Output directory already exists: ${outputRoot}`);
} catch (error) {
  if (error.code !== 'ENOENT') throw error;
}

const evaluatorRoot = dirname(fileURLToPath(import.meta.url));
const evaluatorFiles = ['acceptance.spec.mjs', 'playwright.config.mjs', 'validate-controls.mjs', 'package-lock.json'];
const evaluatorHashes = Object.fromEntries(await Promise.all(evaluatorFiles.map(async file =>
  [file, createHash('sha256').update(await readFile(join(evaluatorRoot, file))).digest('hex')])));
const referenceMain = await readFile(join(evaluatorRoot, 'reference/main.js'), 'utf8');
const referenceCss = await readFile(join(evaluatorRoot, 'reference/style.css'), 'utf8');
const alternateCss = await readFile(join(evaluatorRoot, 'reference/alternate.css'), 'utf8');
const originalFeedBytes = await readFile(join(fixtureApp, 'content/feed.json'));
const originalFeed = JSON.parse(originalFeedBytes);
const playwrightCli = join(evaluatorRoot, 'node_modules/@playwright/test/cli.js');
await access(playwrightCli, constants.R_OK);
await access(join(fixtureApp, 'node_modules'), constants.R_OK);
const portProbe = createServer();
await new Promise((resolve, reject) => portProbe.once('error', reject).listen(4187, '127.0.0.1', resolve));
await new Promise(resolve => portProbe.close(resolve));

const variedFeed = {
  edition: '2031-Q4 <control>',
  topics: [4, 0, 5, 1, 3, 2].map((index) => {
    const id = `signal-${index + 1}`;
    return {
      id,
      rank: [41, 7, 29, 13, 37, 19][index],
      title: `Signal ${index + 1}: <not markup> & “quoted”`,
      source_body: `Control story ${index + 1} keeps <tags> as text, punctuation & symbols.\nLine two says: “don't hardcode me.”`,
      url: index === 0 ? 'https://root.trend-e2e.test#source'
        : `https://varied.trend-e2e.test/${id}?edition=2031-Q4&slot=${index + 1}`,
    };
  }),
};

await mkdir(join(outputRoot, 'apps'), { recursive: true });
await mkdir(join(outputRoot, 'results'), { recursive: true });

function replaceOnce(source, before, after) {
  if (source.split(before).length !== 2) throw new Error(`Expected one mutation target: ${before}`);
  return source.replace(before, after);
}

async function run(command, args, options = {}) {
  const child = spawn(command, args, {
    ...options, stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...(options.env ?? process.env), III_TELEMETRY_ENABLED: 'false' },
  });
  let stdout = '';
  let stderr = '';
  child.stdout.on('data', (chunk) => { stdout += chunk; });
  child.stderr.on('data', (chunk) => { stderr += chunk; });
  const exitCode = await new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('close', (code, signal) => resolve(code ?? (signal ? 128 : 1)));
  });
  return { exitCode, stdout, stderr };
}

async function stop(child) {
  if (child.exitCode !== null) return;
  process.kill(-child.pid, 'SIGTERM');
  const closed = new Promise((resolve) => child.once('close', resolve));
  const timeout = new Promise((resolve) => setTimeout(resolve, 3_000, 'timeout'));
  if (await Promise.race([closed, timeout]) === 'timeout' && child.exitCode === null) {
    process.kill(-child.pid, 'SIGKILL');
    await closed;
  }
}

async function waitForServer(url, child) {
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`Preview exited with code ${child.exitCode}`);
    try {
      if ((await fetch(url)).ok) return;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`Preview did not become ready: ${url}`);
}

function reportTests(suites, titles = []) {
  const tests = [];
  for (const suite of suites ?? []) {
    const path = [...titles, suite.title].filter(Boolean);
    for (const spec of suite.specs ?? []) {
      for (const test of spec.tests ?? []) {
        const result = test.results?.at(-1);
        tests.push({
          title: [...path, spec.title].filter(Boolean).join(' > '),
          expectedStatus: test.expectedStatus,
          project: test.projectName,
          status: result?.status,
          errors: (result?.errors ?? []).map(({ message }) => message),
        });
      }
    }
    tests.push(...reportTests(suite.suites, path));
  }
  return tests;
}

async function prepare(name, dataset, main, css) {
  const app = join(outputRoot, 'apps', name);
  await cp(fixtureApp, app, {
    recursive: true,
    filter(source) {
      const segments = relative(fixtureApp, source).split('/');
      return !segments.some((part) => ['.git', 'node_modules', 'dist', 'test-results', 'playwright-report'].includes(part));
    },
  });
  await symlink(join(fixtureApp, 'node_modules'), join(app, 'node_modules'), 'dir');
  await writeFile(join(app, 'src/main.js'), main);
  await writeFile(join(app, 'src/style.css'), css);
  await writeFile(join(app, 'content/feed.json'), dataset === 'varied' ? `${JSON.stringify(variedFeed, null, 2)}\n` : originalFeedBytes);
  return app;
}

const cases = [
  { name: 'reference-original', dataset: 'original', main: referenceMain, css: referenceCss },
  { name: 'reference-varied', dataset: 'varied', main: referenceMain, css: referenceCss },
  { name: 'reference-alternate', dataset: 'original', main: referenceMain, css: alternateCss },
  { name: 'reference-alternate-varied', dataset: 'varied', main: referenceMain, css: alternateCss },
  {
    name: 'reference-line-breaks', dataset: 'varied', css: referenceCss,
    main: replaceOnce(referenceMain, 'if (text !== undefined) node.textContent = text;',
      "if (text !== undefined) node.replaceChildren(...String(text).split('\\n').flatMap((line, index) => index ? [document.createElement('br'), document.createTextNode(line)] : [document.createTextNode(line)]));"),
  },
  { name: 'placeholder', dataset: 'original', main: await readFile(join(fixtureApp, 'src/main.js'), 'utf8'), css: await readFile(join(fixtureApp, 'src/style.css'), 'utf8'), grep: 'B03' },
  {
    name: 'hardcoded-original', dataset: 'varied', grep: 'B03', css: referenceCss,
    main: replaceOnce(referenceMain, "import feed from '../content/feed.json';", `const feed = ${JSON.stringify(originalFeed)};`),
  },
  {
    name: 'missing-topic', dataset: 'original', grep: 'B03', css: referenceCss,
    main: replaceOnce(referenceMain, '[...feed.topics].sort', '[...feed.topics.slice(1)].sort'),
  },
  {
    name: 'duplicate-topic', dataset: 'original', grep: 'B03', css: referenceCss,
    main: replaceOnce(referenceMain, '[...feed.topics].sort', '[...feed.topics, feed.topics[0]].sort'),
  },
  {
    name: 'hidden-ranks', dataset: 'original', grep: 'B03', main: referenceMain,
    css: `${referenceCss}\n.rank { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0, 0, 0, 0); }\n`,
  },
  {
    name: 'empty-markers', dataset: 'original', grep: 'B03', main: referenceMain,
    css: `${alternateCss}\nli::marker { content: ''; }\n`,
  },
  {
    name: 'false-visible-label', dataset: 'original', grep: 'B03', css: referenceCss,
    main: replaceOnce(referenceMain, "element('a', item.title, { href: `/posts/${item.id}` })",
      "element('a', 'Wrong visible title', { href: `/posts/${item.id}`, 'aria-label': item.title })"),
  },
  {
    name: 'invisible-label-ancestor', dataset: 'original', grep: 'B03', main: referenceMain,
    css: `${referenceCss}\n#app { opacity: 0; }\n`,
  },
  {
    name: 'reversed-sort', dataset: 'original', grep: 'B04', css: referenceCss,
    main: replaceOnce(referenceMain, 'a.rank - b.rank', 'b.rank - a.rank'),
  },
  {
    name: 'css-reversed', dataset: 'original', grep: 'B04', main: referenceMain,
    css: `${referenceCss}\nol { display: flex !important; flex-direction: column-reverse !important; }\n`,
  },
  {
    name: 'wrong-body', dataset: 'original', grep: `B05.*${originalFeed.topics[0].id}`, css: referenceCss,
    main: replaceOnce(referenceMain, "element('p', topic.source_body, { class: 'body' })", "element('p', 'Incomplete.', { class: 'body' })"),
  },
  {
    name: 'clipped-body', dataset: 'original', grep: `B05.*${originalFeed.topics[0].id}`, main: referenceMain,
    css: `${referenceCss}\n.body { max-height: 1em; overflow: hidden; }\n`,
  },
  {
    name: 'invisible-body', dataset: 'original', grep: `B05.*${originalFeed.topics[0].id}`, main: referenceMain,
    css: `${referenceCss}\n.body { opacity: 0; }\n`,
  },
  {
    name: 'wrong-source', dataset: 'original', grep: `B08.*${originalFeed.topics[0].id}`, css: referenceCss,
    main: replaceOnce(referenceMain, "href: topic.url", "href: 'https://wrong.example.test/'"),
  },
  {
    name: 'missing-back', dataset: 'original', grep: `B06.*${originalFeed.topics[0].id}`, css: referenceCss,
    main: replaceOnce(referenceMain, "element('a', 'All topics', { href: '/' })", "element('a', 'All topics', { href: '/missing' })"),
  },
  {
    name: 'direct-entry-disabled', dataset: 'original', grep: `B07.*${originalFeed.topics[0].id}`, css: referenceCss,
    main: replaceOnce(referenceMain, "const topic = topics.find", "const topic = document.referrer && topics.find"),
  },
  {
    name: 'history-replaced', dataset: 'original', grep: `B06.*${originalFeed.topics[0].id}`, css: referenceCss,
    main: `${referenceMain}\ndocument.addEventListener('click', event => { const link = event.target.closest('a'); if (link?.pathname.startsWith('/posts/')) { event.preventDefault(); location.replace(link.href); } });\n`,
  },
  {
    name: 'no-keyboard', dataset: 'original', grep: `B09.*${originalFeed.topics[0].id}`, css: referenceCss,
    main: replaceOnce(referenceMain, 'const node = document.createElement(tag);', "const node = document.createElement(tag); if (tag === 'a') node.tabIndex = -1;"),
  },
  {
    name: 'no-focus', dataset: 'original', grep: `B09.*${originalFeed.topics[0].id}`, main: referenceMain,
    css: `${referenceCss}\na, a:focus, a:focus-visible { outline: none !important; outline-offset: 0 !important; box-shadow: none !important; }\n`,
  },
  {
    name: 'animated-no-focus', dataset: 'original', grep: `B09.*${originalFeed.topics[0].id}`, main: referenceMain,
    css: `${referenceCss}\n@keyframes decoy-focus { from { color: #185d37; } to { color: #d14d2c; } }\na { animation: decoy-focus 50ms infinite alternate linear !important; }\na, a:focus, a:focus-visible { outline: none !important; outline-offset: 0 !important; box-shadow: none !important; }\n`,
  },
  {
    name: 'mobile-overflow', dataset: 'original', grep: 'B10.*home', main: referenceMain,
    css: `${referenceCss}\nbody { min-width: 900px; }\n`,
  },
  {
    name: 'blocking-overlay', dataset: 'original', grep: 'B10.*home', main: referenceMain,
    css: `${referenceCss}\nbody::after { content: ''; position: fixed; inset: 0; z-index: 2147483647; background: white; }\n`,
  },
];

const summaries = [];
for (const control of cases) {
  const resultDir = join(outputRoot, 'results', control.name);
  await mkdir(resultDir, { recursive: true });
  let preview;
  let previewStdout = '';
  let previewStderr = '';
  try {
    const app = await prepare(control.name, control.dataset, control.main, control.css);
    const build = await run('npm', ['run', 'build'], { cwd: app, env: process.env });
    await writeFile(join(resultDir, 'build.stdout.log'), build.stdout);
    await writeFile(join(resultDir, 'build.stderr.log'), build.stderr);
    if (build.exitCode !== 0) throw new Error(`Build failed with code ${build.exitCode}`);

    preview = spawn('npm', ['run', 'preview', '--', '--port', '4187'], {
      cwd: app,
      detached: true,
      env: { ...process.env, III_TELEMETRY_ENABLED: 'false' },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    preview.stdout.on('data', (chunk) => { previewStdout += chunk; });
    preview.stderr.on('data', (chunk) => { previewStderr += chunk; });
    await waitForServer('http://127.0.0.1:4187/', preview);

    const test = await run(process.execPath, [playwrightCli, 'test', '--config', 'playwright.config.mjs', ...(control.grep ? ['--grep', control.grep] : [])], {
      cwd: evaluatorRoot,
      env: {
        ...process.env,
        TT_BASE_URL: 'http://127.0.0.1:4187',
        TT_FEED_PATH: join(app, 'content/feed.json'),
        TT_OUTPUT_DIR: resultDir,
        TT_DATASET: control.dataset,
        TT_COMMIT_SHA: 'reference-control',
      },
    });
    await writeFile(join(resultDir, 'playwright.stdout.log'), test.stdout);
    await writeFile(join(resultDir, 'playwright.stderr.log'), test.stderr);

    const report = JSON.parse(await readFile(join(resultDir, 'results.json'), 'utf8'));
    const tests = reportTests(report.suites);
    const runnerErrors = report.errors ?? [];
    const failed = tests.filter(({ status }) => status === 'failed');
    const expectedFailure = Boolean(control.grep);
    const valid = tests.length > 0 && runnerErrors.length === 0 && !/No tests found/i.test(`${test.stdout}\n${test.stderr}`) && (expectedFailure
      ? test.exitCode === 1 && failed.length > 0
        && tests.every(({ status }) => ['passed', 'failed'].includes(status))
        && failed.every(({ title, errors }) => new RegExp(control.grep).test(title) && errors.length > 0
          && errors.every(message => /expect\(/.test(message)
            || (control.name === 'blocking-overlay' && /locator\.click: Timeout/.test(message) && /intercepts pointer events/.test(message))))
      : test.exitCode === 0 && tests.every(({ expectedStatus, status }) => expectedStatus === 'passed' && status === 'passed'));
    summaries.push({ name: control.name, dataset: control.dataset, expected: expectedFailure ? `failure matching ${control.grep}` : 'all tests pass', valid, exitCode: test.exitCode, runnerErrors, tests });
  } catch (error) {
    summaries.push({ name: control.name, dataset: control.dataset, expected: control.grep ? `failure matching ${control.grep}` : 'all tests pass', valid: false, error: error.stack ?? String(error) });
  } finally {
    if (preview) await stop(preview);
    await writeFile(join(resultDir, 'preview.stdout.log'), previewStdout);
    await writeFile(join(resultDir, 'preview.stderr.log'), previewStderr);
  }
  console.log(`${control.name}: ${summaries.at(-1).valid ? 'expected outcome' : 'INVALID control outcome'}`);
  await writeFile(join(outputRoot, 'progress.json'), `${JSON.stringify(summaries, null, 2)}\n`);
}

const evaluatorUnchanged = (await Promise.all(evaluatorFiles.map(async file =>
  createHash('sha256').update(await readFile(join(evaluatorRoot, file))).digest('hex') === evaluatorHashes[file]))).every(Boolean);
const summary = {
  kind: 'local-trusted-controls-not-a-model-delivery',
  evaluator_sha256: evaluatorHashes,
  evaluator_unchanged: evaluatorUnchanged,
  reference_sha256: createHash('sha256').update(referenceMain).update(referenceCss).digest('hex'),
  alternate_css_sha256: createHash('sha256').update(alternateCss).digest('hex'),
  original_feed_sha256: createHash('sha256').update(originalFeedBytes).digest('hex'),
  valid: evaluatorUnchanged && summaries.every(({ valid }) => valid),
  counts: {
    total: summaries.length,
    valid: summaries.filter(({ valid }) => valid).length,
    invalid: summaries.filter(({ valid }) => !valid).length,
  },
  controls: summaries,
};
await writeFile(join(outputRoot, 'summary.json'), `${JSON.stringify(summary, null, 2)}\n`);
console.log(JSON.stringify(summary.counts));
if (!summary.valid) process.exitCode = 1;
