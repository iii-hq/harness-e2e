import { describe, expect, it } from 'vitest'
import {
  groupChanges,
  groupHighlights,
  groupLetter,
  groupMembers,
  groupRunnerNote,
  lettersText,
  memberValues,
  scoringGroups,
  spreadOf,
  testListCount,
  testListGap,
  versionRange,
} from '@/lib/comparison-group'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import { compareExecutions, comparisonGroup } from '@/lib/execution-comparison'
import { execution, type Row } from '@/test-fixtures/execution-comparison'

const worker = (name: string, version: string) => ({
  name,
  source: 'package',
  requested: version,
  resolved: version,
  observed: version,
  commit: null,
  dirty: null,
})

/** Six nightly runs as the canvas draws them: shell_coder_sandbox varies
 *  with public_correctness and host_execution, database_migration_recovery
 *  with transaction_scope, contention_ledger's tokens double in D, and the
 *  runner moves shell_coder_sandbox's scoring after A. */
function nightly(): DashboardExecutionDetail[] {
  // Every run reports every criterion; `short` are those below full points.
  const shell = (score: number, short: Array<[string, number, number]>) => ({
    scenario: 'shell_coder_sandbox',
    score,
    criteria: [
      ['investigation', 20],
      ['scope', 5],
      ['public_correctness', 25],
      ['host_execution', 10],
    ].map(([id, possible]) => ({
      id: String(id),
      awarded: short.find(([one]) => one === id)?.[1] ?? Number(possible),
      possible: Number(possible),
      reason: `${id} reason`,
    })),
  })
  const database = (score: number) => ({
    scenario: 'database_migration_recovery',
    score,
    criteria:
      score === 100
        ? [{ id: 'transaction_scope', awarded: 15, possible: 15, reason: 'ok' }]
        : [
            {
              id: 'transaction_scope',
              awarded: 0,
              possible: 15,
              reason: 'exact_report=false',
            },
          ],
  })
  const ledger = (tokens: number): Row => ({
    scenario: 'contention_ledger',
    score: 100,
    tokens,
  })
  const shared: Array<[string, number, number]> = [
    ['investigation', 12, 20],
    ['scope', 0, 5],
  ]
  const rows: Row[][] = [
    [
      {
        ...shell(62, [...shared, ['public_correctness', 0, 25]]),
        behavior: 'old',
      },
      database(85),
      ledger(43541),
      { scenario: 'timer_wake', score: 100 },
    ],
    [
      shell(87, shared),
      database(100),
      ledger(49989),
      { scenario: 'timer_wake', score: 100 },
    ],
    [
      shell(87, shared),
      database(85),
      ledger(46717),
      { scenario: 'timer_wake', score: 100 },
    ],
    [
      shell(87, shared),
      database(100),
      ledger(92468),
      { scenario: 'timer_wake', score: 100 },
    ],
    [
      shell(52, [
        ...shared,
        ['public_correctness', 0, 25],
        ['host_execution', 0, 10],
      ]),
      database(85),
      ledger(49668),
      { scenario: 'timer_wake', score: 100 },
    ],
    [
      shell(87, shared),
      database(85),
      ledger(36884),
      { scenario: 'timer_wake', score: 100 },
    ],
  ]
  const runners = ['0.12.4', '0.14.0', '0.14.0', '0.16.2', '0.17.0', '0.17.1']
  const harnesses = ['1.8.31', '1.8.34', '1.8.35', '1.8.36', '1.8.36', '1.8.38']
  return rows.map((row, index) =>
    execution(`n${index}`, row, {
      stack: [
        worker('harness-e2e', runners[index]),
        worker('harness', harnesses[index]),
        worker('state', index < 3 ? '0.22.3' : '0.22.4'),
      ],
    }),
  )
}

function read(details: DashboardExecutionDetail[], reference = 0) {
  const group = comparisonGroup(details)
  const base = details[reference]
  const pairs = details
    .filter((detail) => detail !== base)
    .map((detail) => compareExecutions(base, detail, { group }))
  return { group, pairs, members: groupMembers(details, base.id, pairs) }
}

