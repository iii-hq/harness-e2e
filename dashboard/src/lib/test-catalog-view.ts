import { formatDateTime, formatDay, NOT_REPORTED, plural } from '@/lib/format'
import { type ResultState, runResultState } from '@/lib/result-status'
import type { TestCatalogRow, TestVersionResult } from '@/lib/test-catalog'

export type ResultFilter = 'all' | 'passed' | 'issues' | 'missing' | 'changed'

export type ComparisonUtility = {
  comparable: number
  evidenceOnBothSides: number
  evidenceRows: number
}

export type ComparisonWarning = {
  title: string
  detail: string
}

const REASON_WARNINGS: Record<string, ComparisonWarning> = {
  comparison_side_missing: {
    title: 'Evidence is missing on one side',
    detail:
      'A delta would compare unlike samples, so only the retained side is shown.',
  },
  scenario_contract_changed: {
    title: 'Scenario contract changed',
    detail:
      'The canonical cases, the scenario definition, or the execution policy differ between definitions. Scores and deltas are not comparable.',
  },
  scenario_contract_conflict: {
    title: 'Scenario contract conflict',
    detail:
      'At least one side contains conflicting case or scenario identities. Resolve the retained evidence before comparing.',
  },
  assessment_profile_changed: {
    title: 'Assessment profile changed',
    detail:
      'The scenario definition or the assessment definition differs. The definition digest is the compatibility boundary for prompt and rubric changes.',
  },
  assessment_profile_conflict: {
    title: 'Assessment profile conflict',
    detail:
      'At least one side contains multiple assessment definitions for the same scenario definition.',
  },
  cohort_changed: {
    title: 'Evaluation cohort changed',
    detail:
      'Subject model or lane identity differs. Cross-cohort deltas are not valid.',
  },
  missing_side: {
    title: 'Evidence is missing on one side',
    detail:
      'A delta would compare unlike samples, so only the retained side is shown.',
  },
  contract_changed: {
    title: 'Scenario contract changed',
    detail:
      'The canonical cases, the scenario definition, or the execution policy differ between definitions. Scores and deltas are not comparable.',
  },
  contract_conflict: {
    title: 'Scenario contract conflict',
    detail:
      'At least one side contains conflicting case or scenario identities. Resolve the retained evidence before comparing.',
  },
  assessment_changed: {
    title: 'Assessment profile changed',
    detail:
      'The scenario definition or the assessment definition differs. The definition digest is the compatibility boundary for prompt and rubric changes.',
  },
  assessment_conflict: {
    title: 'Assessment profile conflict',
    detail:
      'At least one side contains multiple assessment definitions for the same scenario definition.',
  },
}

export function comparisonWarnings(
  result: TestVersionResult | null,
): ComparisonWarning[] {
  if (!result || result.compatibility === 'compatible') return []
  const reasons =
    result.compatibility_reasons.length > 0
      ? result.compatibility_reasons
      : [result.compatibility]
  return reasons.map(
    (reason) =>
      REASON_WARNINGS[reason] ?? {
        title: 'Comparison is incompatible',
        detail: `The retained data reported ${reason.replaceAll('_', ' ')}. No delta is calculated.`,
      },
  )
}

function hasIssuesInB(row: TestCatalogRow) {
  const to = row.result?.to
  if (!to) return false
  return to.outcomes.technical_failed + to.outcomes.infra_failed > 0
}

export function hasRetainedEvidence(row: TestCatalogRow) {
  return Boolean(row.result?.from || row.result?.to)
}

export function matchesResultFilter(row: TestCatalogRow, filter: ResultFilter) {
  if (filter === 'all') return true
  const result = row.result
  if (filter === 'missing') return result?.compatibility === 'missing_side'
  if (filter === 'changed') {
    return (
      result?.compatibility === 'contract_changed' ||
      result?.compatibility === 'contract_conflict' ||
      result?.compatibility === 'assessment_changed' ||
      result?.compatibility === 'assessment_conflict'
    )
  }
  if (!result?.to) return false
  return filter === 'issues' ? hasIssuesInB(row) : !hasIssuesInB(row)
}

