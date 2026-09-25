import { describe, expect, it } from 'vitest'
import {
  formatCost,
  formatCount,
  formatDateTime,
  formatDayLabel,
  formatDuration,
  formatTokens,
  plural,
} from './format'

// Local-time dates, so the expectations hold in any time zone.
const now = new Date(2026, 8, 24, 20, 12)

describe('formatDuration', () => {
  it('reads seconds, minutes and hours the way the canvas does', () => {
    expect(formatDuration(42)).toBe('42s')
    expect(formatDuration(8.8)).toBe('8.8s')
    expect(formatDuration(8)).toBe('8s')
    expect(formatDuration(220)).toBe('3m 40s')
    expect(formatDuration(65)).toBe('1m 05s')
    expect(formatDuration(8951)).toBe('2h 29m')
  })

  it('rounds the total first, so a unit never reads 60', () => {
    expect(formatDuration(59.6)).toBe('1m 00s')
    expect(formatDuration(3599.6)).toBe('1h 00m')
  })

  it('shows a dash for what was not reported', () => {
    expect(formatDuration(null)).toBe('—')
    expect(formatDuration(undefined)).toBe('—')
    expect(formatDuration(Number.NaN)).toBe('—')
    expect(formatDuration(-1)).toBe('—')
  })
})

describe('formatTokens', () => {
  it('compacts to K and M with three significant digits', () => {
    expect(formatTokens(812)).toBe('812')
    expect(formatTokens(4224)).toBe('4.2K')
    expect(formatTokens(94_419)).toBe('94.4K')
    expect(formatTokens(353_987)).toBe('354K')
    expect(formatTokens(3_339_305)).toBe('3.34M')
    expect(formatTokens(11_878_141)).toBe('11.9M')
  })

  it('moves to M when K would round to a thousand', () => {
    expect(formatTokens(999_600)).toBe('1.00M')
  })

  it('shows a dash for what was not reported', () => {
    expect(formatTokens(null)).toBe('—')
  })
})

describe('formatCount', () => {
  it('writes the whole number, for titles behind a compact value', () => {
    expect(formatCount(3_339_305)).toBe('3,339,305')
    expect(formatCount(null)).toBe('—')
  })
})

describe('formatCost', () => {
  it('keeps four decimals under a dollar and two above', () => {
    expect(formatCost(0.0025)).toBe('$0.0025')
    expect(formatCost(2.5712)).toBe('$2.57')
    expect(formatCost(0)).toBe('$0.0000')
  })

  it('never rounds a real cost down to zero', () => {
    expect(formatCost(0.00004)).toBe('<$0.0001')
  })

  it('shows a dash for what was not reported', () => {
    expect(formatCost(undefined)).toBe('—')
  })
})

describe('formatDateTime', () => {
  it('writes a short date and time', () => {
    expect(formatDateTime(new Date(2026, 8, 24, 3, 33), now)).toBe(
      'Sep 24, 3:33 AM',
    )
    expect(formatDateTime(new Date(2026, 8, 24, 0, 5), now)).toBe(
      'Sep 24, 12:05 AM',
    )
    expect(formatDateTime(new Date(2026, 8, 24, 12, 0).getTime(), now)).toBe(
      'Sep 24, 12:00 PM',
    )
  })

  it('names the year only when it is not the current one', () => {
    expect(formatDateTime(new Date(2025, 11, 31, 23, 59), now)).toBe(
      'Dec 31, 2025, 11:59 PM',
    )
  })

  it('parses ISO strings and keeps what it cannot parse', () => {
    const iso = new Date(2026, 8, 21, 18, 7).toISOString()
    expect(formatDateTime(iso, now)).toBe('Sep 21, 6:07 PM')
    expect(formatDateTime('last Tuesday', now)).toBe('last Tuesday')
    expect(formatDateTime('', now)).toBe('—')
    expect(formatDateTime(null, now)).toBe('—')
  })
})

describe('formatDayLabel', () => {
  it('says Today and Yesterday by the local calendar', () => {
    expect(formatDayLabel(new Date(2026, 8, 24, 0, 1), now)).toBe('Today')
    expect(formatDayLabel(new Date(2026, 8, 23, 23, 59), now)).toBe('Yesterday')
    expect(formatDayLabel(new Date(2026, 8, 21, 12), now)).toBe('Sep 21')
  })

  it('crosses month and year boundaries', () => {
    const newYear = new Date(2027, 0, 1, 9)
    expect(formatDayLabel(new Date(2026, 11, 31, 22), newYear)).toBe(
      'Yesterday',
    )
    expect(formatDayLabel(new Date(2026, 11, 30, 22), newYear)).toBe(
      'Dec 30, 2026',
    )
  })

  it('shows a dash for a missing date', () => {
    expect(formatDayLabel(undefined, now)).toBe('—')
  })
})

describe('plural', () => {
  it('counts with the right noun', () => {
    expect(plural(1, 'test')).toBe('1 test')
    expect(plural(0, 'test')).toBe('0 tests')
    expect(plural(3, 'test run')).toBe('3 test runs')
    expect(plural(2, 'retry', 'retries')).toBe('2 retries')
  })
})
