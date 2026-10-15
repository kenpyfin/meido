import React from 'react'
import { IconChevronDown } from './icons'

export type LoadNewerMessagesProps = {
  loading?: boolean
  onLoadMore: () => void
}

/** Bottom-of-thread control to paginate toward newer messages (anchored window). */
export function LoadNewerMessages({ loading = false, onLoadMore }: LoadNewerMessagesProps) {
  return (
    <div className="mc-load-newer" role="region" aria-label="Newer messages">
      <button
        type="button"
        className="mc-load-newer-btn cursor-pointer"
        onClick={onLoadMore}
        disabled={loading}
        aria-busy={loading}
      >
        {loading ? (
          <>
            <span className="mc-load-earlier-spinner" aria-hidden />
            Loading newer messages…
          </>
        ) : (
          <>
            <IconChevronDown className="size-3.5 shrink-0 opacity-80" />
            Load newer messages
          </>
        )}
      </button>
    </div>
  )
}
