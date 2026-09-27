/** Pure derivations of the Run tests / Run again dialog: what the tests
 *  column lists, what the footer says and what the Run button starts. */

export type Where = 'harness' | 'docker' | 'github'
export type CheckState = 'on' | 'off' | 'some'

export function plural(count: number, one: string, many: string) {
  return `${count} ${count === 1 ? one : many}`
}

export type TestFamily = {
  key: string
  label: string
  /** A family is named by an id prefix, written in mono; Standalone is prose. */
  mono: boolean
  items: string[]
}

/** Tests sharing an id prefix (before the first `_`) with another test form
 *  a family, sorted by name; every other test is Standalone, last. */
export function testFamilies(tests: string[]): TestFamily[] {
  const byFamily = new Map<string, string[]>()
  for (const id of tests) {
    const family = id.split('_')[0] ?? id
    byFamily.set(family, [...(byFamily.get(family) ?? []), id])
  }
  const named = [...byFamily.entries()]
    .filter(([, items]) => items.length > 1)
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([key, items]) => ({ key, label: key, mono: true, items }))
  const singles = tests
    .filter((id) => (byFamily.get(id.split('_')[0] ?? id)?.length ?? 0) === 1)
    .sort((left, right) => left.localeCompare(right))
  return singles.length > 0
    ? [
        ...named,
        { key: 'standalone', label: 'Standalone', mono: false, items: singles },
      ]
    : named
}

/** The tests a query and the All/Selected filter leave visible. */
export function visibleTests(
  tests: string[],
  selected: string[],
  query: string,
  onlySelected: boolean,
) {
  const q = query.trim().toLowerCase()
  return tests.filter(
    (id) =>
      (!q || id.includes(q) || id.replace(/_/g, ' ').includes(q)) &&
      (!onlySelected || selected.includes(id)),
  )
}

export function checkState(ids: string[], selected: string[]): CheckState {
  const ticked = ids.filter((id) => selected.includes(id)).length
  return ticked === 0 ? 'off' : ticked === ids.length ? 'on' : 'some'
}

/** Ticks every id, or unticks them all when they are all ticked. */
export function toggleAll(ids: string[], selected: string[]) {
  return checkState(ids, selected) === 'on'
    ? selected.filter((id) => !ids.includes(id))
    : [...selected, ...ids.filter((id) => !selected.includes(id))]
}

/** "2 of 3 · in order" for a test of a sequence. */
export function sequenceStep(id: string, groups: string[][]) {
  const group = groups.find((candidate) => candidate.includes(id))
  return group ? `${group.indexOf(id) + 1} of ${group.length} · in order` : ''
}

export function whereHint(where: Where, dockerGroups: number) {
  if (where === 'docker')
    return `In the executor image, one container per group, ${plural(dockerGroups, 'group', 'groups')} at a time. Its page follows the groups; the results arrive once every group has finished.`
  if (where === 'github')
    return 'Dispatches the exact-stack workflow, one job per group, in GitHub’s queue and minutes. Its page follows the run; the results are imported when it ends.'
  return 'On the stack this Console runs on, one execution at a time.'
}

export type PendingInput = {
  ready: boolean
  where: Where
  hasStack: boolean
  githubBlocked: boolean
  hasModel: boolean
  tests: number
}

/** What is still missing before Run; empty when it can run. */
export function pendingReasons({
  ready,
  where,
  hasStack,
  githubBlocked,
  hasModel,
  tests,
}: PendingInput): string[] {
  if (!ready) return []
  const pending: string[] = []
  if (where !== 'harness' && !hasStack)
    pending.push('pick the stack it runs on')
  if (where === 'github' && githubBlocked)
    pending.push('sign in with gh on the worker’s machine')
  if (!hasModel) pending.push('choose a model')
  if (tests === 0) pending.push('tick at least one test')
  return pending
}

export function pendingText(ready: boolean, pending: string[]) {
  if (!ready) return 'The catalog has to load before running.'
  return `Before running, ${pending.join(' and ')}.`
}

export function summaryCounts(tests: number, runs: number) {
  return `${plural(tests, 'test', 'tests')} · ${plural(tests * runs, 'run', 'runs')}`
}

export function summaryDetail({
  runs,
  retries,
  suite,
  where,
  stack,
}: {
  runs: number
  retries: number
  suite: string | null
  where: Where
  stack: string | null
}) {
  const place =
    where === 'docker'
      ? ` · in Docker${stack ? ` on ${stack}` : ''}`
      : where === 'github'
        ? ` · on GitHub${stack ? ` on ${stack}` : ''}`
        : ' · on this harness'
  return `${plural(runs, 'run', 'runs')} per test · ${plural(retries, 'retry', 'retries')} · ${suite ?? 'custom selection'}${place}`
}

export function runLabel(tests: number, where: Where) {
  if (tests === 0) return 'Run tests'
  const place =
    where === 'docker' ? ' in Docker' : where === 'github' ? ' on GitHub' : ''
  return `Run ${plural(tests, 'test', 'tests')}${place}`
}

export function selectionText(selected: number, hidden: number) {
  if (selected === 0) return 'none selected'
  return `${selected} selected${hidden ? ` · ${hidden} hidden` : ''}`
}

/** What a stack declares, in one line: "iii 0.11 · template harness · 5 workers". */
export function stackDeclares(stack: {
  iii: string | null
  template: string | null
  containers: unknown[]
}) {
  return [
    `iii ${stack.iii ?? 'latest'}`,
    stack.template ? `template ${stack.template}` : null,
    plural(stack.containers.length, 'worker', 'workers'),
  ]
    .filter(Boolean)
    .join(' · ')
}
