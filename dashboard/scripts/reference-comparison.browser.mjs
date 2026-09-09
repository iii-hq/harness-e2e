// No models run. Exercise the Console RPC boundary in the real page.
import assert from 'node:assert/strict'
import { chromium } from 'playwright'
import { createServer } from 'vite'

const server = await createServer({ server: { host: '127.0.0.1', port: 0 } })
await server.listen()
const browser = await chromium.launch()
const page = await browser.newPage()
const errors = []
page.on('pageerror', (error) => {
  errors.push(error.message)
  console.error(error.message)
})
const html = `<!doctype html><div id="root"></div><script type="module">
import React from 'react';
import {createRoot} from 'react-dom/client';
import {ExecutionsPage} from '/src/pages/ExecutionsPage.tsx';
import {installDashboardIiiClient} from '/src/lib/iii-client.ts';
import {installDashboardRuntimeConfig} from '/src/lib/dashboard-data-source.ts';
const reference={execution:{id:'remote-1',campaignId:'campaign',planKey:'smoke',attempt:1,trigger:'manual',label:'RC Smoke',phase:'complete',terminal:true,resultState:'complete',requestedAt:'2026-09-08T12:00:00Z',completedAt:'2026-09-08T12:01:00Z',runCount:1,reportCount:1,plan:{subject:{model:'test',provider:'test'}},request:{}},aggregate:{planned_runs:1,observed_runs:1,completion_rate:1,execution_reliability:1},runs:[{attemptsComplete:true,scenarioId:'alpha',scenarioVersion:1,caseId:'alpha',technical:'valid',completion:'completed',objectiveScore:80,wallTimeMs:1000,totalTokens:100,costSubjectUsd:null,turns:1,functionCalls:1}],materialized:{profile:{id:'smoke',repetitions:1}},shards:[{runs:[{scenario_id:'alpha',seed:'42'}]}]};
const detail=id=>({id,label:id,status:'passed',subjects:[],totals:{total_tokens:80,report_coverage:1},reports:[{subject_id:'test',scenario_id:'alpha',available:true,report:{scenarios:[{scenario_id:'alpha',runs:[{run_id:'r1',technical:'valid',completion:'completed',objective_score:90,efficiency:{total_tokens:70},metrics:{complete:true,totals:{cache_read_tokens:10}}}]}]}}]});
const locals=[detail('local-existing')];let starts=0;window.calls=[];window.disconnected=false;
installDashboardIiiClient({browserId:'personal',on:()=>()=>{},registerTrigger:()=>()=>{},async trigger(id,payload){window.calls.push({id,payload});
 if(id.startsWith('release-control::')&&window.disconnected)throw new Error('RC tab disconnected');
 if(id==='release-control::test-plans::list')return {plans:[{recentExecutions:[reference.execution]}]};
 if(id==='release-control::test-executions::reference')return reference;
 if(id==='executions_list')return {executions:structuredClone(locals),total:locals.length};
 if(id==='execution_get')return {detail:structuredClone(locals.find(item=>item.id===payload.execution_id))};
 if(id==='plan_control'&&payload.action==='import_reference')return {id:'imported-reference',baseline_execution_id:starts?'local-1':null};
 if(id==='plan_run_start'){starts++;const next=detail('local-'+starts);next.status='running';locals.unshift(next);return {id:'imported-reference',last_attempt_id:next.id}};
 if(id==='plan_control'&&payload.action==='cancel'){locals.find(item=>item.id===payload.execution_id).status='cancelled';return {}};
 throw new Error('Unexpected RPC '+id);
}});
installDashboardRuntimeConfig({mode:'local',transport:'iii',http_fallback:false,functions:Object.fromEntries(['executions_list','execution_get','plan_control','plan_run_start','changed_trigger'].map(id=>[id,id]))});
createRoot(document.getElementById('root')).render(React.createElement(ExecutionsPage));
</script>`
try {
  await page.route('**/__reference-test', async (route) =>
    route.fulfill({
      contentType: 'text/html',
      body: await server.transformIndexHtml('/__reference-test', html),
    }),
  )
  await page.goto(`${server.resolvedUrls.local[0]}__reference-test`)
  await page
    .getByRole('button', { name: 'load Release Control history' })
    .click()
  await page.locator('[data-execution-id="rc:remote-1"]').waitFor()
  await page.locator('[data-execution-id="local-existing"]').waitFor()
  assert.equal(
    await page.evaluate(() =>
      window.calls.some((c) => c.id === 'plan_control'),
    ),
    false,
  )
  await page.getByLabel('Release Control reference').selectOption('rc:remote-1')
  await page
    .getByLabel('Local execution for comparison')
    .selectOption('local-existing')
  const table = page.getByRole('table', {
    name: 'Reference and local candidate measurements',
  })
  await table.waitFor()
  assert.match(
    await table.getByRole('row', { name: /Mean objective score/ }).innerText(),
    /80.0\s+90.0\s+10.0/,
  )
  await page.getByRole('button', { name: 'run reference locally' }).click()
  await page.locator('option[value="local-1"]').waitFor({ state: 'attached' })
  const first = await page.evaluate(() =>
    window.calls.find((c) => c.id === 'plan_control'),
  )
  assert.equal(first.payload.reference_execution_id, 'remote-1')
  assert.deepEqual(first.payload.shards, [
    { runs: [{ scenario_id: 'alpha', seed: '42' }] },
  ])
  await page.getByRole('button', { name: 'cancel local execution' }).click()
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
  await page.locator('option[value="local-2"]').waitFor({ state: 'attached' })
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
  await page.evaluate(() => {
    window.disconnected = true
  })
  await page
    .getByRole('button', { name: 'load Release Control history' })
    .click()
  await page.getByText('RC tab disconnected', { exact: true }).waitFor()
  await page.locator('[data-execution-id="local-existing"]').waitFor()
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
