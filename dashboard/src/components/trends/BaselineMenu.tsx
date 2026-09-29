import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@iii-dev/console-ui'
import { ChevronDown } from 'lucide-react'
import {
  type BaselineWhy,
  counted,
  pointTime,
  seriesModel,
  type TrendPoint,
  trendMetric,
} from '@/lib/trends'

const PREVIOUS = 'previous'
const score = trendMetric('score')

/** What the trigger reads: the previous execution, the baseline's time, or
 *  why the baseline in the link is not read against. */
export function baselineText(
  point: TrendPoint | null,
  base: string | null,
  why: BaselineWhy | null,
) {
  if (!base) return 'previous execution'
  if (why === 'not_in_view' || !point) return 'not in this view'
  if (why === 'no_counted_run') return `${pointTime(point)} · no counted run`
  return pointTime(point)
}

/** What every execution is read against: the one before it, or one picked
 *  here as the baseline. Newest first, as the executions table lists them. */
export function BaselineMenu({
  points,
  base,
  why,
  onPick,
}: {
  points: TrendPoint[]
  base: string | null
  why: BaselineWhy | null
  onPick: (id: string | null) => void
}) {
  const point = points.find((item) => item.execution_id === base) ?? null
  const mixed = new Set(points.map((item) => seriesModel(item))).size > 1
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button type="button" className="tr-control" data-picker="baseline">
          <span className="tr-faint-ink">Baseline</span>
          <span className="tr-strong tr-ellipsis">
            {baselineText(point, base, why)}
          </span>
          <ChevronDown size={16} aria-hidden="true" className="tr-faint-ink" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        aria-label="Baseline"
        className="tr-menu tr-baseline-menu"
      >
        <DropdownMenuRadioGroup
          value={base ?? PREVIOUS}
          onValueChange={(value) => onPick(value === PREVIOUS ? null : value)}
        >
          <DropdownMenuRadioItem value={PREVIOUS} className="tr-menu-item">
            <span className="tr-menu-text">
              <span className="tr-strong">Previous execution</span>
              <span className="tr-faint">each one against the one before</span>
            </span>
          </DropdownMenuRadioItem>
          <DropdownMenuSeparator />
          {[...points].reverse().map((item) => {
            const value = score.value(item)
            return (
              <DropdownMenuRadioItem
                key={item.execution_id}
                value={item.execution_id}
                disabled={!counted(item)}
                className="tr-menu-item"
              >
                <span className="tr-menu-text">
                  <span className="tr-strong">{pointTime(item)}</span>
                  <span className="tr-faint tr-ellipsis">
                    {counted(item)
                      ? [
                          value === null
                            ? null
                            : `score ${score.figure(value, item)}`,
                          item.workers?.harness
                            ? `harness ${item.workers.harness}`
                            : null,
                          mixed ? seriesModel(item) : null,
                        ]
                          .filter(Boolean)
                          .join(' · ')
                      : 'no counted run'}
                  </span>
                </span>
              </DropdownMenuRadioItem>
            )
          })}
        </DropdownMenuRadioGroup>
        <p className="tr-menu-note">
          The cards and the panel read the picked execution, or the latest,
          against it. A point’s panel can set it too.
        </p>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
