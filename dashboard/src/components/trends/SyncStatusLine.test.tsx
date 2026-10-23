import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { SyncStatusLine, syncText } from '@/components/trends/SyncStatusLine'
import type { CiSyncStatus } from '@/lib/dashboard-data-source'
import { ciSync } from '@/test-fixtures/release-overview'

const k = ciSync.keeping_up
const render = (
  status: CiSyncStatus | null,
  props: { error?: string | null; empty?: boolean } = {},
) =>
  renderToStaticMarkup(
    <SyncStatusLine
      status={status}
      error={props.error ?? null}
      empty={props.empty}
      onSet={() => undefined}
      onCheck={() => undefined}
    />,
  )

describe('sync status text', () => {
  it('says where sync is, in the words of its phase', () => {
    expect(syncText(k)).toBe('Synced from CI at 2:55 PM · 4 imports failed')
    expect(syncText({ ...k, failed: 0 })).toBe('Synced from CI at 2:55 PM')
    expect(syncText({ ...k, failed: 1 })).toBe(
      'Synced from CI at 2:55 PM · 1 import failed',
    )
    expect(syncText(ciSync.quick)).toBe(
      'Importing the newest runs first: 3 of 12',
    )
    expect(syncText(ciSync.backfill)).toBe(
      'Verdict ready · importing older runs: 120 of 3,000 · 2 imports failed',
    )
    expect(syncText({ ...k, checked_at: null, failed: 0 })).toBe('Checking CI…')
    expect(syncText({ ...k, phase: 'off', failed: 0 })).toBe('Starting sync…')
    expect(syncText(ciSync.signed_out)).toBe(
      'GitHub is not signed in on the worker: run `gh auth login` on its machine, then check now.',
    )
    expect(syncText(ciSync.paused)).toBe(
      'GitHub rate limit reached · resumes at 3:40 PM',
    )
    expect(syncText({ ...k, message: '`gh` exited with 1: HTTP 502' })).toBe(
      '`gh` exited with 1: HTTP 502',
    )
    expect(syncText(ciSync.off)).toBe(
      'Downloads every CI run GitHub keeps (90 days), the newest Release Control runs first, so the verdict shows within minutes.',
    )
  })
})

describe('sync status line', () => {
  it('offers the download while off, large on an empty first screen', () => {
    const off = render(ciSync.off)
    expect(off).toContain('Download CI results')
    expect(off).not.toContain('Check now')
    expect(off).not.toContain('ds-button-large')
    const first = render(ciSync.off, { empty: true })
    expect(first).toContain('Download CI results')
    expect(first).toContain('ds-button-large')
    expect(first).toContain('tr-sync-empty')
    expect(first).toContain(syncText(ciSync.off))
  })

  it('checks now, turns off from its menu, and sets gh commands in code', () => {
    const on = render(k)
    expect(on).toContain('aria-label="Check now"')
    expect(on).toContain('aria-label="Sync options"')
    expect(on).toContain('Turn off sync')
    expect(on).toContain('Keeps what was imported')
    expect(on).not.toContain('Download CI results')
    expect(on).not.toContain('data-tone')

    const signedOut = render(ciSync.signed_out)
    expect(signedOut).toContain('<code class="gi-code">gh auth login</code>')
    expect(signedOut).toContain('data-tone="danger"')
    expect(render(ciSync.paused)).toContain('data-tone="danger"')

    const failed = render(k, { error: 'engine unavailable' })
    expect(failed).toContain('role="alert"')
    expect(failed).toContain('engine unavailable')
    expect(render(k)).not.toContain('role="alert"')

    expect(render(null)).toBe('')
  })
})
