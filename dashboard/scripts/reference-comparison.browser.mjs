// Functional Console RPC coverage. No models run; real-host visuals are checked separately.
import assert from 'node:assert/strict'
import { mkdtemp, readFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { chromium } from 'playwright'
import { createConsoleTestHost } from './console-test-host.mjs'

const capturedTransport = JSON.parse(
  await readFile(
    new URL(
      '../../tests/fixtures/history/retained-history.json',
      import.meta.url,
    ),
    'utf8',
  ),
)

const reference = {
  execution: {
    id: 'remote-1',
    local_id:
      'remote-execution-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
    campaignId: 'campaign',
    planKey: 'smoke',
    attempt: 1,
    trigger: 'manual',
    requestedBy: null,
    label: 'RC Smoke',
    phase: 'complete',
    terminal: true,
    resultState: 'complete',
    requestedAt: '2026-09-08T12:00:00Z',
    completedAt: '2026-09-08T12:01:00Z',
    error: null,
    runCount: 1,
    reportCount: 1,
    plan: {
      name: 'Smoke',
      subject: { model: 'test', provider: 'test' },
    },
    request: {},
  },
  aggregate: {
    planned_runs: 1,
    observed_runs: 1,
    completion_rate: 1,
    execution_reliability: 1,
  },
  runs: [
    {
      attemptsComplete: true,
      scenarioId: 'alpha',
      caseId: 'case-a',
      seed: '42',
      repetition: 0,
      technical: 'valid',
      status: 'passed',
      completion: 'completed',
      score: 80,
      wallTimeMs: 1000,
      totalTokens: 1_220_407,
      costSubjectUsd: 0.1,
      turns: 1,
      functionCalls: 1,
      functionCallErrors: 0,
      identity: { subjectModel: 'test' },
      record: {
        run_id: 'imported-run',
        attempt_id: 'imported-attempt',
        session_id: 'imported-session',
        transcript: {
          messages: [
            {
              message: {
                role: 'assistant',
                content: [
                  { type: 'text', text: 'Retained imported transcript.' },
                ],
              },
            },
          ],
        },
      },
    },
  ],
  materialized: {
    profile: { id: 'smoke', repetitions: 1, technical_retries: 0 },
    campaigns: [{ groups: [{ scenarios: ['alpha'] }] }],
  },
  shards: [{ runs: [{ scenario_id: 'alpha', case_id: 'case-a', seed: '42' }] }],
}
const earlierReference = {
  ...structuredClone(reference),
  execution: {
    ...reference.execution,
    id: 'remote-2',
    local_id: `remote-execution-${'b'.repeat(64)}`,
    label: 'Earlier RC Smoke',
    requestedAt: '2026-09-07T12:00:00Z',
    completedAt: '2026-09-07T12:01:00Z',
  },
  runs: [{ ...reference.runs[0], score: 60, totalTokens: 150 }],
}
const references = [reference, earlierReference]
const executionLabels = new Map()
const detail = (id) => ({
  id,
  label: id,
  status: 'passed',
  subjects: [],
  totals: { total_tokens: 80, report_coverage: 1 },
  reports: [
    {
      subject_id: 'test',
      scenario_id: 'alpha',
      available: true,
      report: {
        scenarios: [
          {
            scenario_id: 'alpha',
            behavior_sha256:
              'sha256:a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1',
            aggregate: { planned_runs: 1, observed_runs: 1, deferred_runs: 0 },
            case_id: 'case-a',
            case: {
              seed: 42,
              behavior_sha256:
                'sha256:a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1',
            },
            runs: [
              {
                run_id: 'r1',
                attempt_id: 'local-attempt',
                session_id: 'local-session',
                transcript: {
                  messages: [
                    {
                      message: {
                        role: 'assistant',
                        content: [
                          { type: 'text', text: 'Retained local transcript.' },
                        ],
                      },
                    },
                  ],
                },
                technical: 'valid',
                completion: 'completed',
                score: 90,
                efficiency: { total_tokens: 70 },
                metrics: { complete: true, totals: { cache_read_tokens: 10 } },
              },
            ],
          },
        ],
      },
    },
  ],
})
const locals = []
const additionalPlans = []
let imported = null,
  starts = 0
const calls = []
let disconnected = false
let unavailableExecutionId = null

const trigger = async (name, payload = {}) => {
  const id = name.replace('e2e::dashboard::', '').replaceAll('-', '_')
  // Release Control RPC names retain hyphens.
  const rpc = name.startsWith('release-control::') ? name : id
  calls.push({ id: rpc, payload })
  return route(rpc, payload)
}
async function route(id, payload) {
  if (id === 'execution_get' && payload.execution_id === unavailableExecutionId)
    throw new Error('Historical execution is unavailable')
  if (id.startsWith('release-control::') && disconnected)
    throw new Error('RC tab disconnected')
  if (id === 'release-control::test-plans::history-list')
    return {
      plans: [
        { key: 'smoke', active: true, updated_at: '2026-09-08T12:00:00Z' },
      ],
      next_after: null,
    }
  if (id === 'release-control::test-plans::export') return capturedTransport
  if (id === 'release-control::test-plans::list')
    return {
      plans: [{ key: 'smoke', recentExecutions: [reference.execution] }],
    }
  if (id === 'release-control::test-executions::reference') return reference
  if (id === 'plans_list')
    return {
      mode: 'unified',
      plans: [
        ...(imported ? [structuredClone(imported)] : []),
        ...additionalPlans,
      ],
    }
  if (id === 'plan_get')
    return structuredClone(
      payload.plan_id === imported?.id
        ? imported
        : additionalPlans.find((plan) => plan.id === payload.plan_id),
    )
  if (id === 'executions_list') {
    assert.ok(payload.limit === undefined || payload.limit <= 100)
    const executions = [
      ...references.map((reference) => ({
        id: reference.execution.local_id,
        run_id: reference.execution.id,
        origin: 'remote',
        label:
          executionLabels.get(reference.execution.local_id) ??
          reference.execution.label,
        execution_label:
          executionLabels.get(reference.execution.local_id) ?? null,
        status: 'completed',
        subjects: [],
        started_at: reference.execution.requestedAt,
        completed_at: reference.execution.completedAt,
        plan_id: 'remote-plan-smoke',
      })),
      ...structuredClone(locals),
    ]
    return {
      executions: payload.ids
        ? executions.filter((execution) => payload.ids.includes(execution.id))
        : payload.cursor
          ? executions.slice(1)
          : executions.slice(0, 1),
      total: executions.length,
      next_cursor:
        !payload.ids && !payload.cursor && executions.length > 1 ? '1' : null,
    }
  }
  if (
    id === 'execution_get' &&
    payload.execution_id.startsWith('remote-execution-')
  ) {
    const reference = references.find(
      (reference) => reference.execution.local_id === payload.execution_id,
    )
    assert.ok(reference)
    return {
      detail: {
        id: payload.execution_id,
        origin: 'remote',
        label:
          executionLabels.get(reference.execution.local_id) ??
          reference.execution.label,
        execution_label:
          executionLabels.get(reference.execution.local_id) ?? null,
        status: 'completed',
        subjects: [],
        reports: [],
        plan_id: 'remote-plan-smoke',
        remote_reference: reference,
        retained_reports: [],
      },
    }
  }
  if (id === 'execution_get')
    return {
      detail: structuredClone(
        locals.find((item) => item.id === payload.execution_id),
      ),
    }
  if (id === 'catalog_get')
    return {
      url: 'http://local',
      models: [{ provider: 'test', model: 'test' }],
      scenarios: ['alpha'],
    }
  if (id === 'plan_control' && payload.action === 'rename_imported_execution') {
    assert.ok(
      references.some(
        (reference) => reference.execution.local_id === payload.execution_id,
      ),
    )
    const label = payload.label.trim()
    if (label) executionLabels.set(payload.execution_id, label)
    else executionLabels.delete(payload.execution_id)
    return { execution_id: payload.execution_id, label }
  }
  if (id === 'plan_update') {
    const plan = additionalPlans.find((plan) => plan.id === payload.plan_id)
    assert.ok(plan)
    Object.assign(plan, payload)
    return structuredClone(plan)
  }
  if (id === 'plan_control' && payload.action === 'import_history') {
    imported = {
      origin: 'remote',
      id: 'remote-plan-smoke',
      label: 'RC Smoke',
      purpose: 'imported',
      created_at: null,
      updated_at: '2026-09-08T12:00:00Z',
      template_id: 'smoke',
      source: {
        instance_id: 'rc',
        plan_key: 'smoke',
        captured_at: '2026-09-08T12:00:00Z',
        active: true,
        limitation: null,
      },
      configuration: { subject: { provider: 'test', model: 'test' } },
      execution_ids: references.map(
        (reference) => reference.execution.local_id,
      ),
    }
    return {
      plan_id: imported.id,
      inserted: true,
      updated: false,
      unchanged: false,
    }
  }
  if (id === 'plan_run_start') {
    starts++
    const next = detail(`local-${starts}`)
    next.status = 'running'
    locals.unshift(next)
    imported = {
      ...imported,
      state:
        payload.role === 'baseline' ? 'baseline_running' : 'candidate_running',
      locked: true,
      baseline_execution_id:
        payload.role === 'baseline' ? next.id : imported.baseline_execution_id,
      candidate_execution_ids: payload.role === 'candidate' ? [next.id] : [],
      last_attempt_id: next.id,
    }
    return structuredClone(imported)
  }
  if (id === 'plan_control' && payload.action === 'cancel') {
    locals.find((item) => item.id === payload.execution_id).status = 'cancelled'
    imported = {
      ...imported,
      state: 'baseline_ready',
      last_attempt_id: 'local-1',
    }
    return {}
  }
  if (id === 'test_history_get')
    return {
      test_id: 'alpha',
      test_version:
        'sha256:a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1',
      available_versions: [],
      cases: ['case-a'],
      subjects: [],
      subject_models: [],
      systems: [],
      series: [],
      observations: [
        {
          execution_id: 'local-history',
          evaluated_version_id: 'local',
          cohort_id: 'same',
          completed_at: '2026-09-11T12:00:00Z',
          case_id: 'case-a',
          contract_sha256: 'contract',
          assessment_profile_sha256: 'assessment',
          status: 'passed',
          mean_score: 90,
          run_count: 1,
          scored_runs: 1,
          behavior_sha256:
            'sha256:a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1',
          seed: 1,
          stack_mode: 'source',
          subject_provider: 'test',
          subject_model: 'test',
          median_cost_usd: 0.01,
          median_tokens: 10,
          median_duration_seconds: 1,
          median_function_calls: 1,
          median_function_call_errors: 0,
          median_turns: 1,
        },
      ],
      total: 1,
      next_cursor: null,
    }
  if (id === 'tests_list')
    return {
      rows: [
        {
          test_id: 'alpha',
          lifecycle: 'active',
          current_version:
            'sha256:a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1',
          available_versions: [],
        },
      ],
      total: 1,
      next_cursor: null,
    }
  throw new Error(`Unexpected RPC ${id}`)
}

const screenshots = await mkdtemp(`${tmpdir()}/harness-reference-comparison-`)
const server = await createConsoleTestHost()
const browser = await chromium.launch()
const page = await browser.newPage({ viewport: { width: 1280, height: 900 } })
page.setDefaultTimeout(10_000)
const errors = []
page.on('pageerror', (error) => errors.push(error.message))
try {
  await server.install(page, trigger)
  await page.goto(`${server.url}#/ext/harness-e2e/plans`)
  await page
    .getByRole('button', { name: 'import history', exact: true })
    .click()
  await page
    .getByRole('dialog', { name: 'Import Release Control history' })
    .waitFor()
  await page.evaluate(() => {
    Object.defineProperty(crypto, 'subtle', { value: undefined })
  })
  await page.locator('input[type="file"]').setInputFiles({
    name: 'history.json',
    mimeType: 'application/json',
    buffer: Buffer.from(capturedTransport.json),
  })
  await page.getByText('remote', { exact: true }).waitFor()
  assert.equal(
    calls.find(
      (call) =>
        call.id === 'plan_control' && call.payload.action === 'import_history',
    ).payload.history,
    capturedTransport.json,
  )
  await page.getByRole('combobox').selectOption('smoke')
  await page
    .getByRole('button', { name: 'import selected history', exact: true })
    .click()
  await page.getByText('remote', { exact: true }).waitFor()
  await page.getByRole('button', { name: /^running\s*0$/ }).click()
  await page
    .getByText('No plans match these filters', { exact: true })
    .waitFor()
  assert.equal(await page.getByText('remote', { exact: true }).count(), 0)
  await page.getByRole('button', { name: 'clear filters', exact: true }).click()
  await page.getByText('Imported local copy', { exact: true }).waitFor()
  await page.screenshot({
    path: `${screenshots}/plans-light-desktop.png`,
    fullPage: true,
  })
  await page.setViewportSize({ width: 390, height: 844 })
  await page.screenshot({
    path: `${screenshots}/plans-light-narrow.png`,
    fullPage: true,
  })
  await page.evaluate(() => {
    window.__consoleTheme = 'dark'
  })
  await page.reload()
  await page.screenshot({
    path: `${screenshots}/plans-dark-narrow.png`,
    fullPage: true,
  })
  await page.setViewportSize({ width: 1280, height: 900 })
  assert.equal(
    calls.some(
      (call) => call.id === 'release-control::test-plans::history-list',
    ),
    true,
  )
  assert.equal(
    calls.some((call) => call.id === 'release-control::test-plans::export'),
    true,
  )
  const importCall = calls.find(
    (call) =>
      call.id === 'plan_control' &&
      call.payload.action === 'import_history' &&
      typeof call.payload.history === 'object',
  )
  assert.deepEqual(importCall.payload.history, capturedTransport)
  await page.goto(
    `${server.url}#/ext/harness-e2e/execution/remote-execution-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa`,
  )
  await page.locator('.execution-page [data-scenario-row]').waitFor()
  disconnected = true
  await page.reload()
  await page.locator('.execution-page [data-scenario-row]').waitFor()
  const secondLocal = detail('local-second')
  secondLocal.reports[0].report.scenarios[0].runs[0].score = 70
  const otherReference = {
    ...structuredClone(reference),
    execution: {
      ...reference.execution,
      id: 'remote-other',
      local_id: `remote-execution-${'c'.repeat(64)}`,
      planKey: 'other-smoke',
    },
  }
  references.push(otherReference)
  locals.push(
    {
      ...detail('local-history'),
      plan_id: 'local-smoke',
      started_at: '2026-09-07T18:00:00Z',
    },
    {
      ...secondLocal,
      plan_id: 'local-smoke',
      started_at: '2026-09-09T12:00:00Z',
    },
    { ...detail('unrelated-local'), plan_id: 'local-capability' },
    { ...detail('independent-history'), plan_id: 'independent-smoke' },
    detail('standalone-local'),
  )
  additionalPlans.push(
    {
      id: 'local-smoke',
      label: 'Local Smoke',
      purpose: 'Same plan',
      created_at: '',
      updated_at: '',
      scope_hash: 'scope',
      url: 'http://local',
      model: 'test',
      provider: 'test',
      scenarios: [],
      scenario_ids: ['alpha'],
      runs: 1,
      technical_retries: 0,
      seed: null,
      origin: 'local',
      template_id: 'smoke',
      reference_execution_id: earlierReference.execution.id,
      state: 'comparison_ready',
      baseline_execution_id: 'local-history',
      candidate_execution_ids: ['local-second'],
      incomplete_execution_ids: [],
    },
    {
      id: 'local-capability',
      origin: 'local',
      template_id: 'capability',
      state: 'baseline_ready',
      baseline_execution_id: 'unrelated-local',
      candidate_execution_ids: [],
      incomplete_execution_ids: [],
    },
  )
  additionalPlans.push(
    {
      ...structuredClone(additionalPlans[0]),
      id: 'independent-smoke',
      label: 'Independent Smoke',
      reference_execution_id: undefined,
      baseline_execution_id: 'independent-history',
      candidate_execution_ids: [],
    },
    {
      ...structuredClone(imported),
      id: 'other-remote-smoke',
      source: { ...imported.source, plan_key: 'other-smoke' },
      execution_ids: [otherReference.execution.local_id],
    },
  )
  await page.goto(`${server.url}#/ext/harness-e2e/plans/${imported.id}`)
  await page.getByRole('heading', { name: 'RC Smoke', exact: true }).waitFor()
  await page.locator('#metrics-baseline').waitFor()
  assert.equal(
    await page.locator('[data-plan-run-history] [data-execution-id]').count(),
    4,
  )
  assert.equal(await page.locator('#metrics-baseline option').count(), 4)
  assert.equal(
    await page.locator('[data-execution-id="independent-history"]').count(),
    0,
  )
  assert.equal(
    await page
      .locator(`[data-execution-id="${otherReference.execution.local_id}"]`)
      .count(),
    0,
  )
  assert.equal(
    await page.locator('[data-execution-id="unrelated-local"]').count(),
    0,
  )
  assert.equal(
    await page.locator('[data-execution-id="standalone-local"]').count(),
    0,
  )
  const historyRows = page.locator(
    '[data-plan-run-history] [data-execution-id]',
  )
  assert.deepEqual(
    await historyRows.evaluateAll((rows) =>
      rows.map((row) => row.dataset.executionId),
    ),
    [
      earlierReference.execution.local_id,
      'local-history',
      reference.execution.local_id,
      'local-second',
    ],
  )
  assert.equal(
    await page.locator('#metrics-baseline').inputValue(),
    earlierReference.execution.local_id,
  )
  assert.equal(await page.locator('[data-plan-run-history] code').count(), 0)
  assert.equal(
    await page
      .locator('[data-plan-run-history]')
      .getByText('report', { exact: true })
      .count(),
    0,
  )
  const remoteHistoryRow = () =>
    page.locator(
      `[data-plan-run-history] [data-execution-id="${reference.execution.local_id}"]`,
    )
  await remoteHistoryRow()
    .getByRole('button', { name: /^Rename / })
    .click()
  await remoteHistoryRow()
    .getByRole('textbox')
    .fill('Release after provider update')
  await remoteHistoryRow()
    .getByRole('button', { name: 'Save execution name', exact: true })
    .click()
  await remoteHistoryRow()
    .getByText('Release after provider update', { exact: true })
    .waitFor()
  await page.reload()
  await remoteHistoryRow()
    .getByText('Release after provider update', { exact: true })
    .waitFor()
  await page.goto(`${server.url}#/ext/harness-e2e/plans/local-smoke`)
  await remoteHistoryRow()
    .getByText('Release after provider update', { exact: true })
    .waitFor()
  await remoteHistoryRow()
    .getByRole('button', { name: /^Rename / })
    .click()
  await remoteHistoryRow().getByRole('textbox').fill('Discarded name')
  await remoteHistoryRow()
    .getByRole('button', { name: 'Cancel rename', exact: true })
    .click()
  assert.equal(
    await remoteHistoryRow()
      .getByText('Release after provider update', { exact: true })
      .count(),
    1,
  )
  await remoteHistoryRow()
    .getByRole('button', { name: /^Rename / })
    .click()
  await remoteHistoryRow().getByRole('textbox').fill('')
  await remoteHistoryRow()
    .getByRole('button', { name: 'Save execution name', exact: true })
    .click()
  await remoteHistoryRow()
    .getByRole('button', { name: 'Rename RC Smoke', exact: true })
    .waitFor()
  const localHistoryRow = page.locator(
    '[data-plan-run-history] [data-execution-id="local-second"]',
  )
  await localHistoryRow.getByRole('button', { name: /^Rename / }).click()
  await localHistoryRow.getByRole('textbox').fill('Local rerun')
  await localHistoryRow
    .getByRole('button', { name: 'Save execution name', exact: true })
    .click()
  await localHistoryRow.getByText('Local rerun', { exact: true }).waitFor()
  await page.reload()
  await localHistoryRow.getByText('Local rerun', { exact: true }).waitFor()
  await page.goto(`${server.url}#/ext/harness-e2e/plans/${imported.id}`)
  await historyRows.first().waitFor()
  const selectPair = async (a, b, scoreA, scoreB) => {
    if ((await page.locator('#plan-trends').getAttribute('open')) === null) {
      await page.locator('#plan-trends > summary').click()
    }
    await page.locator('#metrics-baseline').selectOption(a)
    await page.locator('#metrics-candidate').selectOption(b)
    await page
      .waitForFunction(
        ([a, b]) => {
          const values = [
            ...document.querySelectorAll('.pm-headline:first-child .pm-number'),
          ].map((element) => element.firstElementChild?.textContent)
          return values[0] === a && values[1] === b
        },
        [String(scoreA), String(scoreB)],
      )
      .catch(async (error) => {
        console.error(await page.locator('body').innerText())
        throw error
      })
    for (const side of ['a', 'b']) {
      assert.ok(
        (
          await page.locator(`[data-test-coverage="${side}"]`).innerText()
        ).includes('Tests executed: 1 · Scored: 1'),
      )
    }
    const results = page.getByRole('region', { name: 'Metrics by test' })
    assert.equal(
      await page.locator('.primary-metrics .pm-pair').evaluateAll((pairs) =>
        pairs.every((pair) => {
          const values = [...pair.querySelectorAll('.pm-number > span')].map(
            (element) => element.textContent,
          )
          return (
            pair
              .querySelector('.pm-delta')
              .textContent.includes('Not comparable') === values.includes('—')
          )
        }),
      ),
      true,
    )
    assert.equal(await results.locator('tbody tr').count(), 1)
    assert.deepEqual(
      await results
        .locator('td[data-label="Score"] .pm-number > span')
        .allTextContents(),
      [String(scoreA), String(scoreB)],
    )
    assert.equal(
      await results.evaluate((element) =>
        Boolean(
          element.compareDocumentPosition(
            document.querySelector('#plan-executions'),
          ) & Node.DOCUMENT_POSITION_FOLLOWING,
        ),
      ),
      true,
    )
    const metricIds = [
      'score',
      'costUsd',
      'durationMs',
      'totalTokens',
      'turns',
      'functionCalls',
    ]
    await page.waitForFunction(
      (score) =>
        document.querySelector('[data-trend-metric="score"] strong')
          ?.textContent === String(score),
      scoreB,
    )
    for (const [index, id] of metricIds.entries()) {
      assert.equal(
        await page.locator(`[data-trend-metric="${id}"] strong`).innerText(),
        await page
          .locator('.pm-headline')
          .nth(index)
          .locator('.pm-number > span')
          .nth(1)
          .innerText(),
      )
    }
    for (const id of ['score', 'totalTokens', 'turns', 'durationMs']) {
      await page.locator('#plan-movement-metric').selectOption(id)
      assert.equal(
        await page.locator(`[data-diverging-row="${id}"]`).count(),
        1,
      )
      const topDelta = await page
        .locator('.pm-headline')
        .nth(metricIds.indexOf(id))
        .locator('.pm-delta')
        .innerText()
      assert.equal(
        (
          await page.locator(`[data-diverging-row="${id}"]`).textContent()
        ).trim(),
        topDelta.replace(/ · (increase|decrease|No change)$/, ''),
      )
    }
    await page.locator('#plan-movement-metric').selectOption('score')
  }
  await selectPair(
    reference.execution.local_id,
    earlierReference.execution.local_id,
    80,
    60,
  )
  await selectPair(reference.execution.local_id, 'local-history', 80, 90)
  await selectPair('local-history', reference.execution.local_id, 90, 80)
  await selectPair('local-history', 'local-second', 90, 70)
  await selectPair(reference.execution.local_id, 'local-history', 80, 90)
  await page.screenshot({
    path: `${screenshots}/same-plan-comparison.png`,
    fullPage: true,
  })
  for (const width of [1280, 390]) {
    await page.setViewportSize({ width, height: 900 })
    assert.equal(
      await page.locator('.pm-headline').evaluateAll((cards) =>
        cards.every((card) => {
          const [a, b] = card.querySelectorAll('.pm-number > span')
          return (
            !a ||
            !b ||
            (a.getBoundingClientRect().right <=
              b.getBoundingClientRect().left &&
              b.getBoundingClientRect().right <=
                card.getBoundingClientRect().right)
          )
        }),
      ),
      true,
    )
  }
  await page.setViewportSize({ width: 1280, height: 900 })
  await page
    .getByRole('checkbox', { name: 'Hide tests with zero score or no result' })
    .check()
  await page
    .getByRole('checkbox', { name: 'Hide tests with zero score or no result' })
    .uncheck()
  await page.locator('#metrics-candidate').selectOption('')
  await page
    .getByRole('heading', { name: 'Execution results', exact: true })
    .waitFor()
  if ((await page.locator('#plan-trends').getAttribute('open')) === null) {
    await page.locator('#plan-trends > summary').click()
  }
  await page.locator('[data-plan-comparison]').waitFor()
  await page.getByRole('button', { name: 'Run plan', exact: true }).click()
  await page.getByRole('dialog', { name: 'Run plan?' }).waitFor()
  await page.getByRole('button', { name: 'Cancel', exact: true }).click()
  await page.goto(`${server.url}#/ext/harness-e2e/plans/local-smoke`)
  await page
    .getByRole('heading', { name: 'Local Smoke', exact: true })
    .waitFor()
  await page
    .locator('#metrics-baseline option')
    .first()
    .waitFor({ state: 'attached' })
  assert.equal(await page.locator('#metrics-baseline option').count(), 4)
  await selectPair('local-history', reference.execution.local_id, 90, 80)
  await page.goto(`${server.url}#/ext/harness-e2e/plans/independent-smoke`)
  await page
    .getByRole('heading', { name: 'Independent Smoke', exact: true })
    .waitFor()
  await page.locator('[data-plan-run-history] [data-execution-id]').waitFor()
  assert.deepEqual(
    await page
      .locator('[data-plan-run-history] [data-execution-id]')
      .evaluateAll((rows) => rows.map((row) => row.dataset.executionId)),
    ['independent-history'],
  )
  assert.deepEqual(
    await page
      .locator('#metrics-baseline option')
      .evaluateAll((options) => options.map((option) => option.value)),
    ['independent-history'],
  )
  await page.goto(`${server.url}#/ext/harness-e2e/plans/local-smoke`)
  await page
    .getByRole('heading', { name: 'Local Smoke', exact: true })
    .waitFor()
  await selectPair('local-history', reference.execution.local_id, 90, 80)
  assert.equal(
    calls.filter((call) => call.id.startsWith('release-control::')).length,
    2,
  )
  for (const [a, b, scoreA, scoreB, sources] of [
    [
      'local-history',
      reference.execution.local_id,
      90,
      80,
      ['local', 'imported'],
    ],
    [
      reference.execution.local_id,
      'local-history',
      80,
      90,
      ['imported', 'local'],
    ],
    [
      reference.execution.local_id,
      earlierReference.execution.local_id,
      80,
      60,
      ['imported', 'imported'],
    ],
  ]) {
    await selectPair(a, b, scoreA, scoreB)
    for (const [index, source] of sources.entries()) {
      const side = index === 0 ? 'A' : 'B'
      await page
        .getByRole('button', {
          name: `Transcript ${side} for Alpha`,
          exact: true,
        })
        .click()
      const dialog = page.getByRole('dialog')
      await dialog
        .getByText(`Retained ${source} transcript.`, { exact: true })
        .waitFor()
      await dialog
        .getByRole('button', { name: 'Close session transcript', exact: true })
        .click()
    }
  }
  await page
    .locator(
      `[data-plan-run-history] [data-execution-id="${reference.execution.local_id}"]`,
    )
    .getByRole('link', { name: /^Open report for / })
    .click()
  await page.locator('.execution-page [data-identity-band]').waitFor()
  assert.equal(
    await page
      .getByText(
        'Historical evidence imported locally; metrics use the retained run ledger.',
        { exact: true },
      )
      .count(),
    0,
  )
  await page
    .getByRole('button', { name: 'View transcript for Alpha', exact: true })
    .click()
  await page
    .getByRole('dialog')
    .getByText('Retained imported transcript.', { exact: true })
    .waitFor()
  await page
    .getByRole('button', { name: 'Close session transcript', exact: true })
    .click()
  assert.equal(
    await page
      .getByRole('button', { name: 'Delete execution', exact: true })
      .count(),
    0,
  )
  reference.runs[0].status = 'hard_gate_failed'
  await page.reload()
  await page.locator('.execution-header [data-status="failed"]').waitFor()
  await page
    .getByRole('checkbox', {
      name: 'Exclude tests with zero score or failures',
    })
    .check()
  await page.getByText('All scenarios excluded', { exact: true }).waitFor()
  assert.equal(await page.locator('[data-scenario-row]').count(), 0)
  await page
    .getByRole('checkbox', {
      name: 'Exclude tests with zero score or failures',
    })
    .uncheck()
  await page.locator('[data-scenario-row]').waitFor()
  reference.runs[0].status = 'passed'
  locals.push({ ...detail('local-third'), plan_id: 'local-smoke' })
  additionalPlans[0].candidate_execution_ids.push('local-third')
  unavailableExecutionId = earlierReference.execution.local_id
  await page.goto(`${server.url}#/ext/harness-e2e/plans/${imported.id}`)
  await page.reload()
  await page.locator('#metrics-baseline').waitFor()
  await selectPair(reference.execution.local_id, 'local-history', 80, 90)
  await page
    .getByText('Historical execution is unavailable', { exact: true })
    .waitFor()
  assert.ok(
    (
      await page.locator('[data-trend-metric="score"] title').allTextContents()
    ).some((label) => label.endsWith(' · 70')),
  )
  unavailableExecutionId = null
  locals.length = 0
  additionalPlans.length = 0
  imported.execution_ids = []
  await page.goto(`${server.url}#/ext/harness-e2e/plans/${imported.id}`)
  await page.getByRole('heading', { name: 'RC Smoke', exact: true }).waitFor()
  await page
    .getByText('Release Control · rc · smoke', { exact: true })
    .waitFor()
  await page.locator('[data-plan-scope]').waitFor()
  assert.equal(
    await page
      .getByRole('button', { name: 'Update history', exact: true })
      .isEnabled(),
    true,
  )
  assert.equal(
    calls.filter((call) => call.id.startsWith('release-control::')).length,
    2,
  )
  assert.deepEqual(errors, [])
} finally {
  await browser.close()
  await server.close()
}
