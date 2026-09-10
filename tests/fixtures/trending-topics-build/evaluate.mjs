#!/usr/bin/env node

import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';

const PROJECTS = ['desktop', 'mobile'];
const CRITERIA = ['B03', 'B04', 'B05', 'B06', 'B07', 'B08', 'B09', 'B10'];
const EVIDENCE_TITLE = 'evidence captures home and first article through a real click';

export const variedFeed = {
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

function message(error) {
  return typeof error === 'string' ? error : error?.message ?? JSON.stringify(error);
}

function expectedTitles(feed) {
  const ids = feed?.topics?.map(({ id }) => id);
  if (ids?.length !== 6 || new Set(ids).size !== 6 || ids.some(id => typeof id !== 'string')) {
    throw new Error('Feed must contain six topics with unique string ids');
  }
  const titles = {
    B03: ['B03 home contains the edition and exactly six associated titles and ranks'],
    B04: ['B04 home follows ascending rank in DOM and visual reading order'],
    B05: ids.map(id => `B05 ${id} has its full visible article`),
    B06: ids.map(id => `B06 ${id} supports links and browser Back`),
    B07: ids.map(id => `B07 ${id} supports direct entry and reload`),
    B08: ids.map(id => `B08 ${id} exposes the supplied source link`),
    B09: ['B09 home semantics', ...ids.map(id => `B09 ${id} semantics and keyboard navigation with visible focus`)],
    B10: ['B10 home has no horizontal overflow or obstructed topic links',
      ...ids.map(id => `B10 ${id} has no overflow or obstructed article links`)],
  };
  return titles;
}

function flatten(suites) {
  const tests = [];
  for (const suite of suites ?? []) {
    for (const spec of suite.specs ?? []) {
      for (const test of spec.tests ?? []) {
        const result = test.results?.at(-1);
        tests.push({
          title: spec.title,
          project: test.projectName,
          expected_status: test.expectedStatus,
          runner_status: result?.status ?? 'missing',
          errors: (result?.errors ?? []).map(message),
          attachments: result?.attachments ?? [],
        });
      }
    }
    tests.push(...flatten(suite.suites));
  }
  return tests;
}

function productFailure(test) {
  if (test.runner_status !== 'failed') return false;
  const errors = test.errors.join('\n');
  return /expect\(|Expected:|Received:|locator\.[a-z]+: Timeout|page\.(?:click|press|reload|waitForURL): Timeout|Test timeout of \d+ms exceeded/i.test(errors);
}

function infrastructureFailure(test) {
  if (test.runner_status !== 'failed' || productFailure(test)) return false;
  const errors = test.errors.join('\n');
  return /browser(?:Type)?\.launch|executable doesn't exist|failed to launch|page\.goto:.*(?:net::|ERR_)|ECONNREFUSED|browser has been closed|Target page, context or browser has been closed|worker process exited|Cannot find module|No tests found/i.test(errors)
    || /^(?:Reference|Syntax|Type)Error:/im.test(errors);
}

function classify(test) {
  if (test.expected_status !== 'passed' || test.runner_status !== 'failed') {
    return test.runner_status === 'passed' && test.expected_status === 'passed' ? 'passed' : 'unverified';
  }
  return productFailure(test) ? 'failed' : 'unverified';
}

function criterionId(title) {
  return /^(B0[3-9]|B10)\b/.exec(title)?.[1];
}

export function summarize(reports, feeds) {
  const infrastructureErrors = [];
  const observations = Object.fromEntries(CRITERIA.map(id => [id, []]));
  const invalidCriteria = new Set();
  const evidence = [];
  const titlesByDataset = {};

  for (const { dataset, path, value, error } of feeds) {
    try {
      if (error) throw new Error(error);
      titlesByDataset[dataset] = expectedTitles(value);
    } catch (cause) {
      CRITERIA.forEach(id => invalidCriteria.add(id));
      infrastructureErrors.push({ kind: error ? 'input_unavailable' : 'invalid_feed', dataset, path,
        message: message(cause) });
    }
  }

  for (const { dataset, value: report, path, error, exit_code: exitCode } of reports) {
    if (error) {
      CRITERIA.forEach(id => invalidCriteria.add(id));
      if (exitCode !== -1) {
        infrastructureErrors.push({ kind: 'input_unavailable', dataset, path, message: error });
      }
      for (const project of PROJECTS) evidence.push({ dataset, project, title: EVIDENCE_TITLE,
        status: 'unverified', runner_status: 'missing', errors: [], attachments: [] });
      continue;
    }
    const reportErrors = (report.errors ?? []).map(message);
    if (reportErrors.length) {
      infrastructureErrors.push({ kind: 'report_error', dataset, path, messages: reportErrors });
      CRITERIA.forEach(id => invalidCriteria.add(id));
    }

    const actual = flatten(report.suites);
    const expectedExit = reportErrors.length || actual.some(test => test.runner_status !== 'passed') ? 1 : 0;
    if (!Number.isInteger(exitCode) || exitCode !== expectedExit) {
      infrastructureErrors.push({ kind: 'exit_report_mismatch', dataset, path,
        exit_code: Number.isInteger(exitCode) ? exitCode : null, expected_exit_code: expectedExit });
      CRITERIA.forEach(id => invalidCriteria.add(id));
    }
    const evidenceTests = actual.filter(({ title }) => title === EVIDENCE_TITLE);
    for (const project of PROJECTS) {
      const found = evidenceTests.filter(test => test.project === project);
      if (found.length !== 1) infrastructureErrors.push({ kind: found.length ? 'duplicate_evidence' : 'missing_evidence',
        dataset, project, count: found.length });
      if (!found.length) evidence.push({ dataset, project, title: EVIDENCE_TITLE,
        status: 'unverified', runner_status: 'missing', errors: [], attachments: [] });
      for (const test of found) {
        const status = classify(test);
        evidence.push({ dataset, project: test.project, title: test.title,
          status, runner_status: test.runner_status, errors: test.errors, attachments: test.attachments });
        if (status === 'unverified') {
          infrastructureErrors.push({ kind: infrastructureFailure(test) ? 'test_infrastructure' : 'unclassified_test_failure',
            dataset, title: test.title, project: test.project, runner_status: test.runner_status, errors: test.errors });
        }
      }
    }

    if (!titlesByDataset[dataset]) {
      for (const test of actual) {
        const id = criterionId(test.title);
        if (!id) continue;
        const status = classify(test);
        observations[id].push({ dataset, title: test.title, project: test.project, status,
          runner_status: test.runner_status, errors: test.errors });
      }
      continue;
    }

    for (const id of CRITERIA) {
      const expected = titlesByDataset[dataset][id].flatMap(title => PROJECTS.map(project => ({ title, project })));
      const expectedKeys = new Set(expected.map(({ title, project }) => `${title}\0${project}`));
      const matching = actual.filter(({ title }) => criterionId(title) === id);
      const groups = new Map();
      for (const test of matching) {
        const key = `${test.title}\0${test.project}`;
        groups.set(key, [...(groups.get(key) ?? []), test]);
        if (!expectedKeys.has(key)) {
          invalidCriteria.add(id);
          infrastructureErrors.push({ kind: 'unexpected_test', dataset, criterion: id,
            title: test.title, project: test.project, runner_status: test.runner_status, errors: test.errors });
          observations[id].push({ dataset, title: test.title, project: test.project,
            status: 'unverified', runner_status: test.runner_status, errors: test.errors });
        }
      }

      for (const expectedTest of expected) {
        const key = `${expectedTest.title}\0${expectedTest.project}`;
        const found = groups.get(key) ?? [];
        if (found.length !== 1) {
          invalidCriteria.add(id);
          infrastructureErrors.push({ kind: found.length ? 'duplicate_test' : 'missing_test', dataset,
            criterion: id, ...expectedTest, count: found.length });
        }
        if (!found.length) {
          observations[id].push({ dataset, ...expectedTest, status: 'unverified', runner_status: 'missing', errors: [] });
          continue;
        }
        for (const test of found) {
          const status = classify(test);
          observations[id].push({ dataset, title: test.title, project: test.project, status,
            runner_status: test.runner_status, errors: test.errors });
          if (status === 'unverified') {
            invalidCriteria.add(id);
            infrastructureErrors.push({ kind: infrastructureFailure(test) ? 'test_infrastructure' : 'unclassified_test_failure',
              dataset, criterion: id, title: test.title, project: test.project,
              runner_status: test.runner_status, errors: test.errors });
          }
        }
      }
    }
  }

  const evidenceComplete = evidence.length === PROJECTS.length * reports.length
    && evidence.every(item => item.status === 'passed'
      && ['home', 'article', 'evidence'].every(name => item.attachments.some(attachment => attachment.name === name)));
  return {
    criteria: CRITERIA.map(id => {
      const failed = observations[id].filter(({ status }) => status === 'failed').length;
      const status = failed ? 'failed' : invalidCriteria.has(id) ? 'unverified' : 'passed';
      return { id, status,
        reason: failed ? `${failed} acceptance assertion${failed === 1 ? '' : 's'} failed`
          : status === 'unverified' ? 'Acceptance coverage or execution was incomplete'
            : `All ${observations[id].length} expected observations passed`,
        observations: observations[id] };
    }),
    infrastructure_errors: infrastructureErrors,
    evidence,
    evidence_complete: evidenceComplete,
  };
}

async function load(path, dataset) {
  try {
    return { dataset, path, value: JSON.parse(await readFile(path, 'utf8')) };
  } catch (error) {
    return { dataset, path, error: message(error) };
  }
}

async function main(args) {
  if (args[0] === '--feed-variant' && args.length === 3) {
    const original = await load(args[1], 'original');
    if (original.error) throw new Error(`Cannot read original feed: ${original.error}`);
    expectedTitles(original.value);
    await writeFile(args[2], `${JSON.stringify(variedFeed, null, 2)}\n`, { flag: 'wx' });
    return;
  }
  if (args[0] === '--summarize' && args.length === 7) {
    const values = await Promise.all([
      load(args[1], 'original'), load(args[2], 'varied'),
      load(args[3], 'original'), load(args[4], 'varied'),
    ]);
    values[0].exit_code = Number(args[5]);
    values[1].exit_code = Number(args[6]);
    process.stdout.write(`${JSON.stringify(summarize(values.slice(0, 2), values.slice(2)), null, 2)}\n`);
    return;
  }
  throw new Error('Usage: evaluate.mjs --feed-variant <original-feed> <new-varied-feed> | --summarize <original-report> <varied-report> <original-feed> <varied-feed> <original-exit> <varied-exit>');
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  main(process.argv.slice(2)).catch(error => {
    process.stderr.write(`${error.stack ?? error}\n`);
    process.exitCode = 1;
  });
}
