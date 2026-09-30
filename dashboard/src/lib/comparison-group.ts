/**
 * More than two executions read against one reference (canvas: Compare
 * executions · more than three). Every pair of the group already counts the
 * same tests (`comparisonGroup`); this is what the page says about the group
 * as a whole: who is who, what differs, what moved across it.
 */
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import {
  type ComparedMetric,
  type ComparisonGroup,
  compareRuns,
  type ExecutionComparison,
  metricFigure,
  roundedPoints,
  type ScenarioComparison,
  workerRelease,
} from '@/lib/execution-comparison'
import { plural } from '@/lib/format'

const RUNNER = 'harness-e2e'
const HARNESS = 'harness'

/** A, B, … Z, then 27, 28, …: the column's name everywhere on the page. */
export function groupLetter(index: number): string {
  return index < 26 ? String.fromCharCode(65 + index) : String(index + 1)
}

function listText(items: string[]) {
  if (items.length <= 1) return items.join('')
  return `${items.slice(0, -1).join(', ')} and ${items.at(-1)}`
}

/** Members by letter, a run of three or more as a range: `A, C and E–G`. */
export function lettersText(indexes: number[]): string {
  const sorted = [...new Set(indexes)].sort((one, two) => one - two)
  const parts: string[] = []
  for (let start = 0; start < sorted.length; ) {
    let end = start
    while (end + 1 < sorted.length && sorted[end + 1] === sorted[end] + 1)
      end += 1
    if (end - start >= 2)
      parts.push(`${groupLetter(sorted[start])}–${groupLetter(sorted[end])}`)
    else
      for (let index = start; index <= end; index += 1)
        parts.push(groupLetter(sorted[index]))
    start = end + 1
  }
  return listText(parts)
}

export type GroupMember = {
  id: string
  index: number
  letter: string
  title: string
  /** When it started, else when it finished. */
  ranAt: string
  /** GitHub run, Docker attempt or local. */
  source: string
  harness: string | null
  runner: string | null
  reference: boolean
  /** This member against the reference; null for the reference. */
  pair: ExecutionComparison | null
  /** Workers other than the harness and the runner that ran another build
   *  than the reference's; null for the reference. */
  otherChanges: number | null
}

/** Every execution in the order chosen, each with its pair against the
 *  reference. `pairs` are the reference against every other execution. */
export function groupMembers(
  details: DashboardExecutionDetail[],
  referenceId: string,
  pairs: ExecutionComparison[],
): GroupMember[] {
  const [first] = pairs
  return details.map((detail, index) => {
    const reference = detail.id === referenceId
    const pair = reference
      ? null
      : (pairs.find((one) => one.b.id === detail.id) ?? null)
    const side = reference ? first.a : (pair?.b ?? first.a)
    return {
      id: detail.id,
      index,
      letter: groupLetter(index),
      title: side.title,
      ranAt: side.ranAt,
      source: side.source,
      harness: workerRelease(detail, HARNESS),
      runner: workerRelease(detail, RUNNER),
      reference,
      pair,
      otherChanges: pair
        ? pair.stack.changed.filter(
            (change) => change.field !== HARNESS && change.field !== RUNNER,
          ).length
        : null,
    }
  })
}

/** A member's figure: the reference's is any pair's baseline, the others'
 *  their own pair's candidate. Totals when `scenarioId` is null. */
export function memberMetric(
  member: GroupMember,
  first: ExecutionComparison,
  metricId: string,
  scenarioId: string | null = null,
): { metric: ComparedMetric | undefined; side: 'baseline' | 'candidate' } {
  const comparison = member.pair ?? first
  const metrics =
    scenarioId === null
      ? comparison.totals
      : comparison.scenarios.find((scenario) => scenario.id === scenarioId)
          ?.metrics
  return {
    metric: metrics?.find((metric) => metric.id === metricId),
    side: member.reference ? 'baseline' : 'candidate',
  }
}

/** Every member's value of one figure, in order; null where it has none. */
export function memberValues(
  members: GroupMember[],
  first: ExecutionComparison,
  metricId: string,
  scenarioId: string | null = null,
): Array<number | null> {
  return members.map((member) => {
    const { metric, side } = memberMetric(member, first, metricId, scenarioId)
    return metric?.[side] ?? null
  })
}

