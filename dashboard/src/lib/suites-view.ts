import { withSequentialGroups } from '@/components/LocalRunnerDialog'
import type {
  DashboardExecutionSummary,
  Suite,
} from '@/lib/dashboard-data-source'
import {
  buildExecutionPresentation,
  executionOrigin,
  executionResult,
  executionScore,
  executionTitle,
  providerModel,
} from '@/lib/execution-view'
import { formatDateTime, formatDay, plural } from '@/lib/format'
import { RESULT_STATES, type ResultState } from '@/lib/result-status'
import type { CatalogRowView } from '@/lib/test-catalog-view'

// The Suites page as the redesign canvas draws it (Suites.dc.html): the
// repository's suites and this Console's in a list, one suite open beside it
// with its tests' last results and the executions that ran it.

/** `4 in the repository, read-only · none in this Console · …` */
export function suitesSummary(suites: Suite[]) {
  const repository = suites.filter((suite) => suite.source !== 'local').length
  const local = suites.length - repository
  return `${repository} in the repository, read-only · ${local || 'none'} in this Console · a suite holds tests, runs of each and retries; model and stack are picked when it runs`
}

/** `9 tests · 1 run each · 1 retry` */
export function suiteHolds(tests: number, runs: number, retries: number) {
  return [
    plural(tests, 'test'),
    `${plural(runs, 'run')} each`,
    plural(retries, 'retry', 'retries'),
  ].join(' · ')
}

/** The short digest a suite materializes to; `—` when it does not. */
export function suiteDigest(suite: Suite) {
  return suite.sha256 ? suite.sha256.replace(/^sha256:/, '').slice(0, 12) : '—'
}

/** The tests that ran here, but only on an earlier definition: their last
 *  results no longer describe them. A test never run is not among them. */
export function changedTests(
  ids: string[],
  views: ReadonlyMap<string, CatalogRowView>,
) {
  return ids.filter((id) => views.get(id)?.kind === 'changed')
}

export function changedNote(changed: number, total: number) {
  return `${changed} of ${total} changed since their last run`
}

export function changedWarning(changed: number, total: number) {
  return `${changed} of these ${plural(total, 'test')} changed definition since they last ran here, so the results below don’t describe them anymore. Running the suite refreshes them.`
}

/** One execution of a suite, as its detail lists it. */
export type SuiteRun = {
  id: string
  title: string
  /** `GitHub #359… · Sep 24, 4:25 AM · deepseek/deepseek-flash · profile x` */
  meta: string
  result: { state: ResultState; label?: string }
  /** `11 of 15 passed · score 93.2`; empty when neither is known. */
  outcome: string
  /** `Sep 24`; null without a date. */
  day: string | null
}

