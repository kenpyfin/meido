function startOfLocalDay(d: Date): number {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime()
}

export function isSameLocalDay(a: Date, b: Date): boolean {
  return startOfLocalDay(a) === startOfLocalDay(b)
}

/** Compact label for message bubbles: time only today, date + time otherwise. */
export function formatMessageTimestamp(createdAt: Date, now: Date = new Date()): string {
  const time = createdAt.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  if (isSameLocalDay(createdAt, now)) return time

  const yesterday = new Date(now)
  yesterday.setDate(yesterday.getDate() - 1)
  if (isSameLocalDay(createdAt, yesterday)) return `Yesterday, ${time}`

  const dateOpts: Intl.DateTimeFormatOptions =
    createdAt.getFullYear() === now.getFullYear()
      ? { month: 'short', day: 'numeric' }
      : { year: 'numeric', month: 'short', day: 'numeric' }
  const datePart = createdAt.toLocaleDateString([], dateOpts)
  return `${datePart}, ${time}`
}

/** Full local date/time for hover tooltips. */
export function formatMessageTimestampTitle(createdAt: Date): string {
  return createdAt.toLocaleString()
}
