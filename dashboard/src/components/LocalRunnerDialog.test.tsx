import { describe, expect, it } from 'vitest'
import {
  executionStartRequest,
  lastUsedModel,
  namedSuite,
  runnerForm,
  runningExecutionId,
  suiteChoices,
  withSequentialGroups,
} from '@/components/LocalRunnerDialog'
import type {
  DashboardExecutionSummary,
  ExecutionParameters,
  Suite,
} from '@/lib/dashboard-data-source'

const imported: ExecutionParameters = {
  suite: {
    id: 'software-engineering',
    label: 'Software engineering',
    sha256: 'sha256:recorded',
  },
  scenarios: ['minimal_path', 'context_pressure', 'kanban_c1_foundation'],
  runs: 3,
  technical_retries: 0,
  model: 'gpt-5.6-terra',
  provider: 'openai-codex',
  agent: 'tech-lead',
}

describe('run form', () => {
  it('runs an execution again from its own parameters and suite', () => {
    const form = runnerForm(imported)
    expect(form).toMatchObject({
      label: '',
      subject: 'openai-codex\ngpt-5.6-terra',
      suite: 'software-engineering',
      scenarios: imported.scenarios,
      runs: '3',
      technicalRetries: '0',
      agent: 'tech-lead',
    })
    // Unchanged, the form starts the same parameters again, under the same
    // suite; the runner records its digest.
    const suite = namedSuite(form, suiteChoices([], imported))
    expect(executionStartRequest(form, suite)).toEqual({
      label: '',
      parameters: {
        ...imported,
        suite: { id: 'software-engineering', label: 'Software engineering' },
      },
    })
  })

  it('names the new execution after the one it runs again', () => {
    expect(runnerForm(imported, [], 'Regression').label).toBe('Regression')
    expect(runnerForm(null, [], 'Regression').label).toBe('')
  })

  it('opens with only a chosen subset of the scenarios marked', () => {
    const form = runnerForm(imported, ['context_pressure'])
    expect(form.scenarios).toEqual(['context_pressure'])
    // Ticked by hand: an unnamed suite.
    expect(form.suite).toBe('')
    expect(
      executionStartRequest(form, namedSuite(form, suiteChoices([], imported)))
        .parameters,
    ).toEqual({
      ...imported,
      suite: null,
      scenarios: ['context_pressure'],
    })
  })

  it('starts an execution with what Run tests holds', () => {
    const form = {
      ...runnerForm(null, ['minimal_path']),
      label: '  Before the prompt change ',
      subject: 'deepseek\ndeepseek-v4-flash',
      agent: ' ',
    }
    expect(executionStartRequest(form)).toEqual({
      label: 'Before the prompt change',
      parameters: {
        suite: null,
        scenarios: ['minimal_path'],
        runs: 1,
        technical_retries: 1,
        model: 'deepseek-v4-flash',
        provider: 'deepseek',
        agent: null,
      },
    })
  })
})

describe('suite field', () => {
  const regression: Suite = {
    id: 'regression',
    label: 'Regression',
    source: 'repository',
    purpose: '',
    scenarios: ['minimal_path', 'context_pressure'],
    repetitions: 1,
    technical_retries: 1,
    sha256: 'sha256:regression',
    updated_at: null,
  }

  it('keeps a picked suite named only while the form holds what it does', () => {
    const form = {
      ...runnerForm(null),
      suite: 'regression',
      scenarios: ['context_pressure', 'minimal_path'],
      runs: '1',
      technicalRetries: '1',
    }
    expect(namedSuite(form, [regression])?.id).toBe('regression')
    for (const changed of [
      { ...form, scenarios: ['minimal_path'] },
      { ...form, runs: '2' },
      { ...form, technicalRetries: '0' },
      { ...form, suite: '' },
    ])
      expect(namedSuite(changed, [regression])).toBeNull()
  })

  it('offers the suite an execution ran when this runner does not list it', () => {
    expect(suiteChoices([regression], imported)).toEqual([
      regression,
      {
        id: 'software-engineering',
        label: 'Software engineering',
        scenarios: imported.scenarios,
        repetitions: 3,
        technical_retries: 0,
        recorded: true,
      },
    ])
    // A listed suite is offered as listed; an unnamed one adds nothing.
    expect(
      suiteChoices([regression], {
        ...imported,
        suite: { id: 'regression', label: 'Regression' },
      }),
    ).toEqual([regression])
    expect(suiteChoices([regression], { ...imported, suite: null })).toEqual([
      regression,
    ])
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
