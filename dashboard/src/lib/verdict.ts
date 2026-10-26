// The calibrated verdict the worker reads for two executions
// (e2e::dashboard::execution-verdict) and for each combination of the
// Trends overview, as `harness-e2e compare` prints it (src/verdict.rs).
// The rule lives in the worker; only the wording is here.

export type VerdictState = 'better' | 'no_difference' | 'worse' | 'cannot_tell'
export type QualityState =
  | 'improved'
  | 'non_inferior'
  | 'inferior'
  | 'fell'
  | 'inconclusive'
export type CostState =
  | 'equivalent'
  | 'cheaper'
  /** Below +equivalence, but neither within −equivalence nor cheaper. */
  | 'below_bound'
  | 'more_expensive'
  | 'inconclusive'
export type Sides<T> = { a: T; b: T }

export type VerdictRule = {
  margin: number
  equivalence: number
  alpha: number
  permutations: number
  seed: number
}

export type VerdictQuality = {
  state: QualityState
  /** Mean over the tests of each test's mean score. */
  a: number
  b: number
  delta: number
  tests: number
  p_improved: number
  p_non_inferior: number
  p_inferior: number
  p_fell: number
}

export type VerdictCost = {
  state: CostState
  /** Summed over the tests, each test's mean subject cost, in US$. */
  a: number
  b: number
  /** B / A − 1, shown; the tests read B − A. */
  relative: number
  tests: number
  /** Each against α / 2. */
  p_cheaper: number
  p_more_expensive: number
  p_within_low: number
  p_within_high: number
}

export type VerdictRepetitions = {
  /** The fewest counted runs a compared test has on each side. */
  have: Sides<number>
  sigma: number | null
  /** Runs per test on each side, those made kept, that would settle what
   *  is undecided with a predicted 80%. */
  needed: number | null
  missing: number | null
  /** The cap, when more than it would be needed. */
  more_than: number | null
  /** What `needed` or `more_than` settles. */
  settles: Array<'quality' | 'cost'>
  note: string | null
}

/** What the Trends overview carries for a combination. */
export type VerdictSummary = {
  /** Null when there is no calibrated verdict; `unavailable` says why. */
  verdict: VerdictState | null
  qualifier: 'test_inferior' | 'quality_fell' | 'trade_off' | null
  unavailable: string | null
  reasons: string[]
  rule: VerdictRule
  quality: VerdictQuality | null
  cost: VerdictCost | null
  repetitions: VerdictRepetitions
  /** A test that fell beyond the margin on its own among them. */
  warnings: string[]
}

export type CriterionChange = {
  scenario: string
  behavior: string | null
  criteria: string[]
  failing: Sides<number>
  /** Runs that did not evaluate them. */
  not_evaluated: Sides<number>
  runs: Sides<number>
  points: Sides<number>
  delta: number
}

export type CriteriaConfiguration = {
  side: 'a' | 'b'
  scenario: string
  behavior: string | null
  /** `provider/model`; null when its results do not record it. */
  model: string | null
  /** Each worker's resolved version, `iii` and `lock`; empty when not recorded. */
  stack: Record<string, string>
  runs: {
    evaluated: number
    zeroed: number
    prerequisite_failed: number
    infrastructure: number
  }
  sufficient: boolean
  /** The criteria some counted run evaluated. */
  criteria: Array<{
    id: string
    full: number
    n: number
    /** Counted runs that left it unevaluated. */
    not_evaluated: number
    rate: number
    low: number
    high: number
    class: 'saturated' | 'never_full' | 'oscillating' | 'insufficient_data'
  }>
  /** The criteria no counted run evaluated. */
  not_evaluated: string[]
  groups: Array<{ criteria: string[]; failing: number; of: number }>
}

/** The answer of e2e::dashboard::execution-verdict. */
export type ExecutionVerdict = VerdictSummary & {
  figures: Array<{
    id: string
    a: number | null
    b: number | null
    delta: number | null
    relative: number | null
    comparable: boolean
  }>
  tests: Array<{
    scenario: string
    behavior: string | null
    runs: Sides<number>
    a: number
    b: number
    delta: number
    p_fell: number
    /** Its own fall beyond the margin, and that Holm-adjusted. */
    p_inferior: number
    p_inferior_adjusted: number
    warning: boolean
  }>
  dropped: Array<{ scenario: string; behavior: string | null; reason: string }>
  runs: Sides<{
    counted: number
    excluded: number
    causes: Record<string, number>
  }>
  criteria: {
    min_runs: number
    configurations: CriteriaConfiguration[]
    changes: CriterionChange[]
    /** A side whose runs do not record their model or stack. */
    warnings: string[]
  }
}

/** One per change: a scenario can move under two definitions. */
export function changeKey(change: CriterionChange): string {
  return `${change.scenario}:${change.behavior ?? ''}:${change.criteria.join(',')}`
}

const LABEL: Record<VerdictState, string> = {
  better: 'Better',
  no_difference: 'No difference',
  worse: 'Worse',
  cannot_tell: 'Cannot tell yet',
}

const QUALIFIER = {
  test_inferior: 'a test fell beyond the margin on its own',
  quality_fell: 'quality fell, by an amount the runs cannot bound',
  trade_off:
    'quality improved and the cost rose, a trade-off the rule does not weigh',
}

/** `Better`, `Cannot tell yet: quality fell…`, or that there is none. */
export function verdictHeadline(summary: VerdictSummary): string {
  if (!summary.verdict) return 'No calibrated verdict'
  const label = LABEL[summary.verdict]
  return summary.qualifier ? `${label}: ${QUALIFIER[summary.qualifier]}` : label
}

/** Green for better, red for worse or a fall, faint for the rest. */
export function verdictTone(
  summary: VerdictSummary,
): 'positive' | 'negative' | 'neutral' {
  if (summary.verdict === 'better') return 'positive'
  if (
    summary.verdict === 'worse' ||
    summary.qualifier === 'quality_fell' ||
    summary.qualifier === 'test_inferior'
  )
    return 'negative'
  return 'neutral'
}

/** How many runs per test each side has, and what more would settle it. */
export function repetitionsText(summary: VerdictSummary): string {
  const { have, sigma, needed, missing, more_than, settles, note } =
    summary.repetitions
  const parts = [
    `At least ${have.a} runs per test on A, ${have.b} on B`,
    sigma === null ? null : `σ ${sigma.toFixed(1)} points per run`,
  ].filter(Boolean)
  const what =
    settles
      .map((item) =>
        item === 'quality'
          ? `the ${summary.rule.margin}-point margin`
          : 'the cost',
      )
      .join(' and ') || 'it'
  const more =
    needed !== null && missing !== null
      ? `about ${needed} runs per test on each side, those made kept, would settle ${what} with a predicted 80%: ${missing} more`
      : more_than !== null
        ? `more than ${more_than} runs per test on each side would be needed to settle ${what}`
        : null
  return `${parts.join(' · ')}${more ? `; ${more}.` : '.'}${note ? ` ${note}` : ''}`
}

/** The rule in one line, as the reasons read it. */
export function ruleText({
  margin,
  equivalence,
  alpha,
  permutations,
  seed,
}: VerdictRule): string {
  return `Quality within ${margin} points, cost within ±${Math.round(equivalence * 100)}%: permutation tests within each test, one-sided α ${alpha} (improved, fell, cheaper and more expensive at α / 2; each test's fall beyond the margin Holm-adjusted), ${permutations.toLocaleString('en-US')} permutations, seed ${seed}.`
}
