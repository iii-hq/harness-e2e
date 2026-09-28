import { describe, expect, it } from 'vitest'
import type { Stack, StackTemplate } from '@/lib/dashboard-data-source'
import {
  addedWhenItRuns,
  builderStatus,
  type Declared,
  declaredOf,
  judge,
  type Lookups,
  lookupOf,
  markedLines,
  orderTemplates,
  patchYaml,
  pickerGroups,
  pinLabel,
  sameDeclared,
  templateExtra,
  typedName,
  yamlOf,
  yamlScalar,
} from '@/lib/stack-builder'

const HARNESS = [
  'queue',
  'state',
  'session-manager',
  'llm-router',
  'provider-anthropic',
  'provider-openai',
  'provider-deepseek',
  'context-manager',
  'iii-directory',
  'cron',
  'ade',
  'ide',
  'harness',
  'browser',
]

function template(id: string, names: string[]): StackTemplate {
  return {
    id,
    name: id,
    description: '',
    workers: names.map((name) => ({
      name,
      worker: `package://${name}`,
      version: 'latest',
    })),
  }
}

const harness = template('harness', HARNESS)
const pkg = (name: string): Declared => ({
  name,
  worker: `package://${name}`,
  version: 'latest',
  commit: null,
})
const pinned: Declared = { ...pkg('harness'), commit: '8c02f93a1d4e' }

function status(
  declared: Declared[],
  tpl: StackTemplate | null,
  lookups: Lookups = {},
  name = 'mine',
) {
  const verdicts = declared.map((entry) =>
    judge(entry, tpl?.id ?? null, tpl, declared, lookups),
  )
  return {
    verdicts,
    ...builderStatus({
      name,
      source: tpl ? 'template' : 'none',
      template: tpl?.id ?? null,
      tpl,
      declared,
      verdicts,
    }),
  }
}

