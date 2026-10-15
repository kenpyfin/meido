import { describe, expect, it } from 'vitest'
import { formatMessageTimestamp, isSameLocalDay } from './format-message-time'

describe('isSameLocalDay', () => {
  it('matches same calendar day', () => {
    const a = new Date(2026, 5, 1, 9, 0)
    const b = new Date(2026, 5, 1, 23, 59)
    expect(isSameLocalDay(a, b)).toBe(true)
  })

  it('differs across midnight', () => {
    const a = new Date(2026, 5, 1, 23, 59)
    const b = new Date(2026, 5, 2, 0, 1)
    expect(isSameLocalDay(a, b)).toBe(false)
  })
})

describe('formatMessageTimestamp', () => {
  it('shows time only for today', () => {
    const now = new Date(2026, 5, 1, 14, 30)
    const msg = new Date(2026, 5, 1, 9, 15)
    const label = formatMessageTimestamp(msg, now)
    expect(label).toMatch(/\d/)
    expect(label.toLowerCase()).not.toContain('yesterday')
    expect(label).not.toContain('2026')
  })

  it('prefixes yesterday', () => {
    const now = new Date(2026, 5, 2, 12, 0)
    const msg = new Date(2026, 5, 1, 8, 0)
    expect(formatMessageTimestamp(msg, now)).toMatch(/^Yesterday, /)
  })

  it('includes month/day for older same-year messages', () => {
    const now = new Date(2026, 5, 10, 12, 0)
    const msg = new Date(2026, 4, 20, 8, 0)
    const label = formatMessageTimestamp(msg, now)
    expect(label).toContain('May')
    expect(label).toContain('20')
  })
})
