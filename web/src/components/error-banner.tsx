import React from 'react'

export type ErrorBannerProps = {
  message: string
  className?: string
  onDismiss?: () => void
}

export function ErrorBanner({ message, className, onDismiss }: ErrorBannerProps) {
  if (!message.trim()) return null
  return (
    <div role="alert" aria-live="assertive" className={className}>
      <div
        role="status"
        className="rounded-lg border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive"
      >
        {message}
        {onDismiss ? (
          <button
            type="button"
            className="mc-error-dismiss"
            onClick={onDismiss}
            aria-label="Dismiss error"
          >
            Dismiss
          </button>
        ) : null}
      </div>
    </div>
  )
}
