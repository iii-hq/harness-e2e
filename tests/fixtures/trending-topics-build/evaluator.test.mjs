import test from 'node:test';
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { summarize, variedFeed } from './evaluate.mjs';

const execFileAsync = promisify(execFile);
const projects = ['desktop', 'mobile'];

function feed(prefix) {
  return { edition: prefix, topics: Array.from({ length: 6 }, (_, index) => ({
    id: `${prefix}-${index + 1}`,
    rank: index + 1,
    title: `${prefix} ${index + 1}`,
    source_body: `Body ${index + 1}`,
    url: `https://example.test/${prefix}-${index + 1}`,
  })) };
}

function titles(value) {
  const ids = value.topics.map(({ id }) => id);
  return [
    'B03 home contains the edition and exactly six associated titles and ranks',
    'B04 home follows ascending rank in DOM and visual reading order',
    'B09 home semantics',
    'B10 home has no horizontal overflow or obstructed topic links',
    ...ids.flatMap(id => [
      `B05 ${id} has its full visible article`,
      `B06 ${id} supports links and browser Back`,
      `B07 ${id} supports direct entry and reload`,
      `B08 ${id} exposes the supplied source link`,
      `B09 ${id} semantics and keyboard navigation with visible focus`,
      `B10 ${id} has no overflow or obstructed article links`,
    ]),
  ];
}

function report(value) {
  return {
    errors: [],
    suites: [{ specs: [...titles(value), 'evidence captures home and first article through a real click'].map(title => ({
      title,
      tests: projects.map(projectName => ({
        projectName,
        expectedStatus: 'passed',
        results: [{ status: 'passed', errors: [], attachments: title.startsWith('evidence ') ? [
          { name: 'home', path: `/${projectName}-home.png` },
          { name: 'article', path: `/${projectName}-article.png` },
          { name: 'evidence', path: `/${projectName}-evidence.json` },
        ] : [] }],
      })),
    })) }],
  };
}

function inputs() {
  const original = feed('original');
  const varied = feed('varied');
  return {
    original,
    varied,
    reports: [
      { dataset: 'original', path: 'original.json', value: report(original), exit_code: 0 },
      { dataset: 'varied', path: 'varied.json', value: report(varied), exit_code: 0 },
    ],
    feeds: [
      { dataset: 'original', path: 'original-feed.json', value: original },
      { dataset: 'varied', path: 'varied-feed.json', value: varied },
    ],
  };
}

function findTest(reportValue, title, project) {
  const spec = reportValue.suites[0].specs.find(item => item.title === title);
  return spec.tests.find(item => item.projectName === project);
}

test('feed variant CLI writes the deterministic control dataset', async () => {
  const root = await mkdtemp(join(tmpdir(), 'trending-evaluator-'));
  try {
    const originalPath = join(root, 'original.json');
    const variedPath = join(root, 'varied.json');
    await writeFile(originalPath, JSON.stringify(feed('original')));
    await execFileAsync(process.execPath, [fileURLToPath(new URL('./evaluate.mjs', import.meta.url)),
      '--feed-variant', originalPath, variedPath]);
    assert.deepEqual(JSON.parse(await readFile(variedPath, 'utf8')), variedFeed);
  } finally {
    await rm(root, { recursive: true });
  }
});

test('complete reports pass each criterion with exact independent coverage', () => {
  const { reports, feeds } = inputs();
  const result = summarize(reports, feeds);
  assert.deepEqual(result.criteria.map(({ id, status, observations }) => [id, status, observations.length]), [
    ['B03', 'passed', 4], ['B04', 'passed', 4], ['B05', 'passed', 24], ['B06', 'passed', 24],
    ['B07', 'passed', 24], ['B08', 'passed', 24], ['B09', 'passed', 28], ['B10', 'passed', 28],
  ]);
  assert.equal(result.infrastructure_errors.length, 0);
  assert.equal(result.evidence.length, 4);
  assert.equal(result.evidence_complete, true);
});

