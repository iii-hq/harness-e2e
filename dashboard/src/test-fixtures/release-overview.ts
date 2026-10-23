import type { CiSyncStatus } from '@/lib/dashboard-data-source'
import type { ReleaseOverview } from '@/lib/release-overview'
import ciSyncFixture from '@/test-fixtures/ci-sync.json'
import overviewFixture from '@/test-fixtures/release-overview.json'

/** The release overview of the Trends canvas's regression series (stack
 *  default) in release-overview-get's shape: one step per Harness release,
 *  1.8.36 against 1.8.35 as the verdict. Only the verdict's score drops
 *  contention_ledger, the test whose definition changed; the new measures,
 *  the early steps and the outside count are made up. */
export const releaseOverview = overviewFixture as unknown as ReleaseOverview

/** CI sync's status in each state, as ci-sync-status-get answers it. */
export const ciSync = ciSyncFixture as unknown as Record<
  'off' | 'quick' | 'backfill' | 'keeping_up' | 'signed_out' | 'paused',
  CiSyncStatus
>
