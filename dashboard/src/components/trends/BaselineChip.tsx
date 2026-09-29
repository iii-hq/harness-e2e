import { X } from 'lucide-react'
import { type BaselineWhy, pointTime, type TrendPoint } from '@/lib/trends'

/** The pinned baseline in the toolbar: its time picks it, the cross clears
 *  it. When the view does not show it, or it has no counted run, it says so
 *  and reads nothing against it. */
export function BaselineChip({
  point,
  why,
  onPick,
  onClear,
}: {
  point: TrendPoint | null
  why: BaselineWhy | null
  onPick: () => void
  onClear: () => void
}) {
  const text = point ? pointTime(point) : 'not in this view'
  const label =
    why === 'not_in_view'
      ? 'Baseline not in this view'
      : why === 'no_counted_run'
        ? 'Baseline · no counted run'
        : `Baseline ${text}`
  return (
    <span className="tr-chip" data-baseline-chip={why ?? 'set'}>
      <button
        type="button"
        className="tr-chip-pick"
        disabled={!point}
        title={label}
        onClick={onPick}
      >
        <span className="tr-chip-rule" aria-hidden="true" />
        {why === 'not_in_view' ? (
          <span className="tr-faint-ink tr-ellipsis">{label}</span>
        ) : (
          <>
            <span className="tr-faint-ink">Baseline</span>
            <span className="tr-strong tr-ellipsis">
              {why === 'no_counted_run' ? `${text} · no counted run` : text}
            </span>
          </>
        )}
      </button>
      <button
        type="button"
        className="tr-icon-button"
        aria-label="Clear baseline"
        title="Clear baseline"
        onClick={onClear}
      >
        <X size={14} aria-hidden="true" />
      </button>
    </span>
  )
}
