// Functional Console RPC coverage. No models run; real-host visuals are checked separately.
import assert from 'node:assert/strict'
import { mkdir, readFile } from 'node:fs/promises'
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
      judge: { model: 'judge', provider: 'test' },
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
      scenarioVersion: 1,
      caseId: 'case-a',
      seed: '42',
      repetition: 0,
      technical: 'valid',
      completion: 'completed',
      objectiveScore: 80,
      wallTimeMs: 1000,
      totalTokens: 100,
      costSubjectUsd: 0.1,
      turns: 1,
      functionCalls: 1,
      functionCallErrors: 0,
    },
  ],
  materialized: {
    profile: { id: 'smoke', repetitions: 1, technical_retries: 0 },
    campaigns: [{ groups: [{ scenarios: ['alpha'] }] }],
  },
  shards: [{ runs: [{ scenario_id: 'alpha', case_id: 'case-a', seed: '42' }] }],
}
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
            case_id: 'case-a',
            case: { seed: 42 },
            runs: [
              {
                run_id: 'r1',
                technical: 'valid',
                completion: 'completed',
                objective_score: 90,
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
let imported = null,
  starts = 0
const calls = []
let disconnected = false

const trigger = async (name, payload = {}) => {
  const id = name.replace('e2e::dashboard::', '').replaceAll('-', '_')
  // Release Control RPC names retain hyphens.
  const rpc = name.startsWith('release-control::') ? name : id
  calls.push({ id: rpc, payload })
  return route(rpc, payload)
}
async function route(id, payload) {
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
      plans: imported ? [structuredClone(imported)] : [],
    }
  if (id === 'plan_get') return structuredClone(imported)
  if (id === 'executions_list')
    return {
      executions: [
        {
          id: 'remote-execution-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
          origin: 'remote',
          label: 'RC Smoke',
          status: 'completed',
          subjects: [],
        },
        ...structuredClone(locals),
      ],
      total: locals.length + 1,
    }
  if (
    id === 'execution_get' &&
    payload.execution_id.startsWith('remote-execution-')
  )
    return {
      detail: {
        id: payload.execution_id,
        origin: 'remote',
        label: 'RC Smoke',
        status: 'completed',
        subjects: [],
        reports: [],
        plan_id: 'remote-plan-smoke',
        remote_reference: reference,
        retained_reports: [],
      },
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
      local_scenarios: [],
    }
  if (id === 'plan_control' && payload.action === 'import_history') {
    imported = {
      origin: 'remote',
      id: 'remote-plan-smoke',
      label: 'RC Smoke',
      purpose: 'imported',
      created_at: null,
      updated_at: '2026-09-08T12:00:00Z',
      template_id: null,
      source: {
        instance_id: 'rc',
        plan_key: 'smoke',
        captured_at: '2026-09-08T12:00:00Z',
        active: true,
        limitation: null,
      },
      configuration: null,
      execution_ids: [
        'remote-execution-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
      ],
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
      test_version: 1,
      available_versions: [],
      cases: ['case-a'],
      subjects: [],
      subject_models: [],
      judge_models: [],
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
          median_score: 90,
          run_count: 1,
          scored_runs: 1,
          scenario_version: 1,
          seed: 1,
          stack_mode: 'source',
          subject_provider: 'test',
          subject_model: 'test',
          judge_provider: 'test',
          judge_model: 'judge',
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
          current_version: 1,
          available_versions: [],
        },
      ],
      total: 1,
      next_cursor: null,
    }
  throw new Error(`Unexpected RPC ${id}`)
}

const screenshots = '/tmp/rc-harness-unification-d2qr7zx1/ui'
await mkdir(screenshots, { recursive: true })
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
      call.id === 'plan_control' && call.payload.action === 'import_history',
  )
  assert.deepEqual(importCall.payload.history, capturedTransport)
  await page.goto(
    `${server.url}#/ext/harness-e2e/execution/remote-execution-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa`,
  )
  await page.getByText('Historical run ledger', { exact: true }).waitFor()
  disconnected = true
  await page.reload()
  await page.getByText('Historical run ledger', { exact: true }).waitFor()
  locals.push(detail('local-history'))
  await page.goto(
    `${server.url}#/ext/harness-e2e/tests/alpha?reference=remote-execution-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa&candidate=local-history`,
  )
  await page
    .getByRole('button', { name: 'Reference: Release Control', exact: true })
    .waitFor()
  await page.locator('[data-test-comparison]').waitFor()
  assert.equal(
    calls.filter((call) => call.id.startsWith('release-control::')).length,
    2,
  )
  imported.execution_ids = []
  await page.goto(`${server.url}#/ext/harness-e2e/plans/${imported.id}`)
  await page.getByRole('heading', { name: 'RC Smoke', exact: true }).waitFor()
  await page
    .getByText('Release Control · rc · smoke', { exact: true })
    .waitFor()
  await page
    .getByText('No executions in this local copy', { exact: true })
    .waitFor()
  assert.equal(
    await page
      .getByRole('button', { name: 'update history', exact: true })
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
