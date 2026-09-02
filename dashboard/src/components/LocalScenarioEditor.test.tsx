import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  EMPTY_LOCAL_SCENARIO_DRAFT,
  fileNameForTitle,
  isLocalScenarioDraftDirty,
  LOCAL_SCENARIO_TEMPLATE,
  LocalScenarioEditor,
  weightBreakdown,
} from '@/components/LocalScenarioEditor'
import type { DashboardDataBridge } from '@/lib/dashboard-data-source'
import {
  buildLocalScenarioSource,
  distributeValidationWeights,
  type LocalScenarioDraft,
  localScenarioDraftFieldIssues,
  localScenarioDraftIssue,
  parseLocalScenarioSource,
} from '@/lib/local-scenario-authoring'

describe('local Markdown scenario editor', () => {
  it('renders local test creation as a modal with structured fields', () => {
    const html = renderToStaticMarkup(
      <LocalScenarioEditor
        bridge={{} as DashboardDataBridge}
        onClose={() => undefined}
        onCreated={() => undefined}
      />,
    )

    expect(html).toContain('new local test')
    expect(html.startsWith('<dialog')).toBe(true)
    expect(html).toContain('Test name')
    expect(html).toContain('Before test')
    expect(html).toContain('Task prompt')
    expect(html).toContain('Validation criteria')
    expect(html).toContain('import .md')
    expect(html).toContain('template .md')
    expect(html).toContain('generated markdown · live')
    expect(html).toContain('data-markdown-preview')
    expect(html).toContain('create test')
    expect(html).toContain('create and open plan')
    // Audit NT-03: the form starts empty; the template is only a placeholder
    // and the fallback file name does not repeat the compiler prefix.
    expect(html).toContain('local_new_test')
    expect(html).not.toContain('local_local_scenario')
    expect(html).toContain('placeholder="Database recovery"')
    expect(html).toContain('placeholder="Expected outcome"')
    // Audit NT-02 / NT-05: nothing is marked before the first attempt and
    // the primary is enabled.
    expect(html).not.toContain('Add a test name.')
    expect(html).not.toContain('aria-invalid="true"')
    expect(html).not.toContain('Ready to save locally')
    expect(html).toContain('Saved locally as Markdown. No execution starts.')
    // Audit NT-01: state and actions live in a footer, not after the aside.
    expect(html).toContain('<footer')
    expect(html.indexOf('<footer')).toBeGreaterThan(html.indexOf('<aside'))
    expect(html).toContain('ds-dialog-footer')
    expect(html).toContain('0% · 100 missing')
    // Audit NT-04: DS primitives, 6px radius, criteria without a border.
    expect(html).toContain('ds-input')
    expect(html).not.toContain('rounded-xl')
    expect(html).not.toContain('rounded-lg')
    expect(html).toContain('bg-[var(--surface-fill)] p-3')
    for (const section of [
      '## Plans',
      '## Version',
      '## Before Test',
      '## Prompt',
      '## Validations',
    ]) {
      expect(LOCAL_SCENARIO_TEMPLATE).toContain(section)
    }
    expect(LOCAL_SCENARIO_TEMPLATE).toContain('- local')
  })

  // Audit NT-02: every field reports its own problem at once.
  it('reports every field issue keyed by field', () => {
    const issues = localScenarioDraftFieldIssues({
      ...EMPTY_LOCAL_SCENARIO_DRAFT,
      validations: [
        { id: 'a', title: '', weight: '70', instructions: 'x' },
        { id: 'b', title: 'b', weight: '', instructions: '' },
      ],
    })
    expect(Object.keys(issues)).toEqual([
      'title',
      'beforeTest',
      'prompt',
      'validation:a:title',
      'validation:b:weight',
      'validation:b:instructions',
    ])
    expect(issues['validation:b:weight']).toBe(
      'Validation 2 weight must be a positive integer.',
    )
    const weights = localScenarioDraftFieldIssues({
      title: 't',
      version: '1',
      beforeTest: 'b',
      prompt: 'p',
      validations: [
        { id: 'a', title: 'a', weight: '70', instructions: 'x' },
        { id: 'b', title: 'b', weight: '20', instructions: 'y' },
      ],
    })
    expect(weights).toEqual({
      validations: 'Validation weights total 90%; adjust them to exactly 100%.',
    })
    expect(
      distributeValidationWeights({
        ...EMPTY_LOCAL_SCENARIO_DRAFT,
        validations: [
          { id: 'a', title: 'a', weight: '', instructions: '' },
          { id: 'b', title: 'b', weight: '', instructions: '' },
          { id: 'c', title: 'c', weight: '', instructions: '' },
        ],
      }).validations.map((validation) => validation.weight),
    ).toEqual(['34', '33', '33'])
  })

  it('derives the file name from the test name and tracks dirtiness', () => {
    expect(fileNameForTitle('Database recovery')).toBe('database-recovery.md')
    expect(fileNameForTitle('  ')).toBe('')
    expect(isLocalScenarioDraftDirty(EMPTY_LOCAL_SCENARIO_DRAFT, false)).toBe(
      false,
    )
    expect(isLocalScenarioDraftDirty(EMPTY_LOCAL_SCENARIO_DRAFT, true)).toBe(
      true,
    )
    expect(
      isLocalScenarioDraftDirty(
        { ...EMPTY_LOCAL_SCENARIO_DRAFT, title: 'x' },
        false,
      ),
    ).toBe(true)
    expect(
      weightBreakdown({
        ...EMPTY_LOCAL_SCENARIO_DRAFT,
        validations: [
          { id: 'a', title: 'a', weight: '70', instructions: 'x' },
          { id: 'b', title: 'b', weight: '30', instructions: 'y' },
        ],
      }),
    ).toBe('70 + 30 = 100%')
  })

  it('builds the compiler Markdown contract from the form values', () => {
    const draft: LocalScenarioDraft = {
      title: 'Database recovery',
      version: '2',
      beforeTest: 'Create a run-scoped database.',
      prompt: 'Recover the missing record.',
      validations: [
        {
          id: 'result',
          title: 'Record restored',
          weight: '80',
          instructions: 'Verify the target record exists.',
        },
        {
          id: 'cleanup',
          title: 'Safe cleanup',
          weight: '20',
          instructions: 'Verify temporary state was removed.',
        },
      ],
    }

    const source = buildLocalScenarioSource(draft)
    expect(source).toContain('# Database recovery')
    expect(source).toContain('## Plans\n\n- local')
    expect(source).toContain('## Version\n\n2')
    expect(source).toContain('### Record restored (80%)')
    expect(source).toContain('### Safe cleanup (20%)')
    expect(source.endsWith('\n')).toBe(true)
    expect(localScenarioDraftIssue(draft)).toBeNull()
  })

  it('imports a valid Markdown definition back into editable fields', () => {
    const imported = parseLocalScenarioSource(
      `
# Stateful test

## Plans

- local

## Version

3

## Before Test

Prepare the fixture.

\`\`\`md
## Prompt inside a code fence
\`\`\`

## Prompt

Complete the task.

## Validations

### Outcome (100%)

Check the result.
`.trimStart(),
    )

    expect(imported).toMatchObject({
      title: 'Stateful test',
      version: '3',
      prompt: 'Complete the task.',
      validations: [
        {
          title: 'Outcome',
          weight: '100',
          instructions: 'Check the result.',
        },
      ],
    })
    expect(imported.beforeTest).toContain('## Prompt inside a code fence')
  })

  it('rejects imported definitions that cannot populate a valid form', () => {
    expect(() =>
      parseLocalScenarioSource(
        LOCAL_SCENARIO_TEMPLATE.replace(
          '### Safe execution (30%)',
          '### Safe execution (20%)',
        ),
      ),
    ).toThrow('Validation weights total 90%')
  })

  it('keeps generated field content inside the compiler contract', () => {
    const imported = parseLocalScenarioSource(LOCAL_SCENARIO_TEMPLATE)
    expect(
      localScenarioDraftIssue({
        ...imported,
        prompt: 'Complete the task.\n\n## Unsupported section',
      }),
    ).toContain('cannot contain an H1 or H2 heading')
    expect(
      localScenarioDraftIssue({
        ...imported,
        prompt: 'Use {{unknown}} while completing the task.',
      }),
    ).toContain('unsupported template variable')
  })
})
