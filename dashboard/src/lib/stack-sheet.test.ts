import { describe, expect, it } from 'vitest'
import type {
  IiiRelease,
  IiiReleases,
  StackTemplate,
} from '@/lib/dashboard-data-source'
import { type Declared, declaredOf, judge } from '@/lib/stack-builder'
import {
  belowMinimum,
  blockedLines,
  compareVersions,
  containerOf,
  formLocks,
  iiiChoice,
  iiiLabel,
  iiiRefusal,
  patchContainers,
  type Read,
  readOf,
  releasedOn,
  setScalar,
  sheetBlocks,
  sheetStatus,
  splitTemplate,
  templateRef,
} from '@/lib/stack-sheet'

const pkg = (name: string): Declared => ({
  name,
  worker: `package://${name}`,
  version: 'latest',
  commit: null,
})
const pinned: Declared = { ...pkg('harness'), commit: '8c02f93a1d4e' }

/** What stack-preview answers for a draft, as the Rust tests pin it. */
const read = (declared: Declared[], template: string | null = null): Read => ({
  iii: 'latest',
  template: splitTemplate(template),
  containers: declared.map(containerOf),
  warnings: [],
})

const harness: StackTemplate = {
  id: 'harness',
  name: 'Harness',
  description: '',
  workers: ['harness', 'browser'].map((name) => ({
    name,
    worker: `package://${name}`,
    version: null,
  })),
  min_iii_version: '0.24.0',
}