/** Lowest and highest of the values there are; null when there is none. */
export function spreadOf(
  values: Array<number | null>,
): { min: number; max: number } | null {
  const present = values.filter((value): value is number => value !== null)
  if (present.length === 0) return null
  return { min: Math.min(...present), max: Math.max(...present) }
}

function scenarioIds(detail: DashboardExecutionDetail): string[] {
  return [
    ...new Set([
      ...detail.reports.map((record) => record.scenario_id),
      ...compareRuns(detail).map((run) => run.scenarioId),
    ]),
  ].sort()
}

/** Tests whose scoring moved between the executions that ran them (the
 *  runner's behavior digest differs): the members that share each digest,
 *  in order of first appearance. */
export function scoringGroups(
  details: DashboardExecutionDetail[],
): Map<string, number[][]> {
  const digests = details.map((detail) => {
    const byScenario = new Map<string, Set<string>>()
    for (const run of compareRuns(detail)) {
      if (!run.behavior) continue
      const set = byScenario.get(run.scenarioId) ?? new Set<string>()
      set.add(run.behavior)
      byScenario.set(run.scenarioId, set)
    }
    return new Map(
      [...byScenario].map(([id, set]) => [id, [...set].sort().join(',')]),
    )
  })
  const groups = new Map<string, number[][]>()
  const ids = [...new Set(digests.flatMap((map) => [...map.keys()]))].sort()
  for (const id of ids) {
    const byDigest = new Map<string, number[]>()
    digests.forEach((map, index) => {
      const digest = map.get(id)
      if (digest === undefined) return
      byDigest.set(digest, [...(byDigest.get(digest) ?? []), index])
    })
    if (byDigest.size > 1) groups.set(id, [...byDigest.values()])
  }
  return groups
}

/** How many different test lists the members ran. */
export function testListCount(details: DashboardExecutionDetail[]): number {
  return new Set(details.map((detail) => scenarioIds(detail).join('\n'))).size
}

/** Members that ran another test list than most of the group (the
 *  reference's list wins a tie, and the reference is never one of them),
 *  and how many tests leave every total because some member did not run
 *  them. Null when every member ran the same tests. `removable` is false
 *  when taking them out would leave fewer than two executions. */
export function testListGap(
  details: DashboardExecutionDetail[],
  group: ComparisonGroup,
  referenceIndex = 0,
): {
  odd: number[]
  removable: boolean
  missing: number
  total: number
  counted: number
} | null {
  const lists = details.map((detail) => scenarioIds(detail).join('\n'))
  const tally = new Map<string, number>()
  for (const list of lists) tally.set(list, (tally.get(list) ?? 0) + 1)
  const most = Math.max(...tally.values())
  const own = lists[referenceIndex]
  const common =
    tally.get(own) === most
      ? own
      : [...tally].find(([, count]) => count === most)?.[0]
  const odd = lists.flatMap((list, index) =>
    list === common || index === referenceIndex ? [] : [index],
  )
  const missing = [...group.exclusions.values()].filter(
    (gap) => gap.reason === 'missing',
  ).length
  if (odd.length === 0 || missing === 0) return null
  return {
    odd,
    removable: details.length - odd.length >= 2,
    missing,
    total: group.scenarios.length,
    counted: group.scenarios.length - group.exclusions.size,
  }
}

const VERSION = /^(\d+(?:\.\d+)*)(?:-([0-9A-Za-z.-]+))?$/

function parts(text: string) {
  return text.split('.').map((part) => {
    const number = Number(part)
    return Number.isFinite(number) && part !== '' ? number : part
  })
}

function byParts(a: Array<number | string>, b: Array<number | string>) {
  for (let index = 0; index < Math.max(a.length, b.length); index += 1) {
    const x = a[index] ?? 0
    const y = b[index] ?? 0
    if (x === y) continue
    if (typeof x === 'number' && typeof y === 'number') return x - y
    return String(x).localeCompare(String(y))
  }
  return 0
}

/** Semver order: numbers first, then a prerelease before its release. */
function byVersion(one: string, two: string) {
  const [, baseA, preA] = one.match(VERSION) ?? []
  const [, baseB, preB] = two.match(VERSION) ?? []
  return (
    byParts(parts(baseA), parts(baseB)) ||
    (preA === preB
      ? 0
      : preA === undefined
        ? 1
        : preB === undefined
          ? -1
          : byParts(parts(preA), parts(preB)))
  )
}

