import { Download, RefreshCw } from 'lucide-react'
import { withCode } from '@/components/GithubImportDialog'
import { Button, RowMenu } from '@/design-system'
import type { CiSyncStatus } from '@/lib/dashboard-data-source'
import { formatCount, formatTime, plural } from '@/lib/format'

/** What CI sync is doing, in the words of its phase. A `gh` command is
 *  written between backticks. */
export function syncText(status: CiSyncStatus): string {
  if (!status.enabled)
    return 'Downloads every CI run GitHub keeps (90 days), the newest Release Control runs first, so the verdict shows within minutes.'
  if (status.signed_in === false)
    return 'GitHub is not signed in on the worker: run `gh auth login` on its machine, then check now.'
  if (status.paused_until)
    return `GitHub rate limit reached · resumes at ${formatTime(status.paused_until)}`
  if (status.message) return status.message
  const failed =
    status.failed > 0 ? ` · ${plural(status.failed, 'import')} failed` : ''
  const progress = `${formatCount(status.imported)} of ${formatCount(status.imported + status.remaining)}`
  switch (status.phase) {
    case 'quick':
      return `Importing the newest runs first: ${progress}${failed}`
    case 'backfill':
      return `Verdict ready · importing older runs: ${progress}${failed}`
    case 'keeping_up':
      return status.checked_at
        ? `Synced from CI at ${formatTime(status.checked_at)}${failed}`
        : `Checking CI…${failed}`
    default:
      return `Starting sync…${failed}`
  }
}

/** CI sync in one line: the download offer while it is off, else where it
 *  is, with Check now and, under Sync options, Turn off sync. `empty` is the
 *  first screen with nothing to draw: the offer becomes a large button. */
export function SyncStatusLine({
  status,
  error,
  empty,
  onSet,
  onCheck,
}: {
  status: CiSyncStatus | null
  /** Why the last Download, Check now or Turn off failed. */
  error: string | null
  empty?: boolean
  onSet: (enabled: boolean) => void
  onCheck: () => void
}) {
  if (!status) return null
  const failure = error ? (
    <span className="tr-sync-error" role="alert">
      {error}
    </span>
  ) : null
  if (!status.enabled) {
    const download = (
      <Button
        variant={empty ? 'primary' : 'secondary'}
        size={empty ? 'large' : 'compact'}
        onClick={() => onSet(true)}
      >
        <Download size={empty ? 16 : 14} aria-hidden="true" />
        Download CI results
      </Button>
    )
    return empty ? (
      <div className="tr-sync-empty">
        <p className="tr-faint">{syncText(status)}</p>
        {download}
        {failure}
      </div>
    ) : (
      <div className="tr-sync">
        {download}
        {failure}
      </div>
    )
  }
  const trouble =
    status.signed_in === false ||
    status.paused_until !== null ||
    status.message !== null
  return (
    <div className="tr-sync" data-tone={trouble ? 'danger' : undefined}>
      <span className="tr-sync-text">{withCode(syncText(status))}</span>
      <Button
        variant="quiet"
        size="compact"
        aria-label="Check now"
        title="Check now"
        onClick={onCheck}
      >
        <RefreshCw size={14} aria-hidden="true" />
      </Button>
      <RowMenu
        label="Sync options"
        items={[
          {
            label: 'Turn off sync',
            hint: 'Keeps what was imported',
            onSelect: () => onSet(false),
          },
        ]}
      />
      {failure}
    </div>
  )
}