const ORIGINAL = [
  '# A stack with its own environment.',
  'iii: latest # the newest candidate',
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

describe('stack sheet: the form patches the draft line by line', () => {
  it('rewrites iii and template in place, keeping comments and every other line', () => {
    const iii = setScalar(ORIGINAL, 'iii', '0.24.3-rc.1')
    expect(iii.split('\n')[1]).toBe('iii: "0.24.3-rc.1" # the newest candidate')
    expect(iii.split('\n').slice(2)).toEqual(ORIGINAL.split('\n').slice(2))
    const template = setScalar(iii, 'template', 'harness@v1.2')
    expect(template.split('\n').slice(1, 3)).toEqual([
      'iii: "0.24.3-rc.1" # the newest candidate',
      'template: harness@v1.2',
    ])
    expect(setScalar(template, 'template', 'harness')).toContain(
      '\ntemplate: harness\n',
    )
    expect(setScalar(template, 'template', null)).toBe(iii)
    // Absent, iii goes before the first key, after the leading comments.
    expect(setScalar('# mine\ncontainers: {}\n', 'iii', '0.24.2')).toBe(
      '# mine\niii: "0.24.2"\ncontainers: {}\n',
    )
    expect(setScalar('containers: {}\n', 'template', 'quickstart')).toBe(
      'template: quickstart\ncontainers: {}\n',
    )
    // A quoted value keeps the comment after it, even with a # inside.
    expect(setScalar('iii: "a # b" # why\n', 'iii', 'latest')).toBe(
      'iii: latest # why\n',
    )
  })

  it('changes only the lines of the containers the form touched', () => {
    const before = [pkg('harness'), pkg('fp')]
    expect(patchContainers(ORIGINAL, before, before)).toBe(ORIGINAL)
    expect(
      patchContainers(ORIGINAL, before, [pinned, pkg('provider-kimi')]),
    ).toBe(
      [
        '# A stack with its own environment.',
        'iii: latest # the newest candidate',
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
    // A pin being typed is written empty, so the YAML tab holds it too.
    expect(
      patchContainers(ORIGINAL, before, [{ ...pkg('harness'), version: '' }]),
    ).toContain('    version: ""\n    environment:')
    // Everything removed leaves an empty mapping; one added opens it.
    expect(patchContainers(ORIGINAL, before, [])).toContain(
      'containers: {}\n  # The application under test.\n\n\nstartup_timeout',
    )
    expect(
      patchContainers('iii: latest\ncontainers: {}\n', [], [pkg('fp')]),
    ).toBe(
      'iii: latest\ncontainers:\n  fp:\n    worker: package://fp\n    version: latest\n',
    )
  })

  it('leaves to the YAML tab, field by field, what it cannot patch safely', () => {
    const plain = read([pkg('harness'), pkg('fp')])
    expect(formLocks(ORIGINAL, plain)).toEqual({
      iii: null,
      template: null,
      containers: null,
      pins: { harness: null, fp: null },
    })
    // Four spaces in, a flow mapping, an anchor: the containers are locked,
    // and so is every pin; iii stays editable.
    for (const yaml of [
      'iii: latest\ncontainers:\n    harness:\n      worker: package://harness\n',
      'iii: latest\ncontainers: {harness: {worker: package://harness}}\n',
      'iii: latest\ncontainers:\n  harness: &h\n    worker: package://harness\n',
      'iii: latest\ncontainers:\n  harness:\n    <<: *base\n',
    ]) {
      const locks = formLocks(yaml, read([pkg('harness')]))
      expect(locks.containers).toMatch(/Change it in the YAML tab\.$/)
      expect(locks.pins.harness).toBe(locks.containers)
      expect(locks.iii).toBeNull()
    }
    // One container's pin written as more than a value: that pin alone.
    const tagged =
      'containers:\n  harness:\n    worker: package://harness\n    version: !!str 1.0\n  fp:\n    worker: package://fp\n'
    const locks = formLocks(tagged, read([pkg('harness'), pkg('fp')]))
    expect(locks.containers).toBeNull()
    expect(locks.pins).toEqual({
      harness:
        'Its version is written in a way the form doesn’t rewrite. Change it in the YAML tab.',
      fp: null,
    })
    // iii through an anchor, or over two lines: iii alone.
    for (const yaml of [
      'x: &v latest\niii: *v\ncontainers: {}\n',
      'iii:\n  latest\ncontainers: {}\n',
    ]) {
      const locked = formLocks(yaml, read([]))
      expect(locked.iii).toMatch(/^`iii` is written in a way/)
      expect(locked.containers).toBeNull()
    }
  })
})

describe('stack sheet: draft, preview and rules', () => {
  it('reads a listed stack as the preview would, the revision apart', () => {
    expect(templateRef({ id: 'harness', revision: null })).toBe('harness')
    expect(templateRef({ id: 'harness', revision: 'v1.2' })).toBe(
      'harness@v1.2',
    )
    expect(splitTemplate('harness@')).toEqual({ id: 'harness', revision: null })
    expect(
      readOf({
        iii: 'latest',
        template: 'harness@4077e670',
        containers: [],
        warnings: ['w'],
      }),
    ).toEqual({
      iii: 'latest',
      template: { id: 'harness', revision: '4077e670' },
      containers: [],
      warnings: ['w'],
    })
    // A pin being typed reads back as the form holds it.
    expect(
      declaredOf(read([{ ...pkg('fp'), version: '' }, pinned])),
    ).toEqual([{ ...pkg('fp'), version: '' }, { ...pinned, version: 'latest' }])
  })

  it('blocks in both tabs: ignored, empty pins, a revision, the refusal, no name', () => {
    const declared = [
      pkg('harness'),
      pkg('fp'),
      { ...pkg('browser'), commit: '' },
    ]
    const verdicts = declared.map((entry) =>
      judge(entry, 'harness', harness, declared, {}),
    )
    const blocks = sheetBlocks({
      name: '',
      refused: null,
      revisionError: 'iii-hq/templates has no commit, tag or branch nope.',
      declared,
      verdicts,
    })
    expect(blocks.map((block) => block.container ?? null)).toEqual([
      'fp',
      'browser',
      null,
      null,
    ])
    expect(blocks[0].text).toMatch(/^fp: Would be ignored\./)
    expect(blocks[1].text).toBe('browser: its commit is empty.')
    expect(blocks[3].text).toBe('Name the stack.')
    expect(
      sheetBlocks({
        name: 'x',
        refused: 'The stack is not YAML: …',
        revisionError: null,
        declared,
        verdicts,
      }),
    ).toEqual([{ text: 'The runner refuses this YAML: The stack is not YAML: …' }])

    const base = {
      name: 'Mine',
      source: 'template' as const,
      template: 'harness',
      tpl: harness,
      declared: [pkg('harness')],
      verdicts: [judge(pkg('harness'), 'harness', harness, [], {})],
      verb: 'save' as const,
      pending: false,
      refused: null,
      revisionError: null,
    }
    expect(sheetStatus(base)).toEqual({
      text: 'Runs the harness project with 1 pinned.',
      blocked: false,
      alert: false,
    })
    expect(sheetStatus({ ...base, pending: true })).toEqual({
      text: 'Reading the YAML…',
      blocked: true,
      alert: false,
    })
    expect(sheetStatus({ ...base, refused: 'not YAML' }).text).toBe(
      'Can’t save it: the runner refuses this YAML.',
    )
    expect(
      sheetStatus({ ...base, revisionError: 'iii-hq/templates has no x.' }),
    ).toEqual({
      text: 'Can’t save it yet: iii-hq/templates has no x.',
      blocked: true,
      alert: true,
    })
    const ignored = [pkg('fp')]
    expect(
      sheetStatus({
        ...base,
        declared: ignored,
        verdicts: [judge(pkg('fp'), 'harness', harness, ignored, {})],
      }).text,
    ).toBe(
      'Can’t save it yet: fp would be ignored by the harness template.',
    )
  })

  it('marks the lines of a container that holds it back, not its comments', () => {
    expect(blockedLines(ORIGINAL, new Set(['harness']))).toEqual([
      false,
      false,
      false,
      false,
      false,
      true,
      true,
      true,
      true,
      true,
      false,
      false,
      false,
      false,
      false,
      false,
      false,
    ])
  })
})

describe('stack sheet: iii and the template revision', () => {
  const release = (
    version: string,
    published_at: string,
    extra: Partial<IiiRelease> = {},
  ): IiiRelease => ({
    version,
    prerelease: version.includes('-'),
    published_at,
    cli: true,
    ...extra,
  })
  const releases: IiiReleases = {
    latest_candidate: '0.24.3-rc.1',
    releases: [
      release('0.24.3', '2026-09-25T18:38:43Z'),
      release('0.24.3-rc.1', '2026-09-25T13:59:34Z'),
      release('0.24.2', '2026-09-21T23:54:54Z'),
    ],
  }

  it('orders versions as semver: core, then a pre-release before its release', () => {
    expect(compareVersions('0.24.3-rc.1', '0.24.3')).toBeLessThan(0)
    expect(compareVersions('0.23.0-rc.10', '0.23.0-rc.9')).toBeGreaterThan(0)
    expect(compareVersions('0.24.0', '0.23.9')).toBeGreaterThan(0)
    expect(compareVersions('1.2.3', '1.2.3')).toBe(0)
    expect(compareVersions('latest', '1.2.3')).toBeNull()
  })

  it('offers latest as the newest candidate, a listed release or another version', () => {
    expect(iiiLabel('latest', releases)).toBe('latest · 0.24.3-rc.1')
    expect(iiiLabel('latest', null)).toBe('latest')
    expect(iiiLabel(null, releases)).toBe('not set')
    expect(iiiChoice('latest', releases)).toBe('latest')
    expect(iiiChoice('0.24.2', releases)).toBe('release:0.24.2')
    expect(iiiChoice('0.9.0', releases)).toBe('other')
    const now = new Date('2026-09-26T12:00:00')
    expect(releasedOn(releases.releases[0], new Date('2026-09-25T20:00:00'))).toBe(
      'today',
    )
    expect(releasedOn(releases.releases[1], now)).toBe('yesterday')
    expect(releasedOn(releases.releases[2], now)).toBe('Sep 21')
    // A typed version is used only when its release exists and ships the CLI.
    expect(iiiRefusal({ version: '0.24.2', release: releases.releases[2] })).toBeNull()
    expect(iiiRefusal({ version: '9.9.9', release: null })).toBe(
      'iii-hq/iii has no release iii/v9.9.9.',
    )
    expect(
      iiiRefusal({
        version: '0.1.0',
        release: release('0.1.0', '2025-01-01T00:00:00Z', { cli: false }),
      }),
    ).toBe(
      'iii/v0.1.0 publishes no iii CLI for x86_64 Linux, which a group installs.',
    )
  })

  it('warns, never blocks, below the template’s min_iii_version', () => {
    expect(belowMinimum('0.23.2', releases, harness)).toBe(
      'The harness template needs iii 0.24.0 or newer; this is 0.23.2.',
    )
    expect(belowMinimum('0.24.0-rc.1', releases, harness)).toMatch(
      /this is 0\.24\.0-rc\.1\.$/,
    )
    expect(belowMinimum('latest', releases, harness)).toBeNull()
    expect(
      belowMinimum(
        'latest',
        { ...releases, latest_candidate: '0.23.0-rc.2' },
        harness,
      ),
    ).toBe('The harness template needs iii 0.24.0 or newer; latest is 0.23.0-rc.2.')
    expect(
      belowMinimum('0.1.0', releases, { ...harness, min_iii_version: undefined }),
    ).toBeNull()
  })
})
