#!/usr/bin/env node

import { spawn } from 'node:child_process'
import { chmod, mkdir, readFile, writeFile } from 'node:fs/promises'
import { isAbsolute, join } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { randomUUID } from 'node:crypto'

const MAX_BYTES = 256 * 1024
const MAX_COMMAND_BYTES = 64 * 1024
const TERMINAL = new Set(['completed', 'cancelled', 'failed'])
let failureOutput
let evidence
let capturedTranscript
const usage = `Usage: subject.mjs --container <64-hex-id> --keeper <pid> --prompt-file <trusted-file> --output <private-directory> --engine-url <ws-url> --namespace <namespace> --model <model> --provider <provider>

Environment:
  III_SDK_MODULE  Absolute path to the trusted iii-sdk module

Other:
  --help          Show this help`

function argumentsOf(argv) {
  const values = {}
  for (let i = 0; i < argv.length; i += 2) {
    if (!argv[i]?.startsWith('--') || argv[i + 1] === undefined) throw new Error(`Invalid argument: ${argv[i] ?? ''}`)
    values[argv[i].slice(2)] = argv[i + 1]
  }
  return values
}

function run(command, args, timeoutMs, cap = MAX_BYTES, input) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      stdio: [input === undefined ? 'ignore' : 'pipe', 'pipe', 'pipe'],
      env: { ...process.env, III_TELEMETRY_ENABLED: 'false' },
    })
    const chunks = { stdout: [], stderr: [] }
    let bytes = 0
    let settled = false
    const fail = (error) => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      child.kill('SIGKILL')
      reject(error)
    }
    for (const name of ['stdout', 'stderr']) child[name].on('data', (chunk) => {
      bytes += chunk.length
      if (bytes > cap) {
        const error = new Error(`command output exceeded ${cap} bytes`)
        error.bounded = true
        return fail(error)
      }
      chunks[name].push(chunk)
    })
    child.on('error', fail)
    if (input !== undefined) child.stdin.end(input)
    child.on('close', (code, signal) => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      resolve({
        exit_code: code,
        signal,
        stdout: Buffer.concat(chunks.stdout).toString('utf8'),
        stderr: Buffer.concat(chunks.stderr).toString('utf8'),
      })
    })
    const timer = setTimeout(() => {
      const error = new Error(`command timed out after ${timeoutMs}ms`)
      error.bounded = true
      fail(error)
    }, timeoutMs)
  })
}

function requireCandidate(container, inspected) {
  if (!Array.isArray(inspected) || inspected.length !== 1) throw new Error('docker inspect returned an unexpected result')
  const value = inspected[0]
  const tmpfs = value?.HostConfig?.Tmpfs ?? {}
  const sizedTmpfs = (path) => typeof tmpfs[path] === 'string' && /(?:^|,)size=\d+[kKmMgG]?(?:,|$)/.test(tmpfs[path])
  const user = value?.Config?.User
  if (value?.Id !== container || value?.State?.Running !== true) throw new Error('candidate container identity/running state mismatch')
  if (value?.Config?.Labels?.['kanban-eval.role'] !== 'candidate') throw new Error('candidate container ownership label is missing')
  if (value?.HostConfig?.NetworkMode !== 'none' || value?.HostConfig?.ReadonlyRootfs !== true) throw new Error('candidate container network/rootfs isolation is invalid')
  if (!value?.HostConfig?.CapDrop?.some((capability) => capability.toUpperCase() === 'ALL')) throw new Error('candidate container must drop ALL capabilities')
  if (typeof user !== 'string' || !user || /^(?:root|0)(?::|$)/.test(user)) throw new Error('candidate container must use a non-root user')
  if (!sizedTmpfs('/workspace') || !sizedTmpfs('/data')) throw new Error('candidate container requires sized tmpfs at /workspace and /data')
}

function trigger(iii, namespace, functionId, payload, timeoutMs = 10_000) {
  return iii.trigger({ function_id: functionId, namespace, payload, timeoutMs })
}

