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

function round(value: number, decimals: number) {
  return Number(value.toFixed(decimals))
}

// Every formatter writes the magnitude, then a leading minus unless what is
// shown rounded to zero.
function withSign(value: number, magnitude: string) {
  return value < 0 && /[1-9]/.test(magnitude) ? `-${magnitude}` : magnitude
}

/** `8.8s`, `42s`, `3m 40s`, `2h 29m`, from milliseconds. */
export function formatDuration(
  milliseconds: number | null | undefined,
): string {
  if (!known(milliseconds)) return NOT_REPORTED
  const seconds = Math.abs(milliseconds) / 1000
  const tenths = round(seconds, 1)
  // Round first and pick the unit after, so 59.6s reads "1m 00s", not "60s".
  const total = Math.round(seconds)
  let magnitude: string
  if (tenths < 10) magnitude = `${tenths}s`
  else if (total < 60) magnitude = `${total}s`
  else if (total < 3600)
    magnitude = `${Math.floor(total / 60)}m ${pad2(total % 60)}s`
  else {
    const minutes = Math.round(seconds / 60)
    magnitude = `${Math.floor(minutes / 60)}h ${pad2(minutes % 60)}m`
  }
  return withSign(milliseconds, magnitude)
}

const COMPACT_UNITS = ['K', 'M', 'B', 'T']

// K keeps one decimal below 100 (94.4K); M and up keep three significant
// digits (3.34M, 11.9M). The unit and the decimals are chosen after rounding,
// so nothing reads 1000K, 10.00M or 1500.0M.
function compactDecimals(unit: number, value: number) {
  if (unit === 0) return value < 100 ? 1 : 0
  return value < 10 ? 2 : value < 100 ? 1 : 0
}

function compact(magnitude: number) {
  if (Math.round(magnitude) < 1000) return String(Math.round(magnitude))
  let unit = 0
  let value = magnitude / 1000
  let rounded = round(value, compactDecimals(unit, value))
  while (rounded >= 1000 && unit < COMPACT_UNITS.length - 1) {
    unit += 1
    value /= 1000
    rounded = round(value, compactDecimals(unit, value))
  }
  const text = rounded.toFixed(compactDecimals(unit, rounded))
  return `${unit === 0 ? Number(text) : text}${COMPACT_UNITS[unit]}`
}

/** `812`, `94.4K`, `354K`, `3.34M`, `11.9M`, `1.50B`. */
export function formatTokens(count: number | null | undefined): string {
  if (!known(count)) return NOT_REPORTED
  return withSign(count, compact(Math.abs(count)))
}

/** `3,339,305`: the whole number, for the title behind a compact value. */
export function formatCount(count: number | null | undefined): string {
  if (!known(count)) return NOT_REPORTED
  return withSign(count, Math.round(Math.abs(count)).toLocaleString('en-US'))
}

/** `$0.0025`, `$2.57`: four decimals while the rounded amount is under $1. */
export function formatCost(usd: number | null | undefined): string {
  if (!known(usd)) return NOT_REPORTED
  const magnitude = Math.abs(usd)
  const fine = round(magnitude, 4)
  if (magnitude > 0 && fine === 0) return withSign(usd, '<$0.0001')
  return withSign(
    usd,
    fine < 1 ? `$${fine.toFixed(4)}` : `$${magnitude.toFixed(2)}`,
  )
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
  // A bare day is a calendar day where the reader is; `new Date` would read
  // it as UTC midnight, the day before west of Greenwich.
  const day =
    typeof value === 'string' && /^(\d{4})-(\d{2})-(\d{2})$/.exec(value)
  const date = day
    ? new Date(Number(day[1]), Number(day[2]) - 1, Number(day[3]))
    : value instanceof Date
      ? value
      : new Date(value)
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

function clock(date: Date) {
  const hours = date.getHours()
  return `${hours % 12 || 12}:${pad2(date.getMinutes())} ${hours < 12 ? 'AM' : 'PM'}`
}

/** `Sep 24, 3:33 AM`; the year appears only when it is not this one. */
export function formatDateTime(value: DateInput, now = new Date()): string {
  const date = toDate(value)
  if (!date) return unparsed(value)
  return `${shortDay(date, now)}, ${clock(date)}`
}

/** `Sep 24, 2026, 3:33 AM`, the year always: for what names a thing and
 *  must read the same next year, like an untitled execution's title. */
export function formatStamp(value: DateInput): string {
  const date = toDate(value)
  if (!date) return unparsed(value)
  return `${MONTHS[date.getMonth()]} ${date.getDate()}, ${date.getFullYear()}, ${clock(date)}`
}

/** `3:33 AM`, for a row under a heading that already names the day. */
export function formatTime(value: DateInput): string {
  const date = toDate(value)
  return date ? clock(date) : unparsed(value)
}

/** `Sep 24`, or `Dec 31, 2025` outside this year: the day alone. */
export function formatDay(value: DateInput, now = new Date()): string {
  const date = toDate(value)
  return date ? shortDay(date, now) : unparsed(value)
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