function usefulness(row: TestCatalogRow) {
  const result = row.result
  const hasFrom = Boolean(result?.from)
  const hasTo = Boolean(result?.to)
  const hasBoth = hasFrom && hasTo
  if (hasBoth && result?.compatibility === 'compatible') {
    return hasIssuesInB(row) ? 0 : 1
  }
  if (hasBoth) return 2
  if (hasFrom || hasTo) return hasIssuesInB(row) ? 3 : 4
  return row.lifecycle === 'never_run' ? 6 : 5
}

export function sortCatalogRows(rows: TestCatalogRow[]) {
  return [...rows].sort(
    (left, right) =>
      usefulness(left) - usefulness(right) ||
      left.test_id.localeCompare(right.test_id),
  )
}

export function comparisonUtility(rows: TestCatalogRow[]): ComparisonUtility {
  return rows.reduce<ComparisonUtility>(
    (summary, row) => {
      const result = row.result
      if (result?.compatibility === 'compatible') summary.comparable += 1
      if (result?.from && result.to) summary.evidenceOnBothSides += 1
      if (result?.from || result?.to) summary.evidenceRows += 1
      return summary
    },
    { comparable: 0, evidenceOnBothSides: 0, evidenceRows: 0 },
  )
}

export function isMoreUsefulComparison(
  candidate: ComparisonUtility,
  current: ComparisonUtility,
) {
  return (
    candidate.comparable > current.comparable ||
    (candidate.comparable === current.comparable &&
      candidate.evidenceOnBothSides > current.evidenceOnBothSides) ||
    (candidate.comparable === current.comparable &&
      candidate.evidenceOnBothSides === current.evidenceOnBothSides &&
      candidate.evidenceRows > current.evidenceRows)
  )
}

/* ------------------------------------------------------------ catalog */

// The Tests page as the redesign canvas draws it (TestsCatalog.dc.html):
// every test once, grouped by family, with its last run, recent scores,
// runs and suites.

/** Where a test stands: a run on its current definition, runs only on an
 *  earlier one, or no run retained. A retired test, out of the catalog but
 *  with runs retained, stands apart: it cannot run any more. */
export type CatalogKind = 'current' | 'changed' | 'never' | 'retired'

export function catalogKind(row: TestCatalogRow): CatalogKind {
  if (row.lifecycle === 'retired') return 'retired'
  if (row.runs_current > 0) return 'current'
  return row.runs_total > 0 ? 'changed' : 'never'
}

/** Retired tests show only under All. */
export type CatalogFilter = 'all' | 'current' | 'changed' | 'never'
export type CatalogSort = 'name' | 'last_run' | 'runs'

export type CatalogFilters = {
  query: string
  filter: CatalogFilter
  /** `all` or a suite id. */
  suite: string
  sort: CatalogSort
}

export const CATALOG_DEFAULT_FILTERS: CatalogFilters = {
  query: '',
  filter: 'all',
  suite: 'all',
  sort: 'name',
}

const FILTERS: CatalogFilter[] = ['all', 'current', 'changed', 'never']
export const CATALOG_SORTS: Array<{ value: CatalogSort; label: string }> = [
  { value: 'name', label: 'Sort: name' },
  { value: 'last_run', label: 'Sort: last run' },
  { value: 'runs', label: 'Sort: most runs' },
]

/** Audit T-08: the filters live in the hash so "back" restores them. */
export function catalogFiltersFromParams(
  params: URLSearchParams,
): CatalogFilters {
  const filter = params.get('filter')
  const sort = params.get('sort')
  return {
    query: params.get('q') ?? '',
    filter: FILTERS.find((entry) => entry === filter) ?? 'all',
    suite: params.get('suite') || 'all',
    sort: CATALOG_SORTS.find((entry) => entry.value === sort)?.value ?? 'name',
  }
}

export function catalogFiltersToParams(
  filters: CatalogFilters,
): URLSearchParams {
  const params = new URLSearchParams()
  if (filters.query.trim()) params.set('q', filters.query.trim())
  if (filters.filter !== 'all') params.set('filter', filters.filter)
  if (filters.suite !== 'all') params.set('suite', filters.suite)
  if (filters.sort !== 'name') params.set('sort', filters.sort)
  return params
}

