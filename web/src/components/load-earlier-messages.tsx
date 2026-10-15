import React from 'react'
import { IconChevronUp } from './icons'

export type LoadEarlierMessagesProps = {
  loading?: boolean
  onLoadMore: () => void
}

/** Top-of-thread control to paginate older chat history. */
export function LoadEarlierMessages({ loading = false, onLoadMore }: LoadEarlierMessagesProps) {
  return (
    <div className="mc-load-earlier" role="region" aria-label="Earlier messages">
      <button
        type="button"
        className="mc-load-earlier-btn cursor-pointer"
        onClick={onLoadMore}
        disabled={loading}
        aria-busy={loading}
      >
        {loading ? (
          <>
            <span className="mc-load-earlier-spinner" aria-hidden />
            Loading earlier messages…
          </>
        ) : (
          <>
            <IconChevronUp className="size-3.5 shrink-0 opacity-80" />
            Load earlier messages
          </>
        )}
      </button>
    </div>
  )
}