async function transcript(iii, namespace, sessionId) {
  const messages = []
  let cursor
  const deadline = Date.now() + 60_000
  do {
    const remaining = deadline - Date.now()
    if (remaining <= 0) throw new Error('transcript capture timed out after 60 seconds')
    const page = await trigger(iii, namespace, 'session::messages', {
      session_id: sessionId, limit: 500, ...(cursor ? { cursor } : {}), include_custom: true,
    }, Math.min(10_000, remaining))
    if (!Array.isArray(page?.messages)) throw new Error('session::messages returned a malformed page')
    messages.push(...page.messages)
    const next = page.next_cursor
    if (next && next === cursor) throw new Error('session::messages repeated its cursor')
    cursor = next
  } while (cursor)
  return { messages }
}

async function completeMetrics(iii, namespace, sessionId) {
  const deadline = Date.now() + 30_000
  for (;;) {
    const metrics = await trigger(iii, namespace, 'harness::metrics', { root_session_id: sessionId }, 10_000)
    if (metrics?.complete) return metrics
    if (Date.now() >= deadline) throw new Error('harness::metrics remained incomplete after 30 seconds')
    await new Promise((resolve) => setTimeout(resolve, 500))
  }
}

function requireUsage(metrics) {
  const totals = metrics?.totals ?? {}
  for (const name of ['input_tokens', 'output_tokens', 'cost_usd']) {
    if (!Number.isFinite(totals[name]) || totals[name] < 0) throw new Error(`harness::metrics omitted valid ${name}`)
  }
  for (const name of ['cache_read_tokens', 'cache_write_tokens']) {
    if (totals[name] != null && (!Number.isFinite(totals[name]) || totals[name] < 0)) throw new Error(`harness::metrics returned invalid ${name}`)
  }
  return totals
}

