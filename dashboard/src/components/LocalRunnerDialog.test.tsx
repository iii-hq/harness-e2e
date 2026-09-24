import { describe, expect, it } from 'vitest'
import {
  executionStartRequest,
  lastUsedModel,
  runnerForm,
  runningExecutionId,
  withSequentialGroups,
} from '@/components/LocalRunnerDialog'
import type {
  DashboardExecutionSummary,
  ExecutionParameters,
} from '@/lib/dashboard-data-source'

const imported: ExecutionParameters = {
  scenarios: ['minimal_path', 'context_pressure', 'kanban_c1_foundation'],
  runs: 3,
  technical_retries: 0,
  seed: null,
  model: 'gpt-5.6-terra',
  provider: 'openai-codex',
  agent: 'tech-lead',
}

describe('run form', () => {
  it('runs an execution again from its own parameters', () => {
    const form = runnerForm(imported)
    expect(form).toMatchObject({
      label: '',
      subject: 'openai-codex\ngpt-5.6-terra',
      scenarios: imported.scenarios,
      runs: '3',
      technicalRetries: '0',
      seed: '',
      agent: 'tech-lead',
    })
    // Unchanged, the form starts the same parameters again.
    expect(executionStartRequest(form)).toEqual({
      label: '',
      parameters: imported,
    })
    // Seeds stay text end to end, exact beyond 2^53.
    const seed = '18446744073709551615'
    expect(runnerForm({ ...imported, seed }).seed).toBe(seed)
    expect(
      executionStartRequest(runnerForm({ ...imported, seed })).parameters.seed,
    ).toBe(seed)
  })

  it('names the new execution after the one it runs again', () => {
    expect(runnerForm(imported, [], 'Regression').label).toBe('Regression')
    expect(runnerForm(null, [], 'Regression').label).toBe('')
  })

  it('opens with only a chosen subset of the scenarios marked', () => {
    const form = runnerForm(imported, ['context_pressure'])
    expect(form.scenarios).toEqual(['context_pressure'])
    expect(executionStartRequest(form).parameters).toEqual({
      ...imported,
      scenarios: ['context_pressure'],
    })
  })

  it('starts an execution with what Run tests holds', () => {
    const form = {
      ...runnerForm(null, ['minimal_path']),
      label: '  Before the prompt change ',
      subject: 'deepseek\ndeepseek-v4-flash',
      seed: ' 42 ',
      agent: ' ',
    }
    expect(executionStartRequest(form)).toEqual({
      label: 'Before the prompt change',
      parameters: {
        scenarios: ['minimal_path'],
        runs: 1,
        technical_retries: 1,
        seed: '42',
        model: 'deepseek-v4-flash',
        provider: 'deepseek',
        agent: null,
      },
    })
  })
})

describe('Run tests from scratch', () => {
  const catalog = [
    { provider: 'claude-code', model: 'claude-code/claude-fable-5' },
    { provider: 'deepseek', model: 'deepseek-v4-flash' },
  ]
  const execution = (model: string, provider: string) =>
    ({
      id: model,
      parameters: { ...imported, model, provider },
      subjects: [],
    }) as unknown as DashboardExecutionSummary

  it('starts from the model of the latest execution this stack lists', () => {
    expect(
      lastUsedModel(
        [
          // Newest first; the imported model is not in this catalog.
          execution('gpt-5.6-terra', 'openai-codex'),
          execution('deepseek-v4-flash', 'deepseek'),
          execution('claude-code/claude-fable-5', 'claude-code'),
        ],
        catalog,
      ),
    ).toEqual({ provider: 'deepseek', model: 'deepseek-v4-flash' })
    // Without history no model is chosen for the user.
    expect(lastUsedModel([], catalog)).toBeNull()
  })

  it('finds the execution a busy runner is running', () => {
    expect(
      runningExecutionId(
        'handler error: "Nightly" (plan-0123456789abcdef0123456789abcdef) is still running; wait for it to finish or cancel it.',
      ),
    ).toBe('plan-0123456789abcdef0123456789abcdef')
    expect(
      runningExecutionId(
        'Another execution (0123456789abcdef0123456789abcdef) is still running; wait for it to finish or cancel it.',
      ),
    ).toBe('0123456789abcdef0123456789abcdef')
    expect(runningExecutionId('Select an execution model.')).toBeNull()
  })

  it('ticks and unticks a sequential group whole', () => {
    const groups = [['registry_implementation', 'registry_verification']]
    const ticked = withSequentialGroups(
      ['minimal_path', 'registry_verification'],
      ['minimal_path'],
      groups,
    )
    expect(ticked).toEqual([
      'minimal_path',
      'registry_verification',
      'registry_implementation',
    ])
    expect(
      withSequentialGroups(
        ['minimal_path', 'registry_verification'],
        ticked,
        groups,
      ),
    ).toEqual(['minimal_path'])
    expect(withSequentialGroups(['minimal_path'], [], groups)).toEqual([
      'minimal_path',
    ])
  })
})