export type CatalogSuite = { id: string; label: string }

/** One test as the list shows it: every cell already written. */
export type CatalogRowView = {
  id: string
  kind: CatalogKind
  /** The summary, else `4 criteria · human 1–2 h`. */
  sub: string
  result: ResultState
  /** The last run's score; `—` without one, null when it never ran. */
  score: string | null
  /** `Sep 24`, `Sep 8 · older definition` or `No run retained`. */
  when: string
  whenTitle: string | null
  /** Its last run belongs to an earlier definition: shown faded. */
  older: boolean
  /** It can be ticked, to run or to save as a suite: not a retired test. */
  selectable: boolean
  /** Fractions of 100, oldest first; null where a run had no score. */
  spark: Array<number | null>
  sparkLabel: string
  runs: number
  runsTitle: string
  lastAt: string | null
  suites: CatalogSuite[]
  search: string
}

function humanHorizon(row: TestCatalogRow) {
  const horizon = row.characterization?.human_horizon
  const min = horizon?.min_minutes
  const max = horizon?.max_minutes
  if (min === undefined || max === undefined) return null
  const range = min >= 60 ? `${min / 60}–${max / 60} h` : `${min}–${max} min`
  return `human ${range}`
}

function scoreText(score: number) {
  return score.toLocaleString('en-US', { maximumFractionDigits: 1 })
}

export function catalogRowView(
  row: TestCatalogRow,
  suites: CatalogSuite[],
  now = new Date(),
): CatalogRowView {
  const last = row.last_run
  const criteria = row.spec?.criteria.length
  const summary = row.spec?.summary?.trim()
  const kind = catalogKind(row)
  const retired = kind === 'retired'
  const older = retired || last?.definition === 'previous'
  const scores = row.recent_scores
  return {
    id: row.test_id,
    kind,
    sub:
      summary ||
      [
        criteria === undefined
          ? null
          : plural(criteria, 'criterion', 'criteria'),
        humanHorizon(row),
      ]
        .filter(Boolean)
        .join(' · '),
    result: retired ? 'retired' : last ? runResultState(last) : 'never_run',
    score: last
      ? typeof last.score === 'number'
        ? scoreText(last.score)
        : NOT_REPORTED
      : null,
    when: last
      ? `${formatDay(last.at, now)}${retired ? ' · last run' : older ? ' · older definition' : ''}`
      : 'No run retained',
    whenTitle: last ? formatDateTime(last.at, now) : null,
    older,
    selectable: !retired,
    spark: scores.map((score) => (score === null ? null : score / 100)),
    sparkLabel: scores.length
      ? `Recent scores: ${scores.map((score) => (score === null ? 'none' : scoreText(score))).join(', ')}`
      : 'No scores',
    runs: row.runs_current,
    runsTitle: `${row.runs_current} on the current definition · ${row.runs_total} retained in all`,
    lastAt: last?.at ?? null,
    suites,
    search: [row.test_id, summary, ...suites.map((suite) => suite.label)]
      .filter(Boolean)
      .join(' ')
      .toLowerCase(),
  }
}

/** Search, filter and suite, then the sort. */
export function filterCatalog(
  views: CatalogRowView[],
  filters: CatalogFilters,
): CatalogRowView[] {
  const query = filters.query.trim().toLowerCase()
  const matched = views.filter(
    (view) =>
      (filters.filter === 'all' || view.kind === filters.filter) &&
      (filters.suite === 'all' ||
        view.suites.some((suite) => suite.id === filters.suite)) &&
      (!query || view.search.includes(query)),
  )
  const byName = (a: CatalogRowView, b: CatalogRowView) =>
    a.id.localeCompare(b.id)
  const by: Record<
    CatalogSort,
    (a: CatalogRowView, b: CatalogRowView) => number
  > = {
    name: byName,
    last_run: (a, b) =>
      (b.lastAt ?? '').localeCompare(a.lastAt ?? '') || byName(a, b),
    // What the Runs column shows: runs on the current definition.
    runs: (a, b) => b.runs - a.runs || byName(a, b),
  }
  return [...matched].sort(by[filters.sort])
}