/** The lowest and the highest version, or the one there is. Builds that
 *  are not versions (a commit) keep the order given. */
export function versionRange(versions: string[]): string {
  const distinct = [...new Set(versions)]
  if (distinct.length <= 1) return distinct[0] ?? ''
  const sorted = distinct.every((version) => VERSION.test(version))
    ? distinct.sort(byVersion)
    : distinct
  return `${sorted[0]}–${sorted.at(-1)}`
}

export type GroupChange = { label: string; value: string }

/** What differs across the group, once for all of it, and what every
 *  member shares. */
export function groupChanges(
  members: GroupMember[],
  testLists = 1,
): {
  changes: GroupChange[]
  same: string
} {
  const reference = members.find((member) => member.reference) ?? members[0]
  const changes: GroupChange[] = []
  const fields = new Map<string, Set<string>>()
  for (const member of members)
    for (const change of member.pair?.parameters ?? []) {
      // Each pair words its test lists against the reference: counted apart.
      if (change.field === 'scenarios') continue
      const values = fields.get(change.field) ?? new Set<string>()
      values.add(change.a)
      values.add(change.b)
      fields.set(change.field, values)
    }
  for (const [field, values] of fields)
    changes.push({ label: field, value: plural(values.size, 'value') })
  if (testLists > 1)
    changes.push({ label: 'tests', value: plural(testLists, 'list') })
  const versions = (pick: (member: GroupMember) => string | null) =>
    members.map(pick).filter((value): value is string => value !== null)
  for (const [label, pick] of [
    [HARNESS, (member: GroupMember) => member.harness],
    ['runner', (member: GroupMember) => member.runner],
  ] as const) {
    const values = versions(pick)
    const distinct = new Set(values)
    if (distinct.size > 1)
      changes.push({
        label,
        value: `${versionRange(values)} · ${plural(distinct.size, 'version')}`,
      })
  }
  const others = Math.max(
    0,
    ...members.map((member) => member.otherChanges ?? 0),
  )
  if (others > 0)
    changes.push({
      label: 'other workers',
      value: `up to ${others} at another build than ${reference.letter}`,
    })
  const same = [
    !fields.has('suite') ? 'suite' : null,
    !fields.has('model') && !fields.has('provider') ? 'model' : null,
    !fields.has('profile') ? 'profile' : null,
  ].filter((item): item is string => item !== null)
  return {
    changes,
    same:
      same.length > 0 ? `Same ${listText(same)} in all ${members.length}.` : '',
  }
}

/** The runners and the scoring they moved, said once for the group. */
export function groupRunnerNote(
  members: GroupMember[],
  scoring: Map<string, number[][]>,
): { headline: string; detail: string } | null {
  const runners = members
    .map((member) => member.runner)
    .filter((runner): runner is string => runner !== null)
  const differ = new Set(runners).size > 1
  if (!differ && scoring.size === 0) return null
  const moved = [...scoring].map(
    ([id, groups]) => `${id} (${groups.map(lettersText).join(' · ')})`,
  )
  const scored =
    moved.length > 0
      ? `The scoring of ${listText(moved)} changed${differ ? ' with it' : ''}, so part of those differences may come from the runner, not from the stack.`
      : 'Part of any difference may come from the runner, not from the stack.'
  return differ
    ? {
        headline: 'The runners differ across the group',
        detail: `Runner ${versionRange(runners)}. ${scored}`,
      }
    : {
        headline: `The scoring of ${plural(moved.length, 'test')} changed between executions`,
        detail: scored,
      }
}

/** One observed difference across the group worth reading first. */
export type GroupHighlight = {
  /** The test it is about, to open; null for the group as a whole. */
  test: string | null
  /** Which way the others moved from the reference; mixed when both. */
  direction: 'up' | 'down' | 'mixed' | 'same'
  /** The measure that moved, for its colour. */
  metric: 'score' | 'tokens' | null
  text: string
  /** The member to read in detail: the highest when every other rose,
   *  else the lowest. */
  target: string | null
}

function points(value: number) {
  return String(Number(value.toFixed(1)))
}

