// What Run tests and Run again show: the test list, the stack and suite
// hints, the footer. The form and the request it sends live with the dialog
// (components/LocalRunnerDialog.tsx).
import type {
  DashboardExecutionSummary,
  JsonObject,
  Stack,
  Suite,
} from '@/lib/dashboard-data-source'
import {
  buildExecutionPresentation,
  executionTitle,
} from '@/lib/execution-view'
import { plural } from '@/lib/format'

export type Tick = 'off' | 'some' | 'on'

/** A box over several tests: on when every one is ticked. */
export function tickState(ids: string[], selected: string[]): Tick {
  const ticked = ids.filter((id) => selected.includes(id)).length
  return ticked === 0 ? 'off' : ticked === ids.length ? 'on' : 'some'
}

export type TestFamily = {
  key: string
  label: string
  /** A family reads as the ids' prefix; Standalone as words. */
  mono: boolean
  items: string[]
}

/** Tests by family, the id's first word, when a family has more than one;
 *  the rest, sorted, as Standalone. */
export function testFamilies(ids: string[]): TestFamily[] {
  const families = new Map<string, string[]>()
  for (const id of ids) {
    const family = id.split('_')[0]
    families.set(family, [...(families.get(family) ?? []), id])
  }
  const named = [...families.entries()]
    .filter(([, items]) => items.length > 1)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([key, items]) => ({ key, label: key, mono: true, items }))
  const singles = ids
    .filter((id) => families.get(id.split('_')[0])?.length === 1)
    .sort()
  return singles.length > 0
    ? [
        ...named,
        { key: 'standalone', label: 'Standalone', mono: false, items: singles },
      ]
    : named
}

/** The tests a query and the All | Selected filter leave shown. */
export function visibleTests(
  ids: string[],
  query: string,
  filter: 'all' | 'selected',
  selected: string[],
): string[] {
  const q = query.trim().toLowerCase()
  return ids.filter(
    (id) =>
      (!q || id.includes(q) || id.replace(/_/g, ' ').includes(q)) &&
      (filter === 'all' || selected.includes(id)),
  )
}

/** Where a test sits in the sequence it runs in, if any. */
export function sequenceStep(id: string, groups: string[][]): string | null {
  const group = groups.find((ids) => ids.includes(id))
  return group ? `${group.indexOf(id) + 1} of ${group.length} · in order` : null
}

/** What a stack declares: its iii, its template, how many workers, and the
 *  ones pinned at a commit. */
export function stackDeclares(
  stack: Pick<Stack, 'iii' | 'template' | 'containers'>,
): string {
  return [
    `iii ${stack.iii ?? '—'}`,
    stack.template ? `template ${stack.template}` : null,
    plural(stack.containers.length, 'worker'),
    ...stack.containers
      .filter((container) => container.commit)
      .map((container) => `${container.name} at a commit`),
  ]
    .filter(Boolean)
    .join(' · ')
}

/** The iii a stack as recorded pins, read from its YAML; the worker lists
 *  no containers for it. */
export function recordedStackDeclares(yaml: string): string {
  const iii = /^iii:\s*["']?([^"'\s#]+)/m.exec(yaml)?.[1]
  return iii ? `iii ${iii}` : ''
}

type HintedSuite = Pick<
  Suite,
  'label' | 'repetitions' | 'technical_retries'
> & { source?: Suite['source']; recorded?: boolean }

/** Under the suite: where the named suite comes from, its runs and retries;
 *  or that a changed one runs as a custom selection. */
export function suiteHint(
  named: HintedSuite | null,
  picked: HintedSuite | null,
): string | null {
  if (named)
    return [
      named.recorded
        ? 'As this execution ran'
        : named.source === 'local'
          ? 'Saved in this Console'
          : 'Repository suite',
      `${plural(named.repetitions, 'run')} per test`,
      plural(named.technical_retries, 'retry', 'retries'),
    ].join(' · ')
  return picked
    ? `Changed from ${picked.label}. Runs as a custom selection.`
    : null
}

/** The footer's two lines: counts, then how and where. */
export function runSummary({
  tests,
  runs,
  retries,
  suite,
  where,
  stack,
}: {
  tests: number
  runs: number
  retries: number
  suite: string | null
  where: 'harness' | 'docker'
  stack: string | null
}): { counts: string; detail: string } {
  return {
    counts: `${plural(tests, 'test')} · ${plural(tests * runs, 'run')}`,
    detail: [
      `${plural(runs, 'run')} per test`,
      plural(retries, 'retry', 'retries'),
      suite ?? 'custom selection',
      where === 'docker'
        ? `in Docker${stack ? ` on ${stack}` : ''}`
        : 'on this harness',
    ].join(' · '),
  }
}

/** What still keeps the run button off; null when nothing does. */
export function pendingText({
  loading,
  noStack,
  noModel,
  tests,
}: {
  loading: boolean
  noStack: boolean
  noModel: boolean
  tests: number
}): string | null {
  if (loading) return 'The catalog has to load before running.'
  const pending = [
    noStack && 'pick the stack it runs on',
    noModel && 'choose a model',
    tests === 0 && 'tick at least one test',
  ].filter(Boolean)
  return pending.length > 0 ? `Before running, ${pending.join(' and ')}.` : null
}

export function runLabel(tests: number, where: 'harness' | 'docker') {
  return tests > 0
    ? `Run ${plural(tests, 'test')}${where === 'docker' ? ' in Docker' : ''}`
    : 'Run tests'
}

/** The execution holding this harness: running here, not in Docker or on
 *  GitHub, which never wait for it. */
export function harnessBusy(
  executions: DashboardExecutionSummary[],
): { id: string; title: string } | null {
  const running = executions.find(
    (execution) =>
      (execution.status === 'running' || execution.status === 'cancelling') &&
      (execution.parameters?.where ?? 'harness') === 'harness' &&
      !['docker', 'github'].includes(
        String((execution.source as JsonObject | undefined)?.kind ?? ''),
      ),
  )
  return running
    ? {
        id: running.id,
        title: executionTitle(buildExecutionPresentation(running)).title,
      }
    : null
}