export type CatalogGroup = {
  key: string
  label: string
  /** A family is named by its id prefix, in mono; Standalone is not. */
  mono: boolean
  /** `all current`, `none current`, `2 of 4 current`: the whole family. */
  note: string
  rows: CatalogRowView[]
}

/** Families: tests sharing the part of their id before the first `_`, when
 *  two or more do; the rest are Standalone, last. `views` keep their order
 *  inside each group; a group with nothing shown is left out. */
export function groupCatalog(
  all: CatalogRowView[],
  views: CatalogRowView[],
): CatalogGroup[] {
  const family = (id: string) => id.split('_')[0]
  const members = new Map<string, CatalogRowView[]>()
  for (const view of all)
    members.set(family(view.id), [
      ...(members.get(family(view.id)) ?? []),
      view,
    ])
  const named = [...members.keys()]
    .filter((key) => (members.get(key)?.length ?? 0) > 1)
    .sort()
  const note = (whole: CatalogRowView[]) => {
    const current = whole.filter((view) => view.kind === 'current').length
    return current === whole.length
      ? 'all current'
      : current === 0
        ? 'none current'
        : `${current} of ${whole.length} current`
  }
  const groups: CatalogGroup[] = named.map((key) => ({
    key,
    label: key,
    mono: true,
    note: note(members.get(key) ?? []),
    rows: views.filter((view) => family(view.id) === key),
  }))
  const single = (view: CatalogRowView) => !named.includes(family(view.id))
  groups.push({
    key: ':standalone',
    label: 'Standalone',
    mono: false,
    note: note(all.filter(single)),
    rows: views.filter(single),
  })
  return groups.filter((group) => group.rows.length > 0)
}

/** The lifecycle filter: All, Current, Definition changed, Never run, each
 *  with its count over the whole catalog (retired tests count in All). */
export function catalogSegments(views: CatalogRowView[]) {
  const count = (kind: CatalogKind) =>
    views.filter((view) => view.kind === kind).length
  return [
    { value: 'all' as const, label: 'All', count: views.length, help: '' },
    {
      value: 'current' as const,
      label: 'Current',
      count: count('current'),
      help: 'A result on the current definition',
    },
    {
      value: 'changed' as const,
      label: 'Definition changed',
      count: count('changed'),
      help: 'Ran before the definition changed',
    },
    {
      value: 'never' as const,
      label: 'Never run',
      count: count('never'),
      help: 'No run retained',
    },
  ]
}

/** `59 tests · 24 with a current result · 31 changed since they last ran ·
 *  4 never run`, and `· 1 retired` when there are any. */
export function catalogSummary(views: CatalogRowView[]) {
  const [, current, changed, never] = catalogSegments(views)
  const retired = views.filter((view) => view.kind === 'retired').length
  return [
    plural(views.length, 'test'),
    `${current.count} with a current result`,
    `${changed.count} changed since they last ran`,
    `${never.count} never run`,
    retired ? `${retired} retired` : null,
  ]
    .filter(Boolean)
    .join(' · ')
}

/** Suite membership by test id, in the order the suites are listed. */
export function suitesByTest(
  suites: Array<CatalogSuite & { scenarios: string[] }>,
) {
  const byTest = new Map<string, CatalogSuite[]>()
  for (const suite of suites)
    for (const id of suite.scenarios)
      byTest.set(id, [
        ...(byTest.get(id) ?? []),
        { id: suite.id, label: suite.label },
      ])
  return byTest
}

/* ------------------------------------------------------ test history */

const realismLabels = {
  synthetic: 'synthetic',
  realistic_simulator: 'realistic simulator',
  frozen_real_artifact: 'frozen real artifact',
} as const

/** How real a test's environment is, for the test history's header;
 *  `value: null` when the test does not declare it. */
export function catalogRealismPresentation(row: TestCatalogRow): {
  value: string | null
  detail: string | null
} {
  const realism = row.characterization?.realism
  const execution = realism?.execution
  return {
    value: execution ? realismLabels[execution] : null,
    detail: realism?.shadow === 'read_only' ? 'read-only shadow' : null,
  }
}
