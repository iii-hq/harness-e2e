import type { CiSyncStatus } from '@/lib/dashboard-data-source'
import type { ReleaseOverview } from '@/lib/release-overview'
import type { VerdictSummary } from '@/lib/verdict'
import ciSyncFixture from '@/test-fixtures/ci-sync.json'
import overviewFixture from '@/test-fixtures/release-overview.json'

/** The release overview the worker answers for a scenario of hand-made runs:
 *  written by `the_dashboard_fixture_is_this_overview` in
 *  src/dashboard/overview.rs; regenerate with `UPDATE_DASHBOARD_FIXTURE=1
 *  cargo test --locked --lib the_dashboard_fixture_is_this_overview`, then
 *  `pnpm --dir dashboard exec biome format --write
 *  src/test-fixtures/release-overview.json`. */
export const releaseOverview = overviewFixture as unknown as ReleaseOverview

/** CI sync's status in each state, as ci-sync-status-get answers it. */
export const ciSync = ciSyncFixture as unknown as Record<
  'off' | 'quick' | 'backfill' | 'keeping_up' | 'signed_out' | 'paused',
  CiSyncStatus
>

/** The calibrated verdict of the Harness A/B pair 1.8.44 → 1.8.50 on
 *  software engineering, as `harness-e2e compare` reads
 *  tests/fixtures/verdict/ (rounded): cheaper, quality undecided. */
export const cannotTell: VerdictSummary = {
  verdict: 'cannot_tell',
  qualifier: null,
  unavailable: null,
  reasons: [
    'Quality -7.0 points: neither within the 5-point margin (p = 0.625) nor below it (p = 0.376).',
    'Cost -19.5%: cheaper (p < 0.001).',
  ],
  rule: {
    margin: 5,
    equivalence: 0.05,
    alpha: 0.025,
    permutations: 10000,
    seed: 0,
  },
  quality: {
    state: 'inconclusive',
    a: 88.2917,
    b: 81.3,
    delta: -6.9917,
    tests: 6,
    p_improved: 0.9668,
    p_non_inferior: 0.6245,
    p_inferior: 0.3756,
    p_fell: 0.0343,
  },
  cost: {
    state: 'cheaper',
    a: 0.788,
    b: 0.6343,
    relative: -0.1951,
    tests: 6,
    p_cheaper: 0.0003,
    p_more_expensive: 0.9998,
    p_within_low: 0.9987,
    p_within_high: 0.0001,
  },
  repetitions: {
    have: { a: 4, b: 5 },
    sigma: 15.3359,
    needed: null,
    missing: null,
    more_than: 30,
    settles: ['quality'],
    note: null,
  },
  // Neither the Kanban board (−16) nor the Registry implementation (−16.8)
  // shows its own fall beyond the margin at five runs: no warning.
  warnings: [],
}
