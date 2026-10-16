import React from 'react'

export type StatusRegionProps = {
  message: string
  className?: string
}

/** Visually hidden live region for screen readers when status changes (e.g. bookmark saved). */
export function StatusRegion({ message, className }: StatusRegionProps) {
  return (
    <div
      role="status"
      aria-live="polite"
      aria-atomic="true"
      className={className ?? 'mc-sr-status'}
    >
      {message}
    </div>
  )
}