describe('letters', () => {
  it('names members by letter, then by number past Z', () => {
    expect([0, 1, 25, 26].map(groupLetter)).toEqual(['A', 'B', 'Z', '27'])
  })

  it('writes runs of three or more as a range', () => {
    expect(lettersText([0])).toBe('A')
    expect(lettersText([0, 4])).toBe('A and E')
    expect(lettersText([1, 2, 3, 5])).toBe('B–D and F')
    expect(lettersText([5, 0, 1, 2, 3])).toBe('A–D and F')
  })
})

describe('a group of six', () => {
  it('gives every member its letter, versions and pair', () => {
    const { members } = read(nightly())
    expect(members.map((member) => member.letter)).toEqual([
      'A',
      'B',
      'C',
      'D',
      'E',
      'F',
    ])
    expect(members[0]).toMatchObject({
      reference: true,
      pair: null,
      runner: '0.12.4',
      harness: '1.8.31',
      otherChanges: null,
    })
    // Only `state` besides the harness and the runner: from D on.
    expect(members.map((member) => member.otherChanges)).toEqual([
      null,
      0,
      0,
      1,
      1,
      1,
    ])
  })

  it('reads every figure against the reference, the same whichever it is', () => {
    const details = nightly()
    const scores = (reference: number) => {
      const { members, pairs } = read(details, reference)
      return memberValues(members, pairs[0], 'score', 'shell_coder_sandbox')
    }
    expect(scores(0)).toEqual([62, 87, 87, 87, 52, 87])
    expect(scores(3)).toEqual(scores(0))
    expect(spreadOf(scores(0))).toEqual({ min: 52, max: 87 })
    expect(spreadOf([null, null])).toBeNull()
  })

  it('says what differs once for the group', () => {
    const { members } = read(nightly())
    expect(groupChanges(members)).toEqual({
      changes: [
        { label: 'harness', value: '1.8.31–1.8.38 · 5 versions' },
        { label: 'runner', value: '0.12.4–0.17.1 · 5 versions' },
        { label: 'other workers', value: 'up to 1 at another build than A' },
      ],
      same: 'Same suite, model and profile in all 6.',
    })
  })

  it('names the tests whose scoring moved with the runner, by member', () => {
    const details = nightly()
    const scoring = scoringGroups(details)
    expect([...scoring]).toEqual([
      ['shell_coder_sandbox', [[0], [1, 2, 3, 4, 5]]],
    ])
    expect(groupRunnerNote(read(details).members, scoring)).toEqual({
      headline: 'The runners differ across the group',
      detail:
        'Runner 0.12.4–0.17.1. The scoring of shell_coder_sandbox (A · B–F) changed with it, so part of those differences may come from the runner, not from the stack.',
    })
  })

  it('highlights who scored below the reference, what varies and why', () => {
    const { members } = read(nightly())
    const { headline, detail, items } = groupHighlights(members)
    // Four tests here: E's 52 on shell_coder_sandbox takes it below A.
    expect(headline).toBe('E scored below A')
    expect(detail).toBe(
      'A scored 86.8; the others 84.3 to 96.8. Every execution completed all 4 counted runs. These are observed differences, not a verdict.',
    )
    expect(
      items.map((item) => [item.test, item.direction, item.text, item.target]),
    ).toEqual([
      [
        'shell_coder_sandbox',
        'mixed',
        'scored 87 in B–D and F, 62 in A and 52 in E: public_correctness 0/25 in A and E; host_execution 0/10 in E.',
        'n4',
      ],
      [
        'database_migration_recovery',
        'up',
        'scored 100 in B and D and 85 in A, C, E and F: transaction_scope 0/15 in A, C, E and F.',
        'n1',
      ],
      [
        'contention_ledger',
        'up',
        'used 112% more tokens in D (43.5K → 92.5K); the others stayed within ±15% of A.',
        'n3',
      ],
      [
        null,
        'same',
        'The other 2 tests kept their scores in all 6 executions.',
        null,
      ],
    ])
  })

  it('says who scored below when the reference moves', () => {
    const { members } = read(nightly(), 1)
    expect(groupHighlights(members).headline).toBe(
      'A, C, E and F scored below B',
    )
  })
})

