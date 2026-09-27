import { describe, expect, it } from 'vitest'
import type { Stack, StackContainer } from '@/lib/dashboard-data-source'
import {
  pinOf,
  stackDeclares,
  stackDiff,
  stackSub,
  stacksSummary,
  workerWarns,
  yamlLine,
  yamlMeta,
} from '@/lib/stacks-view'

function container(
  name: string,
  facts: Partial<StackContainer> = {},
): StackContainer {
  return {
    name,
    worker: `package://${name}`,
    version: 'latest',
    commit: null,
    ...facts,
  }
}

const base: Stack = {
  id: 'default',
  label: 'default',
  source: 'repository',
  yaml: 'iii: latest\n',
  iii: 'latest',
  template: null,
  containers: ['harness', 'harness-e2e', 'fp'].map((name) => container(name)),
  warnings: [],
  updated_at: null,
}

describe('stacks view', () => {
  it('counts both blocks and names a local stack by id and save', () => {
    const local: Stack = {
      ...base,
      id: 'stack-3f9a1c2e7b40',
      source: 'local',
      updated_at: '2026-09-24T11:40:00',
    }
    expect(stacksSummary([base])).toMatch(
      /^1 in the repository, read-only · none in this Console\. /,
    )
    expect(stacksSummary([base, local])).toMatch(/· 1 in this Console\. /)
    expect(stackSub(base)).toBe('stacks/default.yaml')
    expect(stackSub(local, new Date('2026-09-27T12:00:00'))).toBe(
      'stack-3f9a1c2e7b40 · saved Sep 24, 11:40 AM',
    )
  })

  it('pins a commit before a version, and flags a path or missing worker', () => {
    expect(pinOf(container('a', { commit: '8c02f93a1d4e5f6a' }))).toBe(
      'commit 8c02f93a1d4e',
    )
    expect(pinOf(container('a', { version: null }))).toBe('no version')
    expect(workerWarns(container('a'))).toBe(false)
    expect(workerWarns(container('a', { worker: 'path://../a' }))).toBe(true)
    expect(workerWarns(container('a', { worker: null }))).toBe(true)
  })

  it('says how a stack differs from the default', () => {
    expect(stackDiff(base, base)).toBe(
      'The stack a campaign measures unless it names another.',
    )
    expect(stackDiff({ ...base, id: 'x', template: 'harness' }, base)).toBe(
      'Against default: adds template harness.',
    )
    const pinned: Stack = {
      ...base,
      id: 'stack-1',
      containers: [
        container('harness', { commit: '8c02f93a1d4e', version: null }),
        container('harness-e2e', {
          worker: 'path://../harness-e2e',
          version: null,
        }),
      ],
    }
    expect(stackDiff(pinned, base)).toBe(
      'Against default: changes harness, harness-e2e · drops fp.',
    )
    expect(stackDiff({ ...base, id: 'same' }, base)).toBe('Same as default.')
    expect(stackDiff(pinned, undefined)).toBe('')
    expect(stackDeclares({ ...pinned, warnings: ['w'] })).toBe(
      'iii latest · 2 workers · 1 warning',
    )
  })

  it('splits a YAML line into its key and the rest', () => {
    expect(yamlLine('    worker: package://fp')).toEqual({
      key: '    worker:',
      rest: ' package://fp',
      comment: false,
    })
    expect(yamlLine('  # a note: here')).toEqual({
      key: '',
      rest: '  # a note: here',
      comment: true,
    })
    expect(yamlMeta('iii: latest\ncontainers: {}\n')).toBe('2 lines · 0.0 KB')
  })
})