describe('stack builder', () => {
  it('writes the YAML the form holds, quoting what YAML would read as a number', () => {
    expect(yamlOf('latest', 'harness', [])).toEqual([
      'iii: latest',
      'template: harness',
      '',
      'containers: {}',
      '',
      'startup_timeout: 5m',
      'stop_timeout: 30s',
    ])
    expect(
      yamlOf('latest', null, [
        pinned,
        { ...pkg('fp'), version: '1.10' },
        { ...pkg('queue'), version: '0.21.9' },
      ]).slice(2, 12),
    ).toEqual([
      'containers:',
      '  harness:',
      '    worker: package://harness',
      '    commit: "8c02f93a1d4e"',
      '  fp:',
      '    worker: package://fp',
      '    version: "1.10"',
      '  queue:',
      '    worker: package://queue',
      '    version: "0.21.9"',
    ])
    // What YAML would read as something else, or not at all, is quoted.
    for (const [value, written] of [
      ['latest', 'latest'],
      ['package://harness', 'package://harness'],
      ['harness@4077e670', 'harness@4077e670'],
      ['null', '"null"'],
      ['~', '"~"'],
      ['Yes', '"Yes"'],
      ['1.2.3 #rc', '"1.2.3 #rc"'],
      ['@1.2', '"@1.2"'],
      ['a: b', '"a: b"'],
      ['2026-09-28', '"2026-09-28"'],
      ['12', '"12"'],
      ['end:', '"end:"'],
    ])
      expect(yamlScalar(value)).toBe(written)
    expect(
      yamlOf('null', '~', [{ ...pkg('x'), version: '1.2.3 #rc' }]),
    ).toEqual([
      'iii: "null"',
      'template: "~"',
      '',
      'containers:',
      '  x:',
      '    worker: package://x',
      '    version: "1.2.3 #rc"',
      '',
      'startup_timeout: 5m',
      'stop_timeout: 30s',
    ])
    expect(pinLabel(pinned)).toBe('commit 8c02f93')
    expect(pinLabel({ ...pkg('x'), commit: '' })).toBe('commit …')
    expect(pinLabel({ ...pkg('x'), version: '' })).toBe('version …')
  })

  it('with a template, blocks a worker it would ignore and marks its YAML', () => {
    const declared = [
      pinned,
      pkg('fp'),
      pkg('provider-zai'),
      pkg('harness-e2e'),
    ]
    const { verdicts, text, blocked, alert } = status(declared, harness)
    expect(verdicts.map((verdict) => verdict?.tone)).toEqual([
      'info',
      'block',
      'info',
      'info',
    ])
    expect(verdicts[0]?.lead).toBe('Pins the template’s harness.')
    expect(verdicts[0]?.text).toBe(
      'The template keeps it, built from commit 8c02f93 with the dependencies that commit declares.',
    )
    expect(verdicts[1]?.lead).toBe('Would be ignored.')
    expect(verdicts[2]?.lead).toBe('Pins it for runs that pick a zai model.')
    expect(verdicts[3]?.lead).toBe('Pins the runner.')
    // canvas and iii-directory, which the executor adds where a run needs
    // them, are pinned like the runner and the providers; a worker is
    // compared by package, not by the container's name.
    const exempt = status(
      [
        pkg('canvas'),
        pkg('iii-directory'),
        { ...pkg('router'), worker: 'package://llm-router' },
        { ...pkg('fp'), name: 'queue' },
      ],
      template('bare', ['console', 'state', 'llm-router']),
    )
    expect(exempt.verdicts.map((verdict) => verdict?.lead)).toEqual([
      'Pins canvas for visual tests.',
      'Pins iii-directory for runs with an agent profile.',
      'Pins the template’s llm-router.',
      'Would be ignored.',
    ])
    expect([text, blocked, alert]).toEqual([
      'Can’t create it yet: fp would be ignored by the harness template.',
      true,
      true,
    ])
    const lines = markedLines(
      yamlOf('latest', 'harness', declared),
      new Set(['fp']),
    )
    expect(
      lines.flatMap((line, index) => (line.blocked ? [index + 1] : [])),
    ).toEqual([8, 9, 10])
    expect(status([pinned], harness).text).toBe(
      'Runs the harness project with 1 pinned.',
    )
    expect(status([], harness).text).toBe(
      'Runs the harness project with every worker at the template’s version.',
    )
  })

  it('without a template, blocks a worker another one brings, as the registry says', () => {
    const declared = [pkg('harness'), pkg('fp'), pkg('browser')]
    const lookups: Lookups = {
      harness: {
        state: 'found',
        version: '1.8.36',
        dependencies: ['browser', 'llm-router', 'session-manager', 'state'],
      },
      fp: { state: 'found', version: '0.2.20', dependencies: [] },
      browser: { state: 'found', version: '0.3.1', dependencies: [] },
    }
    const { verdicts, text, blocked } = status(declared, null, lookups)
    expect(verdicts[0]).toEqual({
      tone: 'info',
      lead: 'Brings browser, llm-router, session-manager, state with it.',
      text: 'They start from its package; don’t declare them again.',
    })
    expect(verdicts[1]).toBeNull()
    expect(verdicts[2]?.tone).toBe('block')
    expect(verdicts[2]?.lead).toBe('Already arrives with harness.')
    expect([text, blocked]).toEqual([
      'Can’t create it yet: browser already arrives with another worker.',
      true,
    ])
    expect(status([pkg('fp')], null, lookups).text).toBe(
      'Runs the 1 worker declared, plus what they depend on.',
    )
    // Without a template iii-directory is never added: it has to be
    // declared or come with a declared worker for agent profiles.
    const directory = (declared: Declared[], template: string | null = null) =>
      addedWhenItRuns(template, declared, lookups).at(-1)?.why
    expect(directory([pkg('fp')])).toBe(
      'not added: runs with an agent profile need it declared here or brought by a declared worker',
    )
    expect(
      addedWhenItRuns(null, [pkg('harness')], {
        harness: {
          state: 'found',
          version: '1',
          dependencies: ['iii-directory'],
        },
      }).at(-1)?.why,
    ).toBe('comes with harness, for runs with an agent profile')
    expect(directory([pkg('iii-directory')])).toBe(
      'declared here, for runs with an agent profile',
    )
    expect(directory([], 'harness')).toBe('when a run uses an agent profile')
    const fine = status([pkg('harness'), pkg('fp')], null, lookups)
    expect([fine.text, fine.blocked]).toEqual([
      'Runs the 2 workers declared, plus what they depend on.',
      false,
    ])
  })

  it('never blocks on what the registry has not answered or could not', () => {
    const declared = [pkg('harness'), pkg('fp')]
    const checking = status(declared, null, { harness: { state: 'checking' } })
    expect(checking.verdicts.map((verdict) => verdict?.lead)).toEqual([
      'Checking the registry…',
      'Checking the registry…',
    ])
    expect(checking.blocked).toBe(false)
    const failed = status(declared, null, {
      harness: { state: 'failed', message: 'The worker did not answer.' },
      fp: { state: 'missing', message: "Worker 'fp' was not found." },
    })
    expect(failed.verdicts.map((verdict) => verdict?.tone)).toEqual([
      'warn',
      'warn',
    ])
    expect(failed.blocked).toBe(false)
    expect(failed.verdicts[0]).toMatchObject({
      lead: 'Couldn’t reach the registry.',
      retry: 'harness',
    })
    // Only worker_not_found says a worker does not exist.
    expect(
      lookupOf({
        error: { code: 'worker_not_found', message: 'No such worker.' },
      }),
    ).toEqual({ state: 'missing', message: 'No such worker.' })
    expect(
      lookupOf({ error: { code: 'rate_limited', message: 'Slow down.' } }),
    ).toEqual({ state: 'failed', message: 'Slow down.' })
    // A path worker is never looked up.
    expect(
      judge({ ...pkg('link'), worker: 'path://./link' }, null, null, [], {}),
    ).toBeNull()
  })

  it('waits for a pin being typed, a template, a worker and a name', () => {
    expect(status([{ ...pinned, commit: '' }], harness).text).toBe(
      'Type the commit for harness.',
    )
    expect(status([{ ...pkg('harness'), version: '' }], harness).text).toBe(
      'Type the version for harness.',
    )
    expect(
      builderStatus({
        name: 'x',
        source: 'template',
        template: null,
        tpl: null,
        declared: [],
        verdicts: [],
      }).text,
    ).toBe('Pick a template.')
    expect(status([], null).text).toBe('Add at least one worker.')
    const unnamed = status([pinned], harness, {}, ' ')
    expect([unnamed.text, unnamed.blocked]).toEqual(['Name the stack.', true])
    // A template that did not load is not checked, and nothing is blocked.
    const unread = builderStatus({
      name: 'x',
      source: 'copy',
      template: 'harness@abc',
      tpl: null,
      declared: [pkg('fp')],
      verdicts: [null],
    })
    expect([unread.text, unread.blocked]).toEqual([
      'Runs the harness project with 1 pinned. Its workers couldn’t be read, so nothing was checked.',
      false,
    ])
  })

  it('groups the picker and says what each worker would do', () => {
    const stacks = [
      {
        containers: ['harness', 'harness-e2e', 'fp', 'provider-zai'].map(
          (name) => ({
            name,
            worker: `package://${name}`,
            version: 'latest',
            commit: null,
          }),
        ),
      },
    ] as Stack[]
    const templates = [harness, template('linkly', ['http', 'state'])]
    const groups = pickerGroups({
      query: '',
      template: 'harness',
      tpl: harness,
      declared: [pinned],
      lookups: {},
      stacks,
      templates,
      showAll: false,
    })
    expect(groups.map((group) => group.label)).toEqual([
      'In the harness template',
      'In your stacks',
      'Providers',
      'Other iii workers',
    ])
    expect(groups[0].items.map((item) => item.name)).toEqual([
      'queue',
      'state',
      'session-manager',
      'llm-router',
      'Show 10 more',
    ])
    const tags = Object.fromEntries(
      groups[1].items.map((item) => [item.name, [item.tag, item.off]]),
    )
    expect(tags).toEqual({
      'harness-e2e': ['the runner, always added', false],
      fp: ['not in the template, ignored', true],
      'provider-zai': ['added with its model', false],
    })
    expect(
      pickerGroups({
        query: 'canvas',
        template: 'harness',
        tpl: harness,
        declared: [],
        lookups: {},
        stacks: [],
        templates: [template('other', ['canvas'])],
        showAll: false,
      })[0].items[0],
    ).toMatchObject({
      name: 'canvas',
      tag: 'added for visual tests',
      off: false,
    })
    expect(groups[2].items.map((item) => item.name)).not.toContain(
      'provider-anthropic',
    )
    expect(groups[3].items.map((item) => item.name)).toEqual(['http'])

    // Without a template, what a declared worker brings is not offered again.
    const bare = pickerGroups({
      query: 'brow',
      template: null,
      tpl: null,
      declared: [pkg('harness')],
      lookups: {
        harness: { state: 'found', version: '1', dependencies: ['browser'] },
      },
      stacks,
      templates: [template('other', ['browser'])],
      showAll: false,
    })
    expect(bare[0].items[0]).toMatchObject({
      name: 'browser',
      tag: 'arrives with harness',
      off: true,
    })
  })

  it('checks a typed name in the registry: its version, or its error', () => {
    const typed = (lookups: Lookups, template: StackTemplate | null = null) =>
      pickerGroups({
        query: 'package://Nope ',
        template: template?.id ?? null,
        tpl: template,
        declared: [],
        lookups,
        stacks: [],
        templates: [],
        showAll: false,
      }).at(-1)
    expect(typed({})?.items[0]).toMatchObject({
      name: 'nope',
      sub: 'package://nope · checking the registry…',
      off: true,
    })
    expect(typedName('package://Nope ', [typed({}) as never])).toBe('nope')
    expect(
      typed({ nope: { state: 'found', version: '0.1.0', dependencies: [] } })
        ?.items[0],
    ).toMatchObject({ sub: 'package://nope · found, 0.1.0', tag: 'add' })
    expect(
      typed({
        nope: {
          state: 'missing',
          message: "Worker 'nope' was not found in the registry.",
        },
      })?.items[0],
    ).toMatchObject({
      sub: "package://nope · Worker 'nope' was not found in the registry.",
      off: true,
      alert: true,
    })
    // A network error warns and still lets it be added.
    const unreached = typed({ nope: { state: 'failed', message: 'timed out' } })
    expect(unreached?.retry).toBe('nope')
    expect(unreached?.items[0]).toMatchObject({
      sub: 'package://nope · couldn’t reach the registry: timed out',
      off: false,
      tag: 'add',
    })
    // With a template, a name it does not declare would be ignored.
    const ignored = typed({}, harness)
    expect(ignored?.items[0]).toMatchObject({
      sub: 'package://nope · not in the template, ignored',
      off: true,
    })
    expect(typedName('nope', [ignored as never])).toBeNull()
  })

  it('changes only the lines of the containers the form touched', () => {
    const original = [
      '# A stack with its own environment.',
      'iii: latest',
      '',
      'containers:',
      '  # The application under test.',
      '  harness:',
      '    worker: package://harness',
      '    version: latest # the newest',
      '    environment:',
      '      RUST_LOG: debug',
      '',
      '  fp:',
      '    worker: package://fp',
      '    version: latest',
      '',
      'startup_timeout: 5m',
      '',
    ].join('\n')
    const before = [pkg('harness'), pkg('fp')]
    expect(patchYaml(original, before, before, null, null)).toBe(original)
    expect(
      patchYaml(
        original,
        before,
        [{ ...pinned }, pkg('provider-kimi')],
        'harness',
        null,
      ),
    ).toBe(
      [
        '# A stack with its own environment.',
        'iii: latest',
        'template: harness',
        '',
        'containers:',
        '  # The application under test.',
        '  harness:',
        '    worker: package://harness',
        '    commit: "8c02f93a1d4e"',
        '    environment:',
        '      RUST_LOG: debug',
        '',
        '  provider-kimi:',
        '    worker: package://provider-kimi',
        '    version: latest',
        '',
        'startup_timeout: 5m',
        '',
      ].join('\n'),
    )
    // Everything removed leaves an empty mapping; one added to an empty
    // mapping opens it.
    expect(patchYaml(original, before, [], null, null)).toContain(
      'containers: {}\n  # The application under test.\n\n\nstartup_timeout',
    )
    expect(
      patchYaml('iii: latest\ncontainers: {}\n', [], [pkg('fp')], null, null),
    ).toBe(
      'iii: latest\ncontainers:\n  fp:\n    worker: package://fp\n    version: latest\n',
    )
    // Laid out otherwise, it is not changed line by line.
    expect(
      patchYaml(
        'containers:\n    harness:\n      worker: package://harness\n',
        [pkg('harness')],
        [],
        null,
        null,
      ),
    ).toBeNull()
    expect(
      patchYaml(
        'containers: {harness: {}}\n',
        [pkg('harness')],
        [],
        null,
        null,
      ),
    ).toBeNull()
    expect(sameDeclared(before, [pkg('harness'), pkg('fp')])).toBe(true)
    expect(sameDeclared(before, [pinned, pkg('fp')])).toBe(false)
  })

  it('keeps a container without a worker as it is', () => {
    const bare = { name: 'odd', worker: null, version: 'latest', commit: null }
    expect(yamlOf('latest', null, [bare]).slice(3, 5)).toEqual([
      '  odd:',
      '    version: latest',
    ])
    expect(judge(bare, null, null, [bare], {})).toBeNull()
  })

  it('orders templates with workers first and says what a path worker is', () => {
    const empty = { ...template('starter', []), note: 'Ships nothing.' }
    const linkly = {
      ...template('linkly', ['http']),
      workers: [
        ...template('linkly', ['http']).workers,
        { name: 'link', worker: 'path://./link', version: null },
      ],
    }
    expect(
      orderTemplates([empty, harness, linkly]).map((entry) => entry.id),
    ).toEqual(['harness', 'linkly', 'starter'])
    expect(templateExtra(linkly)).toBe(
      'link runs from path://./link in the project.',
    )
    expect(templateExtra(empty)).toBe('Ships nothing.')
    expect(
      declaredOf({
        containers: [
          { name: 'a', worker: null, version: null, commit: null },
          { name: 'b', worker: 'package://b', version: null, commit: 'abc' },
        ],
      } as Stack),
    ).toEqual([
      { name: 'a', worker: null, version: 'latest', commit: null },
      { name: 'b', worker: 'package://b', version: 'latest', commit: 'abc' },
    ])
  })
})