test('assertion failures fail only their criterion and non-terminal results stay unverified', () => {
  const { reports, feeds, original, varied } = inputs();
  const failed = findTest(reports[0].value, `B05 ${original.topics[0].id} has its full visible article`, 'desktop');
  failed.results[0] = { status: 'failed', errors: [{ message: 'expect(received).toBe(expected)\nExpected: body' }] };
  const locator = findTest(reports[0].value, `B05 ${original.topics[1].id} has its full visible article`, 'mobile');
  locator.results[0] = { status: 'failed', errors: [{ message: 'locator.click: Test timeout of 30000ms exceeded' }] };
  const interrupted = findTest(reports[0].value, `B06 ${original.topics[0].id} supports links and browser Back`, 'mobile');
  interrupted.results[0] = { status: 'interrupted', errors: [] };
  const skipped = findTest(reports[1].value, `B06 ${varied.topics[0].id} supports links and browser Back`, 'desktop');
  skipped.results[0] = { status: 'skipped', errors: [] };
  const unknown = findTest(reports[1].value, `B06 ${varied.topics[0].id} supports links and browser Back`, 'mobile');
  unknown.results[0] = { status: 'unknown', errors: [] };
  const browser = findTest(reports[0].value, `B07 ${original.topics[0].id} supports direct entry and reload`, 'desktop');
  browser.results[0] = { status: 'failed', errors: [{ message: 'browserType.launch: Executable does not exist' }] };
  reports[0].exit_code = 1;
  reports[1].exit_code = 1;

  const result = summarize(reports, feeds);
  assert.equal(result.criteria.find(({ id }) => id === 'B05').status, 'failed');
  assert.equal(result.criteria.find(({ id }) => id === 'B05').observations
    .find(({ project, dataset, status }) => project === 'mobile' && dataset === 'original' && status === 'failed').runner_status, 'failed');
  assert.equal(result.criteria.find(({ id }) => id === 'B06').status, 'unverified');
  assert.equal(result.criteria.find(({ id }) => id === 'B07').status, 'unverified');
  assert.equal(result.criteria.find(({ id }) => id === 'B08').status, 'passed');
  assert.deepEqual(new Set(result.criteria.find(({ id }) => id === 'B06').observations
    .filter(({ runner_status }) => ['interrupted', 'skipped', 'unknown'].includes(runner_status))
    .map(({ runner_status }) => runner_status)), new Set(['interrupted', 'skipped', 'unknown']));
  assert.ok(result.infrastructure_errors.some(({ kind }) => kind === 'test_infrastructure'));
  assert.equal(result.criteria.find(({ id }) => id === 'B07').observations
    .find(({ project, dataset }) => project === 'desktop' && dataset === 'original').status, 'unverified');
});

test('duplicate and missing project observations invalidate coverage without fabricating failures', () => {
  const { reports, feeds } = inputs();
  const spec = reports[0].value.suites[0].specs.find(({ title }) => title.startsWith('B03 '));
  spec.tests = [spec.tests[0], structuredClone(spec.tests[0])];
  const result = summarize(reports, feeds);
  assert.equal(result.criteria.find(({ id }) => id === 'B03').status, 'unverified');
  assert.ok(result.infrastructure_errors.some(({ kind }) => kind === 'duplicate_test'));
  assert.ok(result.infrastructure_errors.some(({ kind }) => kind === 'missing_test'));
  assert.equal(result.criteria.find(({ id }) => id === 'B04').status, 'passed');
});

test('missing screenshot evidence is explicit without changing acceptance results', () => {
  const { reports, feeds } = inputs();
  reports[0].value.suites[0].specs = reports[0].value.suites[0].specs
    .filter(({ title }) => title !== 'evidence captures home and first article through a real click');
  const result = summarize(reports, feeds);
  assert.ok(result.criteria.every(({ status }) => status === 'passed'));
  assert.equal(result.evidence.filter(({ runner_status }) => runner_status === 'missing').length, 2);
  assert.ok(result.infrastructure_errors.some(({ kind }) => kind === 'missing_evidence'));
  assert.equal(result.evidence_complete, false);
});

test('unreadable inputs and report-level errors classify affected work as unverified', () => {
  const unavailable = inputs();
  unavailable.reports[0] = { dataset: 'original', path: 'missing.json', error: 'ENOENT' };
  const knownFailure = findTest(unavailable.reports[1].value,
    `B05 ${unavailable.varied.topics[0].id} has its full visible article`, 'desktop');
  knownFailure.results[0] = { status: 'failed', errors: [{ message: 'expect(received).toBe(expected)' }] };
  const partial = summarize(unavailable.reports, unavailable.feeds);
  assert.equal(partial.criteria.find(({ id }) => id === 'B05').status, 'failed');
  assert.ok(partial.criteria.filter(({ id }) => id !== 'B05').every(({ status }) => status === 'unverified'));
  assert.ok(partial.infrastructure_errors.some(({ kind }) => kind === 'input_unavailable'));

  const runner = inputs();
  runner.reports[0].value.errors = [{ message: 'Worker process exited unexpectedly' }];
  runner.reports[0].exit_code = 1;
  const result = summarize(runner.reports, runner.feeds);
  assert.ok(result.criteria.every(({ status }) => status === 'unverified'));
  assert.ok(result.infrastructure_errors.some(({ kind }) => kind === 'report_error'));
});

test('product prerequisite skips are unverified without becoming infrastructure errors', () => {
  const { reports, feeds } = inputs();
  reports[0] = { dataset: 'original', path: 'not-produced.json', error: 'not produced after build failure', exit_code: -1 };

  const result = summarize(reports, feeds);

  assert.ok(result.criteria.every(({ status }) => status === 'unverified'));
  assert.equal(result.infrastructure_errors.length, 0);
  assert.equal(result.evidence_complete, false);
});

test('Playwright exit code must agree with the report', () => {
  const { reports, feeds } = inputs();
  reports[0].exit_code = 1;

  const result = summarize(reports, feeds);

  assert.ok(result.criteria.every(({ status }) => status === 'unverified'));
  assert.ok(result.infrastructure_errors.some(({ kind, dataset }) =>
    kind === 'exit_report_mismatch' && dataset === 'original'));
});