/** The points a member fell short on, by criterion, for one test. */
function shortCriteria(
  member: GroupMember,
  first: ExecutionComparison,
  scenarioId: string,
): Map<string, { awarded: number; possible: number }> {
  const row = (member.pair ?? first).scenarios.find(
    (scenario) => scenario.id === scenarioId,
  )
  const short = new Map<string, { awarded: number; possible: number }>()
  for (const criterion of [
    ...(row?.criteria ?? []),
    ...(row?.lostOnBoth ?? []),
  ]) {
    const awarded = member.reference ? criterion.a : criterion.b
    if (awarded < criterion.possible)
      short.set(criterion.id, { awarded, possible: criterion.possible })
  }
  return short
}

/** How many runs each member completed of those it planned, over the
 *  counted tests. */
function completion(
  member: GroupMember,
  first: ExecutionComparison,
  counted: ScenarioComparison[],
) {
  const { metric, side } = memberMetric(member, first, 'completed')
  const runs = counted.reduce((total, scenario) => {
    const own = (member.pair ?? first).scenarios.find(
      (row) => row.id === scenario.id,
    )
    return total + (own?.sides[member.reference ? 'a' : 'b'].runs ?? 0)
  }, 0)
  return { done: metric?.[side] ?? 0, runs }
}

const OUTLIER_PERCENT = 50

/**
 * What moved across the group, over the counted tests: who scored below the
 * reference, the tests whose score varies (with the criteria that explain
 * it), token use far from the reference's, and how many tests kept their
 * score everywhere. Every line states a difference; none is a verdict.
 */
