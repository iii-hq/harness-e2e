// One way to write durations, token counts, costs and dates across the
// redesigned screens. English only, like the rest of the UI: dates are
// built by hand so a browser's time-format quirks (the narrow no-break
// space some ICU versions put before AM/PM) never reach the page.

/** What a cell shows when the value was not reported. */
export const NOT_REPORTED = '—'

function known(value: number | null | undefined): value is number {
  return typeof value === 'number' && Number.isFinite(value)
}

function pad2(value: number) {
  return String(value).padStart(2, '0')
}

/** `8.8s`, `42s`, `3m 40s`, `2h 29m`. */
export function formatDuration(seconds: number | null | undefined): string {
  if (!known(seconds) || seconds < 0) return NOT_REPORTED
  if (seconds < 9.95) return `${Number(seconds.toFixed(1))}s`
  // Round the total first, so 59.6s reads "1m 00s", never "60s".
  const total = Math.round(seconds)
  if (total < 60) return `${total}s`
  const minutes = Math.floor(total / 60)
  if (minutes < 60) return `${minutes}m ${pad2(total % 60)}s`
  return `${Math.floor(minutes / 60)}h ${pad2(minutes % 60)}m`
}

/** `812`, `94.4K`, `354K`, `3.34M`, `11.9M`. */
export function formatTokens(count: number | null | undefined): string {
  if (!known(count)) return NOT_REPORTED
  if (Math.abs(count) < 1000) return String(Math.round(count))
  const thousands = count / 1e3
  const tenths = Math.round(thousands * 10) / 10
  if (tenths < 100) return `${tenths}K`
  if (Math.round(thousands) < 1000) return `${Math.round(thousands)}K`
  const millions = count / 1e6
  return `${millions.toFixed(millions < 10 ? 2 : 1)}M`
}

/** `3,339,305`: the whole number, for the title behind a compact value. */
export function formatCount(count: number | null | undefined): string {
  return known(count) ? Math.round(count).toLocaleString('en-US') : NOT_REPORTED
}

/** `$0.0025`, `$2.57`. */
export function formatCost(usd: number | null | undefined): string {
  if (!known(usd)) return NOT_REPORTED
  if (usd > 0 && usd < 0.0001) return '<$0.0001'
  return `$${usd.toFixed(usd < 1 ? 4 : 2)}`
}

const MONTHS = [
  'Jan',
  'Feb',
  'Mar',
  'Apr',
  'May',
  'Jun',
  'Jul',
  'Aug',
  'Sep',
  'Oct',
  'Nov',
  'Dec',
]

export type DateInput = string | number | Date | null | undefined

function toDate(value: DateInput): Date | null {
  if (value === null || value === undefined || value === '') return null
  const date = value instanceof Date ? value : new Date(value)
  return Number.isNaN(date.getTime()) ? null : date
}

function unparsed(value: DateInput): string {
  return typeof value === 'string' && value ? value : NOT_REPORTED
}

function shortDay(date: Date, now: Date) {
  const day = `${MONTHS[date.getMonth()]} ${date.getDate()}`
  return date.getFullYear() === now.getFullYear()
    ? day
    : `${day}, ${date.getFullYear()}`
}

function sameDay(left: Date, right: Date) {
  return (
    left.getFullYear() === right.getFullYear() &&
    left.getMonth() === right.getMonth() &&
    left.getDate() === right.getDate()
  )
}

/** `Sep 24, 3:33 AM`; the year appears only when it is not this one. */
export function formatDateTime(value: DateInput, now = new Date()): string {
  const date = toDate(value)
  if (!date) return unparsed(value)
  const hours = date.getHours()
  const time = `${hours % 12 || 12}:${pad2(date.getMinutes())} ${hours < 12 ? 'AM' : 'PM'}`
  return `${shortDay(date, now)}, ${time}`
}

/** `Today`, `Yesterday`, `Sep 21`, by the local calendar. */
export function formatDayLabel(value: DateInput, now = new Date()): string {
  const date = toDate(value)
  if (!date) return unparsed(value)
  if (sameDay(date, now)) return 'Today'
  const yesterday = new Date(
    now.getFullYear(),
    now.getMonth(),
    now.getDate() - 1,
  )
  if (sameDay(date, yesterday)) return 'Yesterday'
  return shortDay(date, now)
}

/** `1 test`, `3 tests`, `2 retries` with `plural(2, 'retry', 'retries')`. */
export function plural(count: number, one: string, many = `${one}s`): string {
  return `${count} ${count === 1 ? one : many}`
}