function numeric(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

/** The executions that ran the suite (by id, as the list gives them: newest
 *  first). An unnamed selection belongs to no suite. */
export function suiteRuns(
  executions: DashboardExecutionSummary[],
  suiteId: string,
  now = new Date(),
): SuiteRun[] {
  return executions
    .filter((execution) => execution.parameters?.suite?.id === suiteId)
    .map((execution) => {
      const presentation = buildExecutionPresentation(execution)
      const date = presentation.completedAt || presentation.startedAt
      const subject = presentation.subjects[0]
      const passed = numeric(execution.totals?.passed_scenarios)
      const expected = presentation.expectedReports
      const score = executionScore(execution)
      return {
        id: execution.id,
        title: executionTitle(presentation).title,
        meta: [
          executionOrigin(execution).label,
          date ? formatDateTime(date, now) : null,
          subject ? providerModel(subject) : null,
          execution.parameters?.agent
            ? `profile ${execution.parameters.agent}`
            : null,
        ]
          .filter(Boolean)
          .join(' · '),
        result: executionResult(presentation),
        outcome: [
          passed !== null && expected !== null
            ? `${passed} of ${expected} passed`
            : null,
          score !== null
            ? `score ${score.toLocaleString('en-US', { maximumFractionDigits: 1 })}`
            : null,
        ]
          .filter(Boolean)
          .join(' · '),
        day: date ? formatDay(date, now) : null,
      }
    })
}

/** A suite in the list: its size, its last execution, and how many of its
 *  tests changed since they ran. */
export type SuiteListItem = {
  id: string
  label: string
  count: string
  /** The last execution's result, `Failed · Sep 24`; null when it never ran
   *  in this Console. */
  last: { state: ResultState; label: string } | null
  changed: string | null
}

export function suiteListItem(
  suite: Suite,
  runs: SuiteRun[],
  views: ReadonlyMap<string, CatalogRowView>,
): SuiteListItem {
  const last = runs[0]
  const changed = changedTests(suite.scenarios, views).length
  return {
    id: suite.id,
    label: suite.label,
    count: plural(suite.scenarios.length, 'test'),
    last: last
      ? {
          state: last.result.state,
          label: [
            last.result.label ?? RESULT_STATES[last.result.state].label,
            last.day,
          ]
            .filter(Boolean)
            .join(' · '),
        }
      : null,
    changed: changed > 0 ? changedNote(changed, suite.scenarios.length) : null,
  }
}

/** `1 of 2 · in order` for each test of a sequential group the tests hold. */
export function sequenceSteps(ids: string[], groups: string[][]) {
  const steps = new Map<string, string>()
  for (const group of groups) {
    const members = group.filter((id) => ids.includes(id))
    if (members.length < 2) continue
    for (const [index, id] of members.entries())
      steps.set(id, `${index + 1} of ${members.length} · in order`)
  }
  return steps
}

function sentence(items: string[]) {
  return items.length < 2
    ? items.join('')
    : `${items.slice(0, -1).join(', ')} and ${items.at(-1)}`
}

/** The line beside the Tests heading. */
export function testsNote(ids: string[], groups: string[][], editing: boolean) {
  if (editing)
    return 'Untick to take a test out; unticked ones stay listed until you save.'
  const whole = groups
    .map((group) => group.filter((id) => ids.includes(id)))
    .filter((members) => members.length > 1)
    .map(sentence)
  return [
    'Last result of each test in this Console.',
    whole.length ? `${whole.join('; ')} run whole, in order.` : null,
  ]
    .filter(Boolean)
    .join(' ')
}

/* -------------------------------------------------------------- editing */

/** A suite of this Console being edited, until it is saved or discarded. */
export type SuiteDraft = {
  label: string
  runs: number
  retries: number
  /** What it will hold. */
  tests: string[]
  /** Every row the table shows, in place: unticked ones stay listed. */
  shown: string[]
}

export function suiteDraft(suite: Suite): SuiteDraft {
  return {
    label: suite.label,
    runs: suite.repetitions,
    retries: suite.technical_retries,
    tests: suite.scenarios,
    shown: suite.scenarios,
  }
}

/** Ticks or unticks a test; a sequential group follows its test whole. */
export function tickDraft(
  draft: SuiteDraft,
  id: string,
  on: boolean,
  groups: string[][],
): SuiteDraft {
  const next = on
    ? draft.tests.includes(id)
      ? draft.tests
      : [...draft.tests, id]
    : draft.tests.filter((test) => test !== id)
  const tests = withSequentialGroups(next, draft.tests, groups)
  return {
    ...draft,
    tests,
    shown: [
      ...draft.shown,
      ...tests.filter((test) => !draft.shown.includes(test)),
    ],
  }
}

export function draftDirty(draft: SuiteDraft, suite: Suite) {
  const same = (left: string[], right: string[]) =>
    left.length === right.length && left.every((id) => right.includes(id))
  return (
    draft.label !== suite.label ||
    draft.runs !== suite.repetitions ||
    draft.retries !== suite.technical_retries ||
    !same(draft.tests, suite.scenarios)
  )
}

/** What keeps the draft from being saved, said in the page's words. */
export function draftProblem(draft: SuiteDraft): string | null {
  if (!draft.label.trim()) return 'Name the suite.'
  if (draft.tests.length === 0) return 'Keep at least one test.'
  return null
}

/** Tests of the catalog to add: they match the search and are not in the
 *  draft; retired ones cannot run. */
export function testSuggestions(
  query: string,
  views: CatalogRowView[],
  draft: SuiteDraft,
  limit = 6,
) {
  const wanted = query.trim().toLowerCase()
  if (!wanted) return []
  return views
    .filter(
      (view) =>
        view.selectable &&
        !draft.tests.includes(view.id) &&
        view.search.includes(wanted),
    )
    .slice(0, limit)
    .map((view) => view.id)
}