export function groupHighlights(members: GroupMember[]): {
  headline: string
  detail: string
  items: GroupHighlight[]
} {
  const reference = members.find((member) => member.reference) ?? members[0]
  const others = members.filter((member) => !member.reference)
  const first = others[0]?.pair
  if (!first) return { headline: '', detail: '', items: [] }
  const counted = first.scenarios.filter((scenario) => scenario.counted)
  const scores = memberValues(members, first, 'score')
  const referenceScore = scores[reference.index]
  const otherScores = others.map((member) => scores[member.index])
  const scored =
    referenceScore !== null && otherScores.every((score) => score !== null)
  const round = (value: number) => Number(value.toFixed(1))
  const below = others.filter(
    (member) =>
      scored && round(scores[member.index] ?? 0) < round(referenceScore ?? 0),
  )
  const headline =
    counted.length === 0
      ? 'No test is counted'
      : !scored
        ? 'No score to compare'
        : below.length === 0
          ? `No execution scored below ${reference.letter}`
          : `${lettersText(below.map((member) => member.index))} scored below ${reference.letter}`
  const completions = members.map((member) =>
    completion(member, first, counted),
  )
  const allDone = completions.every(
    ({ done, runs }) => runs > 0 && done === runs,
  )
  const values = otherScores.filter((score): score is number => score !== null)
  const low = Math.min(...values)
  const high = Math.max(...values)
  const detail =
    counted.length === 0
      ? 'Count at least one test to compare.'
      : !scored
        ? 'An execution has no score or is short of runs: its figures are shown, and no difference is taken from them.'
        : [
            `${reference.letter} scored ${points(referenceScore ?? 0)}; the others ${low === high ? points(low) : `${points(low)} to ${points(high)}`}.`,
            allDone &&
            completions.every(({ runs }) => runs === completions[0].runs)
              ? `Every execution completed all ${plural(completions[0].runs, 'counted run')}.`
              : allDone
                ? 'Every execution completed all of its counted runs.'
                : `Completed runs: ${members.map((member) => `${member.letter} ${completions[member.index].done}/${completions[member.index].runs}`).join(', ')}.`,
            'These are observed differences, not a verdict.',
          ].join(' ')

  const items: GroupHighlight[] = []
  const varied = counted
    .map((scenario) => {
      const values = memberValues(members, first, 'score', scenario.id)
      const spread = spreadOf(values)
      return { scenario, values, spread }
    })
    .filter(
      ({ values, spread }) =>
        values.every((value) => value !== null) &&
        spread !== null &&
        roundedPoints(spread.max - spread.min) > 0,
    )
    .sort(
      (one, two) =>
        (two.spread?.max ?? 0) -
          (two.spread?.min ?? 0) -
          ((one.spread?.max ?? 0) - (one.spread?.min ?? 0)) ||
        one.scenario.id.localeCompare(two.scenario.id),
    )
  for (const { scenario, values } of varied.slice(0, 5)) {
    const own = values as number[]
    const levels = [...new Set(own.map(round))].sort((one, two) => two - one)
    const parts = levels.map(
      (level) =>
        `${points(level)} in ${lettersText(members.filter((member) => round(own[member.index]) === level).map((member) => member.index))}`,
    )
    // The criteria some members fell short on and others did not.
    const short = members.map((member) =>
      shortCriteria(member, first, scenario.id),
    )
    const ids = [...new Set(short.flatMap((map) => [...map.keys()]))]
    const why = ids.flatMap((id) => {
      const byPoints = new Map<string, number[]>()
      let possible = 0
      short.forEach((map, index) => {
        const entry = map.get(id)
        if (!entry) return
        possible = entry.possible
        const key = points(entry.awarded)
        byPoints.set(key, [...(byPoints.get(key) ?? []), index])
      })
      const everyone =
        byPoints.size === 1 &&
        [...byPoints.values()][0].length === members.length
      return everyone
        ? []
        : [...byPoints].map(
            ([awarded, indexes]) =>
              `${id} ${awarded}/${possible} in ${lettersText(indexes)}`,
          )
    })
    const mine = own[reference.index]
    const rest = others.map((member) => own[member.index])
    const direction = rest.every((value) => round(value) >= round(mine))
      ? 'up'
      : rest.every((value) => round(value) <= round(mine))
        ? 'down'
        : 'mixed'
    // The one to read: the highest when every other rose, else the lowest,
    // since a drop is what a reader checks first.
    const [target] = [...others].sort((one, two) =>
      direction === 'up'
        ? own[two.index] - own[one.index]
        : own[one.index] - own[two.index],
    )
    items.push({
      test: scenario.id,
      direction,
      metric: 'score',
      text: `scored ${listText(parts)}${why.length > 0 ? `: ${why.join('; ')}` : ''}.`,
      target: target?.id ?? null,
    })
  }
  if (varied.length > 5)
    items.push({
      test: null,
      direction: 'same',
      metric: null,
      text: `${plural(varied.length - 5, 'more test')} changed score across the group.`,
      target: null,
    })

  for (const scenario of counted) {
    const tokens = memberValues(members, first, 'tokens', scenario.id)
    const base = tokens[reference.index]
    if (!base) continue
    // A figure short of its runs gives no difference, as in a pair.
    const whole = (member: GroupMember) => {
      const { metric } = memberMetric(member, first, 'tokens', scenario.id)
      if (!metric) return false
      // The reference is any pair's baseline; the others their candidate.
      return member.reference
        ? !metric.partial.baseline
        : !metric.partial.baseline && !metric.partial.candidate
    }
    const change = (member: GroupMember) => {
      const value = tokens[member.index]
      return value === null || !whole(member) || !whole(reference)
        ? null
        : ((value - base) / base) * 100
    }
    const measured = others.filter((member) => change(member) !== null)
    const far = measured.filter(
      (member) => Math.abs(change(member) ?? 0) >= OUTLIER_PERCENT,
    )
    const near = measured
      .filter((member) => !far.includes(member))
      .map((member) => Math.abs(change(member) ?? 0))
    for (const member of far) {
      const percent = change(member) ?? 0
      items.push({
        test: scenario.id,
        direction: percent > 0 ? 'up' : 'down',
        metric: 'tokens',
        text: `used ${Math.round(Math.abs(percent))}% ${percent > 0 ? 'more' : 'fewer'} tokens in ${member.letter} (${metricFigure('tokens', base)} → ${metricFigure('tokens', tokens[member.index] ?? 0)})${near.length > 0 ? `; the others stayed within ±${Math.round(Math.max(...near))}% of ${reference.letter}` : ''}.`,
        target: member.id,
      })
    }
  }

  const kept = counted.filter((scenario) => {
    const values = memberValues(members, first, 'score', scenario.id)
    const spread = spreadOf(values)
    return (
      values.every((value) => value !== null) &&
      spread !== null &&
      roundedPoints(spread.max - spread.min) === 0
    )
  }).length
  if (kept > 0 && scored)
    items.push({
      test: null,
      direction: 'same',
      metric: null,
      text:
        varied.length > 0
          ? `The other ${plural(kept, 'test')} kept ${kept === 1 ? 'its score' : 'their scores'} in all ${members.length} executions.`
          : `All ${plural(kept, 'counted test')} kept their scores in all ${members.length} executions.`,
      target: null,
    })
  return { headline, detail, items }
}