async function main() {
  if (process.argv.includes('--help')) { console.log(usage); return }
  const started = Date.now()
  const args = argumentsOf(process.argv.slice(2))
  for (const name of ['container', 'keeper', 'prompt-file', 'output', 'engine-url', 'namespace', 'model', 'provider']) {
    if (!args[name]) throw new Error(`Missing --${name}`)
  }
  if (!/^[0-9a-f]{64}$/.test(args.container)) throw new Error('--container must be an exact 64-hex Docker ID')
  if (!/^\d+$/.test(args.keeper) || Number(args.keeper) <= 1) throw new Error('--keeper must be a process ID greater than one')
  if (!isAbsolute(args['prompt-file']) || !isAbsolute(args.output)) throw new Error('--prompt-file and --output must be absolute paths')
  if (!args['engine-url'].startsWith('ws://') && !args['engine-url'].startsWith('wss://')) throw new Error('--engine-url must be a WebSocket URL')
  const sdkPath = process.env.III_SDK_MODULE
  if (!sdkPath || !isAbsolute(sdkPath)) throw new Error('III_SDK_MODULE must be an absolute path')

  await mkdir(args.output, { recursive: true, mode: 0o700 })
  await chmod(args.output, 0o700)
  failureOutput = args.output
  evidence = {
    schema: 'kanban-subject/v1',
    started_at: new Date(started).toISOString(),
    duration_ms: null,
    session_id: null,
    turn_id: null,
    status: 'infrastructure_failed',
    result_error: null,
    requested_model: args.model,
    requested_provider: args.provider,
    actual_model: null,
    actual_provider: null,
    input_tokens: null,
    output_tokens: null,
    cache_read_tokens: null,
    cache_write_tokens: null,
    reasoning_tokens: null,
    cost_usd: null,
    metrics_complete: false,
    cost_cap_usd: null,
    send_attempted: false,
    model_invoked: false,
  }
  const prompt = await readFile(args['prompt-file'], 'utf8')
  const inspection = await run('docker', ['inspect', args.container], 10_000, 64 * 1024)
  if (inspection.exit_code !== 0) throw new Error(`docker inspect failed: ${inspection.stderr.trim()}`)
  requireCandidate(args.container, JSON.parse(inspection.stdout))

  const sdk = await import(pathToFileURL(sdkPath).href)
  const registerWorker = sdk.registerWorker ?? sdk.default?.registerWorker
  if (!registerWorker) throw new Error('Trusted iii-sdk has no registerWorker export')
  const iii = registerWorker(args['engine-url'], {
    workerName: `kanban-subject-${process.pid}`,
    workerDescription: 'Trusted isolated Kanban subject bridge',
    namespace: args.namespace,
  })
  const nonce = randomUUID().replaceAll('-', '')
  const functionId = `kanban_eval_${process.pid}_${nonce}::exec`
  const execPath = fileURLToPath(new URL('./exec.py', import.meta.url))
  let registration
  let sessionId = `kanban-eval-${nonce}`
  let status
  let metrics
  let sent
  let commandFailure
  try {
    registration = iii.registerFunction(functionId, async (payload) => {
      const { _caller_worker_id: _callerWorkerId, ...request } = payload ?? {}
      if (Object.keys(request).length !== 1 || typeof request.command !== 'string') throw new Error('command must be the only request field')
      if (Buffer.byteLength(request.command) > MAX_COMMAND_BYTES) throw new Error(`command exceeds ${MAX_COMMAND_BYTES} bytes`)
      try {
        const execution = await run('/usr/bin/python3', [execPath, args.container, args.keeper],
          140_000, MAX_BYTES * 6 + 4096, request.command)
        if (execution.exit_code !== 0) throw new Error(`isolated command boundary failed: ${execution.stderr.trim()}`)
        return JSON.parse(execution.stdout)
      } catch (error) {
        if (error?.bounded || error instanceof SyntaxError || error?.message?.startsWith('isolated command boundary failed:')) {
          commandFailure = error
          await run('docker', ['rm', '-f', args.container], 10_000, 64 * 1024).catch(() => {})
        }
        throw error
      }
    }, {
      description: 'Run one bounded shell command inside the fixed isolated candidate container.',
      request_format: { type: 'object', additionalProperties: false, required: ['command'], properties: { command: { type: 'string', maxLength: MAX_COMMAND_BYTES } } },
      response_format: { type: 'object' },
    })

    const preflight = await trigger(iii, args.namespace, functionId, { command: 'pwd' })
    if (preflight?.exit_code !== 0 || preflight?.stdout?.trim() !== '/workspace') throw new Error('candidate function preflight did not run in /workspace')

    const catalog = await trigger(iii, args.namespace, 'router::models::get', { provider: args.provider, id: args.model })
    const pricing = catalog?.model?.pricing
    const priced = Number.isFinite(pricing?.input) && pricing.input >= 0 && Number.isFinite(pricing?.output) && pricing.output >= 0
    if (!priced) throw new Error(`model ${args.provider}/${args.model} has no enforceable input/output pricing`)
    evidence.cost_cap_usd = 5
    const request = {
      session_id: sessionId,
      message: `${prompt}\n\nExecution environment: your repository is /workspace. Dependencies are installed; external networking is disabled. Execute shell commands through agent_trigger with {"function":"${functionId}","description":"Inspect repository","payload":{"command":"pwd"}}. This function executes commands, it does not delegate tasks. Each command is limited to 120 seconds and 256 KiB of output. A command that reaches either limit returns nonzero feedback after its background processes are stopped; use a narrower command and continue. Inspect, edit and test the repository using this tool; describing a tool call does not execute it.`,
      model: args.model,
      provider: args.provider,
      idempotency_key: `kanban-eval:${nonce}`,
      session: { title: `Kanban isolated smoke ${nonce.slice(0, 8)}` },
      options: {
        max_turns: 100,
        max_output_tokens: 65536,
        max_total_tokens: 1000000,
        max_validation_retries: 0,
        max_cost_usd: 5,
        functions: {
          expose: 'agent_trigger',
          allow: [functionId],
          deny: ['harness::spawn', 'shell::*', 'coder::*', 'compose::*', 'router::*', 'harness::send', 'harness::run'],
        },
        skills: [],
      },
    }
    evidence.session_id = sessionId
    evidence.send_attempted = true
    sent = await trigger(iii, args.namespace, 'harness::send', request)
    if (!sent?.accepted || !sent.session_id || !sent.turn_id) throw new Error('harness::send did not accept a new turn')
    if (sent.session_id !== sessionId) throw new Error('harness::send returned a different session_id')
    evidence.model_invoked = true
    const deadline = Date.now() + 1_800_000
    do {
      status = await trigger(iii, args.namespace, 'harness::status', { session_id: sessionId }, 15_000)
      if (commandFailure) throw commandFailure
      if (!status) throw new Error('harness::status returned no session')
      if (TERMINAL.has(status.status) && !status.expects_wake) break
      if (Date.now() >= deadline) throw new Error('subject timed out after 1800 seconds')
      await new Promise((resolve) => setTimeout(resolve, 1_000))
    } while (true)
    metrics = await completeMetrics(iii, args.namespace, sessionId)
    capturedTranscript = await transcript(iii, args.namespace, sessionId)
    const assistants = capturedTranscript.messages.map((entry) => entry?.message).filter((message) => message?.role === 'assistant')
    const actual = assistants.at(-1) ?? {}
    if (actual.model !== args.model || actual.provider !== args.provider) throw new Error('observed model/provider identity does not match the request')
    const totals = requireUsage(metrics)
    evidence = {
      ...evidence,
      session_id: sessionId,
      turn_id: sent.turn_id,
      duration_ms: Date.now() - started,
      status: status.status,
      result_error: status.result_error ?? null,
      requested_model: args.model,
      requested_provider: args.provider,
      actual_model: actual.model ?? null,
      actual_provider: actual.provider ?? null,
      input_tokens: totals.input_tokens ?? null,
      output_tokens: totals.output_tokens ?? null,
      cache_read_tokens: totals.cache_read_tokens ?? null,
      cache_write_tokens: totals.cache_write_tokens ?? null,
      reasoning_tokens: totals.reasoning_tokens ?? null,
      cost_usd: totals.cost_usd ?? null,
      metrics_complete: metrics?.complete ?? false,
      cost_cap_usd: 5,
    }
    await writeFile(join(args.output, 'transcript.json'), `${JSON.stringify(capturedTranscript, null, 2)}\n`, { mode: 0o600 })
    await writeFile(join(args.output, 'subject.json'), `${JSON.stringify(evidence, null, 2)}\n`, { mode: 0o600 })
    console.log(JSON.stringify(evidence))
    if (status.status !== 'completed') process.exitCode = 2
  } catch (error) {
    if (evidence.send_attempted && sessionId) {
      await trigger(iii, args.namespace, 'harness::stop', { session_id: sessionId }, 10_000).catch(() => {})
      metrics ??= await completeMetrics(iii, args.namespace, sessionId).catch(() => null)
      capturedTranscript ??= await transcript(iii, args.namespace, sessionId).catch(() => null)
    }
    const totals = metrics?.totals ?? {}
    const actual = capturedTranscript?.messages?.map((entry) => entry?.message).filter((message) => message?.role === 'assistant').at(-1) ?? {}
    evidence = {
      ...evidence,
      session_id: sessionId ?? null,
      turn_id: sent?.turn_id ?? null,
      duration_ms: Date.now() - started,
      status: ['failed', 'cancelled'].includes(status?.status) ? status.status : 'evaluation_failed',
      result_error: error instanceof Error ? error.message : String(error),
      actual_model: actual.model ?? null,
      actual_provider: actual.provider ?? null,
      input_tokens: totals.input_tokens ?? null,
      output_tokens: totals.output_tokens ?? null,
      cache_read_tokens: totals.cache_read_tokens ?? null,
      cache_write_tokens: totals.cache_write_tokens ?? null,
      reasoning_tokens: totals.reasoning_tokens ?? null,
      cost_usd: totals.cost_usd ?? null,
      metrics_complete: metrics?.complete ?? false,
    }
    throw error
  } finally {
    if (evidence.send_attempted && sessionId) {
      await trigger(iii, args.namespace, 'harness::stop', { session_id: sessionId }, 10_000).catch(() => {})
      await trigger(iii, args.namespace, 'harness::teardown', { root_session_id: sessionId }, 30_000).catch(() => {})
    }
    await registration?.unregister?.()
    await iii.shutdown?.()
  }
}

main().catch(async (error) => {
  const message = error instanceof Error ? error.message : String(error)
  if (failureOutput && evidence) {
    evidence.result_error ??= message
    evidence.duration_ms ??= Date.now() - Date.parse(evidence.started_at)
    const writes = [writeFile(join(failureOutput, 'subject.json'), `${JSON.stringify(evidence, null, 2)}\n`, { mode: 0o600 })]
    if (capturedTranscript) writes.push(writeFile(join(failureOutput, 'transcript.json'), `${JSON.stringify(capturedTranscript, null, 2)}\n`, { mode: 0o600 }))
    await Promise.all(writes).catch(() => {})
  }
  console.error(message)
  process.exitCode = 2
})
