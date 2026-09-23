import { describe, expect, it } from 'vitest'
import {
  executionStartRequest,
  runnerForm,
} from '@/components/LocalRunnerDialog'
import type { ExecutionParameters } from '@/lib/dashboard-data-source'

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
    expect(
      executionStartRequest({ ...runnerForm({ ...imported, seed: 7 }) })
        .parameters.seed,
    ).toBe(7)
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
      seed: '42',
      agent: ' ',
    }
    expect(executionStartRequest(form)).toEqual({
      label: 'Before the prompt change',
      parameters: {
        scenarios: ['minimal_path'],
        runs: 1,
        technical_retries: 1,
        seed: 42,
        model: 'deepseek-v4-flash',
        provider: 'deepseek',
        agent: null,
      },
    })
  })
})
