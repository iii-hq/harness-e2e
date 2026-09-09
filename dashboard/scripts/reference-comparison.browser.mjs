// No models run. Exercise the Console RPC boundary in the real page.
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { chromium } from 'playwright'
import { createServer } from 'vite'

const server = await createServer({ server: { host: '127.0.0.1', port: 0 } })
await server.listen()
const browser = await chromium.launch()
const page = await browser.newPage()
page.setDefaultTimeout(5_000)
const errors = []
page.on('pageerror', (error) => {
  errors.push(error.message)
  console.error(error.message)
})
const builtIndex = await readFile(
  new URL('../dist/index.html', import.meta.url),
  'utf8',
)
const css = builtIndex.match(/href="([^"]+\.css)"/)[1].replace(/^\.\//, '')
const html = `<!doctype html><html lang="en" data-harness-e2e="standalone" data-theme="light"><head><meta name="viewport" content="width=device-width, initial-scale=1.0"><link rel="stylesheet" href="/dist/${css}"></head><body><div id="root"></div><script type="module">
import React from 'react';
import {createRoot} from 'react-dom/client';
import {App} from '/src/App.tsx';
import {installDashboardIiiClient} from '/src/lib/iii-client.ts';
import {installDashboardRuntimeConfig} from '/src/lib/dashboard-data-source.ts';
const reference={execution:{id:'remote-1',campaignId:'campaign',planKey:'smoke',attempt:1,trigger:'manual',requestedBy:null,label:'RC Smoke',phase:'complete',terminal:true,resultState:'complete',requestedAt:'2026-09-08T12:00:00Z',completedAt:'2026-09-08T12:01:00Z',error:null,runCount:1,reportCount:1,plan:{name:'Smoke',subject:{model:'test',provider:'test'},judge:{model:'judge',provider:'test'}},request:{}},aggregate:{planned_runs:1,observed_runs:1,completion_rate:1,execution_reliability:1},runs:[{attemptsComplete:true,scenarioId:'alpha',scenarioVersion:1,caseId:'case-a',seed:'42',repetition:0,technical:'valid',completion:'completed',objectiveScore:80,wallTimeMs:1000,totalTokens:100,costSubjectUsd:0.1,turns:1,functionCalls:1,functionCallErrors:0}],materialized:{profile:{id:'smoke',repetitions:1,technical_retries:0},campaigns:[{groups:[{scenarios:['alpha']}]}]},shards:[{runs:[{scenario_id:'alpha',case_id:'case-a',seed:'42'}]}]};
const detail=id=>({id,label:id,status:'passed',subjects:[],totals:{total_tokens:80,report_coverage:1},reports:[{subject_id:'test',scenario_id:'alpha',available:true,report:{scenarios:[{scenario_id:'alpha',case_id:'case-a',case:{seed:42},runs:[{run_id:'r1',technical:'valid',completion:'completed',objective_score:90,efficiency:{total_tokens:70},metrics:{complete:true,totals:{cache_read_tokens:10}}}]}]}}]});
const makePlan=(attempt=null)=>({schema_version:1,id:'imported-reference',label:'RC Smoke local',purpose:'reference',created_at:'2026-09-08T12:02:00Z',updated_at:'2026-09-08T12:02:00Z',state:attempt?'baseline_ready':'draft',locked:Boolean(attempt),scope_hash:'scope',url:'http://local',model:'test',provider:'test',judge_model:'judge',judge_provider:'test',scenarios:[{scenario_id:'alpha',scenario_version:1,case_id:'case-a',seed:42,inputs_sha256:'inputs',contract_sha256:'contract',complexity_tier:'l1_sequential'}],scenario_ids:['alpha'],runs:1,technical_retries:0,seed:null,baseline_execution_id:attempt,candidate_execution_ids:[],incomplete_execution_ids:[],last_attempt_id:attempt,reference_execution_id:'remote-1'}); const locals=[];let imported=null,starts=0;window.calls=[];window.disconnected=false;
installDashboardIiiClient({browserId:'personal',on:()=>()=>{},registerTrigger:()=>()=>{},async trigger(id,payload){window.calls.push({id,payload});
 if(id.startsWith('release-control::')&&window.disconnected)throw new Error('RC tab disconnected');
 if(id==='release-control::test-plans::list')return {plans:[{key:'smoke',recentExecutions:[reference.execution]}]};
 if(id==='release-control::test-executions::reference')return reference;
 if(id==='plans_list')return {mode:'local',plans:imported?[structuredClone(imported)]:[]};
 if(id==='plan_get')return structuredClone(imported);
 if(id==='executions_list')return {executions:structuredClone(locals),total:locals.length};
 if(id==='execution_get')return {detail:structuredClone(locals.find(item=>item.id===payload.execution_id))};
 if(id==='catalog_get')return {url:'http://local',models:[{provider:'test',model:'test'}],scenarios:['alpha'],local_scenarios:[]};
 if(id==='plan_control'&&payload.action==='import_reference'){imported=makePlan(starts?'local-1':null);return structuredClone(imported)};
 if(id==='plan_run_start'){starts++;const next=detail('local-'+starts);next.status='running';locals.unshift(next);imported={...imported,state:payload.role==='baseline'?'baseline_running':'candidate_running',locked:true,baseline_execution_id:payload.role==='baseline'?next.id:imported.baseline_execution_id,candidate_execution_ids:payload.role==='candidate'?[next.id]:[],last_attempt_id:next.id};return structuredClone(imported)};
 if(id==='plan_control'&&payload.action==='cancel'){locals.find(item=>item.id===payload.execution_id).status='cancelled';imported={...imported,state:'baseline_ready',last_attempt_id:'local-1'};return {}};
 if(id==='test_history_get')return {test_id:'alpha',test_version:1,available_versions:[],cases:['case-a'],subjects:[],subject_models:[],judge_models:[],systems:[],series:[],observations:[],total:0,next_cursor:null};
 if(id==='tests_list')return {rows:[{test_id:'alpha',lifecycle:'active',current_version:1,available_versions:[]}],total:1,next_cursor:null};
 throw new Error('Unexpected RPC '+id);
}});
installDashboardRuntimeConfig({mode:'local',transport:'iii',http_fallback:false,page_size:50,functions:Object.fromEntries(['executions_list','execution_get','evaluated_versions_list','tests_list','test_version_get','test_history_get','catalog_get','local_scenario_create','run_status','run_start','run_cancel','plan_control','plans_list','plan_get','plan_create','plan_update','plan_run_start','changed_trigger'].map(id=>[id,id]))});
location.hash = '#/plans';
createRoot(document.getElementById('root')).render(React.createElement(App));
</script></body></html>`
try {
  await page.route('**/__reference-test', async (route) =>
    route.fulfill({
      contentType: 'text/html',
      body: await server.transformIndexHtml('/__reference-test', html),
    }),
  )
  await page.goto(`${server.resolvedUrls.local[0]}__reference-test`)
  await page.getByRole('button', { name: 'Reference: Release Control' }).click()
  await page
    .getByRole('table', { name: 'Reference plans from Release Control' })
    .waitFor()
  await page.getByRole('link', { name: 'open' }).click()
  await page.getByRole('heading', { name: 'Smoke' }).waitFor()
  assert.equal(
    await page.evaluate(() =>
      window.calls.some((c) => c.id === 'plan_control'),
    ),
    false,
  )
  await page.getByRole('button', { name: 'run reference locally' }).click()
  await page
    .getByRole('heading', { name: 'Run this reference locally' })
    .waitFor()
  await page.getByRole('button', { name: 'run on current Harness' }).click()
  await page.getByRole('button', { name: 'Candidate B' }).waitFor()
  const first = await page.evaluate(() =>
    window.calls.find((c) => c.id === 'plan_control'),
  )
  assert.equal(first.payload.reference_execution_id, 'remote-1')
  assert.deepEqual(first.payload.shards, [
    { runs: [{ scenario_id: 'alpha', case_id: 'case-a', seed: '42' }] },
  ])
  assert.equal(first.payload.materialized.profile.repetitions, 1)
  await page.getByRole('button', { name: 'cancel' }).last().click()
  assert.ok(
    await page.evaluate(() =>
      window.calls.some(
        (c) =>
          c.payload?.action === 'cancel' &&
          c.payload.execution_id === 'local-1',
      ),
    ),
  )
  await page.getByRole('button', { name: 'run reference locally' }).click()
  await page.getByRole('button', { name: 'run on current Harness' }).click()
  await page.getByRole('button', { name: 'Candidate B' }).waitFor()
  const starts = await page.evaluate(() =>
    window.calls.filter((c) => c.id === 'plan_run_start'),
  )
  assert.deepEqual(
    starts.map((c) => c.payload.role),
    ['baseline', 'candidate'],
  )
  assert.notEqual(
    starts[0].payload.idempotency_key,
    starts[1].payload.idempotency_key,
  )
  await page.getByRole('button', { name: 'Candidate B' }).click()
  await page.locator('[data-comparison-metric="tokens"]').waitFor()
  assert.match(
    await page.locator('[data-comparison-metric="tokens"]').innerText(),
    /100[\s\S]*80/,
  )
  const scoreFilter = page.getByRole('checkbox', {
    name: 'Exclude tests with a zero or missing result in A or B',
  })
  assert.equal(await scoreFilter.isChecked(), false)
  await scoreFilter.check()
  await page
    .getByText('1 test retained on both sides.', { exact: true })
    .waitFor()
  assert.match(
    await page.locator('[data-comparison-metric="tokens"]').innerText(),
    /100[\s\S]*80/,
  )
  await scoreFilter.uncheck()
  const scenario = page.getByRole('link', { name: 'alpha' })
  assert.equal(
    await scenario.getAttribute('href'),
    '#/tests/alpha?reference=remote-1&candidate=local-2',
  )
  await page.screenshot({ path: '/tmp/e2e-reference-plan.png', fullPage: true })
  await page.evaluate(() => {
    location.hash = '#/tests/alpha?reference=remote-1&candidate=local-2'
  })
  await page.getByRole('heading', { name: /alpha/ }).waitFor()
  await page.locator('[data-test-comparison]').waitFor()
  assert.match(
    await page.locator('[data-test-comparison]').innerText(),
    /remote-1[\s\S]*local-2/,
  )
  assert.match(
    await page.locator('[data-comparison-metric="tokens"]').innerText(),
    /100[\s\S]*80/,
  )
  assert.equal(
    await page
      .getByRole('link', { name: 'back to reference plan' })
      .getAttribute('href'),
    '#/plans/rc%3Asmoke',
  )
  await page.screenshot({
    path: '/tmp/e2e-reference-scenario.png',
    fullPage: true,
  })
  assert.ok(
    await page.evaluate(
      () =>
        window.calls.some((call) => call.id === 'test_history_get') &&
        window.calls.some((call) => call.id === 'tests_list'),
    ),
  )
  await page.evaluate(() => {
    window.disconnected = true
    location.hash = '#/plans/imported-reference'
  })
  await page.getByRole('heading', { name: 'RC Smoke local' }).waitFor()
  await page.setViewportSize({ width: 390, height: 844 })
  await page.evaluate(() => {
    window.disconnected = false
    location.hash = '#/plans/rc:smoke'
  })
  await page.getByRole('heading', { name: 'Smoke' }).waitFor()
  await page
    .getByRole('table', { name: 'Smoke shared and local history' })
    .waitFor()
  await page.locator('[data-comparison-metric="tokens"]').waitFor()
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
    true,
  )
  await page.evaluate(() => {
    location.hash = '#/executions'
  })
  await page.getByRole('heading', { name: /executions/i }).waitFor()
  assert.equal(
    await page
      .getByRole('button', { name: 'Reference: Release Control' })
      .count(),
    0,
  )
  assert.ok(
    await page.evaluate(() =>
      window.calls
        .filter((c) => c.id.startsWith('release-control::'))
        .every((c) =>
          [
            'release-control::test-plans::list',
            'release-control::test-executions::reference',
          ].includes(c.id),
        ),
    ),
  )
  assert.deepEqual(errors, [])
  console.log(
    'Reference browser flow passed: compare, rerun, cancel, disconnect, no RC writes.',
  )
} catch (error) {
  console.error(await page.locator('body').innerText(), errors)
  throw error
} finally {
  await browser.close()
  await server.close()
}