describe('a member with another test list', () => {
  it('names it and counts the tests that leave every total', () => {
    const details = nightly()
    details.push(
      execution('old', [
        { scenario: 'shell_coder_sandbox', score: 87 },
        { scenario: 'database_migration_recovery', score: 100 },
        { scenario: 'minimal_path', score: 40 },
      ]),
    )
    const { group } = read(details)
    expect(testListGap(details, group)).toEqual({
      odd: [6],
      removable: true,
      missing: 3,
      total: 5,
      counted: 2,
    })
    expect(testListGap(nightly(), comparisonGroup(nightly()))).toBeNull()
  })
})

describe('test lists', () => {
  const list = (id: string, scenarios: string[]) =>
    execution(
      id,
      scenarios.map((scenario) => ({ scenario, score: 80 })),
    )

  it('keeps the reference in a tie and never offers to remove it', () => {
    const details = [
      list('a', ['x', 'y']),
      list('b', ['x', 'y']),
      list('c', ['x']),
      list('d', ['x']),
    ]
    const gap = testListGap(details, comparisonGroup(details), 2)
    expect(gap).toMatchObject({ odd: [0, 1], removable: true, missing: 1 })
  })

  it('does not offer a removal that would leave fewer than two', () => {
    const details = [list('a', ['x']), list('b', ['y']), list('c', ['z'])]
    const gap = testListGap(details, comparisonGroup(details), 0)
    expect(gap).toMatchObject({ odd: [1, 2], removable: false })
    expect(testListCount(details)).toBe(3)
  })

  it('counts the lists, not how each pair words them', () => {
    const details = [list('a', ['x']), list('b', ['y']), list('c', ['z'])]
    const { members } = read(details)
    expect(
      groupChanges(members, testListCount(details)).changes,
    ).toContainEqual({ label: 'tests', value: '3 lists' })
  })
})

describe('token outliers', () => {
  it('leaves out an execution without tokens and one short of its runs', () => {
    const tokens = (id: string, value: number | null) => {
      const detail = execution(id, [{ tokens: value ?? 100 }])
      const run = detail.reports[0].report?.scenarios[0].runs[0] as {
        efficiency?: unknown
      }
      if (value === null) delete run.efficiency
      return detail
    }
    const details = [tokens('a', 1000), tokens('b', 3000), tokens('c', null)]
    const items = groupHighlights(read(details).members).items
    expect(items.map((item) => item.text)).toContain(
      'used 200% more tokens in B (1K → 3K).',
    )
    const running = [tokens('a', 1000), tokens('b', 300)]
    running.push(execution('c', [{ tokens: 1000 }]))
    running[1].status = 'running'
    running[1].reports[0].report?.scenarios[0].aggregate &&
      Object.assign(running[1].reports[0].report.scenarios[0].aggregate, {
        planned_runs: 2,
      })
    expect(
      groupHighlights(read(running).members).items.some(
        (item) => item.metric === 'tokens',
      ),
    ).toBe(false)
  })
})

describe('versions', () => {
  it('orders releases by number, a prerelease before its release', () => {
    expect(versionRange(['0.9.0', '0.17.1', '0.12.4'])).toBe('0.9.0–0.17.1')
    expect(versionRange(['1.8.36', '1.8.36'])).toBe('1.8.36')
    expect(versionRange(['1.8.8', '1.8.8-rc.3'])).toBe('1.8.8-rc.3–1.8.8')
    expect(versionRange(['0.24.3-rc.2', '0.24.3-rc.10'])).toBe(
      '0.24.3-rc.2–0.24.3-rc.10',
    )
    // A commit is not a version: the order given stays.
    expect(versionRange(['@abc1234', '0.16.0'])).toBe('@abc1234–0.16.0')
    expect(versionRange([])).toBe('')
  })
})
